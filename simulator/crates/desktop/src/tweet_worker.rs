//! Owned asynchronous Tweet consumer. HTTP responses, authorization, edit
//! cancellation and the PE's final status overwrite are separate outcomes.
use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use std::sync::{Arc, atomic::{AtomicU32, Ordering}};
use super::tweet_platform as platform;
use super::tweet_protocol::{self as wire, Credentials};

pub(super) struct Media { pub name: String, pub bytes: std::result::Result<Vec<u8>, String> }
pub(super) struct Request {
    pub key: String, pub secret: String, pub cache_name: Vec<u16>,
    pub text: String, pub media: Vec<Media>, pub private_root: PathBuf,
    pub backend: super::tweet_host::Backend,
    pub suffix: Vec<u8>,
    pub legacy: Option<(PathBuf, [u8; 16])>,
}
pub(super) struct Outcome { pub diagnostics: Vec<String> }

fn signed(backend: &super::tweet_host::Backend, credentials: &Credentials, method: &str, url: &str, fields: &[(String, String)],
    oauth: &[(String, String)], multipart: Option<(&str, &[u8])>) -> Result<platform::Response> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
    let nonce = platform::nonce()?;
    let header = credentials.header(method, url, fields, oauth, &nonce, now);
    if let Some((content, bytes)) = multipart { backend.request(method, url, &header, content, bytes) }
    else {
        let encoded = wire::form(fields);
        backend.request(method, url, &header, "application/x-www-form-urlencoded", encoded.as_bytes())
    }
}

fn successful(response: platform::Response, stage: &str) -> Result<Vec<u8>> {
    if !(200..300).contains(&response.status) {
        // Never log request headers, credentials, PINs or response token data.
        bail!("Tweet {stage} HTTP status {}", response.status);
    }
    Ok(response.bytes)
}

fn access_token_field(fields: &[(String, String)], name: &str) -> Result<String> {
    let mut matching = fields.iter().filter(|(key, _)| key == name);
    let (_, value) = matching.next().ok_or_else(|| anyhow::anyhow!("access-token reply lacks{name}"))?;
    if matching.next().is_some() { bail!("duplicate access-token field{name}"); }
    // A completed exchange with empty token data reaches original status4;
    // do not conflate it with transport/form/authentication failure status3.
    Ok(value.clone())
}

fn cache_path(request: &Request) -> PathBuf {
    let mut identity = request.key.as_bytes().to_vec(); identity.push(0);
    identity.extend(request.cache_name.iter().flat_map(|c| c.to_le_bytes()));
    let name = wire::digest(&identity).iter().map(|b| format!("{b:02x}")).collect::<String>();
    request.private_root.join(format!("tweet-{name}.bin"))
}

fn read_tokens(path: &std::path::Path, backend: &super::tweet_host::Backend) -> Result<Option<(String, String)>> {
    use std::io::Read;
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file, Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut sealed = Vec::new(); file.by_ref().take(32769).read_to_end(&mut sealed)?;
    if sealed.len() > 32768 || !sealed.starts_with(b"CS2TWT1\0") { bail!("invalid private Tweet credential container"); }
    let clear = backend.protect(&sealed[8..], false)?;
    let fields = wire::unform(&clear)?;
    Ok(Some((wire::value(&fields, "oauth_token")?.to_owned(), wire::value(&fields, "oauth_token_secret")?.to_owned())))
}

fn write_tokens(path: &std::path::Path, credentials: &Credentials, backend: &super::tweet_host::Backend) -> Result<()> {
    use std::io::Write;
    let clear = wire::form(&[("oauth_token".into(), credentials.token.clone()),
        ("oauth_token_secret".into(), credentials.token_secret.clone())]);
    let mut sealed = b"CS2TWT1\0".to_vec(); sealed.extend(backend.protect(clear.as_bytes(), true)?);
    if sealed.len() > 32768 { bail!("protected Tweet credential container exceeds32 KiB"); }
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    // A fresh create_new temporary in the same private directory never
    // truncates an existing token file if encryption or writing fails.
    let temporary = path.with_extension(format!("{}.tmp", platform::nonce()?));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new().create_new(true).write(true).open(&temporary)?;
        file.write_all(&sealed)?; file.sync_all()?; drop(file);
        platform::replace_file(&temporary, path)
    })();
    if result.is_err() { let _ = std::fs::remove_file(temporary); }
    result
}

fn invalidate(path: &std::path::Path, diagnostics: &mut Vec<String>) {
    // Original deletes its file. Games are read-only here: suppress subsequent
    // reimports in this private identity instead of deleting the original file.
    if let Err(error) = (|| -> Result<()> {
        use std::io::Write;
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
        let mut file = std::fs::File::create(path.with_extension("legacy-disabled"))?;
        file.write_all(b"disabled\n")?; file.sync_all()?; Ok(())
    })() { diagnostics.push(format!("legacy credential suppression failed: {error:#}")); }
    match std::fs::remove_file(path) {
        Ok(()) => {}, Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(error) => diagnostics.push(format!("private credential invalidation failed: {error}")),
    }
}

fn read_legacy(request: &Request, cache: &std::path::Path) -> Result<Option<(String, String)>> {
    use std::io::Read;
    if cache.with_extension("legacy-disabled").try_exists()? { return Ok(None); }
    let Some((path, key)) = &request.legacy else { return Ok(None); };
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file, Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new(); file.by_ref().take(32769).read_to_end(&mut bytes)?;
    let decoded = cs2_formats::tweet_credentials::TweetCredentials::parse(&bytes, key)?;
    let token = String::from_utf8(decoded.token)?; let secret = String::from_utf8(decoded.secret)?;
    if token.bytes().chain(secret.bytes()).any(|b| b <= 0x20 || b >= 0x7f) {
        bail!("legacy Tweet credentials/key are invalid for an OAuth token");
    }
    Ok(Some((token, secret)))
}

pub(super) fn run(request: Request, status: Arc<AtomicU32>) -> Result<Outcome> {
    if !cfg!(windows) && request.backend.bridge.is_none() {
        bail!("Tweet has no actual authorization/editor/HTTPS consumer on this target");
    }
    let mut diagnostics = Vec::new();
    let cache = cache_path(&request);
    let mut credentials = Credentials { key: request.key.clone(), secret: request.secret.clone(),
        token: String::new(), token_secret: String::new() };
    let backend = &request.backend;
    match read_tokens(&cache, backend) {
        Ok(Some((token, secret))) => { credentials.token = token; credentials.token_secret = secret; }
        Ok(None) => {}, Err(error) => diagnostics.push(format!("private credential read: {error:#}")),
    }
    if credentials.token.is_empty() || credentials.token_secret.is_empty() {
        match read_legacy(&request, &cache) {
            Ok(Some((token, secret))) => { credentials.token = token; credentials.token_secret = secret; }
            Ok(None) => {}, Err(error) => diagnostics.push(format!("legacy credential import: {error:#}")),
        }
    }
    if credentials.token.is_empty() || credentials.token_secret.is_empty() {
        let authorization = (|| -> Result<Option<(String, String)>> {
            let bytes = successful(signed(backend, &credentials, "POST", "https://api.twitter.com/oauth/request_token",
                &[], &[("oauth_callback".into(), "oob".into())], None)?, "request-token")?;
            let fields = wire::unform(&bytes)?;
            credentials.token = wire::value(&fields, "oauth_token")?.to_owned();
            credentials.token_secret = wire::value(&fields, "oauth_token_secret")?.to_owned();
            if fields.iter().any(|(k, v)| k == "oauth_callback_confirmed" && v != "true") {
                bail!("Tweet authorization server did not confirm the callback");
            }
            let url = format!("https://api.twitter.com/oauth/authorize?oauth_token={}", wire::escape(credentials.token.as_bytes()));
            backend.open_url(&url)?;
            let Some(pin) = backend.edit("Twitter authorization", "Authorize in the browser, then enter its PIN.", "", 1023)?
                else { return Ok(None); };
            let bytes = successful(signed(backend, &credentials, "POST", "https://api.twitter.com/oauth/access_token", &[],
                &[("oauth_verifier".into(), pin.trim().to_owned())], None)?, "access-token")?;
            let fields = wire::unform(&bytes)?;
            Ok(Some((access_token_field(&fields, "oauth_token")?, access_token_field(&fields, "oauth_token_secret")?)))
        })();
        match authorization {
            Ok(None) => { status.store(2, Ordering::Release); invalidate(&cache, &mut diagnostics); return Ok(Outcome { diagnostics }); }
            Err(error) => {
                status.store(3, Ordering::Release); invalidate(&cache, &mut diagnostics);
                diagnostics.push(format!("authorization failed: {error:#}")); return Ok(Outcome { diagnostics });
            }
            Ok(Some((token, secret))) => { credentials.token = token; credentials.token_secret = secret; }
        }
    }
    // Original attempts to persist even reused credentials before checking
    // the account. Host cache format is deliberately private and independent
    // of the original machine-seeded encrypted file; never overwrite it.
    if let Err(error) = write_tokens(&cache, &credentials, backend) { diagnostics.push(format!("private credential write: {error:#}")); }
    if credentials.token.is_empty() || credentials.token_secret.is_empty() {
        status.store(4, Ordering::Release); invalidate(&cache, &mut diagnostics); return Ok(Outcome { diagnostics });
    }
    let verified = signed(backend, &credentials, "GET", "https://api.twitter.com/1.1/account/verify_credentials.json", &[], &[], None)
        .and_then(|response| successful(response, "verify-credentials"))
        .and_then(|bytes| wire::api_reply(&bytes));
    if let Err(error) = verified {
        status.store(7, Ordering::Release); invalidate(&cache, &mut diagnostics);
        diagnostics.push(format!("account verification failed: {error:#}")); return Ok(Outcome { diagnostics });
    }
    let prompt = if request.media.is_empty() { "Edit the text. OK submits this Tweet to Twitter.".to_owned() }
        else { format!("OK submits this Tweet with {} image(s): {}", request.media.len(),
            request.media.iter().map(|m| m.name.as_str()).collect::<Vec<_>>().join(", ")) };
    let suffix = super::ScriptTextEncoding::Cp932.decode(&request.suffix);
    let mut initial = request.text.clone();
    let mut editor_prompt = prompt.clone();
    let text = loop {
        match backend.edit_tweet(&editor_prompt, &initial, &suffix).context("Tweet editor")? {
            platform::EditorResult::Cancelled => {
                status.store(6, Ordering::Release); return Ok(Outcome { diagnostics });
            }
            platform::EditorResult::Forget => {
                invalidate(&cache, &mut diagnostics);
                status.store(6, Ordering::Release); return Ok(Outcome { diagnostics });
            }
            platform::EditorResult::Edited(text) => {
                let count = super::tweet_host::tweet_character_count(&text, &suffix)?;
                if count <= 140 { break format!("{text}{suffix}"); }
                initial = text;
                editor_prompt = format!("{prompt}\nOriginal Tweet limit exceeded: {count}/140.");
            }
        }
    };
    let submitted = (|| -> Result<()> {
        let mut ids = Vec::new();
        for media in &request.media {
            let bytes = media.bytes.as_ref().map_err(|cause| anyhow::anyhow!("image {}: {cause}", media.name))?;
            let boundary = format!("cs2{}", platform::nonce()?);
            let mut body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"media\"; filename=\"image\"\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
            body.extend_from_slice(bytes); body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
            let content = format!("multipart/form-data; boundary={boundary}");
            let bytes = successful(signed(backend, &credentials, "POST", "https://upload.twitter.com/1.1/media/upload.json",
                &[], &[], Some((&content, &body)))?, "media-upload")?;
            ids.push(wire::media_id(&bytes)?);
        }
        let mut fields = vec![("status".into(), text)];
        if !ids.is_empty() { fields.push(("media_ids".into(), ids.join(","))); }
        let bytes = successful(signed(backend, &credentials, "POST", "https://api.twitter.com/1.1/statuses/update.json", &fields, &[], None)?, "statuses-update")?;
        wire::api_reply(&bytes)?;
        Ok(())
    })();
    if let Err(error) = submitted {
        status.store(5, Ordering::Release);
        diagnostics.push(format!("submission failed (PE later overwrites status5 with1): {error:#}"));
    } else { diagnostics.push("statuses-update accepted by the actual HTTP server".into()); }
    // 590b6d/590c01 write5, then unconditional 590c93 writes1, even when
    // upload/post failed. It is a native completion value, not success proof.
    status.store(1, Ordering::Release);
    Ok(Outcome { diagnostics })
}
