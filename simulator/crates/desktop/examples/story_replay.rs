use anyhow::{Context, Result, ensure};
use cs2_desktop::{HeadlessPlayer, HostWindowRequest};
use cs2_runtime::{InputEvent, InputKey};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

enum Action {
    Pointer { x: i32, y: i32, click: bool },
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
    let mut inputs = BTreeMap::new();
    let mut captures = BTreeSet::new();
    let mut probes = BTreeSet::new();
    let mut capture_every = 60;
    let mut story_events = false;
    for arg in args {
        let values = arg.to_string_lossy();
        if values == "story-events" {
            story_events = true;
            continue;
        }
        if let Some(interval) = values.strip_prefix("capture-every:") {
            capture_every = interval.parse()?;
            continue;
        }
        let checkpoint = values.strip_prefix("capture:").map(|frame| (frame, false))
            .or_else(|| values.strip_prefix("inspect:").map(|frame| (frame, true)));
        if let Some((frame, inspect)) = checkpoint {
            let frame = frame.parse::<u64>()?;
            ensure!(
                (1..=frames).contains(&frame),
                "capture frame {frame} outside run"
            );
            captures.insert(frame);
            if inspect {
                probes.insert(frame);
            }
            continue;
        }
        let (position_only, values) = match values.strip_prefix("move:") {
            Some(values) => (true, values),
            None => (false, values.as_ref()),
        };
        let (frame, point) = values
            .split_once(':')
            .context("expected frame:x,y or frame:key")?;
        let command = if let Some((x, y)) = point.split_once(',') {
            Action::Pointer {
                x: x.parse()?,
                y: y.parse()?,
                click: !position_only,
            }
        } else {
            ensure!(!position_only, "expected move:frame:x,y");
            Action::Key(InputKey::Keyboard(point.parse()?))
        };
        let frame = frame.parse::<u64>()?;
        ensure!(frame < frames, "input frame {frame} outside run");
        ensure!(
            inputs.insert(frame, command).is_none(),
            "duplicate input at frame {frame}"
        );
    }

    let mut player = HeadlessPlayer::open_with_save_root(game, output.with_extension("profile"))?;
    player.enable_host_window_requests();
    player.initialize_general_window_host(1)?;
    player.set_host_input_update_rate(60);
    player.enable_host_system_open_queue();
    player.enable_host_tweet_queue();
    player.enable_host_dialog_queue();
    player.load_startup()?;
    player.apply_host_input(InputEvent::FocusChanged(true))?;
    let mut sent_events = 0;
    let mut sent_logs = 0;
    let mut windowed = 1;
    let mut release = None;
    for completed in 0..frames {
        if release.is_some_and(|(frame, _)| completed == frame) {
            let (_, key) = release.take().unwrap();
            player.apply_host_input(InputEvent::KeyUp(key))?;
        }
        if let Some(input) = inputs.get(&completed) {
            let key = match *input {
                Action::Pointer { x, y, click } => {
                    player.apply_host_input(InputEvent::MouseMove { x, y })?;
                    eprintln!(
                        "frame {completed}: pointer ({x},{y}) unmasked={:?} masked={:?}",
                        player.input_region_at([x, y]),
                        player.masked_input_region_at([x, y])?
                    );
                    click.then_some(InputKey::MouseButton(0))
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
            for event in &player.events()[sent_events..] {
                if !story_events || event.kind != "native-call"
                    || event.detail.contains(" debug=") || event.detail.contains(" movie_")
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
                let mode = if matches!(request, HostWindowRequest::ToggleFullscreen) {
                    windowed ^= 1;
                    Some(windowed)
                } else {
                    None
                };
                player.complete_host_window_request(mode)?;
            }
            let advanced = result.with_context(|| format!("frame {completed}"))?;
            if player.quit_requested() {
                eprintln!("frame {completed}: game requested normal exit");
                return Ok(());
            }
            if advanced {
                break;
            }
            retries += 1;
            ensure!(
                retries < 128,
                "stalled after {completed} completed frames: {}",
                player.host_tick_state()
            );
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
        if probes.contains(&frame) {
            eprintln!("frame {frame}:\n{}", player.host_composition_diagnostic());
        }
        if (capture_every != 0 && frame % capture_every == 0)
            || frame == frames
            || captures.contains(&frame)
        {
            let [width, height] = player.logical_size();
            let path = output.with_extension(format!("{:04}.png", completed + 1));
            image::save_buffer(
                &path,
                player.frame_rgba(),
                width,
                height,
                image::ColorType::Rgba8,
            )?;
            eprintln!("frame {}: {}", completed + 1, path.display());
        }
    }
    Ok(())
}
