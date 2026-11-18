use anyhow::{Context, Result, bail};
use cs2_desktop::{
    CompatibilityReport, HeadlessPlayer, HeadlessReport, LogicEvent, ScriptTextEncoding,
    run_windowed, select_game_directory,
};
use cs2_runtime::{
    ArchiveStatus, ConfigCandidate, GameRoot, GameScan, ResourceLocation, ScanDiagnosticLevel,
};
use cs2_script::KcsProgram;
use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::time::Instant;

mod content_audit;
mod report;

const USAGE: &str = "\
CatSystem2 player and diagnostics

USAGE:
  cs2-player help
  cs2-player <game-directory>
  cs2-player <game-directory> [run options]
  cs2-player scan <game-directory> [--json]
  cs2-player list <game-directory> [--archives|--resources]
  cs2-player load <game-directory> <entry>
  cs2-player run <game-directory> [run options]
  cs2-player log <game-directory> [run options]
  cs2-player trace <game-directory> [run options]
  cs2-player diff <left-game-directory> <right-game-directory> [run options]
  cs2-player smoke <sample>... [run options]
  cs2-player select [--window]
  cs2-player compat <game-directory> [--json]
  cs2-player compat-batch [--json] [--output <json-path>] <game-directory>...
  cs2-player report <game-directory> [--output <json-path>]
  cs2-player report-batch [--output <json-path>] <game-directory>...
  cs2-player audit <game-directory>
  cs2-player audit-batch [--output <json-path>] <game-directory>...
  cs2-player window [--json] [<game-directory>]

The player reads game files and INT archives directly. It never starts the
original game executable. A directory without options opens the game window;
`run` or explicit run options use the headless diagnostic host.
Run options: --entry <name>, --frames <n>, --budget <n>, --message <a,b,c,d>,
             --encoding <cp932|gbk|utf-8>, --json.
`trace` reports static KCS opcodes plus runtime
native/VM events; dynamic opcode callbacks are not exposed by the desktop
headless API.
Set CS2_LOG_PROGRESS=1 to show `log` stages on stderr as they begin.
";

#[derive(Debug, Clone, PartialEq, Eq)]
struct RunOptions {
    entry: Option<String>,
    frames: u64,
    budget: usize,
    encoding: ScriptTextEncoding,
    messages: Vec<[u32; 4]>,
    json: bool,
}

fn main() {
    if let Err(error) = run_cli() {
        eprintln!("error: {error:#}");
        process::exit(2);
    }
}

fn run_cli() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let Some(command_arg) = args.next() else {
        print!("{USAGE}");
        return Ok(());
    };
    let command = command_arg.to_string_lossy();

    match command.as_ref() {
        "help" | "--help" | "-h" => {
            ensure_no_args(args, "help")?;
            print!("{USAGE}");
            Ok(())
        }
        "scan" => {
            let path = required_path(&mut args, "scan requires <game-directory>")?;
            let json = parse_scan_options(args.collect())?;
            let scan = scan_game(&path)?;
            let compatibility = CompatibilityReport::from_scan(&scan);
            if json {
                print_json(&report::scan_json(&scan)?)?;
            } else {
                print_scan(&scan);
                print_compatibility(compatibility);
            }
            Ok(())
        }
        "compat" => {
            let path = required_path(&mut args, "compat requires <game-directory>")?;
            let json = parse_json_flag(args.collect(), "compat")?;
            let scan = scan_game(&path)?;
            let compatibility = CompatibilityReport::from_scan(&scan);
            if json {
                print_json(&report::compatibility_json(&compatibility, Some(&scan)))?;
            } else {
                print_compatibility(compatibility);
            }
            Ok(())
        }
        "compat-batch" => run_compatibility_batch(args.collect()),
        "report" => {
            let path = required_path(&mut args, "report requires <game-directory>")?;
            let output = parse_output_option(args.collect(), "report")?;
            let scan = scan_game(&path)?;
            let value = report::game_report(&path, &scan)?;
            write_or_print_report(output.as_deref(), &value)?;
            Ok(())
        }
        "report-batch" => run_game_report_batch(args.collect()),
        "audit" => {
            let path = required_path(&mut args, "audit requires <game-directory>")?;
            ensure_no_args(args, "audit")?;
            let audit = content_audit::audit_game(&path)?;
            print_json(&report::audit_json(&audit)?)?;
            Ok(())
        }
        "audit-batch" => run_content_audit_batch(args.collect()),
        "list" => {
            let path = required_path(&mut args, "list requires <game-directory>")?;
            let archives = parse_list_options(args.collect())?;
            let scan = scan_game(&path)?;
            if archives {
                print_archives(&scan);
            } else {
                print_resources(&scan);
            }
            Ok(())
        }
        "load" => {
            let path = required_path(&mut args, "load requires <game-directory>")?;
            let entry = required_text(args.next(), "load requires <entry>")?;
            ensure_no_args(args, "load")?;
            let mut player = open_player(&path)?;
            let loaded = player
                .load_entry(&entry)
                .with_context(|| format!("load entry `{entry}`"))?;
            println!(
                "loaded {} {} ({} bytes): {}",
                loaded.kind, loaded.name, loaded.byte_length, loaded.detail
            );
            print_logs(player.log());
            print_events(player.events());
            Ok(())
        }
        "run" => {
            let path = required_path(&mut args, "run requires <game-directory>")?;
            let options = parse_run_options(args.collect(), 1)?;
            let report = run_path(path, &options)?;
            if options.json {
                print_json(&report::headless_json(&report, "run"))?;
            } else {
                print_report(&report);
            }
            Ok(())
        }
        "log" => {
            let path = required_path(&mut args, "log requires <game-directory>")?;
            let options = parse_run_options(args.collect(), 1)?;
            let report = run_path_unchecked(path, &options, true)?;
            if options.json {
                print_json(&report::headless_json(&report, "log"))?;
            } else {
                print_logs(&report.log);
                print_events(&report.events);
            }
            ensure_no_runtime_failures(&report)
        }
        "trace" => {
            let path = required_path(&mut args, "trace requires <game-directory>")?;
            let options = parse_run_options(args.collect(), 1)?;
            print_trace(&path, &options)
        }
        "diff" => {
            let left = required_path(&mut args, "diff requires <left-game-directory>")?;
            let right = required_path(&mut args, "diff requires <right-game-directory>")?;
            let (options, output) = parse_diff_options(args.collect())?;
            run_diff(left, right, &options, output.as_deref())
        }
        "smoke" => {
            let (paths, options) = parse_smoke_args(args.collect())?;
            run_smoke(paths, &options)
        }
        "select" => {
            let launch_window = match args.next() {
                None => false,
                Some(value) if value == OsStr::new("--window") => {
                    ensure_no_args(args, "select")?;
                    true
                }
                Some(value) => bail!(
                    "unknown select option {}; use --window",
                    value.to_string_lossy()
                ),
            };
            match select_game_directory()? {
                Some(path) if launch_window => run_windowed(path)?,
                Some(path) => {
                    println!("selected-game-directory: {}", stable_path(&path));
                    let scan = scan_game(&path)?;
                    print_compatibility(CompatibilityReport::from_scan(&scan));
                }
                None => println!("selection-cancelled"),
            }
            Ok(())
        }
        "window" => {
            let mut json = false;
            let first = args.next();
            let first = match first {
                Some(value) if value == OsStr::new("--json") => {
                    json = true;
                    args.next()
                }
                value => value,
            };
            let path = match first {
                Some(path) => {
                    ensure_no_args(args, "window")?;
                    Some(PathBuf::from(path))
                }
                None => {
                    ensure_no_args(args, "window")?;
                    select_game_directory()?
                }
            };
            if let Some(path) = path {
                let scan = scan_game(&path)?;
                if json {
                    print_json(&report::window_preflight_json(&scan)?)?;
                    Ok(())
                } else {
                    run_windowed(path)
                }
            } else {
                Ok(())
            }
        }
        _ if Path::new(&command_arg).is_dir() => {
            let path = PathBuf::from(command_arg);
            let args = args.collect::<Vec<_>>();
            if args.is_empty() {
                return run_windowed(path);
            }
            let options = parse_run_options(args, 1)?;
            let report = run_path(path, &options)?;
            if options.json {
                print_json(&report::headless_json(&report, "run"))?;
            } else {
                print_report(&report);
            }
            Ok(())
        }
        _ => bail!("unknown command {command}\n\n{USAGE}"),
    }
}

fn run_content_audit_batch(args: Vec<OsString>) -> Result<()> {
    let mut output = None;
    let mut paths = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == OsStr::new("--output") {
            if output.is_some() {
                bail!("audit-batch option --output was specified more than once");
            }
            index += 1;
            output = Some(PathBuf::from(
                args.get(index)
                    .context("audit-batch --output requires a JSON path")?,
            ));
        } else {
            paths.push(PathBuf::from(&args[index]));
        }
        index += 1;
    }
    if paths.is_empty() {
        bail!("audit-batch requires at least one <game-directory>");
    }

    let audit = content_audit::audit_workspace(&paths)?;
    let value = report::workspace_audit_json(&audit)?;
    let game_count = value
        .get("game_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    if let Some(path) = output {
        report::write_json(&path, &value)?;
        println!("audited {} sample(s); wrote {}", game_count, path.display());
    } else {
        print_json(&value)?;
    }
    Ok(())
}

fn run_game_report_batch(args: Vec<OsString>) -> Result<()> {
    let mut output = None;
    let mut paths = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == OsStr::new("--output") {
            if output.is_some() {
                bail!("report-batch option --output was specified more than once");
            }
            index += 1;
            output = Some(PathBuf::from(
                args.get(index)
                    .context("report-batch --output requires a JSON path")?,
            ));
        } else {
            paths.push(PathBuf::from(&args[index]));
        }
        index += 1;
    }
    if paths.is_empty() {
        bail!("report-batch requires at least one <game-directory>");
    }

    let value = report::game_report_batch(&paths)?;
    let report_count = value
        .get("sample_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    if let Some(path) = output {
        report::write_json(&path, &value)?;
        println!(
            "reported {} sample(s); wrote {}",
            report_count,
            path.display()
        );
    } else {
        print_json(&value)?;
    }
    let failure_count = value
        .get("failure_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    if failure_count != 0 {
        bail!(
            "report-batch failed: {failure_count}/{} sample(s)",
            paths.len()
        );
    }
    Ok(())
}

fn open_player(path: &Path) -> Result<HeadlessPlayer> {
    HeadlessPlayer::open(path).with_context(|| format!("open game directory `{}`", path.display()))
}

fn scan_game(path: &Path) -> Result<GameScan> {
    GameRoot::open(path)
        .with_context(|| format!("open game directory `{}`", path.display()))?
        .scan()
        .with_context(|| format!("scan game directory `{}`", path.display()))
}

fn run_path(path: PathBuf, options: &RunOptions) -> Result<HeadlessReport> {
    let report = run_path_unchecked(path, options, false)?;
    ensure_no_runtime_failures(&report)?;
    Ok(report)
}

fn run_path_unchecked(path: PathBuf, options: &RunOptions, preserve_failed_report: bool) -> Result<HeadlessReport> {
    let progress_start = (preserve_failed_report
        && env::var_os("CS2_LOG_PROGRESS").as_deref() == Some(OsStr::new("1")))
        .then(Instant::now);
    let progress = |stage| {
        if let Some(start) = progress_start.as_ref() {
            let _ = writeln!(
                io::stderr(),
                "log-progress stage={stage} elapsed_ms={}",
                start.elapsed().as_millis()
            );
        }
    };

    progress("open-game");
    let mut player = open_player(&path)?;
    player.set_script_encoding(options.encoding);
    progress("load-entry");
    if let Some(entry) = &options.entry {
        player
            .load_entry(entry)
            .with_context(|| format!("load entry `{entry}`"))?;
    } else {
        player.load_startup()?;
    }
    for message in &options.messages {
        player.queue_message(*message);
    }
    progress("run-frames");
    let result = player.run_frames(options.frames, options.budget);
    progress("collect-report");
    let report = player.report();
    progress("report-ready");
    if let Err(cause) = result {
        // Hosts retain detailed failures in events. Include the final events
        // before returning so a failed run does not discard its only cause.
        if preserve_failed_report {
            if options.json { print_json(&report::headless_json(&report, "log"))?; }
            else { print_logs(&report.log); print_events(&report.events); }
        }
        let recent = report.events[report.events.len().saturating_sub(8)..]
            .iter()
            .map(|event| format!("frame={} {} {}", event.frame, event.kind, event.detail))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(cause.context(format!(
            "run {} frame(s) with instruction budget {}; recent events:\n{}",
            options.frames, options.budget, recent
        )));
    }
    Ok(report)
}

fn ensure_no_runtime_failures(report: &HeadlessReport) -> Result<()> {
    let failures = report
        .events
        .iter()
        .filter(|event| is_runtime_failure(event))
        .map(|event| format!("frame={} {} {}", event.frame, event.kind, event.detail))
        .collect::<Vec<_>>();
    if !failures.is_empty() {
        bail!(
            "runtime reported {} failure event(s):\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
    Ok(())
}

fn is_runtime_failure(event: &LogicEvent) -> bool {
    event.kind == "vm-error"
        || event.kind == "task-error"
        || event.kind == "native-call"
            && event
                .detail
                .to_ascii_lowercase()
                .contains("is not implemented")
}

fn parse_smoke_args(args: Vec<OsString>) -> Result<(Vec<PathBuf>, RunOptions)> {
    let mut paths = Vec::new();
    let mut run_args = Vec::new();
    let mut options_enabled = true;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if options_enabled && arg == OsStr::new("--") {
            options_enabled = false;
            index += 1;
            continue;
        }
        if options_enabled
            && matches!(
                arg.as_os_str(),
                value if value == OsStr::new("--entry")
                    || value == OsStr::new("--frames")
                    || value == OsStr::new("--budget")
                    || value == OsStr::new("--encoding")
                    || value == OsStr::new("--message")
            )
        {
            run_args.push(arg.clone());
            index += 1;
            if index == args.len() {
                run_args.push(OsString::new());
                break;
            }
            run_args.push(args[index].clone());
        } else if options_enabled && arg == OsStr::new("--json") {
            run_args.push(arg.clone());
        } else if options_enabled && arg.to_string_lossy().starts_with("--") {
            run_args.push(arg.clone());
        } else {
            paths.push(PathBuf::from(arg));
        }
        index += 1;
    }
    if paths.is_empty() {
        bail!("smoke requires at least one <sample> path");
    }
    Ok((paths, parse_run_options(run_args, 12)?))
}

fn required_path(
    args: &mut impl Iterator<Item = OsString>,
    message: &'static str,
) -> Result<PathBuf> {
    args.next().map(PathBuf::from).context(message)
}

fn required_text(value: Option<OsString>, message: &'static str) -> Result<String> {
    let value = value.context(message)?;
    let text = value.to_string_lossy().into_owned();
    if text.is_empty() {
        bail!("{message}");
    }
    Ok(text)
}

fn ensure_no_args(mut args: impl Iterator<Item = OsString>, command: &str) -> Result<()> {
    if let Some(value) = args.next() {
        bail!(
            "{command} does not accept argument `{}`",
            value.to_string_lossy()
        );
    }
    Ok(())
}

fn parse_scan_options(args: Vec<OsString>) -> Result<bool> {
    let mut json = false;
    for arg in args {
        if arg == OsStr::new("--json") {
            if json {
                bail!("scan option --json was specified more than once");
            }
            json = true;
        } else {
            bail!("unknown scan option {}; use --json", arg.to_string_lossy());
        }
    }
    Ok(json)
}

fn parse_json_flag(args: Vec<OsString>, command: &str) -> Result<bool> {
    let mut json = false;
    for arg in args {
        if arg == OsStr::new("--json") {
            if json {
                bail!("{command} option --json was specified more than once");
            }
            json = true;
        } else {
            bail!(
                "unknown {command} option {}; use --json",
                arg.to_string_lossy()
            );
        }
    }
    Ok(json)
}

fn parse_output_option(args: Vec<OsString>, command: &str) -> Result<Option<PathBuf>> {
    let mut output = None;
    let mut index = 0;
    while index < args.len() {
        if args[index] != OsStr::new("--output") {
            bail!(
                "unknown {command} option {}; use --output <json-path>",
                args[index].to_string_lossy()
            );
        }
        if output.is_some() {
            bail!("{command} option --output was specified more than once");
        }
        index += 1;
        output = Some(PathBuf::from(
            args.get(index)
                .context(format!("{command} --output requires a JSON path"))?,
        ));
        index += 1;
    }
    Ok(output)
}

fn parse_list_options(args: Vec<OsString>) -> Result<bool> {
    let mut archives = false;
    let mut resources = false;
    for arg in args {
        if arg == OsStr::new("--archives") {
            if archives {
                bail!("list option --archives was specified more than once");
            }
            archives = true;
        } else if arg == OsStr::new("--resources") {
            if resources {
                bail!("list option --resources was specified more than once");
            }
            resources = true;
        } else {
            bail!(
                "unknown list option {}; use --archives or --resources",
                arg.to_string_lossy()
            );
        }
    }
    if archives && resources {
        bail!("list accepts only one of --archives or --resources");
    }
    Ok(archives)
}

fn parse_run_options(args: Vec<OsString>, default_frames: u64) -> Result<RunOptions> {
    if default_frames == 0 {
        bail!("internal error: default frame count must be greater than zero");
    }
    let mut options = RunOptions {
        entry: None,
        frames: default_frames,
        budget: 10_000,
        encoding: ScriptTextEncoding::default(),
        messages: Vec::new(),
        json: false,
    };
    let mut seen_frames = false;
    let mut seen_budget = false;
    let mut seen_encoding = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].to_string_lossy().as_ref() {
            "--entry" => {
                if options.entry.is_some() {
                    bail!("run option --entry was specified more than once");
                }
                index += 1;
                options.entry = Some(required_text(
                    args.get(index).cloned(),
                    "--entry requires a non-empty value",
                )?);
            }
            "--frames" => {
                if seen_frames {
                    bail!("run option --frames was specified more than once");
                }
                seen_frames = true;
                index += 1;
                options.frames = parse_positive_u64(args.get(index), "--frames")?;
            }
            "--budget" => {
                if seen_budget {
                    bail!("run option --budget was specified more than once");
                }
                seen_budget = true;
                index += 1;
                options.budget = parse_positive_usize(args.get(index), "--budget")?;
            }
            "--encoding" => {
                if seen_encoding {
                    bail!("run option --encoding was specified more than once");
                }
                seen_encoding = true;
                index += 1;
                options.encoding = parse_encoding(args.get(index))?;
            }
            "--message" => {
                index += 1;
                options.messages.push(parse_message(args.get(index))?);
            }
            "--json" => {
                if options.json {
                    bail!("run option --json was specified more than once");
                }
                options.json = true;
            }
            value => bail!("unknown run option {value}; see `cs2-player help`"),
        }
        index += 1;
    }
    Ok(options)
}

fn parse_diff_options(args: Vec<OsString>) -> Result<(RunOptions, Option<PathBuf>)> {
    let mut run_args = Vec::new();
    let mut output = None;
    let mut index = 0;
    while index < args.len() {
        if args[index] == OsStr::new("--output") {
            if output.is_some() {
                bail!("diff option --output was specified more than once");
            }
            index += 1;
            output = Some(PathBuf::from(
                args.get(index)
                    .context("diff --output requires a JSON path")?,
            ));
        } else {
            run_args.push(args[index].clone());
        }
        index += 1;
    }
    Ok((parse_run_options(run_args, 1)?, output))
}

fn parse_encoding(value: Option<&OsString>) -> Result<ScriptTextEncoding> {
    let value = value.context("--encoding requires cp932, gbk, or utf-8")?;
    let normalized = value.to_string_lossy().to_ascii_lowercase();
    match normalized.as_str() {
        "cp932" | "shift-jis" | "shift_jis" => Ok(ScriptTextEncoding::Cp932),
        "gbk" | "gb2312" => Ok(ScriptTextEncoding::Gbk),
        "utf-8" | "utf8" => Ok(ScriptTextEncoding::Utf8),
        value => bail!("unknown script encoding {value}; use cp932, gbk, or utf-8"),
    }
}

fn parse_message(value: Option<&OsString>) -> Result<[u32; 4]> {
    let value = value.context("--message requires four comma-separated DWORDs")?;
    let text = value.to_string_lossy();
    let parts = text.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.len() != 4 {
        bail!("--message requires exactly four comma-separated DWORDs");
    }
    let mut output = [0_u32; 4];
    for (index, part) in parts.into_iter().enumerate() {
        if part.is_empty() {
            bail!("--message DWORD {} is empty", index + 1);
        }
        output[index] =
            if let Some(hex) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
                u32::from_str_radix(hex, 16)
            } else {
                part.parse()
            }
            .map_err(|_| anyhow::anyhow!("invalid message DWORD: {part}"))?;
    }
    Ok(output)
}

fn parse_positive_u64(value: Option<&OsString>, option: &str) -> Result<u64> {
    let value = value.context(format!("{option} requires a positive integer"))?;
    let text = value.to_string_lossy();
    let parsed = text
        .parse::<u64>()
        .map_err(|error| anyhow::anyhow!("{option}: invalid positive integer `{text}`: {error}"))?;
    if parsed == 0 {
        bail!("{option} must be greater than zero");
    }
    Ok(parsed)
}

fn parse_positive_usize(value: Option<&OsString>, option: &str) -> Result<usize> {
    let value = value.context(format!("{option} requires a positive integer"))?;
    let text = value.to_string_lossy();
    let parsed = text
        .parse::<usize>()
        .map_err(|error| anyhow::anyhow!("{option}: invalid positive integer `{text}`: {error}"))?;
    if parsed == 0 {
        bail!("{option} must be greater than zero");
    }
    Ok(parsed)
}

fn print_scan(scan: &GameScan) {
    println!("root: {}", scan.root.display());
    println!("engine: {:?}/{:?}", scan.engine.family, scan.engine.variant);
    println!("signature: {}", scan.engine.signature);
    println!("files: {}", scan.resources.files().len());
    println!("archives: {}", scan.resources.archives().len());
    println!("config candidates: {}", scan.config_candidates.len());
    for diagnostic in &scan.diagnostics {
        println!(
            "diagnostic: {}: {}: {}",
            diagnostic_level(&diagnostic.level),
            stable_path(&diagnostic.path),
            diagnostic.message
        );
    }
}

fn print_resources(scan: &GameScan) {
    let mut files = scan.resources.files().iter().collect::<Vec<_>>();
    files.sort_by_key(|file| stable_path_key(&file.relative_path));
    for file in files {
        println!("{} {}", file.size, stable_path(&file.relative_path));
    }
}

fn print_archives(scan: &GameScan) {
    let mut archives = scan.resources.archives().iter().collect::<Vec<_>>();
    archives.sort_by_key(|archive| stable_path_key(&archive.path));
    for archive in archives {
        let status = match &archive.status {
            ArchiveStatus::Indexed => "indexed".to_string(),
            ArchiveStatus::Encrypted { raw_entry_count } => {
                format!("encrypted raw_entries={raw_entry_count}")
            }
            ArchiveStatus::NotAnArchive => "not-an-archive".to_string(),
            ArchiveStatus::Malformed { reason } => format!("malformed: {reason}"),
        };
        println!(
            "{} entries={} status={status}",
            stable_path(&archive.path),
            archive.entry_count
        );
        for entry in &archive.entries {
            println!("  {} {}", entry.size, entry.name);
        }
    }
}

fn print_report(report: &HeadlessReport) {
    println!(
        "ran {} frame(s), engine={:?}/{:?}, files={}, archives={}, scene_nodes={}, frame_hash={:016x}",
        report.state.clock.frame,
        report.scan.engine.family,
        report.scan.engine.variant,
        report.scan.resources.files().len(),
        report.scan.resources.archives().len(),
        report.scene_nodes.len(),
        report.frame_hash,
    );
    print_compatibility(report.compatibility.clone());
    if let Some(loaded) = &report.loaded {
        println!(
            "loaded {} {} ({} bytes): {}",
            loaded.kind, loaded.name, loaded.byte_length, loaded.detail
        );
    }
    print_logs(&report.log);
    print_events(&report.events);
}

fn print_logs(log: &[String]) {
    for line in log {
        println!("log: {line}");
    }
}

fn print_events(events: &[LogicEvent]) {
    for event in events {
        println!(
            "event: frame={} {} {}",
            event.frame, event.kind, event.detail
        );
    }
}

fn print_compatibility(report: CompatibilityReport) {
    println!(
        "compatibility: engine={} variant={} pe={} primary={}",
        report.engine_family,
        report.detected_variant,
        report.pe_architecture.as_deref().unwrap_or("unknown"),
        report.primary_executable.as_deref().unwrap_or("none")
    );
    println!(
        "compatibility-window: {}: {}",
        report.window.status, report.window.detail
    );
    println!(
        "compatibility-input: {}: {}",
        report.input.status, report.input.detail
    );
    println!(
        "compatibility-audio: {}: {}",
        report.audio.status, report.audio.detail
    );
    println!(
        "compatibility-video: {}: {}",
        report.video.status, report.video.detail
    );
    println!(
        "compatibility-saves: {}: {}",
        report.saves.status, report.saves.detail
    );
    if !report.runtime_profiles.is_empty() {
        println!("compatibility-abi: {}", report.runtime_profiles.join("; "));
    }
    if !report.script_formats.is_empty() {
        println!(
            "compatibility-scripts: {}",
            report.script_formats.join(", ")
        );
    }
    if !report.audio_formats.is_empty() {
        println!(
            "compatibility-audio-formats: {}",
            report.audio_formats.join(", ")
        );
    }
    if !report.video_formats.is_empty() {
        println!(
            "compatibility-video-formats: {}",
            report.video_formats.join(", ")
        );
    }
    if !report.save_formats.is_empty() {
        println!(
            "compatibility-save-formats: {}",
            report.save_formats.join(", ")
        );
    }
    println!(
        "compatibility-archives: total={} indexed={} encrypted={} malformed={}",
        report.archive_count,
        report.indexed_archives,
        report.encrypted_archives,
        report.malformed_archives
    );
    for boundary in report.boundaries {
        println!("compatibility-boundary: {boundary}");
    }
}

fn run_compatibility_batch(args: Vec<OsString>) -> Result<()> {
    let mut paths = Vec::new();
    let mut output = None;
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        if args[index] == OsStr::new("--json") {
            if json {
                bail!("compat-batch option --json was specified more than once");
            }
            json = true;
        } else if args[index] == OsStr::new("--output") {
            if output.is_some() {
                bail!("compat-batch option --output was specified more than once");
            }
            index += 1;
            output = Some(PathBuf::from(
                args.get(index)
                    .context("compat-batch --output requires a JSON path")?,
            ));
        } else {
            paths.push(PathBuf::from(&args[index]));
        }
        index += 1;
    }
    if paths.is_empty() {
        bail!("compat-batch requires at least one <game-directory>");
    }
    let mut failures = 0;
    let mut results = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        match scan_game(path) {
            Ok(scan) => {
                let compatibility = CompatibilityReport::from_scan(&scan);
                results.push(report::compatibility_json(&compatibility, Some(&scan)));
                if !json {
                    println!(
                        "compat[{}]: path={} engine={} variant={} pe={} scripts={} audio={} video={} saves={} window={} input={} audio-host={} video-host={} archives={}/{} indexed boundaries={}",
                        index + 1,
                        stable_path(path),
                        compatibility.engine_family,
                        compatibility.detected_variant,
                        compatibility
                            .pe_architecture
                            .as_deref()
                            .unwrap_or("unknown"),
                        compatibility.script_formats.join(","),
                        compatibility.audio_formats.join(","),
                        compatibility.video_formats.join(","),
                        compatibility.save_formats.join(","),
                        compatibility.window.status,
                        compatibility.input.status,
                        compatibility.audio.status,
                        compatibility.video.status,
                        compatibility.indexed_archives,
                        compatibility.archive_count,
                        compatibility.boundaries.len()
                    );
                    for boundary in &compatibility.boundaries {
                        println!("  boundary: {boundary}");
                    }
                    if !compatibility.runtime_profiles.is_empty() {
                        println!("  abi: {}", compatibility.runtime_profiles.join("; "));
                    }
                }
            }
            Err(error) => {
                failures += 1;
                results.push(serde_json::json!({
                    "schema_version": report::SCHEMA_VERSION,
                    "kind": "compatibility",
                    "status": "error",
                    "root": stable_path(path),
                    "error": format!("{error:#}"),
                }));
                if !json {
                    println!(
                        "compat[{}]: error path={} detail={error:#}",
                        index + 1,
                        stable_path(path)
                    );
                }
            }
        }
    }
    if json {
        let value = serde_json::json!({
            "schema_version": report::SCHEMA_VERSION,
            "kind": "compatibility-batch",
            "status": if failures == 0 { "complete" } else { "complete-with-errors" },
            "sample_count": paths.len(),
            "failure_count": failures,
            "samples": results,
        });
        write_or_print_report(output.as_deref(), &value)?;
    } else if output.is_some() {
        bail!("compat-batch --output requires --json");
    }
    if failures == 0 {
        Ok(())
    } else {
        bail!("compat-batch failed: {failures}/{} sample(s)", paths.len())
    }
}

fn print_trace(path: &Path, options: &RunOptions) -> Result<()> {
    let player = open_player(path)?;
    let entry = options
        .entry
        .clone()
        .or_else(|| player.default_entry().map(str::to_owned))
        .context("trace requires --entry when no default script entry is detected")?;
    let bytes = player
        .resources()
        .read(&entry)
        .with_context(|| format!("read trace entry `{entry}`"))?;
    let program = KcsProgram::parse(&bytes)
        .with_context(|| format!("trace entry `{entry}` is not a valid KCS program"))?;
    let report = run_path(path.to_path_buf(), options)?;

    let static_native_count = program
        .instructions
        .iter()
        .filter(|instruction| instruction.opcode == 0x2b)
        .count();
    println!(
        "trace: entry={} opcode=static native=runtime frames={}",
        entry, report.state.clock.frame
    );
    println!(
        "trace-summary: static_opcodes={} static_native_calls={} dynamic_native_calls={} vm_events={}",
        program.instructions.len(),
        static_native_count,
        count_events(&report.events, "native-call"),
        report
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind.as_str(),
                    "vm-event" | "vm-error" | "vm-halted" | "vm-yield"
                )
            })
            .count()
    );
    for instruction in &program.instructions {
        println!(
            "opcode-static: pc=0x{:x} opcode=0x{:02x} operands={}",
            instruction.offset,
            instruction.opcode,
            instruction.operands.len()
        );
        if instruction.opcode == 0x2b
            && instruction
                .operands
                .first()
                .is_some_and(|operand| operand.flags.bits() == 3)
        {
            let native_index = instruction.operands[0].payload;
            let argument_bytes = instruction
                .operands
                .get(1)
                .filter(|operand| operand.flags.bits() == 3)
                .map(|operand| operand.payload);
            println!(
                "native-static: pc=0x{:x} index={} argument_bytes={}",
                instruction.offset,
                native_index,
                argument_bytes.map_or_else(|| "unknown".to_string(), |value| value.to_string())
            );
        }
    }
    for event in &report.events {
        if event.kind == "native-call" {
            println!("native-runtime: frame={} {}", event.frame, event.detail);
        } else if matches!(
            event.kind.as_str(),
            "vm-event" | "vm-error" | "vm-halted" | "vm-yield"
        ) {
            println!(
                "vm-runtime: frame={} {} {}",
                event.frame, event.kind, event.detail
            );
        }
    }
    println!("trace-note: dynamic opcode callbacks are unavailable from HeadlessPlayer");
    Ok(())
}

fn run_diff(
    left: PathBuf,
    right: PathBuf,
    options: &RunOptions,
    output: Option<&Path>,
) -> Result<()> {
    let left_report = run_path(left.clone(), options)?;
    let right_report = run_path(right.clone(), options)?;
    let differences = report_differences(&left_report, &right_report);
    if options.json {
        let value = report::diff_json(&left_report, &right_report, &differences);
        write_or_print_report(output, &value)?;
        if !differences.is_empty() {
            bail!(
                "headless diff failed with {} mismatch(es) between `{}` and `{}`",
                differences.len(),
                left.display(),
                right.display()
            );
        }
        return Ok(());
    }
    if differences.is_empty() {
        println!(
            "diff: equal frames={} frame_hash={:016x}",
            left_report.state.clock.frame, left_report.frame_hash
        );
        return Ok(());
    }

    println!("diff: different mismatches={}", differences.len());
    for difference in &differences {
        println!("difference: {difference}");
    }
    bail!(
        "headless diff failed with {} mismatch(es) between `{}` and `{}`",
        differences.len(),
        left.display(),
        right.display()
    )
}

fn report_differences(left: &HeadlessReport, right: &HeadlessReport) -> Vec<String> {
    let mut differences = Vec::new();
    if left.scan.engine != right.scan.engine {
        differences.push("engine fingerprint".to_string());
    }
    if left.scan.resources.files() != right.scan.resources.files() {
        differences.push("resource file manifest".to_string());
    }
    if left.scan.resources.archives() != right.scan.resources.archives() {
        differences.push("resource archive manifest".to_string());
    }
    if normalized_config_candidates(left) != normalized_config_candidates(right) {
        differences.push("config candidates".to_string());
    }
    if left.scan.diagnostics != right.scan.diagnostics {
        differences.push("scan diagnostics".to_string());
    }
    if left.compatibility != right.compatibility {
        differences.push("host compatibility profile".to_string());
    }
    if left.loaded != right.loaded {
        differences.push("loaded entry".to_string());
    }
    if left.state != right.state {
        differences.push("runtime state".to_string());
    }
    if left.scene_nodes != right.scene_nodes {
        differences.push("scene nodes".to_string());
    }
    if left.menu_item_requests != right.menu_item_requests {
        differences.push("menu requests".to_string());
    }
    if left.debug_mode != right.debug_mode {
        differences.push("debug mode".to_string());
    }
    if left.sound_config_enabled != right.sound_config_enabled {
        differences.push("sound configuration".to_string());
    }
    if left.frame_hash != right.frame_hash {
        differences.push(format!(
            "frame hash left={:016x} right={:016x}",
            left.frame_hash, right.frame_hash
        ));
    }
    if left.frame_rgba != right.frame_rgba {
        differences.push(format!(
            "frame pixels left={:016x} right={:016x}",
            stable_bytes_hash(&left.frame_rgba),
            stable_bytes_hash(&right.frame_rgba)
        ));
    }
    if left.fes_exit_code != right.fes_exit_code {
        differences.push("FES exit code".to_string());
    }
    if left.events != right.events {
        differences.push(format!(
            "runtime events left={} right={}",
            left.events.len(),
            right.events.len()
        ));
    }
    let left_log = normalized_logs(left);
    let right_log = normalized_logs(right);
    if left_log != right_log {
        differences.push(format!(
            "runtime logs left={} right={}",
            left_log.len(),
            right_log.len()
        ));
    }
    differences
}

fn normalized_config_candidates(report: &HeadlessReport) -> Vec<ConfigCandidate> {
    let mut candidates = report.scan.config_candidates.clone();
    for candidate in &mut candidates {
        match &mut candidate.resource.location {
            ResourceLocation::File { path } | ResourceLocation::Archive { path, .. } => {
                if let Ok(relative) = path.strip_prefix(&report.root) {
                    *path = relative.to_path_buf();
                }
            }
        }
    }
    candidates
}

fn normalized_logs(report: &HeadlessReport) -> Vec<String> {
    let opened_prefix = format!("opened {}:", report.root.display());
    report
        .log
        .iter()
        .map(|line| {
            line.strip_prefix(&opened_prefix).map_or_else(
                || line.clone(),
                |rest| format!("opened <game-directory>:{rest}"),
            )
        })
        .collect()
}

fn run_smoke(paths: Vec<PathBuf>, options: &RunOptions) -> Result<()> {
    let sample_count = paths.len();
    let mut failures = 0;
    let mut results = Vec::with_capacity(sample_count);
    for (index, path) in paths.iter().enumerate() {
        match run_path(path.clone(), options) {
            Ok(report) if report.frame_in_progress => {
                failures += 1;
                results.push(serde_json::json!({
                    "index": index + 1,
                    "status": "budget-exhausted",
                    "root": stable_path(path),
                    "frames": report.state.clock.frame,
                    "frame_in_progress": true,
                }));
                if !options.json {
                    println!(
                        "smoke[{}]: budget-exhausted path={} frame={}",
                        index + 1, path.display(), report.state.clock.frame
                    );
                }
            }
            Ok(report) => {
                results.push(serde_json::json!({
                    "index": index + 1,
                    "status": "ok",
                    "root": stable_path(path),
                    "frames": report.state.clock.frame,
                    "engine": {
                        "family": format!("{:?}", report.scan.engine.family),
                        "variant": format!("{:?}", report.scan.engine.variant),
                        "signature": report.scan.engine.signature,
                    },
                    "file_count": report.scan.resources.files().len(),
                    "archive_count": report.scan.resources.archives().len(),
                    "frame_hash": format!("{:016x}", report.frame_hash),
                    "native_event_count": count_events(&report.events, "native-call"),
                }));
                if !options.json {
                    println!(
                        "smoke[{}]: ok path={} frames={} engine={:?}/{:?} files={} archives={} frame_hash={:016x} native_events={}",
                        index + 1,
                        path.display(),
                        report.state.clock.frame,
                        report.scan.engine.family,
                        report.scan.engine.variant,
                        report.scan.resources.files().len(),
                        report.scan.resources.archives().len(),
                        report.frame_hash,
                        count_events(&report.events, "native-call")
                    );
                }
            }
            Err(error) => {
                failures += 1;
                results.push(serde_json::json!({
                    "index": index + 1,
                    "status": "error",
                    "root": stable_path(path),
                    "error": format!("{error:#}"),
                }));
                if !options.json {
                    println!(
                        "smoke[{}]: error path={} detail={}",
                        index + 1,
                        path.display(),
                        format!("{error:#}")
                    );
                }
            }
        }
    }
    if options.json {
        print_json(&report::smoke_json(&results, failures, options.frames))?;
    }
    if failures == 0 {
        if !options.json {
            println!(
                "smoke: passed samples={sample_count} frames={}",
                options.frames
            );
        }
        Ok(())
    } else {
        if !options.json {
            println!(
                "smoke: failed samples={failures}/{sample_count} frames={}",
                options.frames
            );
        }
        bail!("smoke failed: {failures}/{sample_count} sample(s) failed")
    }
}

fn count_events(events: &[LogicEvent], kind: &str) -> usize {
    events.iter().filter(|event| event.kind == kind).count()
}

fn print_json(value: &serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn write_or_print_report(output: Option<&Path>, value: &serde_json::Value) -> Result<()> {
    if let Some(path) = output {
        report::write_json(path, value)?;
        println!("wrote {}", path.display());
    } else {
        print_json(value)?;
    }
    Ok(())
}

fn diagnostic_level(level: &ScanDiagnosticLevel) -> &'static str {
    match level {
        ScanDiagnosticLevel::Warning => "warning",
        ScanDiagnosticLevel::Error => "error",
    }
}

fn stable_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn stable_path_key(path: &Path) -> (String, String) {
    let path = stable_path(path);
    (path.to_ascii_lowercase(), path)
}

fn stable_bytes_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_mentions_all_headless_workflows() {
        assert!(USAGE.contains("scan"));
        assert!(USAGE.contains("list"));
        assert!(USAGE.contains("load"));
        assert!(USAGE.contains("run"));
        assert!(USAGE.contains("trace"));
        assert!(USAGE.contains("diff"));
        assert!(USAGE.contains("smoke"));
    }

    #[test]
    fn run_options_parse_defaults_and_values() {
        let args = vec![
            OsString::from("--entry"),
            OsString::from("start.kcs"),
            OsString::from("--frames"),
            OsString::from("4"),
            OsString::from("--budget"),
            OsString::from("32"),
            OsString::from("--message"),
            OsString::from("2,0x0,0,0"),
            OsString::from("--message"),
            OsString::from("3,1,0,0"),
            OsString::from("--encoding"),
            OsString::from("utf8"),
        ];
        assert_eq!(
            parse_run_options(args, 1).unwrap(),
            RunOptions {
                entry: Some("start.kcs".to_string()),
                frames: 4,
                budget: 32,
                encoding: ScriptTextEncoding::Utf8,
                messages: vec![[2, 0, 0, 0], [3, 1, 0, 0]],
                json: false,
            }
        );
    }

    #[test]
    fn run_options_reject_zero_and_duplicate_values() {
        let error = parse_run_options(vec![OsString::from("--frames"), OsString::from("0")], 1)
            .unwrap_err()
            .to_string();
        assert!(error.contains("--frames must be greater than zero"));

        let error = parse_run_options(
            vec![
                OsString::from("--budget"),
                OsString::from("1"),
                OsString::from("--budget"),
                OsString::from("2"),
            ],
            1,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("specified more than once"));
    }
}
