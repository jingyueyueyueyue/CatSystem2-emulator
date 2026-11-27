use anyhow::{Context, Result, ensure};
use cs2_desktop::{HeadlessPlayer, HostWindowRequest};
use cs2_runtime::message_object::ButtonSource;
use cs2_runtime::resource_router::GameLanguage;
use cs2_runtime::{InputEvent, InputKey};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

mod story_route;
#[path = "story_route/button_probe.rs"]
mod button_probe;
#[path = "story_route/happiness.rs"]
mod happiness;
#[path = "story_route/yukikoi_save.rs"]
mod yukikoi_save;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Pointer { x: i32, y: i32, button: Option<u8> },
    Key(InputKey),
    Menu(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HitGuard {
    Region(u32),
    Priority(i32),
}

impl HitGuard {
    fn matches(self, region: [u32; 15]) -> bool {
        region[8] != 0 && match self {
            Self::Region(id) => region[0] == id,
            Self::Priority(priority) => region[1] as i32 == priority,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum LiveInput {
    Key(u32),
    Click { region: u32, point: [i32; 2] },
    Inspect,
    Ctrl(bool),
}

impl LiveInput {
    fn parse(line: &str) -> Result<Self> {
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        match fields.as_slice() {
            ["key", code] => Ok(Self::Key(code.parse()?)),
            ["click", region, x, y] => Ok(Self::Click {
                region: region.parse()?,
                point: [x.parse()?, y.parse()?],
            }),
            ["inspect"] => Ok(Self::Inspect),
            ["ctrl", "up"] => Ok(Self::Ctrl(false)),
            ["ctrl", "down"] => Ok(Self::Ctrl(true)),
            _ => anyhow::bail!("invalid live input {line:?}"),
        }
    }
}

const FRAME_RETRY_LIMIT: u64 = 128;
const ADVANCING_VM_RETRY_LIMIT: u64 = 4096;
const FRAME_RETRY_TIME: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Default)]
struct OrdinaryExecutionProgress {
    previous: Vec<(u32, usize, u64)>,
    last_change: u64,
}

impl OrdinaryExecutionProgress {
    fn stalled(
        &mut self,
        retries: u64,
        elapsed: std::time::Duration,
        states: impl Iterator<Item = (u32, usize, u64)>,
    ) -> bool {
        let current: Vec<_> = states.collect();
        if current != self.previous {
            self.previous = current;
            self.last_change = retries;
        }
        retries >= ADVANCING_VM_RETRY_LIMIT
            || (retries >= FRAME_RETRY_LIMIT
                && (self.previous.is_empty() || retries - self.last_change >= FRAME_RETRY_LIMIT
                    || elapsed >= FRAME_RETRY_TIME))
    }
}

fn main() -> Result<()> {
    let mut error_status = "scenario_error";
    let result = replay_command(&mut error_status);
    #[cfg(windows)]
    if error_status == "engine_error" {
        HeadlessPlayer::disable_host_gpu_raster();
    }
    if let Err(cause) = &result {
        eprintln!("replay error: {cause:#}");
        println!("replay_status={error_status}");
    }
    result
}

fn replay_command(error_status: &mut &'static str) -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let first = args.next().context("game directory required")?;
    if first == "--source-sha256" {
        ensure!(args.next().is_none(), "--source-sha256 takes no arguments");
        println!("{}", option_env!("CS2_REPLAY_SOURCE_SHA256").unwrap_or("unrecorded"));
        return Ok(());
    }
    let game = PathBuf::from(first);
    let output = PathBuf::from(args.next().context("output prefix required")?);
    let frames: u64 = args
        .next()
        .context("frame limit required")?
        .to_string_lossy()
        .parse()?;
    ensure!(frames > 0, "frame limit must be positive");
    let mut args = args.peekable();
    let (mut route, flags) = if args.peek().is_some_and(|arg| arg == "--route-json") {
        args.next();
        let path = PathBuf::from(args.next().context("--route-json requires a path")?);
        let flags: Vec<_> = args.collect();
        ensure!(flags.iter().all(|flag| flag.to_str().is_some_and(is_route_diagnostic)),
            "route mode accepts only trace-native and inspect-log diagnostics");
        let controller = story_route::Controller::read(&path)?;
        (Some(controller), flags)
    } else {
        (None, args.collect())
    };
    replay(game, &output, frames, flags.into_iter(), route.as_mut(), error_status)
}

fn is_route_diagnostic(flag: &str) -> bool {
    flag.strip_prefix("trace-native:").is_some_and(|index| index.parse::<u32>().is_ok())
        || flag.strip_prefix("inspect-log:").is_some_and(|frame| frame.parse::<u64>().is_ok_and(|frame| frame > 0))
}

fn replay(
    game: PathBuf,
    output: &Path,
    frames: u64,
    args: impl Iterator<Item = std::ffi::OsString>,
    mut route: Option<&mut story_route::Controller>,
    error_status: &mut &'static str,
) -> Result<()> {
    let state_mode = route.is_some();
    if let Some(route) = &route {
        println!("replay route={:?}", route.label());
    }
    let mut inputs = BTreeMap::new();
    let mut ctrl_windows = BTreeMap::new();
    let mut required_hits = BTreeMap::<u64, HitGuard>::new();
    let mut same_frame_taps = BTreeSet::new();
    let mut captures = BTreeSet::new();
    let mut probes = BTreeSet::new();
    let mut log_probes = BTreeSet::new();
    let mut traced_natives = BTreeSet::new();
    let mut capture_every = if state_mode { 0 } else { 60 };
    let mut capture_final = !state_mode;
    let mut story_events = state_mode;
    let mut story_trace = state_mode;
    let mut session_random = false;
    let mut neko_film = None;
    let mut neko_clear_title = None;
    let mut guarded_body: Option<(&[(i32, i32)], u32)> = None;
    let mut happiness_produce_save = false;
    let mut happiness_resume_sha = None;
    let mut yukikoi_produce_save = false;
    let mut labyrinth15_observe_after_r19 = false;
    let mut labyrinth15_cross_window_successor = false;
    let mut button_probe = button_probe::Probe::default();
    let mut eye08_title = None;
    let mut eden_entry = None;
    let mut ready_entry = button_probe::ReadyEntry::default();
    let mut ctrl_after_ready = false;
    let mut auto_first_choice = false;
    let mut auto_choice_banks = std::collections::VecDeque::new();
    let mut live_input = false;
    let mut campanella_choice = false;
    let mut fes_choices = std::collections::VecDeque::new();
    for arg in args {
        let values = arg.to_string_lossy();
        if values == "live-input" {
            live_input = true;
            continue;
        }
        if values == "auto-first-choice" {
            auto_first_choice = true;
            continue;
        }
        if let Some(banks) = values.strip_prefix("auto-choice-banks:") {
            ensure!(auto_choice_banks.is_empty(), "duplicate auto-choice-banks");
            auto_choice_banks = banks.split(',').map(str::parse::<u32>)
                .collect::<std::result::Result<_, _>>().context("expected comma-separated choice banks")?;
            auto_first_choice = true;
            continue;
        }
        if values == "campanella-first-choice" {
            ensure!(!campanella_choice, "duplicate 18 choice entry");
            campanella_choice = true;
            continue;
        }
        if let Some(click) = values.strip_prefix("ready-click:") {
            ready_entry.add(click)?;
            continue;
        }
        if values == "ctrl-after-ready" {
            ensure!(!ctrl_after_ready, "duplicate ctrl-after-ready");
            ctrl_after_ready = true;
            continue;
        }
        if let Some(choice) = values.strip_prefix("fes-choice:") {
            let mut fields = choice.split(':');
            let scene = fields.next().filter(|value| !value.is_empty()).context("expected fes-choice:scene:group:target")?;
            let group = fields.next().context("expected choice group")?.parse::<usize>()?;
            let target = fields.next().filter(|value| !value.is_empty()).context("expected choice target")?;
            ensure!(fields.next().is_none(), "expected fes-choice:scene:group:target");
            fes_choices.push_back((scene.to_owned(), group, target.to_owned()));
            continue;
        }
        if values == "eden-live-entry" {
            ensure!(eden_entry.is_none(), "duplicate 13 live entry");
            eden_entry = Some(button_probe::EdenEntry::default());
            continue;
        }
        if let Some(button) = values.strip_prefix("eye08-title:") {
            ensure!(eye08_title.is_none(), "duplicate 08 title button");
            eye08_title = Some(button_probe::Eye08Title::new(button)?);
            button_probe.enabled = true;
            story_trace = true;
            continue;
        }
        if values == "button-probe" {
            button_probe.enabled = true;
            continue;
        }
        if values == "settings-apply" {
            button_probe.apply_settings = true;
            continue;
        }
        if let Some(value) = values.strip_prefix("text-entry:") {
            button_probe.enabled = true;
            button_probe.text_entries.push_back(Some(value.to_owned()));
            continue;
        }
        if values == "text-entry-cancel" {
            button_probe.enabled = true;
            button_probe.text_entries.push_back(None);
            continue;
        }
        if let Some(frame) = values.strip_prefix("tap:") {
            let frame = frame.parse::<u64>()?;
            ensure!(same_frame_taps.insert(frame), "duplicate tap at frame {frame}");
            continue;
        }
        if let Some(action) = values.strip_prefix("menu:") {
            let (frame, command) = action.split_once(':').context("expected menu:frame:command")?;
            let frame = frame.parse::<u64>()?;
            ensure!(frame < frames, "menu action frame {frame} outside run");
            ensure!(inputs.insert(frame, Action::Menu(command.parse()?)).is_none(),
                "duplicate input at frame {frame}");
            continue;
        }
        if values == "happiness-save-slot-zero" {
            ensure!(!happiness_produce_save, "duplicate Happiness Save production control");
            happiness_produce_save = true;
            continue;
        }
        if let Some(digest) = values.strip_prefix("happiness-load-slot-zero:") {
            ensure!(happiness_resume_sha.is_none() && digest.len() == 64
                && digest.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                "Happiness cold Load requires one ordinary slot SHA-256");
            happiness_resume_sha = Some(digest.to_owned());
            continue;
        }
        if values == "yukikoi-save-slot-zero" {
            ensure!(!yukikoi_produce_save, "duplicate Yukikoi Save production control");
            yukikoi_produce_save = true;
            continue;
        }
        let legacy_fes_waits = match values.as_ref() {
            "legacy-02-enter" => Some((&[(0, 101), (1, 123)][..], 0xe0d)),
            "legacy-04-enter" => Some((&[(0, 54), (1, 67)][..], 0xea8)),
            "legacy-05-enter" => Some((&[(0, 49), (1, 54)][..], 0x929)),
            "legacy-06-enter" => Some((&[(0, 65), (1, 107)][..], 0xc72)),
            "legacy-16-enter" => Some((&[(0, 16), (1, 24)][..], 0xd0c)),
            "legacy-19-enter" => Some((&[(0, 8), (1, 10)][..], 0x918)),
            "legacy-02-continue" => Some((&[(0, 101), (1, 123), (2, 125)][..], 0xe0d)),
            "legacy-06-continue" => Some((&[(0, 65), (1, 107), (2, 110)][..], 0xc72)),
            "legacy-16-continue" => Some((&[(0, 16), (1, 24), (2, 27)][..], 0xd0c)),
            "legacy-19-continue" => Some((&[(0, 8), (1, 10), (2, 12)][..], 0x918)),
            "legacy-02-opening" => Some((&[
                (0, 101), (1, 123), (2, 125), (3, 127), (4, 130), (5, 132), (6, 147),
                (7, 149), (8, 160), (9, 163), (10, 165), (11, 168), (12, 174), (13, 178),
            ][..], 0xe0d)),
            "legacy-02-segment" => Some((&happiness::OPENING_WAITS[..47], 0xe0d)),
            "legacy-02-next-segment" => Some((&happiness::OPENING_WAITS[..58], 0xe0d)),
            "legacy-02-continuous" => Some((happiness::OPENING_WAITS, 0xe0d)),
            "legacy-06-segment" => Some((&[
                (0, 65), (1, 107), (2, 110), (3, 112), (4, 118), (5, 127), (6, 132),
                (7, 135), (8, 137), (9, 140), (10, 147), (0, 47), (1, 49), (2, 51),
                (3, 53), (4, 55),
            ][..], 0xc72)),
            "legacy-16-segment" => Some((&HeadlessPlayer::host_newton_opening_waits()[..57], 0xd0c)),
            "legacy-16-continuous" => Some((HeadlessPlayer::host_newton_opening_waits(), 0xd0c)),
            "legacy-19-segment" => Some((&[
                (0, 8), (1, 10), (2, 12), (3, 14), (4, 30), (5, 42), (6, 50),
                (7, 52), (8, 55), (9, 58), (10, 67), (11, 74), (12, 76), (13, 78),
                (14, 80), (15, 82), (16, 91), (17, 95), (18, 98), (19, 110), (20, 117),
                (21, 120), (22, 127), (23, 130), (24, 145), (25, 152), (26, 171), (27, 179),
                (28, 182), (29, 184), (30, 186), (31, 188), (32, 190), (33, 193), (34, 200),
                (35, 203),
            ][..], 0x918)),
            "legacy-04-segment" => Some((&[
                (0, 54), (1, 67), (2, 70), (3, 73), (4, 85), (5, 87), (6, 99),
                (7, 102), (8, 104), (9, 107), (10, 123), (11, 126), (12, 128), (13, 130),
                (14, 132), (15, 135), (16, 157), (17, 169), (18, 172), (19, 184), (20, 187),
                (21, 199), (22, 202), (23, 219),
            ][..], 0xea8)),
            "legacy-05-segment" => Some((&[
                (0, 49), (1, 54), (2, 58), (3, 62), (4, 66), (5, 94), (6, 98),
                (7, 107), (8, 110), (9, 118), (10, 121), (11, 140), (12, 161), (13, 164),
                (14, 168), (15, 171), (16, 177), (17, 180), (18, 189), (19, 192), (20, 195),
                (21, 218), (22, 220), (23, 222),
            ][..], 0x929)),
            "legacy-04-opening" => Some((&[
                (0, 54), (1, 67), (2, 70), (3, 73), (4, 85), (5, 87), (6, 99),
                (7, 102), (8, 104), (9, 107),
            ][..], 0xea8)),
            "legacy-05-opening" => Some((&[
                (0, 49), (1, 54), (2, 58), (3, 62), (4, 66),
            ][..], 0x929)),
            "legacy-06-opening" => Some((&[
                (0, 65), (1, 107), (2, 110), (3, 112), (4, 118), (5, 127), (6, 132),
                (7, 135), (8, 137), (9, 140),
            ][..], 0xc72)),
            "legacy-16-opening" => Some((&[
                (0, 16), (1, 24), (2, 27), (3, 40), (4, 42), (5, 44), (6, 47),
                (7, 49), (8, 54), (9, 57),
            ][..], 0xd0c)),
            "legacy-19-opening" => Some((&[
                (0, 8), (1, 10), (2, 12), (3, 14), (4, 30), (5, 42), (6, 50),
                (7, 52), (8, 55), (9, 58),
            ][..], 0x918)),
            _ => None,
        };
        if let Some(waits) = legacy_fes_waits {
            ensure!(guarded_body.replace(waits).is_none(), "duplicate legacy body action");
            continue;
        }
        if matches!(values.as_ref(), "labyrinth15-next" | "labyrinth15-opening"
            | "labyrinth15-r19-enter-observe" | "labyrinth15-cross-window-successor")
        {
            labyrinth15_observe_after_r19 = values == "labyrinth15-r19-enter-observe";
            labyrinth15_cross_window_successor = values == "labyrinth15-cross-window-successor";
            let points = &[(0, 7), (1, 9), (2, 11), (3, 13), (4, 15), (5, 17), (6, 19), (7, 38), (8, 41)][..];
            let waits = match values.as_ref() {
                "labyrinth15-next" => &points[..2],
                "labyrinth15-cross-window-successor" => points,
                _ => &points[..7],
            };
            ensure!(guarded_body.replace((waits, 0xa04)).is_none(), "duplicate legacy body action");
            continue;
        }
        if values == "djibril10-op01-next" {
            let waits = &[(0, 4), (1, 15)][..];
            ensure!(guarded_body.replace((waits, 0x9f6)).is_none(), "duplicate legacy body action");
            continue;
        }
        if values == "legacy-11-radio-stop" || values == "legacy-11-radio-r94-enter" {
            let points = &[(0, 81), (1, 86), (2, 90), (3, 94), (4, 97)][..];
            let waits = if values == "legacy-11-radio-stop" { &points[..4] } else { points };
            ensure!(guarded_body.replace((waits, 0xd6d)).is_none(), "duplicate legacy body action");
            continue;
        }
        if values == "legacy-13-r139" || values == "legacy-13-r188" {
            let segment = &[
                (4, 103), (5, 107), (6, 109), (7, 111), (8, 113),
                (9, 115), (10, 119), (11, 121), (12, 139),
                (13, 151), (14, 163), (15, 174), (16, 186), (17, 188),
            ][..];
            let waits = if values == "legacy-13-r139" { &segment[..9] } else { segment };
            ensure!(guarded_body.replace((waits, 0xad0)).is_none(), "duplicate legacy body action");
            continue;
        }
        if let Some((sample, continuation)) = values.strip_prefix("legacy-11-13-enter:")
            .map(|sample| (sample, false))
            .or_else(|| values.strip_prefix("legacy-11-13-continue:").map(|sample| (sample, true)))
        {
            let (waits, address): (&[(i32, i32)], u32) = match sample {
                "11" => (&[(0, 81), (1, 86), (2, 90)], 0xd6d),
                "13" => (&[(4, 103), (5, 107), (6, 109), (7, 111), (8, 113), (9, 115), (10, 119), (11, 121)], 0xad0),
                _ => anyhow::bail!("legacy body action requires sample 11 or 13"),
            };
            let waits = if continuation { waits } else { &waits[..2] };
            ensure!(guarded_body.replace((waits, address)).is_none(), "duplicate legacy body action");
            continue;
        }
        if values == "story-events" {
            story_events = true;
            continue;
        }
        if values == "story-trace" {
            story_trace = true;
            continue;
        }
        if values == "session-random" {
            session_random = true;
            continue;
        }
        if let Some(window) = values.strip_prefix("ctrl-hold:") {
            let (start, end) = window.split_once(':').context("expected ctrl-hold:start:end")?;
            let start = start.parse::<u64>().context("invalid Ctrl hold start frame")?;
            let end = end.parse::<u64>().context("invalid Ctrl hold end frame")?;
            ensure!(start < end && end < frames, "Ctrl hold must satisfy start < end < {frames}");
            ensure!(ctrl_windows.insert(start, end).is_none(), "duplicate Ctrl hold at frame {start}");
            continue;
        }
        if values == "no-final-capture" {
            capture_final = false;
            continue;
        }
        if let Some(interval) = values.strip_prefix("capture-every:") {
            capture_every = interval.parse()?;
            continue;
        }
        if let Some(index) = values.strip_prefix("trace-native:") {
            traced_natives.insert(index.parse().context("expected trace-native:index")?);
            continue;
        }
        if let Some(guard) = values.strip_prefix("require-hit:") {
            let (frame, region) = guard.split_once(':').context("expected require-hit:frame:region")?;
            let expected = HitGuard::Region(region.parse()?);
            ensure!(required_hits.insert(frame.parse()?, expected).is_none(),
                "duplicate hit guard at frame {frame}");
            continue;
        }
        if let Some(guard) = values.strip_prefix("require-hit-priority:") {
            let (frame, priority) = guard.split_once(':').context("expected require-hit-priority:frame:priority")?;
            let expected = HitGuard::Priority(priority.parse()?);
            ensure!(required_hits.insert(frame.parse()?, expected).is_none(),
                "duplicate hit guard at frame {frame}");
            continue;
        }
        let checkpoint = values.strip_prefix("capture:").map(|frame| (frame, false, true))
            .or_else(|| values.strip_prefix("inspect:").map(|frame| (frame, true, true)))
            .or_else(|| values.strip_prefix("inspect-log:").map(|frame| (frame, true, false)));
        if let Some((frame, inspect, should_capture)) = checkpoint {
            let frame = frame.parse::<u64>()?;
            ensure!(
                (1..=frames).contains(&frame),
                "checkpoint frame {frame} outside run"
            );
            if should_capture {
                captures.insert(frame);
            }
            if inspect {
                probes.insert(frame);
                if !should_capture {
                    log_probes.insert(frame);
                }
            }
            continue;
        }
        let (pointer_button, values) = if let Some(values) = values.strip_prefix("move:") {
            (None, values)
        } else if let Some(values) = values.strip_prefix("right:") {
            (Some(1), values)
        } else {
            (Some(0), values.as_ref())
        };
        let (frame, point) = values
            .split_once(':')
            .context("expected frame:x,y or frame:key")?;
        let command = if let Some((x, y)) = point.split_once(',') {
            Action::Pointer {
                x: x.parse()?,
                y: y.parse()?,
                button: pointer_button,
            }
        } else {
            ensure!(pointer_button == Some(0), "expected move:frame:x,y or right:frame:x,y");
            Action::Key(InputKey::Keyboard(point.parse()?))
        };
        let frame = frame.parse::<u64>()?;
        ensure!(frame < frames, "input frame {frame} outside run");
        ensure!(
            inputs.insert(frame, command).is_none(),
            "duplicate input at frame {frame}"
        );
    }
    for frame in required_hits.keys() {
        ensure!(matches!(inputs.get(frame), Some(Action::Pointer { button: Some(_), .. })),
            "hit guard at frame {frame} needs a scheduled pointer click");
    }
    for frame in &same_frame_taps {
        ensure!(matches!(inputs.get(frame), Some(Action::Pointer { button: Some(_), .. })),
            "tap at frame {frame} needs a scheduled pointer click");
    }
    let ctrl = InputKey::Keyboard(HeadlessPlayer::FAST_FORWARD_KEY);
    if !ctrl_windows.is_empty() {
        ensure!(!inputs.values().any(|action| *action == Action::Key(ctrl)),
            "Ctrl hold cannot be combined with individual Ctrl presses");
        ensure!(ctrl_windows.iter().zip(ctrl_windows.keys().skip(1)).all(|((_, end), next)| end < next),
            "Ctrl holds must be separated by at least one released frame");
        ensure!(guarded_body.is_none(), "guarded legacy body actions require the original input timeline");
    }
    if eye08_title.is_some() {
        ensure!(!state_mode && guarded_body.is_none() && inputs.is_empty(),
            "08 title button uses only its live title input");
    }
    if eden_entry.is_some() {
        ensure!(guarded_body.is_some_and(|(_, address)| address == 0xad0) && !state_mode
            && inputs.is_empty() && eye08_title.is_none(), "13 live entry requires a guarded Eden route");
    }
    let has_ready_entry = !ready_entry.done();
    let has_fes_choices = !fes_choices.is_empty();
    ensure!(!live_input || (!state_mode && guarded_body.is_none() && !has_fes_choices
        && !campanella_choice && eden_entry.is_none() && eye08_title.is_none()),
        "live input requires an unguarded frame replay");
    if ctrl_after_ready {
        ensure!(has_ready_entry && ctrl_windows.is_empty() && !state_mode && guarded_body.is_none(),
            "ctrl-after-ready requires ready-click and no other Ctrl or guarded body timeline");
    }
    if auto_first_choice {
        ensure!((ctrl_after_ready || !ctrl_windows.is_empty()) && !state_mode && !has_fes_choices
            && !campanella_choice && eden_entry.is_none() && eye08_title.is_none(),
            "auto-first-choice requires Ctrl holds and a compatible story entry");
    }
    if has_fes_choices {
        ensure!(!state_mode && !campanella_choice && guarded_body.is_none() && inputs.is_empty()
            && eden_entry.is_none() && eye08_title.is_none() && story_trace,
            "fes-choice requires tracing and compatible frame inputs");
    }
    if campanella_choice {
        ensure!(has_ready_entry && ready_entry.regions().eq([11]) && guarded_body.is_none()
            && !state_mode && story_trace, "18 first choice requires the live title entry and story tracing");
    }
    if has_ready_entry {
        ensure!(inputs.is_empty() && !state_mode && eden_entry.is_none() && eye08_title.is_none(),
            "ready-click cannot be mixed with other entry inputs");
        if guarded_body.is_some() {
            ensure!(guarded_body.is_some_and(|(_, address)| address == 0xa04)
                && ready_entry.regions().eq([20, 67]), "invalid 15 live entry regions");
        }
    }

    if happiness_produce_save {
        let (waits, address) = guarded_body.context("Happiness Save needs the 02 opening route")?;
        ensure!(address == 0xe0d && waits.last() == Some(&(46, 657)),
            "Happiness Save needs the verified 0_01_01 46/657 wait");
    }
    if happiness_resume_sha.is_some() {
        let (waits, address) = guarded_body.context("Happiness Load needs the 02 continuous route")?;
        ensure!(address == 0xe0d && waits == happiness::OPENING_WAITS && inputs.is_empty()
            && !happiness_produce_save && !state_mode, "invalid Happiness cold Load route");
        guarded_body = Some((&waits[46..], address));
    }
    if yukikoi_produce_save {
        let (waits, address) = guarded_body.context("Yukikoi Save needs the 04 opening route")?;
        ensure!(address == 0xea8 && waits.get(..3) == Some(&[(0, 54), (1, 67), (2, 70)]),
            "Yukikoi Save needs the verified __tar01 2/70 wait");
        guarded_body = Some((&waits[..3], address));
    }
    if guarded_body.is_some() {
        ensure!(!state_mode && story_trace && story_events && capture_every == 0
                && !capture_final && captures.is_empty(),
                "legacy body action requires a log-only frame scenario");
        let enter = Action::Key(InputKey::Keyboard(13));
        let click = |x, y| Action::Pointer { x, y, button: Some(0) };
        let (expected, hits) =
            if happiness_resume_sha.is_some() || eden_entry.is_some() || has_ready_entry {
                (Vec::new(), Vec::new())
            } else if guarded_body.is_some_and(|(_, address)| address == 0xea8)
                && inputs.contains_key(&1174)
            {
                (vec![(1172, Action::Pointer { x: 638, y: 446, button: None }),
                    (1174, click(638, 446)),
                    (1358, Action::Pointer { x: 165, y: 386, button: None }),
                    (1360, click(165, 386))], vec![(1174, 19), (1360, 15)])
            } else if guarded_body.is_some_and(|(_, address)| address == 0xea8) {
                (vec![(215, click(638, 446)),
                    (372, Action::Pointer { x: 165, y: 386, button: None }),
                    (390, click(165, 386))], vec![(215, 19), (390, 15)])
            } else if guarded_body.is_some_and(|(_, address)| address == 0x929) {
                (vec![(260, click(400, 300)), (380, click(400, 300)),
                    (860, Action::Pointer { x: 235, y: 509, button: None }),
                    (885, click(235, 509))], vec![(260, 13), (380, 13), (885, 15)])
            } else if guarded_body.is_some_and(|(_, address)| address == 0xc72) {
                (vec![(500, Action::Pointer { x: 98, y: 208, button: None }),
                    (530, click(98, 208)),
                    (640, Action::Pointer { x: 98, y: 104, button: None }),
                    (660, click(98, 104))], vec![(530, 18), (660, 10)])
            } else if guarded_body.is_some_and(|(_, address)| address == 0xe0d) {
                (vec![(695, Action::Pointer { x: 105, y: 64, button: None }),
                    (730, Action::Pointer { x: 400, y: 144, button: None }),
                    (745, click(400, 144)), (805, click(400, 144))],
                    vec![(745, 26), (805, 35)])
            } else if guarded_body.is_some_and(|(_, address)| address == 0xd0c) {
                (vec![(425, click(640, 480)), (661, click(66, 72)), (1041, click(640, 420))],
                    vec![(425, 14), (661, 15), (1041, 62)])
            } else if guarded_body.is_some_and(|(_, address)| address == 0x918) {
                (vec![(2270, click(335, 488)),
                    (2304, Action::Pointer { x: 40, y: 32, button: None })],
                    vec![(2270, 17)])
            } else if guarded_body.is_some_and(|(_, address)| address == 0xa04) {
                (vec![(691, Action::Pointer { x: 512, y: 353, button: None }),
                    (692, click(512, 353)), (821, click(400, 130))], vec![(692, 20), (821, 67)])
            } else if guarded_body.is_some_and(|(_, address)| address == 0x9f6) {
                (vec![(708, Action::Pointer { x: 190, y: 222, button: None }),
                    (710, click(190, 222))], vec![(710, 39)])
            } else if guarded_body.is_some_and(|(_, address)| address == 0xd6d) {
                (vec![(215, enter), (385, enter), (550, enter),
                    (605, Action::Pointer { x: 1154, y: 473, button: None }),
                    (612, click(1154, 473)), (810, click(640, 360)),
                    (890, click(640, 360)), (970, click(640, 360))],
                    vec![(612, 15), (810, 62), (890, 62), (970, 62)])
            } else {
                (vec![(241, click(640, 441)), (361, click(640, 549))], vec![(241, 20), (361, 55)])
            };
        ensure!(inputs.iter().map(|(&frame, &action)| (frame, action)).eq(expected)
                && required_hits.iter().map(|(&frame, &guard)| (frame, guard))
                    .eq(hits.into_iter().map(|(frame, id)| (frame, HitGuard::Region(id)))),
                "legacy body action requires the known guarded entry timeline");
    }

    *error_status = "engine_error";
    let measure = std::env::var_os("CS2_REPLAY_PERF").is_some_and(|value| value == "1");
    let detail = measure && std::env::var_os("CS2_REPLAY_PERF_DETAIL").is_some_and(|value| value == "1");
    let interactive = measure && std::env::var_os("CS2_REPLAY_INTERACTIVE").is_some_and(|value| value == "1");
    let fixed_retry_sleep = std::env::var_os("CS2_REPLAY_FIXED_RETRY_SLEEP").is_some_and(|value| value == "1");
    if measure {
        HeadlessPlayer::enable_host_gpu_raster();
    }
    let profile = output.with_extension("profile");
    let mut player = if session_random {
        HeadlessPlayer::open_with_game_language(game, profile, GameLanguage::Automatic)?
    } else {
        HeadlessPlayer::open_with_save_root(game, profile)?
    };
    if ctrl_after_ready || !ctrl_windows.is_empty() {
        player.enable_host_fast_forward(0);
    }
    let mut happiness_load = happiness_resume_sha.map(|digest| happiness::Load::new(&player, digest)).transpose()?;
    player.enable_host_window_requests();
    player.initialize_general_window_host(1)?;
    player.set_host_input_update_rate(60);
    if interactive {
        player.enable_host_emote_pose_jobs();
        player.enable_host_async_images();
        player.set_host_iteration_work_budget(Some(std::time::Duration::from_millis(4)));
    }
    player.enable_host_system_open_queue();
    player.enable_host_tweet_queue();
    player.enable_host_dialog_queue();
    if story_trace {
        player.enable_host_story_trace();
    }
    player.load_startup()?;
    player.apply_host_input(InputEvent::FocusChanged(true))?;
    let mut sent_events = 0;
    let mut sent_logs = 0;
    let mut windowed = 1;
    let mut release = None;
    let mut release_ctrl_at = None;
    let mut auto_choice_started = None;
    let mut last_auto_choice = None;
    const AUTO_CHOICE_TIMEOUT: u64 = 120;
    let mut release_pending = None;
    let mut last_story = None;
    let mut logged_fonts = BTreeSet::new();
    let mut legacy_press: Option<(u64, (u32, u32), (u32, u32, u64), u64)> = None;
    let mut legacy_wait = 0_usize;
    let mut legacy_op_finished = false;
    let mut legacy_chapter_op = story_route::NaturalOpening::default();
    let mut legacy_chapter_fes_requested = false;
    let mut legacy_chapter_fes_owner = None;
    let mut legacy_chapter_bottom_button = None;
    let mut happiness_save: Option<happiness::Save> = None;
    let mut ordinary_save: Option<yukikoi_save::Save> = None;
    let live_path = output.with_extension("inputs.txt");
    let mut live_commands = Vec::new();
    let mut live_next = 0;
    let replay_started = std::time::Instant::now();
    for completed in 0..frames {
        let tick_started = std::time::Instant::now();
        if release_ctrl_at == Some(completed) {
            if auto_choice_started.is_none() {
                player.apply_host_input(InputEvent::KeyUp(ctrl))?;
            }
            release_ctrl_at = None;
        }
        if let Some(&end) = ctrl_windows.get(&completed) {
            player.apply_host_input(InputEvent::KeyDown(ctrl))?;
            release_ctrl_at = Some(end);
        }
        if ctrl_after_ready && ready_entry.done() && release.is_none() {
            player.apply_host_input(InputEvent::KeyDown(ctrl))?;
            release_ctrl_at = Some(frames);
            ctrl_after_ready = false;
            eprintln!("frame {completed}: Ctrl held after confirmed entry");
        }
        let releasing = release.is_some_and(|(frame, _)| completed == frame);
        if releasing {
            let (_, key) = release.take().unwrap();
            player.apply_host_input(InputEvent::KeyUp(key))?;
            if state_mode || guarded_body.is_some_and(|(waits, address)| waits.len() > 2 || matches!(address, 0xa04 | 0x9f6)) {
                release_pending = Some(completed);
            }
        }
        if let Some(started) = auto_choice_started {
            if ready_entry.done() && release.is_none() && !releasing {
                if release_ctrl_at.is_some() {
                    player.apply_host_input(InputEvent::KeyDown(ctrl))?;
                    eprintln!("frame {completed}: auto-choice resumed Ctrl");
                }
                auto_choice_started = None;
                auto_choice_banks.pop_front();
            } else if completed - started >= AUTO_CHOICE_TIMEOUT {
                eprintln!("frame {completed}: auto-choice timed out: {:?}; pointer={:?} movement={:?}",
                    ready_entry.waiting(), player.host_pointer_logical_position(), player.host_pointer_move_state());
                println!("replay_status=input_missed");
                return Ok(());
            }
        } else if auto_first_choice && (!live_input || !auto_choice_banks.is_empty())
            && release_ctrl_at.is_some() && ready_entry.done()
            && release.is_none() && !releasing
            && inputs.range(completed..completed.saturating_add(AUTO_CHOICE_TIMEOUT)).next().is_none()
        {
            // Suppress the outgoing panel even if the next bank resolves to
            // a different button on that same panel during its exit animation.
            if last_auto_choice.is_some_and(|(owner, region)| {
                !button_probe::choice_target_enabled(&player, owner, region)
            }) {
                last_auto_choice = None;
            }
            if last_auto_choice.is_none()
                && let Some((owner, region, point)) =
                    player.host_replay_fes_choice_bank(auto_choice_banks.front().copied())?
            {
                player.apply_host_input(InputEvent::KeyUp(ctrl))?;
                ready_entry.add_choice(region, point);
                auto_choice_started = Some(completed);
                last_auto_choice = Some((owner, region));
                eprintln!("frame {completed}: auto-choice scene={} receiver={owner} region={region} point={point:?}",
                    player.host_replay_story_position().0.2);
            }
        }
        let mut live_action = None;
        if live_input {
            if completed % 30 == 0 {
                match std::fs::read_to_string(&live_path) {
                    Ok(contents) => {
                        // Only process complete lines. The operator appends commands.
                        for line in contents.split_inclusive('\n').skip(live_commands.len()) {
                            if !line.ends_with('\n') { break; }
                            live_commands.push(LiveInput::parse(line.trim())?);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error).context("read live replay input"),
                }
            }
            if ready_entry.done() && auto_choice_started.is_none() && release.is_none()
                && !releasing && !inputs.contains_key(&completed)
                && let Some(&command) = live_commands.get(live_next)
            {
                match command {
                    LiveInput::Key(code) => {
                        ensure!(code != HeadlessPlayer::FAST_FORWARD_KEY, "use ctrl up or ctrl down");
                        live_action = Some(Action::Key(InputKey::Keyboard(code)));
                    }
                    LiveInput::Click { region, point } => {
                        ensure!(player.masked_input_region_at(point)?
                            .is_some_and(|hit| hit[0] == region && hit[8] != 0)
                            && player.mouse_press_candidate_regions(point)?.contains(&region),
                            "live click at {point:?} does not match enabled region {region}");
                        ready_entry.add_choice(region, point);
                    }
                    LiveInput::Inspect => {
                        button_probe.inspect(&mut player, completed)?;
                        eprintln!("frame {completed}: live wait {}", player.host_replay_wait_diagnostics());
                        capture(&player, output, completed)?;
                    }
                    LiveInput::Ctrl(held) => {
                        player.apply_host_input(if held { InputEvent::KeyDown(ctrl) }
                            else { InputEvent::KeyUp(ctrl) })?;
                        release_ctrl_at = held.then_some(frames);
                    }
                }
                live_next += 1;
                eprintln!("frame {completed}: live input {live_next} accepted {command:?}");
            }
        }
        if campanella_choice && ready_entry.done() && release_ctrl_at.is_none()
            && let Some((region, point)) = player.host_campanella_first_choice()?
        {
            eprintln!("frame {completed}: 18 first choice ready region={region} point={point:?}");
            capture(&player, output, completed)?;
            ready_entry.add_choice(region, point);
            campanella_choice = false;
        }
        if ready_entry.done() && release_ctrl_at.is_none()
            && let Some((scene, group, target)) = fes_choices.front()
            && let Some((region, point)) = player.host_replay_fes_choice(scene, *group, target)?
        {
            eprintln!("frame {completed}: FES choice scene={scene} group={group} target={target} region={region} point={point:?}");
            ready_entry.add_choice(region, point);
            fes_choices.pop_front();
        }
        let input = if let Some(controller) = route.as_deref_mut() {
            if let Some(released_at) = release_pending {
                if !releasing && player.device_key_scan_state().records().iter().flatten()
                    .all(|key| key.mask & 3 == 0)
                {
                    release_pending = None;
                } else if completed - released_at >= controller.idle_frames() {
                    capture(&player, output, completed)?;
                    eprintln!("replay frame={completed}: input scan did not observe the key release");
                    println!("replay_status=input_missed");
                    return Ok(());
                }
            }
            if release.is_some() || release_pending.is_some() {
                None
            } else {
                controller.observe_lifecycle_events(player.events(), sent_events);
                match controller.poll(&mut player, completed, output)? {
                    story_route::Decision::Wait => None,
                    story_route::Decision::Input(input) => Some(input),
                    story_route::Decision::Done => {
                        println!("replay_status=completed");
                        return Ok(());
                    }
                    story_route::Decision::Stop(status, reason) => {
                        capture(&player, output, completed)?;
                        eprintln!("replay frame={completed}: {reason}");
                        eprintln!("replay wait: {}", player.host_replay_wait_diagnostics());
                        println!("replay_status={status}");
                        return Ok(());
                    }
                }
            }
        } else if let Some(entry) = eden_entry.as_mut().filter(|entry| !entry.done()) {
            match entry.poll(&mut player, completed, release.is_some() || releasing) {
                Ok(action) => action,
                Err(error) => {
                    eprintln!("replay frame={completed}: {error}");
                    eprintln!("replay wait: {}", player.host_replay_wait_diagnostics());
                    println!("replay_status=input_missed");
                    return Ok(());
                }
            }
        } else if !ready_entry.done() {
            match ready_entry.poll(&mut player, completed, release.is_some() || releasing) {
                Ok(action) => action,
                Err(error) => {
                    eprintln!("replay frame={completed}: {error}");
                    eprintln!("replay wait: {}", player.host_replay_wait_diagnostics());
                    println!("replay_status=input_missed");
                    return Ok(());
                }
            }
        } else if let Some((waits, address)) = guarded_body
            && (eden_entry.is_some() || has_ready_entry || address == 0xe0d && inputs.is_empty()
                || inputs.last_key_value().is_some_and(|(last, _)| completed > *last))
        {
            let sample = legacy_body_sample(address);
            let body = |group, record| {
                if sample.is_some() {
                    player.host_replay_legacy_fes_body(address, group, record)
                } else if address == 0x9f6 {
                    player.host_replay_djibril10_body(group, record)
                } else if address == 0xa04 {
                    player.host_replay_labyrinth15_body(group, record)
                } else if address == 0xd6d && waits.last() == Some(&(4, 97)) {
                    player.host_replay_11_radio_body(group, record)
                } else {
                    player.host_replay_11_13_body(group, record)
                }
            };
            if !releasing && release_pending.is_some() && player.device_key_scan_state().records()
                .iter().flatten().all(|key| key.mask & 3 == 0)
            {
                release_pending = None;
                if (labyrinth15_observe_after_r19 || labyrinth15_cross_window_successor)
                    && waits.get(legacy_wait.wrapping_sub(1)) == Some(&(6, 19))
                {
                    println!("labyrinth15-r19 release frame={completed}");
                }
            }
            if release.is_some() || releasing || release_pending.is_some() {
                None
            } else if labyrinth15_observe_after_r19 && legacy_wait == waits.len() {
                None
            } else if let Some(load) = happiness_load.as_mut() {
                match load.poll(&mut player, completed)? {
                    happiness::LoadDecision::Wait => None,
                    happiness::LoadDecision::Input(action) => Some(action),
                    happiness::LoadDecision::Restored => {
                        happiness_load = None;
                        None
                    }
                    happiness::LoadDecision::Stop(status) => {
                        println!("replay_status={status}");
                        return Ok(());
                    }
                }
            } else if let Some(save) = happiness_save.as_mut() {
                match save.poll(&mut player, completed)? {
                    happiness::SaveDecision::Wait => None,
                    happiness::SaveDecision::Input(action) => Some(action),
                    happiness::SaveDecision::Stop(status) => {
                        println!("replay_status={status}");
                        return Ok(());
                    }
                }
            } else if let Some(save) = ordinary_save.as_mut() {
                match save.poll(&mut player, completed)? {
                    yukikoi_save::Decision::Wait => None,
                    yukikoi_save::Decision::Input(action) => Some(action),
                    yukikoi_save::Decision::Stop(status) => {
                        println!("replay_status={status}");
                        return Ok(());
                    }
                }
            } else {
                let (group, record) = waits[legacy_wait];
                let anchor = (matches!(address, 0xd6d | 0xa04 | 0x9f6) || sample.is_some() || legacy_op_finished)
                    .then(|| body(group, record)).flatten();
                if let Some((binding, lifetime, (epoch, source_frame, marked))) = anchor {
                    let successor = legacy_press.is_some();
                    if let Some((started, previous_binding, previous_lifetime, previous_epoch)) = legacy_press {
                        let chapter_jump = address == 0xc72 && waits[legacy_wait - 1] == (10, 147)
                            && (group, record) == (0, 47);
                        let cross_window = labyrinth15_cross_window_successor && (group, record) == (7, 38);
                        let previous = waits[legacy_wait - 1];
                        let newton_scene = address == 0xd0c && previous == (56, 258)
                            && (group, record) == (0, 28) && player.host_story_trace().2 == "com_001B";
                        let newton_window = address == 0xd0c
                            && (previous == (11, 66) && (group, record) == (12, 71)
                                || player.host_story_trace().2 == "com_001B"
                                    && matches!(group, 19 | 37 | 209) && previous.0 == group - 1);
                        let happiness_scene = address == 0xe0d
                            && happiness::scene_successor(waits[legacy_wait - 1], (group, record))
                                .is_some_and(|scene| player.host_story_trace().2 == scene);
                        let valid_load = if newton_window {
                            lifetime.0 != previous_lifetime.0
                                && (lifetime.1, lifetime.2) == (previous_lifetime.1, previous_lifetime.2)
                        } else if cross_window {
                            (lifetime.1, lifetime.2) == (previous_lifetime.1, previous_lifetime.2)
                        } else if happiness_scene || newton_scene {
                            lifetime.2 > previous_lifetime.2
                        } else if chapter_jump {
                            lifetime.0 == previous_lifetime.0 && lifetime.2 > previous_lifetime.2
                                && legacy_chapter_op.released_between(started, source_frame)
                        } else {
                            lifetime == previous_lifetime
                        };
                        let same_input_owner = if chapter_jump {
                            binding.1 == previous_binding.1 && Some(binding.0) == legacy_chapter_bottom_button
                        } else if happiness_scene || newton_scene {
                            binding.1 == previous_binding.1
                        } else {
                            binding == previous_binding
                        };
                        let require_new_epoch = !newton_window && !newton_scene && !happiness_scene
                            && (!cross_window || lifetime.0 == previous_lifetime.0);
                        if (!cross_window && !same_input_owner) || !valid_load
                            || (require_new_epoch && epoch <= previous_epoch)
                            || source_frame <= started || marked <= started
                        {
                            println!("replay_status=checkpoint_mismatch");
                            return Ok(());
                        }
                        if (waits.len() > 2 || address == 0x9f6) && sample.is_none() {
                            eprintln!("frame {completed}: 11/13 Enter released after frame {started}");
                        }
                    }
                    log_legacy_body(&player, completed)?;
                    legacy_wait += 1;
                    if let Some(sample) = sample {
                        let (event, release) = if successor { ("checkpoint", " release=observed") }
                            else { ("enter_guard", "") };
                        println!("{sample} {event} frame={completed} group={group} record={record} binding={binding:?} lifetime={lifetime:?} epoch={epoch} source_frame={source_frame} marked={marked}{release}");
                    }
                    if legacy_wait == waits.len() && !labyrinth15_observe_after_r19 {
                        if !yukikoi_produce_save && !happiness_produce_save {
                            println!("replay_status=completed");
                            return Ok(());
                        }
                        let (owner, context, load) = lifetime;
                        legacy_press = None;
                        if happiness_produce_save {
                            happiness_save = Some(happiness::Save::new((owner, context, load, epoch), completed));
                        } else {
                            ordinary_save = Some(yukikoi_save::Save::new((owner, context, load, epoch), completed));
                        }
                        None
                    } else {
                        legacy_press = Some((completed, binding, lifetime, epoch));
                        Some(Action::Key(InputKey::Keyboard(13)))
                    }
                } else {
                    None
                }
            }
        } else if let Some(title) = &mut eye08_title {
            match title.poll(&mut player, completed)? {
                button_probe::TitleAction::Wait => None,
                button_probe::TitleAction::Move([x, y]) => {
                    button_probe.inspect(&mut player, completed)?;
                    capture(&player, output, completed)?;
                    Some(Action::Pointer { x, y, button: None })
                }
                button_probe::TitleAction::Click([x, y]) => Some(Action::Pointer { x, y, button: Some(0) }),
                button_probe::TitleAction::CaptureResult => {
                    button_probe.inspect(&mut player, completed)?;
                    capture(&player, output, completed)?;
                    return Ok(());
                }
            }
        } else {
            inputs.get(&completed).copied().or(live_action)
        };
        if let Some(input) = input {
            let key = match input {
                Action::Pointer { x, y, button } => {
                    player.apply_host_input(InputEvent::MouseMove { x, y })?;
                    let masked = player.masked_input_region_at([x, y])?;
                    eprintln!(
                        "frame {completed}: pointer ({x},{y}) unmasked={:?} masked={:?} press_candidates={:?}",
                        player.input_region_at([x, y]),
                        masked,
                        player.mouse_press_candidate_regions([x, y])
                    );
                    if let Some(&expected) = required_hits.get(&completed)
                        && !masked.is_some_and(|region| expected.matches(region))
                    {
                        match expected {
                            HitGuard::Region(id) => eprintln!("frame {completed}: click withheld; expected enabled region {id}"),
                            HitGuard::Priority(priority) => eprintln!("frame {completed}: click withheld; expected enabled region with priority {priority}"),
                        }
                        println!("replay_status=input_missed");
                        return Ok(());
                    }
                    button.map(InputKey::MouseButton)
                }
                Action::Key(key) => Some(key),
                Action::Menu(command) => {
                    button_probe.menu_command(&mut player, completed, command)?;
                    None
                }
            };
            if let Some(key) = key {
                ensure!(
                    release.is_none(),
                    "overlapping presses at frame {completed}"
                );
                eprintln!("frame {completed}: down {key:?}");
                player.apply_host_input(InputEvent::KeyDown(key))?;
                if same_frame_taps.contains(&completed) {
                    eprintln!("frame {completed}: up {key:?}");
                    player.apply_host_input(InputEvent::KeyUp(key))?;
                } else {
                    release = Some((completed + 3, key));
                }
            }
        }
        let mut retries = 0;
        let mut ordinary_execution = OrdinaryExecutionProgress::default();
        let mut retry_sleep = std::time::Duration::ZERO;
        loop {
            let result = if measure {
                let start = std::time::Instant::now();
                let mut advance = || cs2_desktop::HostFrameTiming::capture(|| player.advance_host_frame(20_000));
                let ((result, timing), detail) = if detail {
                    let (result, detail) = cs2_desktop::HostPerfDetails::capture(false, advance);
                    (result, Some(detail))
                } else {
                    (advance(), None)
                };
                eprintln!("replay_perf frame={completed} call={retries} elapsed_us={} phase_ns={:?}",
                    start.elapsed().as_micros(), timing.phase_ns);
                if let Some(detail) = detail {
                    eprint!("{}", detail.rows(0, completed, retries));
                }
                result
            } else {
                player.advance_host_frame(20_000)
            };
            if let Some(controller) = route.as_deref_mut() {
                controller.observe_lifecycle_events(player.events(), sent_events);
            }
            for event in &player.events()[sent_events..] {
                legacy_op_finished |= guarded_body.is_some() && event.kind == "story-op-finished";
                if guarded_body.is_some_and(|(waits, address)| address == 0xc72
                    && waits.get(legacy_wait) == Some(&(0, 47)))
                    && let Some((started, (_, receiver), _, _)) = legacy_press
                    && event.frame > started
                {
                    if event.kind == "native-call" && event.detail.ends_with("debug=\"[command] fes opmovie\"") {
                        legacy_chapter_fes_requested = true;
                    }
                    if legacy_chapter_fes_requested && legacy_chapter_fes_owner.is_none()
                        && event.kind == "task-create"
                        && let Some((id, fields)) = event.detail.strip_prefix("id=").and_then(|detail| detail.split_once(' '))
                        && fields == format!("kind=34 parent={receiver} name=\"child fes\" ordinary_unloaded=false")
                    {
                        legacy_chapter_fes_owner = id.parse::<u32>().ok();
                    }
                    legacy_chapter_op.observe(event);
                    if legacy_chapter_fes_owner.is_some() && legacy_chapter_op.released_between(started, event.frame)
                        && event.kind == "task-create"
                        && let Some((id, fields)) = event.detail.strip_prefix("id=").and_then(|detail| detail.split_once(' '))
                        && fields == format!("kind=6 parent={receiver} name=\"bottom button\" ordinary_unloaded=false")
                    {
                        legacy_chapter_bottom_button = id.parse::<u32>().ok();
                    }
                }
                if (!story_events || event.kind != "native-call")
                    && (!state_mode || event.kind != "logic-frame")
                    || event.detail.contains(" debug=") || event.detail.contains(" movie_")
                    || event.kind == "native-call" && selected_native(&event.detail, &traced_natives)
                {
                    println!("frame={} {} {}", event.frame, event.kind, event.detail);
                }
            }
            for line in &player.log()[sent_logs..] {
                if !story_events || !line.starts_with("ran ") {
                    println!("log: {line}");
                }
            }
            player.limit_host_diagnostics(10_000, 1_000);
            sent_events = player.events().len();
            sent_logs = player.log().len();
            while let Some(request) = player.take_host_window_request() {
                eprintln!("frame {completed}: {request:?}");
                if button_probe.window_request(&mut player, completed, request)? {
                    continue;
                }
                let mode = if matches!(request, HostWindowRequest::ToggleFullscreen) {
                    windowed ^= 1;
                    Some(windowed)
                } else {
                    None
                };
                player.complete_host_window_request(mode)?;
            }
            button_probe.complete_local_requests(&mut player, completed)?;
            if state_mode && result.is_err() {
                capture(&player, output, completed).ok();
            }
            if story_trace && result.is_err() {
                log_font_substitutions(&player, &mut logged_fonts);
            }
            let advanced = result.with_context(|| format!("frame {completed}"))?;
            if player.quit_requested() {
                eprintln!("frame {completed}: game requested normal exit");
                let previous_events = player.events().len();
                player.persist_host_configuration().context("save configuration on game exit")?;
                for event in &player.events()[previous_events..] {
                    println!("frame={} {} {}", event.frame, event.kind, event.detail);
                }
                if button_probe.enabled {
                    eprintln!("frame {}: pointer {:?}, routes {:?}",
                        completed + 1, player.host_pointer_logical_position(),
                        player.host_input_route_receipts());
                }
                if state_mode || button_probe.enabled {
                    capture(&player, output, completed)?;
                }
                println!("replay_status=unexpected_exit");
                return Ok(());
            }
            if advanced {
                break;
            }
            retries += 1;
            let stalled = if interactive {
                tick_started.elapsed() >= FRAME_RETRY_TIME
            } else {
                // Child bodies and synchronous script loads can advance in
                // short VM slices while the frame and root PC stay parked.
                ordinary_execution.stalled(
                    retries, tick_started.elapsed(), player.host_pending_ordinary_execution(),
                )
            };
            if stalled {
                if state_mode {
                    capture(&player, output, completed).ok();
                }
                eprintln!("stalled after {completed} completed frames: {} retries={retries} pending_ordinary_vms={:?}",
                    player.host_tick_state(), player.host_pending_ordinary_execution()
                        .map(|(id, pc, _)| (id, format!("{pc:#x}"))).collect::<Vec<_>>());
                println!("replay_status=frame_stalled");
                return Ok(());
            }
            if interactive {
                let delay = if fixed_retry_sleep { 1 } else { player.host_frame_retry_delay_ms() };
                if delay != 0 {
                    let wait_started = std::time::Instant::now();
                    std::thread::sleep(std::time::Duration::from_millis(u64::from(delay)));
                    retry_sleep += wait_started.elapsed();
                }
            }
        }
        if interactive {
            eprintln!(
                "replay_tick frame={completed} call=0 ready_us={} retry_sleep_us={} retries={retries}",
                tick_started.elapsed().as_micros(), retry_sleep.as_micros(),
            );
            eprintln!("replay_preview frame={completed} monotonic_us={} skipped={}",
                replay_started.elapsed().as_micros(), u8::from(player.fast_forward_preview_skipped()));
        }
        if let Some(controller) = route.as_deref_mut() {
            controller.observe_frame(&player);
        }
        if release.is_some() || same_frame_taps.contains(&completed)
            || inputs.contains_key(&completed.saturating_sub(3)) {
            eprintln!(
                "frame {}: pointer {:?}, routes {:?}",
                completed + 1,
                player.host_pointer_logical_position(),
                player.host_input_route_receipts()
            );
        }
        let frame = completed + 1;
        if let Some(load) = happiness_load.as_mut()
            && let Some(status) = load.observe(&player, frame)
        {
            println!("replay_status={status}");
            return Ok(());
        }
        if let Some(save) = happiness_save.as_mut()
            && let Some(status) = save.observe(&player, frame)
        {
            println!("replay_status={status}");
            return Ok(());
        }
        if let Some(save) = ordinary_save.as_mut()
            && let Some(status) = save.observe(&player, frame)
        {
            println!("replay_status={status}");
            return Ok(());
        }
        if let Some((started, (region, receiver), (owner, _, _), _)) = legacy_press {
            let address = guarded_body.unwrap().1;
            let mut consumed = player.host_input_route_receipts().iter()
                .filter(|receipt| receipt.frame == frame && receipt.pressed != 0 && receipt.response == 0);
            let mut events = player.events().iter().rev().take_while(|event| event.frame == frame);
            let observing_after_r19 = (labyrinth15_observe_after_r19 || labyrinth15_cross_window_successor)
                && guarded_body.unwrap().0.get(legacy_wait.wrapping_sub(1)) == Some(&(6, 19))
                && frame > started + 1;
            let waits = guarded_body.unwrap().0;
            let happiness_scene_loaded = address == 0xe0d && frame > started + 1
                && happiness::scene_successor(waits[legacy_wait - 1], waits[legacy_wait])
                    .is_some_and(|scene| player.host_story_trace().2 == scene);
            let newton_scene_boundary = address == 0xd0c && frame > started + 1
                && waits.get(legacy_wait.wrapping_sub(1)) == Some(&(56, 258))
                && waits.get(legacy_wait) == Some(&(0, 28));
            let recreated = !observing_after_r19
                && (guarded_body.unwrap().0.len() > 2 || matches!(address, 0xa04 | 0x9f6))
                && events.clone().any(|event| {
                event.kind == "task-create"
                    && event.detail.strip_prefix("id=").and_then(|detail| detail.split_once(' '))
                        .is_some_and(|(id, fields)| {
                            let Ok(id) = id.parse::<u32>() else { return false; };
                            let opening_region_reuse = address == 0xc72 && frame > started + 1
                                && guarded_body.unwrap().0.get(legacy_wait) == Some(&(0, 47))
                                && id == region && id != owner && id != receiver
                                && (legacy_chapter_fes_owner.is_some_and(|opening| fields
                                    == format!("kind=0 parent={opening} name=\"\" ordinary_unloaded=true"))
                                    || legacy_chapter_fes_owner.is_some()
                                        && legacy_chapter_op.released_between(started, frame));
                            // The validated new scene may reuse the retired window and region IDs.
                            // Its first body still requires a fresh load and the same receiver.
                            let happiness_window_reuse = happiness_scene_loaded && id != receiver
                                && (id == owner || id == region);
                            let newton_window_reuse = newton_scene_boundary && id == owner
                                && id != receiver && id != region
                                && fields == format!("kind=2 parent={receiver} name=\"plane 27\" ordinary_unloaded=false");
                            [owner, region, receiver].contains(&id)
                                && !opening_region_reuse && !happiness_window_reuse && !newton_window_reuse
                        })
            });
            if recreated {
                eprintln!("frame {frame}: body owner {owner}, receiver {receiver}, or region {region} was recreated");
                println!("replay_status=checkpoint_mismatch");
                return Ok(());
            }
            let root_wait = format!("native 71 address={address:#x}");
            let root = events.clone().any(|event| event.kind == "input-wait" && event.detail == root_wait);
            let mut resumes = events.by_ref().filter(|event| event.kind == "message-resume");
            let resume = resumes.next();
            let receipt = consumed.next();
            let valid = if frame == started + 1 {
                root && resume.is_some_and(|event| event.detail == "message=[2, 0, 0, 0]")
                    && resumes.next().is_none() && consumed.next().is_none()
                    && receipt.is_some_and(|receipt| !receipt.repeated && receipt.pressed == 1
                        && receipt.raw_mask == ButtonSource::Keyboard.mask()
                        && (receipt.region, receipt.receiver, receipt.key, receipt.group) == (region, receiver, 2, 0)
                        && matches!(receipt.physical.as_slice(),
                            [button] if button.source == ButtonSource::Keyboard && button.code == 13))
            } else if observing_after_r19 {
                receipt.is_none() && !resume.into_iter().chain(resumes)
                    .any(|event| event.detail == "message=[2, 0, 0, 0]")
            } else {
                resume.is_none() && receipt.is_none()
            };
            if !valid {
                println!("replay_status=input_missed");
                return Ok(());
            }
            if let Some(sample) = legacy_body_sample(address)
                && frame == started + 1
            {
                println!("{sample} enter_consumed frame={frame} region={region} receiver={receiver} logical_key=2 group=0 response=0 root_resume=[2,0,0,0]");
            }
        }
        if story_trace {
            let (frame, state, scene, group, record) = player.host_story_trace();
            let next = (state, scene, group, record);
            if let Some(sample @ (name, handle, playback, pts)) = player.host_neko_composited_film()
                && neko_film != Some(sample)
            {
                println!("frame={frame} neko-film-composed {name} handle={handle} playback={playback} pts={pts}");
                neko_film = Some(sample);
            }
            if let Some(owner) = player.host_neko_cleared_title() && neko_clear_title != Some(owner) {
                println!("frame={frame} neko-cleared-title owner={owner} flag550=1");
                neko_clear_title = Some(owner);
            }
            if last_story.as_ref().map(|(state, scene, group, record): &(_, String, _, _)|
                (*state, scene.as_str(), *group, *record)) != Some(next)
            {
                println!("story frame={frame} state={state} scene={scene} group={group} record={record}");
                last_story = Some((state, scene.to_owned(), group, record));
                log_font_substitutions(&player, &mut logged_fonts);
            }
        }
        let inspect_after_r19 = (labyrinth15_observe_after_r19 || labyrinth15_cross_window_successor)
            && guarded_body.is_some_and(|(waits, _)| waits.get(legacy_wait.wrapping_sub(1)) == Some(&(6, 19)))
            && release.is_none() && release_pending.is_none();
        if probes.contains(&frame)
            && (guarded_body.is_none() || inspect_after_r19 || log_probes.contains(&frame))
        {
            button_probe.inspect(&mut player, frame)?;
            eprintln!("frame {frame}:\n{}", player.host_composition_diagnostic());
            eprintln!("frame {frame}: wait {}", player.host_replay_wait_diagnostics());
            for movie in player.host_replay_movie_diagnostics()? {
                eprintln!("frame {frame}: movie {movie}");
            }
        }
        if (capture_every != 0 && frame % capture_every == 0)
            || (capture_final && frame == frames)
            || captures.contains(&frame)
        {
            capture(&player, output, frame)?;
        }
        if interactive {
            let interval = std::time::Duration::from_nanos(1_000_000_000 / 60);
            if let Some(remaining) = interval.checked_sub(tick_started.elapsed()) {
                std::thread::sleep(remaining);
            }
        }
    }
    let ready_waiting = ready_entry.waiting();
    if let Some(waiting) = &ready_waiting {
        eprintln!("replay frame={frames}: ready-click {waiting}");
    }
    if state_mode || guarded_body.is_some() || eye08_title.is_some() || has_ready_entry || has_fes_choices || auto_first_choice {
        eprintln!("replay wait: {}", player.host_replay_wait_diagnostics());
        if guarded_body.is_some_and(|(waits, address)| address == 0xd6d
            && matches!(waits.last().copied(), Some((3, 94) | (4, 97)))) {
            for movie in player.host_replay_movie_diagnostics()? {
                eprintln!("frame {frames}: movie {movie}");
            }
        }
        let status = if ready_waiting.is_some() || campanella_choice || !fes_choices.is_empty() {
            "input_missed"
        } else { "frame_limit" };
        println!("replay_status={status}");
    }
    if story_trace {
        log_font_substitutions(&player, &mut logged_fonts);
    }
    Ok(())
}

fn legacy_body_sample(address: u32) -> Option<&'static str> {
    match address {
        0xe0d => Some("02"),
        0xea8 => Some("04"),
        0x929 => Some("05"),
        0xc72 => Some("06"),
        0xd0c => Some("16"),
        0x918 => Some("19"),
        _ => None,
    }
}

fn log_legacy_body(player: &HeadlessPlayer, frame: u64) -> Result<()> {
    eprintln!("frame {frame}: wait {}", player.host_replay_wait_diagnostics());
    for movie in player.host_replay_movie_diagnostics()? {
        eprintln!("frame {frame}: movie {movie}");
    }
    Ok(())
}

fn selected_native(detail: &str, traced_natives: &BTreeSet<u32>) -> bool {
    !traced_natives.is_empty() && detail.strip_prefix("index=")
        .and_then(|fields| fields.split_ascii_whitespace().next())
        .and_then(|index| index.parse().ok())
        .is_some_and(|index| traced_natives.contains(&index))
}

fn log_font_substitutions(
    player: &HeadlessPlayer,
    logged: &mut BTreeSet<cs2_runtime::message_object::MessageFontSubstitution>,
) {
    for face in player.message_font_substitutions() {
        if !logged.contains(&face) {
            eprintln!("message-font substitute requested={:?} selected={:?} source={:?} face={}",
                String::from_utf16_lossy(&face.requested_name), face.selected_name, face.source, face.collection_index);
            logged.insert(face);
        }
    }
}

fn capture(player: &HeadlessPlayer, output: &Path, frame: u64) -> Result<()> {
    let [width, height] = player.logical_size();
    let path = output.with_extension(format!("{frame:04}.png"));
    image::save_buffer(&path, player.frame_rgba(), width, height, image::ColorType::Rgba8)?;
    eprintln!("frame {frame}: {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_frame_watchdog_allows_initializers_that_update_their_stack() {
        use std::time::Duration;

        let mut progress = OrdinaryExecutionProgress::default();
        for retry in 1..600 {
            let state = (28, 0x100, retry);
            assert!(!progress.stalled(retry, Duration::ZERO, [state].into_iter()));
        }
        let unchanged = (28, 0x100, 599);
        for retry in 600..(599 + FRAME_RETRY_LIMIT) {
            assert!(!progress.stalled(retry, Duration::ZERO, [unchanged].into_iter()));
        }
        assert!(progress.stalled(599 + FRAME_RETRY_LIMIT, Duration::ZERO, [unchanged].into_iter()));
        assert!(progress.stalled(800, FRAME_RETRY_TIME, [(28, 0x100, 800)].into_iter()));
        assert!(progress.stalled(ADVANCING_VM_RETRY_LIMIT, Duration::ZERO, [(28, 0x100, 900)].into_iter()));

        let mut no_execution = OrdinaryExecutionProgress::default();
        assert!(no_execution.stalled(FRAME_RETRY_LIMIT, Duration::ZERO, std::iter::empty()));
    }

    #[test]
    fn invalid_frame_inputs_are_scenario_errors_before_opening_the_game() {
        for (args, expected) in [
            (&["3600:13"][..], "input frame 3600 outside run"),
            (&["menu:3600:1"][..], "menu action frame 3600 outside run"),
            (&["30:13", "30:17"][..], "duplicate input at frame 30"),
            (&["tap:45"][..], "tap at frame 45 needs a scheduled pointer click"),
        ] {
            let mut error_status = "scenario_error";
            let result = replay(
                PathBuf::from("unused-game"),
                Path::new("unused-output"),
                3600,
                args.iter().copied().map(std::ffi::OsString::from),
                None,
                &mut error_status,
            );
            assert_eq!(result.unwrap_err().to_string(), expected);
            assert_eq!(error_status, "scenario_error");
        }
    }

    #[test]
    fn route_diagnostics_cannot_send_input_or_capture_images() {
        for flag in ["trace-native:530", "inspect-log:1635"] {
            assert!(super::is_route_diagnostic(flag));
        }
        for flag in ["1600:13", "1600:640,360", "capture:1600", "inspect:1600", "inspect-log:0", "trace-native:-1"] {
            assert!(!super::is_route_diagnostic(flag));
        }
    }

    #[test]
    fn selects_only_exact_native_indices() {
        let selected = BTreeSet::from([530, 925]);
        assert!(selected_native("index=530 arguments=20 bytes [00]", &selected));
        assert!(selected_native("index=925 arguments=0 bytes []", &selected));
        assert!(!selected_native("index=5300 arguments=4 bytes [00]", &selected));
        assert!(!selected_native("native_index=530 status=4", &selected));
        assert!(!selected_native("index=530 arguments=0 bytes []", &BTreeSet::new()));
    }
}
