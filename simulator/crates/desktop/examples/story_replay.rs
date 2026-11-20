use anyhow::{Context, Result, ensure};
use cs2_desktop::{HeadlessPlayer, HostWindowRequest};
use cs2_runtime::{InputEvent, InputKey};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

mod story_route;

#[derive(Clone, Copy)]

enum Action {
    Pointer { x: i32, y: i32, button: Option<u8> },
    Key(InputKey),
}

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let game = PathBuf::from(args.next().context("game directory required")?);
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
        let controller = story_route::Controller::read(&path).inspect_err(|_| {
            println!("replay_status=scenario_error");
        })?;
        (Some(controller), flags)
    } else {
        (None, args.collect())
    };
    let result = replay(game, &output, frames, flags.into_iter(), route.as_mut());
    if let Err(cause) = &result {
        eprintln!("replay error: {cause:#}");
        println!("replay_status=engine_error");
    }
    result
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
) -> Result<()> {
    let state_mode = route.is_some();
    if let Some(route) = &route {
        println!("replay route={:?}", route.label());
    }
    let mut inputs = BTreeMap::new();
    let mut required_hits = BTreeMap::<u64, u32>::new();
    let mut captures = BTreeSet::new();
    let mut probes = BTreeSet::new();
    let mut traced_natives = BTreeSet::new();
    let mut capture_every = if state_mode { 0 } else { 60 };
    let mut capture_final = !state_mode;
    let mut story_events = state_mode;
    let mut story_trace = state_mode;
    for arg in args {
        let values = arg.to_string_lossy();
        if values == "story-events" {
            story_events = true;
            continue;
        }
        if values == "story-trace" {
            story_trace = true;
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
            ensure!(required_hits.insert(frame.parse()?, region.parse()?).is_none(),
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

    let mut player = HeadlessPlayer::open_with_save_root(game, output.with_extension("profile"))?;
    player.enable_host_window_requests();
    player.initialize_general_window_host(1)?;
    player.set_host_input_update_rate(60);
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
    let mut release_pending = None;
    let mut last_story = None;
    let mut logged_fonts = BTreeSet::new();
    for completed in 0..frames {
        let releasing = release.is_some_and(|(frame, _)| completed == frame);
        if releasing {
            let (_, key) = release.take().unwrap();
            player.apply_host_input(InputEvent::KeyUp(key))?;
            if state_mode {
                release_pending = Some(completed);
            }
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
        } else {
            inputs.get(&completed).copied()
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
                    if let Some(expected) = required_hits.get(&completed)
                        && !masked.is_some_and(|region| region[0] == *expected && region[8] != 0)
                    {
                        eprintln!("frame {completed}: click withheld; expected enabled region {expected}");
                        println!("replay_status=input_missed");
                        return Ok(());
                    }
                    button.map(InputKey::MouseButton)
                }
                Action::Key(key) => Some(key),
            };
            if let Some(key) = key {
                ensure!(
                    release.is_none(),
                    "overlapping presses at frame {completed}"
                );
                eprintln!("frame {completed}: down {key:?}");
                player.apply_host_input(InputEvent::KeyDown(key))?;
                release = Some((completed + 3, key));
            }
        }
        let mut retries = 0;
        loop {
            let result = player.advance_host_frame(20_000);
            if let Some(controller) = route.as_deref_mut() {
                controller.observe_lifecycle_events(player.events(), sent_events);
            }
            for event in &player.events()[sent_events..] {
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
                if request == HostWindowRequest::Settings {
                    let settings = player.host_effect_settings()?;
                    let pages: Vec<_> = settings.pages.iter().map(|page| page.resource).collect();
                    eprintln!("frame {completed}: settings opened pages={pages:?}");
                    player.close_host_effect_settings(settings.id)?;
                    eprintln!("frame {completed}: settings closed");
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
            if state_mode && result.is_err() {
                capture(&player, output, completed).ok();
            }
            if story_trace && result.is_err() {
                log_font_substitutions(&player, &mut logged_fonts);
            }
            let advanced = result.with_context(|| format!("frame {completed}"))?;
            if player.quit_requested() {
                eprintln!("frame {completed}: game requested normal exit");
                if state_mode {
                    capture(&player, output, completed)?;
                }
                println!("replay_status=unexpected_exit");
                return Ok(());
            }
            if advanced {
                break;
            }
            retries += 1;
            if retries >= 128 {
                if state_mode {
                    capture(&player, output, completed).ok();
                }
                eprintln!("stalled after {completed} completed frames: {}", player.host_tick_state());
                println!("replay_status=frame_stalled");
                return Ok(());
            }
        }
        if let Some(controller) = route.as_deref_mut() {
            controller.observe_frame(&player);
        }
        if release.is_some() || inputs.contains_key(&completed.saturating_sub(3)) {
            eprintln!(
                "frame {}: pointer {:?}, routes {:?}",
                completed + 1,
                player.host_pointer_logical_position(),
                player.host_input_route_receipts()
            );
        }
        let frame = completed + 1;
        if story_trace {
            let (frame, state, scene, group, record) = player.host_story_trace();
            let next = (state, scene, group, record);
            if last_story.as_ref().map(|(state, scene, group, record): &(_, String, _, _)|
                (*state, scene.as_str(), *group, *record)) != Some(next)
            {
                println!("story frame={frame} state={state} scene={scene} group={group} record={record}");
                last_story = Some((state, scene.to_owned(), group, record));
                log_font_substitutions(&player, &mut logged_fonts);
            }
        }
        if probes.contains(&frame) {
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
    }
    if state_mode {
        eprintln!("replay wait: {}", player.host_replay_wait_diagnostics());
        println!("replay_status=frame_limit");
    }
    if story_trace {
        log_font_substitutions(&player, &mut logged_fonts);
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
