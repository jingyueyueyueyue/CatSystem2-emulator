//! Android C ABI. A Java worker thread owns each player and all its calls.
//! The small NDK JNI bridge borrows Surface pixels; no original game code is
//! loaded. Failed operations retain their diagnostic and return an error.
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::time::Instant;
use cs2_desktop::{HeadlessPlayer, HostFrameTiming, HostPerfDetails};
use cs2_runtime::{InputEvent, InputKey};
use cs2_runtime::message_object::MessageTextInputEvent;
use cs2_runtime::resource_router::GameLanguage;

#[derive(Default)]
struct Sessions {
    next: u64,
    players: BTreeMap<u64, HeadlessPlayer>,
    displayed_frames: BTreeMap<u64, DisplayedFrame>,
    story_traces: BTreeMap<u64, StoryTrace>,
    error: CString,
    perf_details: CString,
    clipboard: CString,
    clipboard_units: Vec<u16>,
    message_lists: Vec<u8>,
    message_revisions: BTreeMap<u64, u64>,
    scene_tools: Vec<u8>,
    scene_revisions: BTreeMap<u64, u64>,
    system_open: Vec<u8>,
    tweet: Vec<u8>,
    dialog: Vec<u8>,
    window_ui: Vec<u8>,
    stalled_steps: usize,
}

#[derive(Default)]
struct DisplayedFrame {
    size: [u32; 2],
    rgba: Vec<u8>,
    device_revision: Option<u64>,
}

impl DisplayedFrame {
    fn matches(&self, size: [u32; 2], rgba: &[u8]) -> bool {
        self.device_revision.is_none() && self.size == size && self.rgba == rgba
    }

    fn record(&mut self, size: [u32; 2], rgba: &[u8]) {
        self.rgba.clear();
        self.rgba.extend_from_slice(rgba);
        self.size = size;
        self.device_revision = None;
    }

    fn matches_player(&self, player: &HeadlessPlayer) -> bool {
        if let Some(revision) = player.host_device_frame_revision() {
            return self.device_revision == Some(revision);
        }
        self.device_revision.is_none() && self.matches(player.logical_size(), player.frame_rgba())
    }

    fn record_player(&mut self, player: &HeadlessPlayer) {
        if let Some(revision) = player.host_device_frame_revision() {
            self.device_revision = Some(revision);
            self.rgba.clear();
        } else {
            self.record(player.logical_size(), player.frame_rgba());
        }
    }
}

const FRAME_CHANGED: i32 = 1;
const FRAME_UNCHANGED: i32 = 4;

const STORY_HEARTBEAT_FRAMES: u64 = 30;
const STORY_SCENE_LIMIT: usize = 256;
static LAST_STORY_SESSION: Mutex<u64> = Mutex::new(0);

#[derive(Default)]
struct StoryTrace {
    session: u64,
    seq: u64,
    last_frame: u64,
    state: &'static str,
    scene: String,
    group: i32,
    record: i32,
    row: CString,
}

impl StoryTrace {
    fn new() -> Option<Self> {
        let mut last = LAST_STORY_SESSION.lock().ok()?;
        *last = last.checked_add(1)?;
        Some(Self { session: *last, ..Self::default() })
    }

    fn observe(&mut self, snapshot: (u64, &'static str, &str, i32, i32)) -> Option<&CStr> {
        let (frame, state, scene, group, record) = snapshot;
        let state = match state {
            "page" | "choice" | "active" | "ended" => state,
            _ => "unknown",
        };
        let scene = if !scene.is_empty() && scene.len() <= STORY_SCENE_LIMIT
            && scene.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte)) {
            scene
        } else {
            "-"
        };
        if (state, scene, group, record) != (self.state, self.scene.as_str(), self.group, self.record) {
            self.seq = self.seq.checked_add(1)?;
            self.state = state;
            self.scene.clear();
            self.scene.push_str(scene);
            self.group = group;
            self.record = record;
        } else {
            self.last_frame = self.last_frame.min(frame);
            if frame - self.last_frame < STORY_HEARTBEAT_FRAMES { return None; }
        }
        self.last_frame = frame;
        self.row = CString::new(format!(
            "v1 session={} seq={} frame={frame} state={state} scene={scene} group={group} record={record}",
            self.session, self.seq,
        )).ok()?;
        Some(&self.row)
    }
}

struct PendingInput {
    kind: u32,
    a: i32,
    b: i32,
    input_id: u64,
    input_mono_us: u64,
    event_to_ui_ms: i64,
    enqueued: Instant,
}

struct ConsumedInput {
    input_id: u64,
    input_mono_us: u64,
    event_to_ui_ms: i64,
    ui_to_worker_us: u64,
}

#[derive(Default)]
struct InputQueues {
    pending: BTreeMap<u64, VecDeque<PendingInput>>,
    consumed: BTreeMap<u64, VecDeque<ConsumedInput>>,
    photon_pending: BTreeMap<u64, VecDeque<(u64, u64)>>,
}

static INPUT_QUEUES: Mutex<InputQueues> = Mutex::new(InputQueues {
    pending: BTreeMap::new(), consumed: BTreeMap::new(), photon_pending: BTreeMap::new(),
});

#[cfg(target_os = "android")]
fn input_report(message: &str) {
    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(priority: std::ffi::c_int, tag: *const c_char, text: *const c_char) -> std::ffi::c_int;
    }
    if let Ok(message) = CString::new(message) {
        unsafe { __android_log_write(4, c"CS2Perf".as_ptr(), message.as_ptr()); }
    }
}

#[cfg(not(target_os = "android"))]
fn input_report(_message: &str) {}

fn resolve_inputs_without_present(id: u64, reason: &str) {
    let Ok(mut queues) = INPUT_QUEUES.lock() else { return };
    if let Some(inputs) = queues.photon_pending.remove(&id) {
        for (input_id, input) in inputs {
            input_report(&format!("input_photon_unresolved session={id} input_id={input_id} input_mono_us={input} reason={reason}"));
        }
    }
}

thread_local! {
    static SESSIONS: RefCell<Sessions> = RefCell::new(Sessions::default());
}

fn call<T: Copy>(failure: T, operation: impl FnOnce(&mut Sessions) -> Result<T, String>) -> T {
    SESSIONS.with(|sessions| {
        let mut sessions = sessions.borrow_mut();
        match catch_unwind(AssertUnwindSafe(|| operation(&mut sessions))) {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => {
                sessions.error = CString::new(error.replace('\0', " ")).unwrap_or_default();
                failure
            }
            Err(_) => {
                sessions.error = CString::new("Android player operation panicked").unwrap();
                failure
            }
        }
    })
}

unsafe fn path(value: *const c_char) -> Result<String, String> {
    if value.is_null() { return Err("Android path is null".into()); }
    unsafe { CStr::from_ptr(value) }.to_str().map(str::to_owned)
        .map_err(|_| "Android path is not UTF-8".into())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_open(game: *const c_char, saves: *const c_char, story: i32) -> u64 {
    unsafe { open_game(game, saves, story, GameLanguage::Automatic) }
}

unsafe fn open_game(game: *const c_char, saves: *const c_char, story: i32, language: GameLanguage) -> u64 {
    call(0, |sessions| {
        let game = unsafe { path(game)? };
        let saves = unsafe { path(saves)? };
        HeadlessPlayer::enable_host_gpu_raster();
        let mut player = HeadlessPlayer::open_with_game_language(&game, &saves, language)
            .map_err(|error| format!("Open game: {error:#}"))?;
        player.disable_script_evidence_capture();
        player.enable_host_window_requests();
        player.initialize_general_window_host(1).map_err(|error| format!("Window configuration: {error:#}"))?;
        // Java schedules completed logical ticks at 60 Hz; budget retries
        // continue the same tick and do not count as another input update.
        player.set_host_input_update_rate(60);
        player.enable_host_system_open_queue();
        player.enable_host_tweet_queue();
        player.enable_host_dialog_queue();
        if story != 0 {
            player.enable_host_story_trace();
        }
        player.load_startup().map_err(|error| format!("Load startup: {error:#}"))?;
        let id = sessions.next.checked_add(1).ok_or("Android session ids exhausted")?;
        sessions.next = id;
        sessions.players.insert(id, player);
        if story != 0 {
            if let Some(trace) = StoryTrace::new() {
                sessions.story_traces.insert(id, trace);
            }
        }
        Ok(id)
    })
}

/// Reads a device timestamp from the engine thread. Both outputs must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_gpu_timing(sequence: *mut u64, elapsed_ns: *mut u64) -> i32 {
    if sequence.is_null() || elapsed_ns.is_null() { return 0; }
    let Some((next_sequence, ns)) = HeadlessPlayer::latest_host_gpu_timing() else { return 0; };
    unsafe {
        sequence.write(next_sequence);
        elapsed_ns.write(ns);
    }
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_open_profiled_v4(
    game: *const c_char, saves: *const c_char, story: i32, language: i32, timing: *mut HostFrameTiming,
) -> u64 {
    let language = match language {
        1 => GameLanguage::Japanese,
        2 => GameLanguage::Localized,
        _ => GameLanguage::Automatic,
    };
    let open = || unsafe { open_game(game, saves, story, language) };
    if let Some(timing) = unsafe { timing.as_mut() } {
        let (id, sample) = HostFrameTiming::capture(open);
        *timing = sample;
        id
    } else {
        open()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cs2_android_close(id: u64) -> i32 {
    call(-1, |sessions| {
        let mut player = sessions.players.remove(&id).ok_or("Android session is absent")?;
        player.cancel_host_tweet_requests();
        let saved = player.persist_host_configuration();
        sessions.displayed_frames.remove(&id);
        sessions.story_traces.remove(&id);
        sessions.message_revisions.remove(&id);
        sessions.scene_revisions.remove(&id);
        if let Ok(mut queues) = INPUT_QUEUES.lock() {
            queues.pending.remove(&id);
            queues.consumed.remove(&id);
            if let Some(inputs) = queues.photon_pending.remove(&id) {
                for (input_id, input) in inputs {
                    input_report(&format!("input_photon_unresolved session={id} input_id={input_id} input_mono_us={input} reason=close"));
                }
            }
        }
        saved.map(|()| 0).map_err(|error| format!("Save game settings: {error:#}"))
    })
}

/// Changed message-list views as bounded little-endian records:
/// view_count; [task, visible, row_count; [input, name_len, UTF8, text_len, UTF8]].
/// Returned storage stays valid until the next call on this engine thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_message_lists(
    id: u64, output: *mut *const u8, length: *mut usize,
) -> i32 {
    call(-1, |sessions| {
        if output.is_null() || length.is_null() { return Err("Message-list output is null".into()); }
        unsafe { *output = std::ptr::null(); *length = 0; }
        let player = sessions.players.get(&id).ok_or("Android session is absent")?;
        let revision = player.message_list_revision();
        if sessions.message_revisions.get(&id) == Some(&revision) { return Ok(0); }
        let views = player.message_list_views();
        let mut bytes = Vec::new();
        append_message_bytes(&mut bytes, &(views.len() as u32).to_le_bytes())?;
        for view in views {
            for word in [view.task, u32::from(view.visible), view.rows.len() as u32] {
                append_message_bytes(&mut bytes, &word.to_le_bytes())?;
            }
            for row in view.rows {
                append_inspector_row(&mut bytes, row.input,
                    &view.encoding.decode(&row.name), &view.encoding.decode(&row.message))?;
            }
        }
        sessions.message_lists = bytes;
        sessions.message_revisions.insert(id, revision);
        unsafe {
            *output = sessions.message_lists.as_ptr();
            *length = sessions.message_lists.len();
        }
        Ok(1)
    })
}

fn append_message_bytes(bytes: &mut Vec<u8>, value: &[u8]) -> Result<(), String> {
    let length = bytes.len().checked_add(value.len()).filter(|&length| length <= 64 * 1024 * 1024)
        .ok_or("Android message-list snapshot exceeds 64 MiB")?;
    bytes.try_reserve(length - bytes.len()).map_err(|_| "Android message-list allocation failed")?;
    bytes.extend_from_slice(value);
    Ok(())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_scene_tools(
    id: u64, output: *mut *const u8, length: *mut usize,
) -> i32 {
    call(-1, |sessions| {
        if output.is_null() || length.is_null() { return Err("Scene-tool output is null".into()); }
        unsafe { *output = std::ptr::null(); *length = 0; }
        let player = sessions.players.get(&id).ok_or("Android session is absent")?;
        let revision = player.scene_tool_revision();
        if sessions.scene_revisions.get(&id) == Some(&revision) { return Ok(0); }
        let views = player.scene_tool_views();
        let mut bytes = Vec::new();
        append_message_bytes(&mut bytes, &(views.len() as u32).to_le_bytes())?;
        for view in views {
            let row_count = if view.kind == 33 { view.scenes.len() }
                else { view.records.len() + view.variables.len() + view.log.len() + 1 };
            for word in [view.task, view.kind, u32::from(view.visible), row_count as u32] {
                append_message_bytes(&mut bytes, &word.to_le_bytes())?;
            }
            if view.kind == 33 {
                for scene in view.scenes {
                    append_inspector_row(&mut bytes, scene.line, &view.encoding.decode(&scene.group),
                        &format!("{} | {} | {}", view.encoding.decode(&scene.title),
                            view.encoding.decode(&scene.scene), scene.value as i32))?;
                }
            } else {
                for (index, record) in view.records.into_iter().enumerate() {
                    let mut text = view.encoding.decode(&record.name);
                    for offset in (0x108..0x160).step_by(4) {
                        let value = record.word(offset).ok_or("Scene-tool record lost its fixed fields")?;
                        text.push_str(&format!(" [{offset:03x}]={value:08x}"));
                    }
                    append_inspector_row(&mut bytes, index as u32,
                        &view.encoding.decode(&record.label), &text)?;
                }
                for (index, value) in view.variables.into_iter().enumerate() {
                    append_inspector_row(&mut bytes, 1_000_000 + index as u32, "variable",
                        &format!("{index}: {} (0x{value:08x})", value as i32))?;
                }
                for (index, text) in view.log.into_iter().enumerate() {
                    append_inspector_row(&mut bytes, 2_000_000 + index as u32, "log",
                        &view.encoding.decode(&text))?;
                }
                append_inspector_row(&mut bytes, 3_000_000, "stack",
                    &format!("offset={} (0x{:08x})", view.stack_offset, view.stack_offset))?;
            }
        }
        sessions.scene_tools = bytes;
        sessions.scene_revisions.insert(id, revision);
        unsafe { *output = sessions.scene_tools.as_ptr(); *length = sessions.scene_tools.len(); }
        Ok(1)
    })
}

fn append_inspector_row(bytes: &mut Vec<u8>, id: u32, name: &str, text: &str) -> Result<(), String> {
    append_message_bytes(bytes, &id.to_le_bytes())?;
    for value in [name, text] {
        append_message_bytes(bytes, &(value.len() as u32).to_le_bytes())?;
        append_message_bytes(bytes, value.as_bytes())?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn cs2_android_scene_tool_action(
    id: u64, task: u32, operation: u32, parameter: u32, value: u32,
) -> i32 {
    call(-1, |sessions| {
        let player = sessions.players.get_mut(&id).ok_or("Android session is absent")?;
        match operation {
            0 => player.select_scene_list_entry(task, parameter),
            1 => player.set_scene_tool_visible(task, parameter != 0),
            2 => player.set_scene_debug_variable(task, parameter, value),
            _ => return Err("Unknown scene-tool UI action".into()),
        }.map_err(|cause| format!("Scene tool: {cause:#}"))?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_tweet(
    id: u64, output: *mut *const u8, length: *mut usize,
) -> i32 {
    call(-1, |sessions| {
        if output.is_null() || length.is_null() { return Err("Tweet output is null".into()); }
        unsafe { *output = std::ptr::null(); *length = 0; }
        sessions.tweet.fill(0); sessions.tweet.clear();
        let player = sessions.players.get_mut(&id).ok_or("Android session is absent")?;
        let Some(bytes) = player.take_host_tweet_request() else { return Ok(0); };
        sessions.tweet = bytes;
        unsafe { *output = sessions.tweet.as_ptr(); *length = sessions.tweet.len(); }
        Ok(1)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn cs2_android_tweet_cancel(id: u64) -> i32 {
    call(-1, |sessions| {
        sessions.players.get_mut(&id).ok_or("Android session is absent")?.cancel_host_tweet_requests();
        sessions.tweet.fill(0); sessions.tweet.clear();
        Ok(0)
    })
}

/// Stateless text conversion/count for editor feedback, independent of Sessions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_tweet_count(
    body: *const u8, body_length: usize, suffix: *const u8, suffix_length: usize,
) -> i32 {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<i32, String> {
        if body.is_null() || suffix.is_null() || body_length > 4092 || suffix_length > 4092 {
            return Err("Invalid Tweet count input".into());
        }
        let body = std::str::from_utf8(unsafe { std::slice::from_raw_parts(body, body_length) }).map_err(|_| "Invalid UTF8")?;
        let suffix = std::str::from_utf8(unsafe { std::slice::from_raw_parts(suffix, suffix_length) }).map_err(|_| "Invalid UTF8")?;
        let count = cs2_desktop::tweet_character_count(body, suffix).map_err(|cause| cause.to_string())?;
        i32::try_from(count).map_err(|_| "Tweet count overflow".into())
    }));
    match result { Ok(Ok(count)) => count, _ => -1 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_tweet_result(
    id: u64, request: u64, outcome: u32, value: u32, bytes: *const u8, length: usize,
) -> i32 {
    call(-1, |sessions| {
        if length > 1024*1024 || (bytes.is_null() && length != 0) { return Err("Invalid Tweet receipt storage".into()); }
        let bytes = if length == 0 { Vec::new() } else { unsafe { std::slice::from_raw_parts(bytes, length) }.to_vec() };
        sessions.players.get_mut(&id).ok_or("Android session is absent")?
            .complete_host_tweet_request(request, outcome, value, bytes).map_err(|cause| cause.to_string())?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_system_open(
    id: u64, output: *mut *const u8, length: *mut usize,
) -> i32 {
    call(-1, |sessions| {
        if output.is_null() || length.is_null() { return Err("System-open output is null".into()); }
        unsafe { *output = std::ptr::null(); *length = 0; }
        let player = sessions.players.get_mut(&id).ok_or("Android session is absent")?;
        let Some(pending) = player.take_host_system_open_request() else { return Ok(0); };
        let request = pending.request;
        let mut bytes = Vec::new();
        append_message_bytes(&mut bytes, &pending.id.to_le_bytes())?;
        for text in [
            request.encoding.decode(&request.target),
            request.encoding.decode(&request.parameters),
            request.encoding.decode(&request.directory),
            pending.game_root.to_str().ok_or("System-open game root is not UTF-8")?.to_owned(),
        ] {
            append_message_bytes(&mut bytes, &(text.len() as u32).to_le_bytes())?;
            append_message_bytes(&mut bytes, text.as_bytes())?;
        }
        sessions.system_open = bytes;
        unsafe { *output = sessions.system_open.as_ptr(); *length = sessions.system_open.len(); }
        Ok(1)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_system_open_result(
    id: u64, request: u64, error: *const u8, length: usize,
) -> i32 {
    call(-1, |sessions| {
        let result = if error.is_null() { Ok(()) } else {
            if length > 32768 { return Err("System-open diagnostic exceeds 32 KiB".into()); }
            Err(std::str::from_utf8(unsafe { std::slice::from_raw_parts(error, length) })
                .map_err(|_| "System-open diagnostic is not UTF-8")?.to_owned())
        };
        sessions.players.get_mut(&id).ok_or("Android session is absent")?
            .complete_host_system_open(request, result).map_err(|cause| cause.to_string())?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_dialog(
    id: u64, output: *mut *const u8, length: *mut usize,
) -> i32 {
    call(-1, |sessions| {
        if output.is_null() || length.is_null() { return Err("Dialog output is null".into()); }
        unsafe { *output = std::ptr::null(); *length = 0; }
        let player = sessions.players.get_mut(&id).ok_or("Android session is absent")?;
        let Some(request) = player.take_host_dialog_request() else { return Ok(0); };
        let mut bytes = Vec::new();
        append_message_bytes(&mut bytes, &request.id.to_le_bytes())?;
        for text in [request.title, request.text] {
            append_message_bytes(&mut bytes, &(text.len() as u32).to_le_bytes())?;
            append_message_bytes(&mut bytes, text.as_bytes())?;
        }
        sessions.dialog = bytes;
        unsafe { *output = sessions.dialog.as_ptr(); *length = sessions.dialog.len(); }
        Ok(1)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn cs2_android_dialog_result(id: u64, request: u64, shown: i32) -> i32 {
    call(-1, |sessions| {
        sessions.players.get_mut(&id).ok_or("Android session is absent")?
            .complete_host_dialog(request, if shown != 0 { Ok(()) } else {
                Err("Android dialog could not be displayed".into())
            }).map_err(|cause| cause.to_string())?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn cs2_android_message_list_action(
    id: u64, task: u32, operation: u32, value: u32,
) -> i32 {
    call(-1, |sessions| {
        let player = sessions.players.get_mut(&id).ok_or("Android session is absent")?;
        match operation {
            0 => player.select_message_list_input(task, value),
            1 => player.set_message_list_visible(task, value != 0),
            _ => return Err("Unknown message-list UI action".into()),
        }.map_err(|error| format!("Message list: {error:#}"))?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn cs2_android_enqueue_input(
    id: u64, kind: u32, a: i32, b: i32, input_mono_us: u64, event_to_ui_ms: i64, input_id: u64,
) -> i32 {
    if !matches!(kind, 0..=2) { return -1; }
    let Ok(mut queues) = INPUT_QUEUES.lock() else { return -1 };
    let pending = queues.pending.entry(id).or_default();
    if kind == 0 && let Some(last) = pending.back_mut() && last.kind == 0 {
        last.a = a;
        last.b = b;
        last.enqueued = Instant::now();
        return 0;
    }
    if pending.len() >= 4096 {
        return if kind == 0 { 0 } else { -2 };
    }
    pending.push_back(PendingInput {
        kind, a, b, input_id, input_mono_us, event_to_ui_ms, enqueued: Instant::now(),
    });
    0
}

fn drain_pending_input(id: u64, player: &mut HeadlessPlayer) -> Result<(), String> {
    let pending = INPUT_QUEUES.lock().map_err(|_| "Android input mailbox is poisoned")?
        .pending.remove(&id);
    let Some(events) = pending else { return Ok(()) };
    for event in events {
        apply_input(player, event.kind, event.a, event.b)?;
        if event.kind == 1 && event.a == 0 && event.input_mono_us != 0 {
            let mut queues = INPUT_QUEUES.lock().map_err(|_| "Android input mailbox is poisoned")?;
            let consumed = queues.consumed.entry(id).or_default();
            if consumed.len() == 64 { consumed.pop_front(); }
            consumed.push_back(ConsumedInput {
                input_id: event.input_id,
                input_mono_us: event.input_mono_us,
                event_to_ui_ms: event.event_to_ui_ms,
                ui_to_worker_us: event.enqueued.elapsed().as_micros().min(u64::MAX as u128) as u64,
            });
            let pending = queues.photon_pending.entry(id).or_default();
            if pending.len() == 64
                && let Some((input_id, input)) = pending.pop_front()
            {
                input_report(&format!("input_photon_unresolved session={id} input_id={input_id} input_mono_us={input} reason=queue_overflow"));
            }
            pending.push_back((event.input_id, event.input_mono_us));
        }
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_take_consumed_input(
    id: u64, input_id: *mut u64, input_mono_us: *mut u64, event_to_ui_ms: *mut i64, ui_to_worker_us: *mut u64,
) -> i32 {
    if input_id.is_null() || input_mono_us.is_null() || event_to_ui_ms.is_null() || ui_to_worker_us.is_null() { return -1 }
    let Ok(mut queues) = INPUT_QUEUES.lock() else { return -1 };
    let Some(event) = queues.consumed.get_mut(&id).and_then(VecDeque::pop_front) else { return 0 };
    unsafe {
        *input_id = event.input_id;
        *input_mono_us = event.input_mono_us;
        *event_to_ui_ms = event.event_to_ui_ms;
        *ui_to_worker_us = event.ui_to_worker_us;
    }
    1
}

/// The optional story row is borrowed from the session; consume it before the next native call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_step_profiled_v4(
    id: u64, budget: u32, timing: *mut HostFrameTiming, story_row: *mut *const c_char,
    frame: u64, step_call: u64, detail_rows: *mut *const c_char,
) -> i32 {
    let story_row = unsafe { story_row.as_mut() }.map(|output| {
        *output = std::ptr::null();
        output
    });
    let detail_rows = unsafe { detail_rows.as_mut() }.map(|output| {
        *output = std::ptr::null();
        output
    });
    call(-1, |sessions| {
        let player = sessions.players.get_mut(&id).ok_or("Android session is absent")?;
        drain_pending_input(id, player)?;
        let step = if let Some(timing) = unsafe { timing.as_mut() } {
            let mut advance = || HostFrameTiming::capture(|| player.advance_host_frame_with_retries(budget as usize));
            let (result, sample) = if let Some(output) = detail_rows {
                let (step, detail) = HostPerfDetails::capture(frame == 1 || frame % 60 == 0, advance);
                let rows = detail.rows(id, frame, step_call);
                if !rows.is_empty() {
                    sessions.perf_details = CString::new(rows).unwrap_or_default();
                    *output = sessions.perf_details.as_ptr();
                }
                step
            } else {
                advance()
            };
            *timing = sample;
            result
        } else {
            player.advance_host_frame_with_retries(budget as usize)
        };
        let completed = match step {
            Ok(completed) => completed,
            Err(error) => {
                let events = player.events();
                let recent = events[events.len().saturating_sub(8)..].iter()
                    .map(|event| format!("frame={} {} {}", event.frame, event.kind, event.detail))
                    .collect::<Vec<_>>().join("\n");
                return Err(format!("Script: {error:#}\n{recent}"));
            }
        };
        if completed {
            sessions.stalled_steps = 0;
        } else {
            sessions.stalled_steps += 1;
            if sessions.stalled_steps == 128 {
                let events = player.events();
                let recent = events[events.len().saturating_sub(12)..].iter()
                    .map(|event| format!("frame={} {} {}", event.frame, event.kind, event.detail))
                    .collect::<Vec<_>>().join("\n");
                sessions.error = CString::new(format!("{}\n{recent}", player.host_tick_state()).replace('\0', " ")).unwrap_or_default();
                return Ok(3);
            }
        }
        player.limit_host_diagnostics(10_000, 1_000);
        let result = if player.quit_requested() {
            2
        } else if !completed {
            0
        } else if !player.has_unsubmitted_movie_frame().map_err(|cause| cause.to_string())?
            && sessions.displayed_frames.get(&id).is_some_and(|frame| frame.matches_player(player))
        {
            FRAME_UNCHANGED
        } else {
            FRAME_CHANGED
        };
        if result == FRAME_UNCHANGED {
            resolve_inputs_without_present(id, "unchanged");
        } else if result == 2 {
            resolve_inputs_without_present(id, "quit");
        }
        if completed {
            if let (Some(output), Some(trace)) = (story_row, sessions.story_traces.get_mut(&id)) {
                *output = trace.observe(player.host_story_trace()).map_or(std::ptr::null(), CStr::as_ptr);
            }
        }
        Ok(result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn cs2_android_size(id: u64, axis: u32) -> i32 {
    call(-1, |sessions| {
        let player = sessions.players.get(&id).ok_or("Android session is absent")?;
        if axis > 1 { return Err("Android size axis exceeds one".into()); }
        Ok(player.logical_size()[axis as usize] as i32)
    })
}

/// Touch platforms consume game cursor moves as a virtual pointer. The
/// logical input state was already updated by the common input device.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_take_pointer(id: u64, output: *mut i32) -> i32 {
    call(-1, |sessions| {
        if output.is_null() { return Err("Android pointer output is null".into()); }
        let player = sessions.players.get_mut(&id).ok_or("Android session is absent")?;
        let Some(point) = player.take_host_pointer_request() else { return Ok(0); };
        unsafe { std::ptr::copy_nonoverlapping(point.as_ptr(), output, 2); }
        Ok(1)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn cs2_android_take_clipboard(id: u64) -> *const c_char {
    call(std::ptr::null(), |sessions| {
        let player = sessions.players.get_mut(&id).ok_or("Android session is absent")?;
        let Some(text) = player.take_host_clipboard_request() else { return Ok(std::ptr::null()); };
        sessions.clipboard = CString::new(text).map_err(|_| "Clipboard contains a zero byte")?;
        Ok(sessions.clipboard.as_ptr())
    })
}

/// 0=no new request, 1=hidden, 2=visible, -1=session failure.
#[unsafe(no_mangle)]
pub extern "C" fn cs2_android_take_cursor_visibility(id: u64) -> i32 {
    call(-1, |sessions| {
        let player = sessions.players.get_mut(&id).ok_or("Android session is absent")?;
        Ok(match player.take_host_cursor_visibility_request() {
            None => 0, Some(false) => 1, Some(true) => 2,
        })
    })
}

/// Shared General policy: bit0 inhibits screen power-off, bit1 enables camouflage.
#[unsafe(no_mangle)]
pub extern "C" fn cs2_android_general_window_policy(id: u64) -> i32 {
    call(-1, |sessions| {
        let player = sessions.players.get(&id).ok_or("Android session is absent")?;
        Ok(player.host_general_window_policy() as i32)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_general_window_caption(
    id: u64, output: *mut *const u8, length: *mut usize,
) -> i32 {
    call(-1, |sessions| {
        if output.is_null() || length.is_null() { return Err("General caption output is null".into()); }
        let player = sessions.players.get_mut(&id).ok_or("Android session is absent")?;
        sessions.window_ui = player.host_general_camouflage_title()
            .map_err(|error| format!("General camouflage title: {error:#}"))?.into_bytes();
        unsafe { *output = sessions.window_ui.as_ptr(); *length = sessions.window_ui.len(); }
        Ok(1)
    })
}

/// 0=no command, 1=toggle fullscreen, 2=minimize, -1=session failure.
#[unsafe(no_mangle)]
pub extern "C" fn cs2_android_take_window_request(id: u64) -> i32 {
    call(-1, |sessions| {
        let player = sessions.players.get_mut(&id).ok_or("Android session is absent")?;
        Ok(match player.take_host_window_request() {
            None => 0,
            Some(cs2_desktop::HostWindowRequest::ToggleFullscreen) => 1,
            Some(cs2_desktop::HostWindowRequest::Minimize) => 2,
            Some(cs2_desktop::HostWindowRequest::Settings) => 3,
            Some(cs2_desktop::HostWindowRequest::Menu) => 4,
            Some(cs2_desktop::HostWindowRequest::MenuConfirmation) => 5,
            Some(cs2_desktop::HostWindowRequest::SystemVolume) => 6,
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn cs2_android_window_mode(id: u64, windowed: i32) -> i32 {
    call(-1, |sessions| {
        let player = sessions.players.get_mut(&id).ok_or("Android session is absent")?;
        let mode = match windowed {
            -1 => None, 0 | 1 => Some(windowed as u32),
            _ => return Err("Android window result must be -1, zero or one".into()),
        };
        player.complete_host_window_request(mode).map_err(|error| format!("Window request: {error:#}"))?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_window_ui(
    id: u64, output: *mut *const u8, length: *mut usize,
) -> i32 {
    call(-1, |sessions| {
        if output.is_null() || length.is_null() { return Err("Window UI output is null".into()); }
        unsafe { *output = std::ptr::null(); *length = 0; }
        let player = sessions.players.get_mut(&id).ok_or("Android session is absent")?;
        sessions.window_ui = player.host_window_ui_payload().map_err(|error| format!("Window UI: {error:#}"))?;
        unsafe { *output = sessions.window_ui.as_ptr(); *length = sessions.window_ui.len(); }
        Ok(1)
    })
}

/// 1=close settings, 2=menu installed, 3=menu command, 4=confirmation, 5=OS volume receipt.
#[unsafe(no_mangle)]
pub extern "C" fn cs2_android_window_ui_action(
    id: u64, request: u64, action: u32, a: u32, _b: u32, _c: u32,
) -> i32 {
    call(-1, |sessions| {
        let player = sessions.players.get_mut(&id).ok_or("Android session is absent")?;
        let result = match action {
            1 => player.close_host_effect_settings(request),
            2 => player.complete_host_game_menu(request),
            3 => player.host_game_menu_command(request, a),
            4 => {
                if a > 1 { return Err("Menu confirmation must be Boolean".into()); }
                player.complete_host_menu_confirmation(request, a != 0)
            }
            5 => {
                if a > 1 { return Err("System-volume result must be Boolean".into()); }
                player.complete_host_system_volume(request, if a != 0 { Ok(()) }
                    else { Err("Android Settings.ACTION_SOUND_SETTINGS was rejected by the actual host".into()) })
            }
            _ => return Err("Invalid window UI action".into()),
        };
        result.map_err(|error| format!("Window UI result: {error:#}"))?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_settings_font_preview(
    id: u64, request: u64, face: u32, bold: u32, output: *mut *const u8, length: *mut usize,
) -> i32 {
    call(-1, |sessions| {
        if output.is_null() || length.is_null() { return Err("Font preview output is null".into()); }
        unsafe { *output = std::ptr::null(); *length = 0; }
        let player = sessions.players.get(&id).ok_or("Android session is absent")?;
        sessions.window_ui = player.host_settings_font_preview(request, face, bold)
            .map_err(|error| format!("Font preview: {error:#}"))?;
        unsafe { *output = sessions.window_ui.as_ptr(); *length = sessions.window_ui.len(); }
        Ok(1)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_settings_apply(
    id: u64, request: u64, bytes: *const u8, length: usize,
) -> i32 {
    call(-1, |sessions| {
        if bytes.is_null() || length > 16 * 1024 { return Err("Invalid settings Apply record".into()); }
        let bytes = unsafe { std::slice::from_raw_parts(bytes, length) };
        let player = sessions.players.get_mut(&id).ok_or("Android session is absent")?;
        player.apply_host_settings_payload(request, bytes).map_err(|error| format!("Settings Apply: {error:#}"))?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_take_clipboard_wide(
    id: u64, output: *mut *const u16, length: *mut usize,
) -> i32 {
    call(-1, |sessions| {
        if output.is_null() || length.is_null() { return Err("Clipboard output is null".into()); }
        unsafe { *output = std::ptr::null(); *length = 0; }
        let player = sessions.players.get_mut(&id).ok_or("Android session is absent")?;
        let Some(units) = player.take_host_clipboard_units() else { return Ok(0); };
        sessions.clipboard_units = units;
        unsafe { *output = sessions.clipboard_units.as_ptr(); *length = sessions.clipboard_units.len(); }
        Ok(1)
    })
}

fn apply_input(player: &mut HeadlessPlayer, kind: u32, a: i32, b: i32) -> Result<(), String> {
    let event = match kind {
        0 => InputEvent::MouseMove { x: a, y: b },
        1 | 2 => {
            let key = InputKey::MouseButton(u8::try_from(a).map_err(|_| "Invalid pointer button")?);
            if kind == 1 { InputEvent::KeyDown(key) } else { InputEvent::KeyUp(key) }
        }
        3 | 4 => {
            player.apply_host_text_key(a as u32, kind == 3)
                .map_err(|error| format!("Text key: {error:#}"))?;
            return Ok(());
        }
        5 => {
            player.set_host_paused(a == 0).map_err(|error| format!("Pause host: {error:#}"))?;
            player.apply_host_text_input(MessageTextInputEvent::Focus(a != 0))
                .map_err(|error| format!("Input focus: {error:#}"))?;
            return Ok(());
        }
        6 => InputEvent::KeyDown(InputKey::MouseWheel(a)),
        7 => InputEvent::QuitRequested,
        8 => {
            player.set_audio_suspended(a != 0).map_err(|error| format!("Audio focus: {error:#}"))?;
            return Ok(());
        }
        9 | 10 => {
            player.apply_host_text_input(if kind == 9 {
                MessageTextInputEvent::ImeEnabled(a != 0)
            } else { MessageTextInputEvent::Cancel })
                .map_err(|error| format!("IME mode: {error:#}"))?;
            return Ok(());
        }
        11 => {
            player.apply_host_text_input(MessageTextInputEvent::Focus(a != 0))
                .map_err(|error| format!("Window focus: {error:#}"))?;
            return Ok(());
        }
        _ => return Err("Unknown Android input kind".into()),
    };
    player.apply_host_input(event).map_err(|error| format!("Input: {error:#}"))
}

#[unsafe(no_mangle)]
pub extern "C" fn cs2_android_input(id: u64, kind: u32, a: i32, b: i32) -> i32 {
    if kind == 5 && a == 0 && let Ok(mut queues) = INPUT_QUEUES.lock() {
        queues.pending.remove(&id);
        queues.consumed.remove(&id);
        if let Some(inputs) = queues.photon_pending.remove(&id) {
            for (input_id, input) in inputs {
                input_report(&format!("input_photon_unresolved session={id} input_id={input_id} input_mono_us={input} reason=pause"));
            }
        }
    }
    call(-1, |sessions| {
        let player = sessions.players.get_mut(&id).ok_or("Android session is absent")?;
        apply_input(player, kind, a, b)?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_text(
    id: u64, kind: u32, bytes: *const u8, length: usize, start: i32, end: i32,
) -> i32 {
    call(-1, |sessions| {
        if bytes.is_null() || length > 1024 * 1024 { return Err("Android text is absent or exceeds 1 MiB".into()); }
        let text = std::str::from_utf8(unsafe { std::slice::from_raw_parts(bytes, length) })
            .map_err(|_| "Android text is not UTF-8")?.to_owned();
        let player = sessions.players.get_mut(&id).ok_or("Android session is absent")?;
        let event = match kind {
            0 => MessageTextInputEvent::Commit(text),
            1 => MessageTextInputEvent::Preedit {
                text, selection: if start < 0 || end < 0 { None } else { Some([start as u32, end as u32]) },
            },
            2 => MessageTextInputEvent::HardwareText(text),
            _ => return Err("Unknown Android text kind".into()),
        };
        player.apply_host_text_input(event).map_err(|error| format!("Text input: {error:#}"))?;
        Ok(0)
    })
}

/// Caller owns a locked RGBA_8888 ANativeWindow buffer. The stride is in
/// pixels, as specified by ANativeWindow_Buffer, and may exceed the width.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_copy_frame(
    id: u64, buffer: *mut u8, width: i32, height: i32, stride: i32,
) -> i32 {
    call(-1, |sessions| {
        let player = sessions.players.get(&id).ok_or("Android session is absent")?;
        let [logical_width, logical_height] = player.logical_size();
        if buffer.is_null() || width != logical_width as i32 || height != logical_height as i32 || stride < width {
            return Err("Android surface buffer does not match the logical frame".into());
        }
        let stride_bytes = (stride as usize).checked_mul(4).ok_or("Android surface stride overflows")?;
        let length = stride_bytes.checked_mul(height as usize).filter(|&n| n <= 512 * 1024 * 1024)
            .ok_or("Android surface exceeds its size limit")?;
        let destination = unsafe { std::slice::from_raw_parts_mut(buffer, length) };
        let pixels = player.frame_rgba();
        let row_bytes = logical_width as usize * 4;
        for row in 0..logical_height as usize {
            destination[row * stride_bytes..row * stride_bytes + row_bytes]
                .copy_from_slice(&pixels[row * row_bytes..(row + 1) * row_bytes]);
        }
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn cs2_android_frame_posted(id: u64) -> i32 {
    call(-1, |sessions| {
        let player = sessions.players.get(&id).ok_or("Android session is absent")?;
        player.acknowledge_movie_frame().map_err(|cause| cause.to_string())?;
        sessions.displayed_frames.entry(id).or_default().record_player(player);
        Ok(0)
    })
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "C" fn cs2_android_vulkan_candidate(window: *mut std::ffi::c_void) -> i32 {
    i32::from(std::ptr::NonNull::new(window).is_some_and(HeadlessPlayer::android_surface_candidate))
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_present_vulkan(id: u64, window: *mut std::ffi::c_void) -> i32 {
    let mut inputs = INPUT_QUEUES.lock().ok()
        .and_then(|mut queues| queues.photon_pending.remove(&id))
        .map(Vec::from).unwrap_or_default();
    let result = call(-1, |sessions| {
        let player = sessions.players.get(&id).ok_or("Android session is absent")?;
        let Some(window) = std::ptr::NonNull::new(window) else { return Ok(-2) };
        Ok(unsafe { player.present_android_surface(window, id, &inputs) })
    });
    if result == -2 && !inputs.is_empty() {
        if let Ok(mut queues) = INPUT_QUEUES.lock() {
            inputs.extend(queues.photon_pending.remove(&id).into_iter().flatten());
            queues.photon_pending.insert(id, VecDeque::from(inputs));
        }
    } else if result != 0 {
        for (input_id, input) in inputs {
            input_report(&format!("input_photon_unresolved session={id} input_id={input_id} input_mono_us={input} reason=vulkan_unavailable"));
        }
    }
    result
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "C" fn cs2_android_detach_vulkan() {
    HeadlessPlayer::detach_android_surface();
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "C" fn cs2_android_vulkan_display_stats_enable(enabled: i32) {
    HeadlessPlayer::set_android_vulkan_display_stats(enabled != 0);
}

/// The output must accommodate seven consecutive u64 values.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cs2_android_vulkan_display_stats(output: *mut u64) -> i32 {
    if output.is_null() { return 0; }
    let Some(stats) = HeadlessPlayer::android_vulkan_display_stats() else { return 0; };
    unsafe { std::ptr::copy_nonoverlapping(stats.as_ptr(), output, stats.len()) };
    1
}

/// Valid until the next failing operation on this same worker thread.
#[unsafe(no_mangle)]
pub extern "C" fn cs2_android_last_error() -> *const c_char {
    SESSIONS.with(|sessions| sessions.borrow().error.as_ptr())
}

#[cfg(test)]
mod frame_tests {
    use super::DisplayedFrame;

    #[test]
    fn suppresses_only_identical_frames_at_the_same_dimensions() {
        let pixels = [0, 1, 2, 255, 3, 4, 5, 255];
        let mut frame = DisplayedFrame::default();
        assert!(!frame.matches([2, 1], &pixels));
        frame.record([2, 1], &pixels);
        assert!(frame.matches([2, 1], &pixels));
        assert!(!frame.matches([1, 2], &pixels));
        let mut changed = pixels;
        changed[7] = 254;
        assert!(!frame.matches([2, 1], &changed));
        frame.record([2, 1], &changed);
        assert!(frame.matches([2, 1], &changed));
    }
}

#[cfg(test)]
mod story_trace_tests {
    use super::*;

    #[test]
    fn sequences_transitions_and_repeats_only_on_heartbeat() {
        let mut trace = StoryTrace::new().unwrap();
        let session = trace.session;
        let expected = format!("v1 session={session} seq=1 frame=5 state=page scene=Scene_1.cst-2 group=-1 record=9");
        assert_eq!(trace.observe((5, "page", "Scene_1.cst-2", -1, 9)).unwrap().to_str().unwrap(), expected);
        assert!(trace.observe((5, "page", "Scene_1.cst-2", -1, 9)).is_none());
        assert!(trace.observe((34, "page", "Scene_1.cst-2", -1, 9)).is_none());
        assert_eq!(trace.observe((35, "page", "Scene_1.cst-2", -1, 9)).unwrap().to_str().unwrap(),
            expected.replace("frame=5", "frame=35"));
        for (index, snapshot) in [
            (36, "choice", "Scene_1.cst-2", -1, 9),
            (37, "choice", "next", -1, 9),
            (38, "choice", "next", 2, 9),
            (39, "choice", "next", 2, 10),
            (40, "ended", "next", 2, 10),
        ].into_iter().enumerate() {
            let (frame, state, scene, group, record) = snapshot;
            let seq = index + 2;
            assert_eq!(trace.observe(snapshot).unwrap().to_str().unwrap(),
                format!("v1 session={session} seq={seq} frame={frame} state={state} scene={scene} group={group} record={record}"));
        }
        assert!(trace.observe((0, "ended", "next", 2, 10)).is_none());
        assert!(trace.observe((29, "ended", "next", 2, 10)).is_none());
        assert!(trace.observe((30, "ended", "next", 2, 10)).unwrap().to_str().unwrap().contains(" seq=6 "));
    }

    #[test]
    fn resets_sequence_per_session_and_redacts_invalid_fields() {
        let mut first = StoryTrace::new().unwrap();
        let mut second = StoryTrace::new().unwrap();
        assert!(first.session > 0 && second.session > first.session);
        let too_long = "a".repeat(STORY_SCENE_LIMIT + 1);
        for (index, scene) in ["", "line of dialogue", "dir/scene", "dir\\scene", "a\nforged", "\0", "\u{00e9}", &too_long].into_iter().enumerate() {
            let frame = index as u64;
            let session = first.session;
            let record = index as i32;
            let seq = index + 1;
            assert_eq!(first.observe((frame, "bad state", scene, i32::MIN, record)).unwrap().to_str().unwrap(),
                format!("v1 session={session} seq={seq} frame={frame} state=unknown scene=- group=-2147483648 record={record}"));
        }
        let session = second.session;
        let frame = u64::MAX;
        let scene = "Z".repeat(STORY_SCENE_LIMIT);
        assert_eq!(second.observe((frame, "active", &scene, i32::MAX, i32::MIN)).unwrap().to_str().unwrap(),
            format!("v1 session={session} seq=1 frame={frame} state=active scene={scene} group=2147483647 record=-2147483648"));
    }
}
