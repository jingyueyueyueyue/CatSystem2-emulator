use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new() -> Self {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after the epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("cs2-cli-{suffix}"));
        fs::create_dir_all(&path).expect("temporary directory should be created");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_kcs(path: &std::path::Path, code: &[u8]) {
    let mut body = Vec::with_capacity(code.len() + 11);
    body.extend_from_slice(&[0x78, 0x01, 0x01]);
    let length = u16::try_from(code.len()).expect("test KCS code should fit in a stored block");
    body.extend_from_slice(&length.to_le_bytes());
    body.extend_from_slice(&(!length).to_le_bytes());
    body.extend_from_slice(code);
    body.extend_from_slice(&adler32(code).to_be_bytes());

    let code_length = u32::try_from(code.len()).expect("test KCS code should fit in a header");
    let body_offset = 52_u32 + code_length;
    let fields = [
        2_u32,
        0,
        52,
        code_length,
        body_offset,
        0,
        0,
        0,
        64,
        body_offset,
        0,
        code_length,
    ];
    let mut output = Vec::with_capacity(52 + body.len());
    output.extend_from_slice(b"KCS\0");
    for field in fields {
        output.extend_from_slice(&field.to_le_bytes());
    }
    output.extend_from_slice(&body);
    fs::write(path, output).expect("KCS fixture should be written");
}

fn adler32(bytes: &[u8]) -> u32 {
    let mut first = 1_u32;
    let mut second = 0_u32;
    for byte in bytes {
        first = (first + u32::from(*byte)) % 65_521;
        second = (second + first) % 65_521;
    }
    (second << 16) | first
}

fn push_immediate(value: u32, output: &mut Vec<u8>) {
    output.extend_from_slice(&[0x25, 0x23]);
    output.extend_from_slice(&value.to_le_bytes());
}

fn unimplemented_native_program(index: u32) -> Vec<u8> {
    let mut code = Vec::new();
    push_immediate(index, &mut code);
    push_immediate(0, &mut code);
    code.extend_from_slice(&[0x2b, 0x23]);
    code.extend_from_slice(&index.to_le_bytes());
    code.extend_from_slice(&[0x23, 0, 0, 0, 0, 0x3b]);
    code
}

#[test]
fn help_and_scan_are_executable_workflows() {
    let help = Command::new(env!("CARGO_BIN_EXE_cs2-player"))
        .arg("help")
        .output()
        .expect("cs2-player help should start");
    assert!(help.status.success());
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("cs2-player scan"));
    assert!(help.contains("cs2-player run"));

    let temp = TempDir::new();
    fs::write(temp.0.join("sample.txt"), b"fixture").expect("fixture should be written");
    let scan = Command::new(env!("CARGO_BIN_EXE_cs2-player"))
        .args(["scan", temp.0.to_str().expect("temporary path is UTF-8")])
        .output()
        .expect("cs2-player scan should start");
    assert!(scan.status.success());
    let scan = String::from_utf8_lossy(&scan.stdout);
    assert!(scan.contains("files: 1"));
    assert!(scan.contains("archives: 0"));
}

#[test]
fn direct_run_and_smoke_work_without_an_original_executable() {
    let first = TempDir::new();
    let second = TempDir::new();
    let third = TempDir::new();
    for temp in [&first, &second, &third] {
        fs::write(temp.0.join("sample.txt"), b"fixture").expect("fixture should be written");
        write_kcs(&temp.0.join("main.kcs"), &[0x3b]);
    }

    let direct = Command::new(env!("CARGO_BIN_EXE_cs2-player"))
        .args([
            first.0.to_str().expect("temporary path is UTF-8"),
            "--frames",
            "2",
            "--budget",
            "8",
        ])
        .output()
        .expect("direct headless run should start");
    assert!(direct.status.success());
    let direct_stdout = String::from_utf8_lossy(&direct.stdout);
    assert!(direct_stdout.contains("ran 2 frame(s)"));
    assert!(direct_stdout.contains("log: ran 2 logical frame(s)"));
    assert!(direct_stdout.contains("event: frame=1 logic-frame headless tick"));

    let smoke = Command::new(env!("CARGO_BIN_EXE_cs2-player"))
        .args([
            "smoke",
            first.0.to_str().expect("temporary path is UTF-8"),
            second.0.to_str().expect("temporary path is UTF-8"),
            third.0.to_str().expect("temporary path is UTF-8"),
            "--frames",
            "2",
            "--budget",
            "8",
        ])
        .output()
        .expect("three-sample smoke workflow should start");
    assert!(smoke.status.success());
    let smoke_stdout = String::from_utf8_lossy(&smoke.stdout);
    assert!(smoke_stdout.contains("smoke[1]: ok"));
    assert!(smoke_stdout.contains("smoke[2]: ok"));
    assert!(smoke_stdout.contains("smoke[3]: ok"));
    assert!(smoke_stdout.contains("smoke: passed samples=3 frames=2"));
}

#[test]
fn run_rejects_vm_errors_but_accepts_normal_vm_halt() {
    let healthy = TempDir::new();
    let broken = TempDir::new();
    write_kcs(&healthy.0.join("healthy.kcs"), &[0x3b]);
    write_kcs(
        &broken.0.join("broken.kcs"),
        &unimplemented_native_program(99),
    );

    let halted = Command::new(env!("CARGO_BIN_EXE_cs2-player"))
        .args([
            "run",
            healthy.0.to_str().expect("temporary path is UTF-8"),
            "--entry",
            "healthy.kcs",
            "--frames",
            "1",
            "--budget",
            "8",
        ])
        .output()
        .expect("normal halt run should start");
    assert!(
        halted.status.success(),
        "normal halt failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&halted.stdout),
        String::from_utf8_lossy(&halted.stderr)
    );
    assert!(String::from_utf8_lossy(&halted.stdout).contains("vm-halted"));

    let failed = Command::new(env!("CARGO_BIN_EXE_cs2-player"))
        .args([
            "run",
            broken.0.to_str().expect("temporary path is UTF-8"),
            "--entry",
            "broken.kcs",
            "--frames",
            "1",
            "--budget",
            "8",
        ])
        .output()
        .expect("VM error run should start");
    assert!(!failed.status.success());
    let stderr = String::from_utf8_lossy(&failed.stderr);
    assert!(stderr.contains("runtime reported 1 failure event"));
    assert!(stderr.contains("vm-error"));
    assert!(stderr.contains("pc="));
    assert!(stderr.contains("opcode="));
    assert!(stderr.contains("KCS native call 99 is not implemented"));
}

#[test]
fn smoke_accepts_one_or_more_paths_and_reports_each_failure() {
    let first = TempDir::new();
    let second = TempDir::new();
    let third = TempDir::new();
    write_kcs(&first.0.join("start.kcs"), &[0x3b]);
    write_kcs(
        &second.0.join("start.kcs"),
        &unimplemented_native_program(123),
    );
    write_kcs(&third.0.join("start.kcs"), &[0x3b]);

    let single = Command::new(env!("CARGO_BIN_EXE_cs2-player"))
        .args([
            "smoke",
            first.0.to_str().expect("temporary path is UTF-8"),
            "--frames",
            "1",
            "--budget",
            "8",
            "--entry",
            "start.kcs",
        ])
        .output()
        .expect("single-sample smoke should start");
    assert!(single.status.success());
    let single_stdout = String::from_utf8_lossy(&single.stdout);
    assert!(single_stdout.contains("smoke[1]: ok"));
    assert!(single_stdout.contains("smoke: passed samples=1 frames=1"));

    let batch = Command::new(env!("CARGO_BIN_EXE_cs2-player"))
        .args([
            "smoke",
            "--frames",
            "1",
            "--budget",
            "8",
            "--entry",
            "start.kcs",
            first.0.to_str().expect("temporary path is UTF-8"),
            second.0.to_str().expect("temporary path is UTF-8"),
            third.0.to_str().expect("temporary path is UTF-8"),
        ])
        .output()
        .expect("multi-sample smoke should start");
    assert!(
        !batch.status.success(),
        "unexpected smoke success:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&batch.stdout),
        String::from_utf8_lossy(&batch.stderr)
    );
    let batch_stdout = String::from_utf8_lossy(&batch.stdout);
    assert!(batch_stdout.contains("smoke[1]: ok"));
    assert!(batch_stdout.contains("smoke[2]: error"));
    assert!(batch_stdout.contains("smoke[3]: ok"));
    assert!(batch_stdout.contains("vm-error"));
    assert!(batch_stdout.contains("KCS native call 123 is not implemented"));
    assert!(batch_stdout.contains("smoke: failed samples=1/3 frames=1"));
    assert!(String::from_utf8_lossy(&batch.stderr).contains("smoke failed: 1/3"));
}

#[test]
fn diff_has_stable_success_and_failure_exit_codes() {
    let left = TempDir::new();
    let equal = TempDir::new();
    let different = TempDir::new();
    fs::write(left.0.join("sample.txt"), b"fixture").expect("fixture should be written");
    fs::write(equal.0.join("sample.txt"), b"fixture").expect("fixture should be written");
    fs::write(different.0.join("sample.txt"), b"different").expect("fixture should be written");
    for temp in [&left, &equal, &different] {
        fs::create_dir_all(temp.0.join("config")).expect("config directory should be created");
        fs::write(temp.0.join("config/startup.xml"), b"<startup/>")
            .expect("config fixture should be written");
    }

    let same = Command::new(env!("CARGO_BIN_EXE_cs2-player"))
        .args([
            "diff",
            left.0.to_str().expect("temporary path is UTF-8"),
            equal.0.to_str().expect("temporary path is UTF-8"),
            "--frames",
            "2",
        ])
        .output()
        .expect("equal diff should start");
    assert!(
        same.status.success(),
        "equal diff failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&same.stdout),
        String::from_utf8_lossy(&same.stderr)
    );
    assert!(String::from_utf8_lossy(&same.stdout).contains("diff: equal frames=2"));

    let changed = Command::new(env!("CARGO_BIN_EXE_cs2-player"))
        .args([
            "diff",
            left.0.to_str().expect("temporary path is UTF-8"),
            different.0.to_str().expect("temporary path is UTF-8"),
            "--frames",
            "2",
        ])
        .output()
        .expect("different diff should start");
    assert!(!changed.status.success());
    assert!(String::from_utf8_lossy(&changed.stdout).contains("diff: different"));
    assert!(String::from_utf8_lossy(&changed.stderr).contains("headless diff failed"));
}

#[test]
fn scan_and_run_reject_ambiguous_or_invalid_arguments() {
    let temp = TempDir::new();
    fs::write(temp.0.join("z.txt"), b"z").expect("fixture should be written");
    fs::write(temp.0.join("a.txt"), b"a").expect("fixture should be written");

    let scan = Command::new(env!("CARGO_BIN_EXE_cs2-player"))
        .args([
            "scan",
            temp.0.to_str().expect("temporary path is UTF-8"),
            "--unexpected",
        ])
        .output()
        .expect("invalid scan should start");
    assert!(!scan.status.success());
    assert!(String::from_utf8_lossy(&scan.stderr).contains("unknown scan option"));

    let run = Command::new(env!("CARGO_BIN_EXE_cs2-player"))
        .args([
            "run",
            temp.0.to_str().expect("temporary path is UTF-8"),
            "--frames",
            "0",
        ])
        .output()
        .expect("invalid run should start");
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stderr).contains("--frames must be greater than zero"));

    let resources = Command::new(env!("CARGO_BIN_EXE_cs2-player"))
        .args([
            "list",
            temp.0.to_str().expect("temporary path is UTF-8"),
            "--resources",
        ])
        .output()
        .expect("resource listing should start");
    assert!(resources.status.success());
    let stdout = String::from_utf8_lossy(&resources.stdout);
    let a = stdout.find("a.txt").expect("a.txt should be listed");
    let z = stdout.find("z.txt").expect("z.txt should be listed");
    assert!(a < z, "resource output should be stable and sorted");
}

#[test]
fn json_workflows_use_the_stable_cli_schema() {
    let left = TempDir::new();
    let right = TempDir::new();
    for temp in [&left, &right] {
        fs::write(temp.0.join("sample.txt"), b"fixture").expect("fixture should be written");
    }

    let run_json = |args: &[&str]| -> serde_json::Value {
        let output = Command::new(env!("CARGO_BIN_EXE_cs2-player"))
            .args(args)
            .output()
            .expect("JSON workflow should start");
        assert!(
            output.status.success(),
            "workflow failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("workflow should emit JSON")
    };

    let left_path = left.0.to_str().expect("temporary path is UTF-8");
    let right_path = right.0.to_str().expect("temporary path is UTF-8");
    for value in [
        run_json(&["scan", left_path, "--json"]),
        run_json(&["compat", left_path, "--json"]),
        run_json(&["audit", left_path]),
        run_json(&["run", left_path, "--frames", "1", "--json"]),
        run_json(&["log", left_path, "--frames", "1", "--json"]),
        run_json(&["smoke", left_path, "--frames", "1", "--json"]),
        run_json(&["diff", left_path, right_path, "--frames", "1", "--json"]),
        run_json(&["window", "--json", left_path]),
    ] {
        assert_eq!(value["schema_version"], "cs2-cli-report/v1");
        assert!(value["kind"].is_string());
        assert!(value["status"].is_string());
    }

    let output_path = left.0.join("batch.json");
    let output = Command::new(env!("CARGO_BIN_EXE_cs2-player"))
        .args([
            "audit-batch",
            "--output",
            output_path.to_str().expect("output path is UTF-8"),
            left_path,
            right_path,
        ])
        .output()
        .expect("audit-batch should start");
    assert!(output.status.success());
    let batch: serde_json::Value =
        serde_json::from_slice(&fs::read(&output_path).expect("batch report should be written"))
            .expect("batch report should be valid JSON");
    assert_eq!(batch["schema_version"], "cs2-cli-report/v1");
    assert_eq!(batch["kind"], "content-audit-batch");
    assert_eq!(batch["game_count"], 2);
}
