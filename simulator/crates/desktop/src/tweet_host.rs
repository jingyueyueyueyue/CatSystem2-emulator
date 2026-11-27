//! Versioned, owned Tweet host operations. OAuth and PE status transitions stay
//! in the same worker on every target; a host receipt is never a post receipt.
use anyhow::{Result, bail};
use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};
use super::{HeadlessPlayer, tweet_platform as platform};

const BODY_LIMIT: usize = 65 * 1024 * 1024;
#[derive(Default)]
struct State { next: u64, closed: bool, pending: BTreeMap<u64, Pending> }
struct Pending { wire: Vec<u8>, operation: u32, delivered: bool, result: Option<Receipt> }
struct Receipt { outcome: u32, value: u32, bytes: Vec<u8> }
#[derive(Clone, Default)]
pub(super) struct Bridge(Arc<(Mutex<State>, Condvar)>);
#[derive(Clone, Default)]
pub(super) struct Backend { pub bridge: Option<Bridge>, pub task: u32 }

impl Bridge {
    pub(super) fn cancel(&self) {
        let (lock, wake) = &*self.0;
        let mut state = lock.lock().unwrap_or_else(|p| p.into_inner());
        state.closed = true;
        // Drop headers, tokens and request bytes as soon as the player closes.
        state.pending.clear();
        wake.notify_all();
    }
    fn issue(&self, task: u32, operation: u32, payload: Vec<u8>) -> Result<Receipt> {
        if payload.len() > BODY_LIMIT { bail!("Tweet host request exceeds65 MiB"); }
        let (lock, wake) = &*self.0;
        let mut state = lock.lock().map_err(|_| anyhow::anyhow!("Tweet host queue poisoned"))?;
        if state.closed { bail!("Tweet host session closed; in-flight submission must not be replayed"); }
        let id = state.next.checked_add(1).ok_or_else(|| anyhow::anyhow!("Tweet host ids exhausted"))?;
        state.next = id;
        let mut wire = 1u32.to_le_bytes().to_vec();
        wire.extend(id.to_le_bytes()); wire.extend(task.to_le_bytes()); wire.extend(operation.to_le_bytes());
        wire.extend(payload);
        state.pending.insert(id, Pending { wire, operation, delivered: false, result: None });
        loop {
            if state.closed { bail!("Tweet host session closed; no submission retry"); }
            if let Some(result) = state.pending.get_mut(&id).and_then(|p| p.result.take()) {
                state.pending.remove(&id);
                if result.outcome == 2 {
                    bail!("Tweet host: {}", std::str::from_utf8(&result.bytes)?);
                }
                return Ok(result);
            }
            state = wake.wait(state).map_err(|_| anyhow::anyhow!("Tweet host queue poisoned"))?;
        }
    }
    fn take(&self) -> Option<Vec<u8>> {
        let mut state = self.0.0.lock().unwrap_or_else(|p| p.into_inner());
        let item = state.pending.values_mut().find(|p| !p.delivered)?;
        item.delivered = true;
        Some(std::mem::take(&mut item.wire))
    }
    fn complete(&self, id: u64, outcome: u32, value: u32, bytes: Vec<u8>) -> Result<()> {
        let (lock, wake) = &*self.0;
        let mut state = lock.lock().map_err(|_| anyhow::anyhow!("Tweet host queue poisoned"))?;
        let item = state.pending.get_mut(&id).ok_or_else(|| anyhow::anyhow!("Tweet receipt has no pending request"))?;
        if !item.delivered || item.result.is_some() { bail!("Tweet receipt is duplicate or undelivered"); }
        if outcome > 3 || (outcome == 1 && !matches!(item.operation, 2 | 6))
            || (outcome == 3 && item.operation != 6) { bail!("invalid Tweet host outcome"); }
        let limit = if outcome == 2 { 32768 } else { match item.operation { 1 => 1024*1024, 2 | 6 => 4092, 3 => 0, _ => 32768 } };
        if bytes.len() > limit { bail!("Tweet receipt exceeds operation limit"); }
        if outcome == 2 || (outcome == 0 && matches!(item.operation, 2 | 6)) { std::str::from_utf8(&bytes)?; }
        if outcome == 0 && item.operation == 1 && !(100..=599).contains(&value) { bail!("invalid Tweet HTTP status"); }
        if matches!(outcome, 1 | 3) && !bytes.is_empty() { bail!("Tweet cancellation contains unexpected payload"); }
        item.result = Some(Receipt { outcome, value, bytes }); wake.notify_all(); Ok(())
    }
}

fn field(out: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    out.extend(u32::try_from(bytes.len())?.to_le_bytes()); out.extend(bytes); Ok(())
}
impl Backend {
    pub(super) fn request(&self, method: &str, url: &str, header: &str, content: &str, bytes: &[u8]) -> Result<platform::Response> {
        let Some(bridge) = &self.bridge else { return platform::request(method, url, header, content, bytes); };
        let mut payload = Vec::new();
        for bytes in [method.as_bytes(), url.as_bytes(), header.as_bytes(), content.as_bytes(), bytes] { field(&mut payload, bytes)?; }
        let response = bridge.issue(self.task, 1, payload)?;
        Ok(platform::Response { status: response.value, bytes: response.bytes })
    }
    pub(super) fn edit(&self, caption: &str, prompt: &str, initial: &str, limit: usize) -> Result<Option<String>> {
        let Some(bridge) = &self.bridge else { return platform::edit(caption, prompt, initial, limit); };
        let mut payload = u32::try_from(limit)?.to_le_bytes().to_vec();
        for text in [caption, prompt, initial] { field(&mut payload, text.as_bytes())?; }
        let result = bridge.issue(self.task, 2, payload)?;
        if result.outcome == 1 { return Ok(None); }
        let text = String::from_utf8(result.bytes)?;
        if text.encode_utf16().count() > limit { bail!("Tweet edit exceeds UTF16 limit"); }
        Ok(Some(text))
    }
    pub(super) fn open_url(&self, url: &str) -> Result<()> {
        let Some(bridge) = &self.bridge else { return platform::open_url(url); };
        let mut payload = Vec::new(); field(&mut payload, url.as_bytes())?;
        bridge.issue(self.task, 3, payload)?; Ok(())
    }
    pub(super) fn edit_tweet(&self, prompt: &str, initial: &str, suffix: &str) -> Result<platform::EditorResult> {
        let Some(bridge) = &self.bridge else { return platform::edit_tweet(prompt, initial, suffix); };
        let mut payload = 1023u32.to_le_bytes().to_vec();
        for text in ["Tweet", prompt, initial, suffix] { field(&mut payload, text.as_bytes())?; }
        let result = bridge.issue(self.task, 6, payload)?;
        Ok(match result.outcome {
            1 => platform::EditorResult::Cancelled, 3 => platform::EditorResult::Forget,
            _ => {
                let text = String::from_utf8(result.bytes)?;
                if text.encode_utf16().count() > 1023 { bail!("Tweet edit exceeds UTF16 limit"); }
                platform::EditorResult::Edited(text)
            }
        })
    }
    pub(super) fn protect(&self, bytes: &[u8], seal: bool) -> Result<Vec<u8>> {
        let Some(bridge) = &self.bridge else { return if seal { platform::seal(bytes) } else { platform::unseal(bytes) }; };
        if bytes.len() > 32768 { bail!("Tweet credential data exceeds32 KiB"); }
        let mut payload = Vec::new(); field(&mut payload, bytes)?;
        Ok(bridge.issue(self.task, if seal {4} else {5}, payload)?.bytes)
    }
}

/// Stateless UI helper, with exactly the same CP932 conversion/count as worker
/// acceptance. It never reads an engine, account, credential file or network.
pub fn tweet_character_count(body: &str, suffix: &str) -> Result<usize> {
    let (body, _, bad_body) = encoding_rs::SHIFT_JIS.encode(body);
    let (suffix, _, bad_suffix) = encoding_rs::SHIFT_JIS.encode(suffix);
    if bad_body || bad_suffix { bail!("Tweet editor text is not representable in CP932"); }
    if body.len() >= 1024 { bail!("Tweet editor text exceeds original1024-byte buffer"); }
    Ok(cs2_formats::tweet_credentials::count_text(&body, &suffix)?)
}

impl HeadlessPlayer {
    /// Install before executing type62. Queue ownership is local to this player.
    pub fn enable_host_tweet_queue(&mut self) { self.data_services.tweets.enable_host(); }
    /// LE version1,u64 id,u32 task,u32 operation, then length-prefixed UTF8/bytes.
    pub fn take_host_tweet_request(&mut self) -> Option<Vec<u8>> { self.data_services.tweets.bridge.as_ref()?.take() }
    /// outcome0=operation completed,1=editor cancelled,2=diagnostic UTF8,
    /// 3=confirmed Forget in the operation6 Tweet editor.
    /// value is the actual HTTP status for operation1, otherwise zero.
    pub fn complete_host_tweet_request(&mut self, id: u64, outcome: u32, value: u32, bytes: Vec<u8>) -> Result<()> {
        self.data_services.tweets.bridge.as_ref().ok_or_else(|| anyhow::anyhow!("Tweet host queue disabled"))?.complete(id, outcome, value, bytes)
    }
    pub fn cancel_host_tweet_requests(&mut self) {
        if let Some(bridge) = &self.data_services.tweets.bridge { bridge.cancel(); }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn tweet_editor_host_error_finishes_as_cancellation() -> Result<()> {
        let bridge = Bridge::default();
        let status = Arc::new(AtomicU32::new(0));
        let worker_status = status.clone();
        let request = super::super::tweet_worker::Request {
            key: "test-key".into(),
            secret: "test-secret".into(),
            cache_name: Vec::new(),
            text: "draft".into(),
            media: Vec::new(),
            private_root: std::env::temp_dir().join(format!(
                "cs2-tweet-test-{}-{}", std::process::id(), platform::nonce()?,
            )),
            backend: Backend { bridge: Some(bridge.clone()), task: 1 },
            suffix: Vec::new(),
            legacy: None,
        };
        let worker = std::thread::spawn(move || super::super::tweet_worker::run(request, worker_status));
        let receipts: &[(u32, u32, u32, &[u8])] = &[
            (1, 0, 200, b"oauth_token=temporary&oauth_token_secret=temporary-secret"),
            (3, 0, 0, b""),
            (2, 0, 0, b"1234"),
            (1, 0, 200, b"oauth_token=access&oauth_token_secret=access-secret"),
            (4, 2, 0, b"credential storage unavailable"),
            (1, 0, 200, b"{}"),
            (6, 2, 0, b"editor closed"),
        ];
        let result = (|| -> Result<()> {
            for &(operation, outcome, value, body) in receipts {
                let deadline = Instant::now() + Duration::from_secs(5);
                let wire = loop {
                    if let Some(wire) = bridge.take() {
                        break wire;
                    }
                    anyhow::ensure!(!worker.is_finished() && Instant::now() < deadline,
                        "Tweet host operation {operation} did not arrive");
                    std::thread::sleep(Duration::from_millis(1));
                };
                anyhow::ensure!(u32::from_le_bytes(wire[16..20].try_into()?) == operation,
                    "unexpected Tweet host operation");
                bridge.complete(u64::from_le_bytes(wire[4..12].try_into()?), outcome, value, body.to_vec())?;
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while !worker.is_finished() {
                anyhow::ensure!(bridge.take().is_none(), "unexpected host request after editor error");
                anyhow::ensure!(Instant::now() < deadline, "Tweet worker did not finish after editor error");
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(())
        })();
        bridge.cancel();
        let outcome = worker.join().map_err(|_| anyhow::anyhow!("Tweet worker panicked"))?;
        result?;
        assert_eq!(status.load(Ordering::Acquire), 6);
        assert!(outcome.diagnostics.iter().any(|line| line.contains("editor closed")));
        Ok(())
    }
}
