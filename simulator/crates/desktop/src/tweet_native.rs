//! Registered type62 / ordinary1018..1023, independent of Effekseer/Emote.
//! All pointer payloads become owned bytes before task routing or threading.
use std::collections::BTreeMap;
use std::sync::{Arc, atomic::{AtomicU32, Ordering}};
use std::thread::JoinHandle;
use cs2_runtime::{TaskCommand, TaskCommandBuffers, TaskCommandOutcome};
use cs2_script::{NativeReturn, Result, Vm};
use crate::{LoggingHost, ScriptTextEncoding, expand_kcs_format, native_state_error, read_u32, trim_nul};
use crate::native_tasks::OwnedTaskBuffers;
use crate::tweet_worker::{Media, Outcome, Request};

#[derive(Default)]
pub(super) struct TweetStore {
    tasks: BTreeMap<u32, TweetTask>,
    pub(super) bridge: Option<super::tweet_host::Bridge>,
    legacy_key: Option<[u8; 16]>,
}
impl Drop for TweetStore {
    fn drop(&mut self) {
        // Must wake host-bound workers before the task fields join them.
        if let Some(bridge) = &self.bridge { bridge.cancel(); }
    }
}
struct TweetTask {
    config: Option<Vec<u8>>, images: [Vec<u8>; 4], tags: [Vec<u8>; 8], body: Vec<u8>,
    status: Arc<AtomicU32>, started: bool, worker: Option<JoinHandle<Outcome>>,
}
impl Drop for TweetTask {
    fn drop(&mut self) {
        // A player shutdown also owns the worker lifetime. Never detach a
        // pending network/UI operation that still owns this task's request.
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
    }
}
impl TweetStore {
    pub(super) fn enable_host(&mut self) {
        if self.bridge.is_none() { self.bridge = Some(Default::default()); }
    }
    pub(super) fn can_release(&self, id: u32) -> bool {
        self.tasks.get(&id).and_then(|t| t.worker.as_ref()).is_none_or(JoinHandle::is_finished)
    }
    pub(super) fn release(&mut self, id: u32) -> Result<()> {
        if !self.can_release(id) { return Err(native_state_error("Tweet destructor must await its actual worker")); }
        if let Some(mut task) = self.tasks.remove(&id) {
            if let Some(worker) = task.worker.take() {
                worker.join().map_err(|_| native_state_error("Tweet worker panicked during release"))?;
            }
        }
        Ok(())
    }
}

fn receiver(host: &mut LoggingHost<'_>, command: TaskCommand, buffers: &mut OwnedTaskBuffers,
    output: &mut u32, budget: usize) -> Result<Option<TaskCommandOutcome>> {
    host.receive_tweet_command(command, buffers.data1(), output, budget)
}

impl LoggingHost<'_> {
    fn has_registered_tweet_family(&self) -> bool {
        self.data_services.ordinary_native_table.as_ref()
            .is_some_and(cs2_runtime::native_table::OrdinaryNativeTable::has_tweet_family)
    }

    pub(super) fn register_tweet_task(&mut self, id: u32) -> Result<()> {
        if !self.has_registered_tweet_family() {
            return Err(native_state_error("type62 Tweet layout requires its registered receivers"));
        }
        self.data_services.tweets.tasks.insert(id, TweetTask {
            config: None, images: Default::default(), tags: Default::default(), body: Vec::new(),
            status: Arc::new(AtomicU32::new(8)), started: false, worker: None,
        });
        self.data_services.tasks.install_buffer_receiver(62, receiver);
        Ok(())
    }

    pub(super) fn tweet_native(&mut self, index: u32, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        if !self.has_registered_tweet_family() {
            return Err(native_state_error("Tweet ordinary ABI requires its registered receivers"));
        }
        let expected = match index { 1018 => 16, 1019 | 1020 | 1023 => 8, 1021 | 1022 => 4,
            _ => return Err(native_state_error("not a Tweet native")) };
        if args.len() != expected { return Err(native_state_error("Tweet native argument size differs from its fixed wrapper")); }
        let target = read_u32(args, 0)?;
        let command = TaskCommand { target: target as i32, sender: self.current_task_id as i32,
            code: 583 + index - 1018, data: [0; 3] };
        // Do not reread input after a receiver yields. Native1021 really
        // formats arg1 even though its registration consumes just four bytes.
        let mut buffers = if let Some(saved) = self.pending_owned_task_buffers(command) { saved }
        else {
            let mut payload = match index {
                1018 => {
                    let mut record = vec![0; 2048];
                    for (argument, offset) in [(4,0), (8,512)] {
                        let text = expand(vm, read_u32(args, argument)?, 512)?;
                        record[offset..offset + text.len()].copy_from_slice(&text);
                    }
                    let text = expand(vm, read_u32(args, 12)?, 8192)?;
                    let wide = self.expand_tweet_application_path(&text, false)?;
                    if wide.len() >= 512 { return Err(native_state_error("Tweet token path exceeds its original 512 UTF16 units")); }
                    for (chunk, c) in record[1024..].chunks_exact_mut(2).zip(wide) { chunk.copy_from_slice(&c.to_le_bytes()); }
                    Some(record)
                }
                1019 => {
                    let text = expand(vm, read_u32(args, 4)?, 4096)?;
                    let wide = self.expand_tweet_application_path(&text, true)?;
                    if wide.len() >= 2048 { return Err(native_state_error("Tweet image path exceeds its original wide buffer")); }
                    let text = String::from_utf16(&wide).map_err(|_| native_state_error("Tweet image path has invalid UTF16"))?;
                    let (bytes, _, invalid) = encoding_rs::SHIFT_JIS.encode(&text);
                    if invalid { return Err(native_state_error("Tweet image path is not CP932 encodable")); }
                    Some(bytes.into_owned())
                }
                1020 | 1023 => Some(expand(vm, read_u32(args, 4)?, 8192)?),
                1021 => {
                    let original = vm.native_argument_storage(8)?;
                    Some(expand(vm, read_u32(original, 4)?, 8192)?)
                }
                _ => None,
            };
            if index != 1018 { if let Some(bytes) = &mut payload { bytes.push(0); } }
            OwnedTaskBuffers::new(None, payload)?
        };
        let mut output = 0;
        if index == 1022 && (!self.data_services.tweets.tasks.contains_key(&target)
            || !self.data_services.tasks.message_task_state(target)
                .is_some_and(|(kind, deletion, _)| kind == 62 && deletion == 0)) {
            return Err(native_state_error("Tweet status query has no type62 receiver; original output alias remains unresolved"));
        }
        let result = self.route_task_command_buffers(command, &mut buffers, &mut output, 4096)?;
        Ok(if result.is_some_and(|r| !r.done) { NativeReturn::Retry }
            else if index == 1022 { NativeReturn::Value(output) } else { NativeReturn::Continue })
    }

    pub(super) fn receive_tweet_command(&mut self, command: TaskCommand, payload: Option<&[u8]>,
        output: &mut u32, budget: usize) -> Result<Option<TaskCommandOutcome>> {
        if !(583..=588).contains(&command.code) { return Ok(None); }
        let id = command.target as u32;
        if !self.data_services.tweets.tasks.contains_key(&id) { return Ok(None); }
        self.update_tweet_task(id)?;
        if command.code == 586 {
            // Original executes both queries before checking worker+74.
            let mut input = 0;
            let root = TaskCommand { target: 0, sender: id as i32, code: 34, data: [0;3] };
            if self.receive_task_command(root, &mut TaskCommandBuffers::default(), &mut input, budget)?
                .is_some_and(|r| !r.done) { return Ok(Some(TaskCommandOutcome { done: false, propagate: false })); }
            if !self.data_services.tasks.message_task_state(input).is_some_and(|(kind, deleted, _)| kind == 11 && deleted == 0) {
                input = self.input_task.owner_id.unwrap_or(input);
            }
            let show = TaskCommand { target: input as i32, sender: id as i32, code: 150, data: [0,1,0] };
            if self.receive_device_input_buffers(show, &mut Default::default(), &mut 0)?.is_none() {
                if input == 0 && self.input_task.owner_id.is_none() && self.input_task.alive {
                    self.data_services.message_device.show_device_cursor(1);
                    self.input_task.cursor_requested = self.data_services.message_device.device_cursor_visible();
                } else {
                    self.receive_task_command(show, &mut TaskCommandBuffers::default(), &mut 0, budget)?;
                }
            }
            if !self.data_services.tweets.tasks[&id].started { self.start_tweet_worker(id)?; }
        } else {
            let task = self.data_services.tweets.tasks.get_mut(&id).unwrap();
            match command.code {
                583 => if let Some(bytes) = payload {
                    let bytes = bytes.get(..2048).ok_or_else(|| native_state_error("Tweet config requires all 2048 bytes"))?;
                    task.config = Some(bytes.to_vec());
                },
                584 => if let (Some(bytes), Some(slot)) = (payload, task.images.iter_mut().find(|s| s.is_empty())) {
                    let bytes = terminated(bytes)?;
                    if bytes.len() >= 512 { return Err(native_state_error("Tweet image record exceeds512 bytes; original would overflow")); }
                    *slot = bytes.to_vec();
                },
                585 => if let (Some(bytes), Some(slot)) = (payload, task.tags[..4].iter_mut().find(|s| s.is_empty())) {
                    let bytes = terminated(bytes)?;
                    if !bytes.is_empty() { *slot = unescape(bytes, 64)?; }
                },
                587 => *output = task.status.load(Ordering::Acquire),
                588 => if let Some(bytes) = payload {
                    let bytes = terminated(bytes)?;
                    if !bytes.is_empty() { task.body = unescape(bytes, 1024)?; }
                },
                _ => unreachable!(),
            }
        }
        Ok(Some(TaskCommandOutcome { done: true, propagate: false }))
    }

    fn start_tweet_worker(&mut self, id: u32) -> Result<()> {
        let task = &self.data_services.tweets.tasks[&id];
        let config = task.config.as_ref().ok_or_else(|| native_state_error("Tweet start has no initialized2048-byte config; original memory is indeterminate"))?;
        let key = String::from_utf8(terminated(&config[..512])?.to_vec())
            .map_err(|_| native_state_error("Tweet consumer key is not ASCII/UTF8"))?;
        let secret = String::from_utf8(terminated(&config[512..1024])?.to_vec())
            .map_err(|_| native_state_error("Tweet consumer secret is not ASCII/UTF8"))?;
        let cache_name: Vec<u16> = config[1024..].chunks_exact(2).map(|c| u16::from_le_bytes(c.try_into().unwrap())).take_while(|&c| c != 0).collect();
        let mut suffix = Vec::new();
        // The native scans all eight initialized64-byte records, although
        // command585 can append only to the first four.
        for tag in &task.tags { if !tag.is_empty() { suffix.push(b' '); suffix.extend_from_slice(tag); } }
        let mut media = Vec::new();
        for path in task.images.iter().filter(|p| !p.is_empty()) {
            let name = ScriptTextEncoding::Cp932.decode(path);
            let bytes = self.read_tweet_media(&name).map_err(|error| format!("{error:#}"));
            media.push(Media { name, bytes });
        }
        let status = task.status.clone();
        let legacy_key = self.data_services.tweets.legacy_key.or_else(super::tweet_platform::legacy_key);
        let legacy_path = String::from_utf16(&cache_name).map_err(|_| native_state_error("Tweet legacy filename is not valid UTF16"))?;
        let legacy_path = std::path::PathBuf::from(legacy_path.replace('\\', "/"));
        let legacy = if legacy_path.as_os_str().is_empty() { None } else { legacy_key.map(|key| (
            if legacy_path.is_absolute() { legacy_path } else { self.resources.root().join(legacy_path) }, key)) };
        let request = Request { key, secret, cache_name, text: ScriptTextEncoding::Cp932.decode(&task.body), suffix, media, legacy,
            private_root: self.save_root.to_owned(),
            backend: super::tweet_host::Backend { bridge: self.data_services.tweets.bridge.clone(), task: id } };
        status.store(0, Ordering::Release);
        let worker = std::thread::Builder::new().name(format!("cs2-tweet-{id}")).spawn(move || crate::tweet_worker::run(request, status))
            .map_err(|_| native_state_error("Tweet worker could not be created"))?;
        let task = self.data_services.tweets.tasks.get_mut(&id).unwrap();
        task.started = true; task.worker = Some(worker);
        Ok(())
    }

    fn read_tweet_media(&self, name: &str) -> anyhow::Result<Vec<u8>> {
        use std::io::Read;
        let path = std::path::PathBuf::from(name.replace('\\', "/"));
        let bytes = if path.is_absolute() {
            let mut output = Vec::new();
            std::fs::File::open(path)?.take(64 * 1024 * 1024 + 1).read_to_end(&mut output)?; output
        } else { self.resources.read(name)? };
        if bytes.len() > 64 * 1024 * 1024 { anyhow::bail!("Tweet image exceeds64 MiB"); }
        Ok(bytes)
    }

    pub(super) fn update_tweet_task(&mut self, id: u32) -> Result<()> {
        let Some(task) = self.data_services.tweets.tasks.get_mut(&id) else { return Ok(()); };
        if !task.worker.as_ref().is_some_and(JoinHandle::is_finished) { return Ok(()); }
        let worker = task.worker.take().unwrap();
        let outcome = worker.join().map_err(|_| native_state_error("Tweet worker panicked"))?;
        for detail in outcome.diagnostics {
            self.events.push(crate::LogicEvent { frame: self.frame, kind: "tweet-worker".into(), detail: format!("task={id} {detail}") });
        }
        Ok(())
    }

    pub(super) fn dispatch_tweet_message(&mut self, target: u32, packet: [u32;4], queued: bool,
        output: &mut u32, vm: Option<&Vm>) -> Result<Option<TaskCommandOutcome>> {
        if !(583..=588).contains(&packet[1]) { return Ok(None); }
        let command = TaskCommand { target: target as i32, sender: packet[0] as i32,
            code: packet[1], data: [packet[2],packet[3],0] };
        let mut buffers = if let Some(saved) = self.pending_owned_task_buffers(command) { saved } else {
            let payload = if packet[3] == 0 || matches!(packet[1],586|587) { None }
                else {
                    let vm = vm.ok_or_else(|| native_state_error("Tweet raw pointer command requires its caller VM"))?;
                    Some(if packet[1] == 583 { vm.read(packet[3], 2048)? } else {
                        let mut bytes = vm.read_string(packet[3])?; bytes.push(0); bytes
                    })
                };
            OwnedTaskBuffers::new(None, payload)?
        };
        if queued {
            return self.queue_task_command_buffers(command, &buffers, output)
                .map(|found| found.then_some(TaskCommandOutcome { done: true, propagate: false }));
        }
        self.route_task_command_buffers(command, &mut buffers, output, 4096)
    }
}

impl crate::HeadlessPlayer {
    /// Android cannot infer the original Windows machine's serial. Supply it
    /// explicitly for read-only import; None selects the PE's failed-query zero key.
    pub fn set_tweet_legacy_volume_serial(&mut self, serial: Option<u32>) {
        self.data_services.tweets.legacy_key = Some(cs2_formats::tweet_credentials::volume_key(serial));
    }
}

fn expand(vm: &Vm, address: u32, capacity: usize) -> Result<Vec<u8>> {
    let text = expand_kcs_format(vm, &vm.read_string(address)?, 0)?;
    let text = trim_nul(&text);
    if text.len() >= capacity { return Err(native_state_error("Tweet formatted text exceeds its original buffer")); }
    Ok(text.to_vec())
}
fn terminated(bytes: &[u8]) -> Result<&[u8]> {
    bytes.iter().position(|&b| b == 0).map(|end| &bytes[..end])
        .ok_or_else(|| native_state_error("Tweet buffer is not NUL terminated"))
}
fn unescape(bytes: &[u8], capacity: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new(); let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if matches!(b, 0x81..=0x9f | 0xe0..=0xfc) {
            let tail = *bytes.get(i+1).ok_or_else(|| native_state_error("Tweet CP932 lead byte has no trail; original overread"))?;
            out.extend_from_slice(&[b,tail]); i += 2;
        } else {
            if b == b'\\' { i += 1; }
            out.push(*bytes.get(i).ok_or_else(|| native_state_error("Tweet trailing escape would overread"))?); i += 1;
        }
        if out.len() >= capacity { return Err(native_state_error("Tweet decoded text exceeds its original record; original would overflow")); }
    }
    Ok(out)
}
