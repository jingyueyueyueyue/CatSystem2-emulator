"""Android inventory, APK build and install.

Default build/install require the current implementation-completion audit.
Explicit --integration permits unfinished implementation validation, with its
own artifacts/provenance and without asserting implementation completion.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SIMULATOR = ROOT / "simulator"
DEFAULT_SDK = Path.home() / "AppData/Local/Android/Sdk"


def source_snapshot() -> dict:
    """Fingerprint build inputs, including assets, without generated outputs.

    Metadata is compared between build steps as well as content: an edit and
    revert during compilation must not turn a mixed build into a valid one.
    """
    digest = hashlib.sha256()
    ignored = {"build", ".gradle", "target", ".git", ".cxx", ".externalNativeBuild", "__pycache__"}
    paths = {
        path for folder in [SIMULATOR / "crates", SIMULATOR / "android", SIMULATOR / "vendor"]
        for path in folder.rglob("*")
        if path.is_file() and not any(part in ignored for part in path.relative_to(folder).parts)
    }
    paths.update(SIMULATOR / name for name in [
        "Cargo.toml", "Cargo.lock", "THIRD_PARTY.md", "rust-toolchain", "rust-toolchain.toml",
        ".cargo/config", ".cargo/config.toml",
    ] if (SIMULATOR / name).is_file())
    paths.update((ROOT / "scripts/build").glob("*.py"))
    files = {}
    for path in sorted(paths):
        before = path.stat()
        content = path.read_bytes()
        after = path.stat()
        if (before.st_size, before.st_mtime_ns) != (after.st_size, after.st_mtime_ns):
            raise RuntimeError(f"Build input changed while fingerprinting: {path}")
        name = path.relative_to(ROOT).as_posix()
        digest.update(name.encode("utf-8") + b"\0" + content + b"\0")
        files[name] = {"sha256": hashlib.sha256(content).hexdigest(),
                       "size": after.st_size, "mtime_ns": after.st_mtime_ns}
    return {"schema": "cs2-build-inputs/v1", "sha256": digest.hexdigest(), "files": files}


def source_identity() -> str:
    return source_snapshot()["sha256"]


def ascii_workspace() -> Path:
    """Use a verified junction; never replace an existing different target."""
    if os.name != "nt" or str(ROOT).isascii():
        return ROOT
    identity = hashlib.sha256(str(ROOT).casefold().encode("utf-8")).hexdigest()[:12]
    alias = Path(os.environ.get("CS2_ASCII_WORKSPACE", str(Path(ROOT.anchor) / f"cs2-workspace-{identity}")))
    if not alias.is_absolute() or not str(alias).isascii():
        raise RuntimeError("CS2_ASCII_WORKSPACE must be an absolute ASCII path")
    if not os.path.lexists(alias):
        environment = os.environ.copy()
        environment["CS2_ALIAS_LINK"] = str(alias)
        environment["CS2_ALIAS_TARGET"] = str(ROOT)
        powershell = Path(os.environ["SystemRoot"]) / "System32/WindowsPowerShell/v1.0/powershell.exe"
        # Environment values preserve Unicode without constructing shell code
        # from paths. No -Force, removal, or replacement of an existing link.
        result = subprocess.run([
            str(powershell), "-NoProfile", "-NonInteractive", "-Command",
            "$ErrorActionPreference = 'Stop'; New-Item -ItemType Junction -Path $env:CS2_ALIAS_LINK -Target $env:CS2_ALIAS_TARGET | Out-Null",
        ], env=environment, capture_output=True, text=True, errors="replace", timeout=30)
        if result.returncode and not os.path.lexists(alias):
            raise RuntimeError(f"Cannot create ASCII workspace junction: {result.stderr}")
    if not alias.is_dir() or not os.path.samefile(alias, ROOT):
        raise RuntimeError(f"Existing ASCII alias points elsewhere; refusing to overwrite: {alias}")
    return alias


def ascii_path(path: Path) -> Path:
    physical = path.resolve()
    if os.name != "nt":
        return physical
    if physical.is_relative_to(ROOT):
        return ascii_workspace() / physical.relative_to(ROOT)
    if not str(physical).isascii():
        raise RuntimeError(f"External build input/tool path must be ASCII: {physical}")
    return physical


def new_build_directory(name: str, integration: bool) -> Path:
    stamp = datetime.now().astimezone().strftime("%Y%m%d_%H%M%S_%f")
    stage = "integration" if integration else "complete"
    output = ROOT / "archive" / f"{stamp[:4]}-{stamp[4:6]}" / f"{stamp}_{name}_{stage}"
    output.mkdir(parents=True, exist_ok=False)
    return output


def build_cache_directory(name: str, integration: bool) -> Path:
    date = datetime.now().astimezone().strftime("%Y%m%d")
    stage = "integration" if integration else "complete"
    month = f"{date[:4]}-{date[4:6]}"
    cache_name = f"{date}_{name}_{stage}_cache"
    override = os.environ.get("CS2_BUILD_CACHE_ROOT")
    if override:
        cache_root = Path(override)
        if not cache_root.is_absolute() or not str(cache_root.resolve()).isascii():
            raise RuntimeError("CS2_BUILD_CACHE_ROOT must be an absolute, physically ASCII path")
        cache_root = cache_root.resolve()
    elif os.name != "nt" or str(ROOT).isascii():
        return ROOT / "archive" / month / cache_name
    else:
        # rustc canonicalizes dependency rlibs before passing them to GNU ld.
        # A junction pointing INTO a Unicode directory therefore fixes output
        # paths only. Keep the actual cache ASCII and link it back into archive.
        identity = hashlib.sha256(str(ROOT).casefold().encode("utf-8")).hexdigest()[:12]
        cache_root = Path(ROOT.anchor) / f"cs2-build-cache-{identity}"
    marker = cache_root / "workspace.json"
    if cache_root.exists():
        if not marker.is_file() or json.loads(marker.read_text(encoding="utf-8")).get("root") != str(ROOT):
            raise RuntimeError(f"Build cache belongs to another workspace: {cache_root}")
    else:
        cache_root.mkdir()
        marker.write_text(json.dumps({"root": str(ROOT)}, ensure_ascii=True) + "\n", encoding="utf-8")
    if override:
        physical = cache_root / f"{name}_{stage}"
        physical.mkdir(exist_ok=True)
        return physical
    physical = cache_root / month / cache_name
    physical.mkdir(parents=True, exist_ok=True)
    archived = ROOT / "archive" / month / f"{cache_name}_ascii"
    archived.parent.mkdir(parents=True, exist_ok=True)
    if not os.path.lexists(archived):
        environment = os.environ.copy()
        environment["CS2_ALIAS_LINK"] = str(archived)
        environment["CS2_ALIAS_TARGET"] = str(physical)
        powershell = Path(os.environ["SystemRoot"]) / "System32/WindowsPowerShell/v1.0/powershell.exe"
        result = subprocess.run([
            str(powershell), "-NoProfile", "-NonInteractive", "-Command",
            "$ErrorActionPreference = 'Stop'; New-Item -ItemType Junction -Path $env:CS2_ALIAS_LINK -Target $env:CS2_ALIAS_TARGET | Out-Null",
        ], env=environment, capture_output=True, text=True, errors="replace", timeout=30)
        if result.returncode and not os.path.lexists(archived):
            raise RuntimeError(f"Cannot archive ASCII build cache: {result.stderr}")
    if not archived.is_dir() or not os.path.samefile(archived, physical):
        raise RuntimeError(f"Archive cache link points elsewhere: {archived}")
    return physical


def build_report_path(integration: bool) -> Path:
    return ROOT / "reports" / ("android_integration_build.json" if integration else "android_build.json")


def host_c_compiler() -> Path:
    located = shutil.which("gcc") or shutil.which("cc")
    compiler = Path(located) if located else Path("C:/msys64/mingw64/bin/gcc.exe")
    if not compiler.is_file():
        raise RuntimeError("A native host C compiler is required for FFmpeg build tools and Cargo host dependencies")
    return ascii_path(compiler)


class BuildAttempt:
    """One source-bound attempt; never reuse a successful record for new bytes."""

    def __init__(self, output: Path, integration: bool):
        self.output = output
        self.integration = integration
        try:
            self.source = source_snapshot()
        except (OSError, RuntimeError) as error:
            if not integration:
                raise
            self.source = {"sha256": None, "files": {}, "error": str(error)}
        audit = None if integration else require_complete(self.source["sha256"])
        self.record = {
            "schema": "cs2-build-attempt/v1", "started_at": self.now(), "status": "building",
            "build_stage": "integration" if integration else "implementation-complete",
            "implementation_completion_verified": not integration, "completion_audit": audit,
            "source_before": self.source, "source_consistent": "error" not in self.source, "steps": [],
            "original_game_executed": False, "tested": False,
        }
        self.write()

    @staticmethod
    def now() -> str:
        return datetime.now().astimezone().isoformat(timespec="seconds")

    def write(self) -> None:
        (self.output / "build.json").write_text(json.dumps(self.record, indent=2) + "\n", encoding="utf-8")

    def environment(self, environment: dict) -> dict:
        result = environment.copy()
        result["CS2_BUILD_STAGE"] = self.record["build_stage"]
        result.pop("CS2_IMPLEMENTATION_COMPLETE", None)
        if not self.integration:
            result["CS2_IMPLEMENTATION_COMPLETE"] = "1"
        return result

    def verify_source(self, stage: str) -> None:
        try:
            current = source_snapshot()
        except (OSError, RuntimeError) as error:
            self.record.update(source_consistent=False, source_changed_at=stage,
                               source_snapshot_error=str(error))
            self.write()
            if not self.integration:
                raise
            return
        self.record["source_after"] = current
        before, after = self.source["files"], current["files"]
        changed = sorted(name for name in before.keys() | after.keys() if before.get(name) != after.get(name))
        if changed:
            self.record.update(source_consistent=False, source_changed_at=stage,
                               changed_inputs=sorted(set(self.record.get("changed_inputs", [])) | set(changed)))
            if not self.integration:
                self.record["status"] = "source-changed-during-build"
            self.write()
            if not self.integration:
                raise RuntimeError(f"Source changed during {stage}; artifact is not a validated build. See {self.output / 'build.json'}")

    def run(self, name: str, command: list[str], *, env: dict, cwd: Path = SIMULATOR,
            timeout: int | None = None) -> None:
        self.verify_source(f"before {name}")
        cwd = ascii_path(cwd)
        self.record["workspace"] = {"physical": str(ROOT), "build_alias": str(ascii_workspace())}
        log_path = self.output / f"{name}.log"
        step = {"name": name, "command": command, "cwd": str(cwd), "log": str(log_path),
                "started_at": self.now(), "source_before_sha256": self.source["sha256"],
                "build_environment": {key: env[key] for key in [
                    "CS2_BUILD_STAGE", "CS2_IMPLEMENTATION_COMPLETE", "CARGO_TARGET_DIR", "RUSTFLAGS",
                    "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER", "CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER",
                ] if key in env}}
        self.record["steps"].append(step)
        self.write()
        try:
            with log_path.open("w", encoding="utf-8") as log:
                result = subprocess.run(command, cwd=cwd, env=env, stdout=log, stderr=log, timeout=timeout)
            step["returncode"] = result.returncode
        except Exception as error:
            step["error"] = str(error)
            raise
        finally:
            step["finished_at"] = self.now()
            self.write()
            self.verify_source(f"after {name}")
        if result.returncode:
            raise RuntimeError(f"{name} failed ({result.returncode}); see {log_path}")

    def finish(self, artifacts: list[Path]) -> None:
        self.record["artifacts"] = {str(path): {"sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                                               "size": path.stat().st_size} for path in artifacts}
        self.verify_source("build completion")
        self.record.update(status="built" if self.record["source_consistent"] else "diagnostic-mixed-source",
                           finished_at=self.now())
        self.write()
        if not self.record["source_consistent"]:
            print(f"Diagnostic artifacts only: source changed during this attempt; installation is disabled. See {self.output / 'build.json'}")

    def fail(self, error: Exception) -> None:
        if self.record["status"] != "source-changed-during-build":
            self.record["status"] = "failed"
        self.record.update(error=str(error), finished_at=self.now())
        self.write()


def run(command: list[str], *, env: dict | None = None, cwd: Path = ROOT, timeout: int = 30) -> str:
    result = subprocess.run(command, cwd=cwd, env=env, capture_output=True, text=True,
                            encoding="utf-8", errors="replace", timeout=timeout)
    if result.returncode:
        raise RuntimeError(f"{Path(command[0]).name} failed ({result.returncode}):\n{result.stderr[-4000:]}")
    return result.stdout.strip()


def inventory(sdk: Path, serial: str | None) -> dict:
    adb = sdk / "platform-tools/adb.exe"
    if not adb.is_file():
        located = shutil.which("adb")
        if not located:
            raise RuntimeError("Android platform-tools/adb was not found")
        adb = Path(located)
    listing = run([str(adb), "devices", "-l"])
    connected = [line.split()[0] for line in listing.splitlines()
                 if len(line.split()) > 1 and line.split()[1] == "device"]
    if serial is None and len(connected) == 1:
        serial = connected[0]
    device = None
    if serial:
        if serial not in connected:
            raise RuntimeError("The selected Android device is not connected/authorized")
        properties = {}
        for key in ["ro.product.model", "ro.product.cpu.abilist", "ro.build.version.release", "ro.build.version.sdk"]:
            properties[key] = run([str(adb), "-s", serial, "shell", "getprop", key])
        device = {"serial": serial, "properties": properties}
    report = {
        "generated_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "status": "read-only-inventory-no-build-install-or-launch",
        "sdk": str(sdk), "adb": str(adb), "device": device,
        "connected_device_count": len(connected),
        "ndk": sorted(path.name for path in (sdk / "ndk").glob("*") if path.is_dir()),
        "platforms": sorted(path.name for path in (sdk / "platforms").glob("*") if path.is_dir()),
        "java": shutil.which("java"),
        "rust_targets": run([str(Path.home() / ".cargo/bin/rustup.exe"), "target", "list", "--installed"]).splitlines(),
        "required_target": "aarch64-linux-android",
        "implementation_source_sha256": source_identity(),
    }
    (ROOT / "reports/android_device_20261117.json").write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    return report


def require_complete(expected_source: str | None = None) -> dict:
    proof = ROOT / "reports/implementation_completion.json"
    if not proof.is_file():
        raise RuntimeError("Formal build/install needs implementation_completion.json; use explicit --integration for unfinished integration validation.")
    content = proof.read_bytes()
    report = json.loads(content)
    if (
        report.get("implementation_complete") is not True
        or report.get("remaining_required_functions") != 0
        or report.get("remaining_ordinary_native_calls") != 0
        or report.get("remaining_android_host_features") != 0
        or report.get("source_tree_sha256") != (expected_source if expected_source is not None else source_identity())
    ):
        raise RuntimeError("The current source tree has not passed the formal completion audit; use --integration for integration validation.")
    return {"path": str(proof), "sha256": hashlib.sha256(content).hexdigest(),
            "source_tree_sha256": report["source_tree_sha256"]}


def gradle_executable() -> Path:
    configured = shutil.which("gradle")
    if configured:
        return Path(configured)
    cache = Path.home() / ".gradle/wrapper/dists/gradle-9.5.1-bin"
    candidates = sorted(cache.glob("*/gradle-9.5.1/bin/gradle.bat"))
    if not candidates:
        raise RuntimeError("Gradle 9.5.1 is unavailable; install the official distribution before the final build.")
    return candidates[0]

def build_media(sdk: Path, output: Path, env: dict, attempt: BuildAttempt,
                staging: Path) -> tuple[Path, Path]:
    """Build pinned media in the same explicit stage/source-bound attempt."""
    prepare = ROOT / "scripts/build/prepare_android_ffmpeg.py"
    media = build_cache_directory("android", attempt.integration) / "ffmpeg"
    preparation = [
        sys.executable, "-B", "-X", "utf8", str(ascii_path(prepare)), "--prepare",
        "--ndk", str(ascii_path(sdk / "ndk/28.2.13676358")), "--output", str(media),
    ]
    if attempt.integration:
        preparation.append("--integration")
    source = ROOT / "archive/2026-11/20261116_ffmpeg-8.0.2.tar.xz"
    if source.is_file():
        preparation += ["--source-archive", str(ascii_path(source))]
    attempt.run("prepare-media", preparation, env=env, cwd=ROOT, timeout=300)
    plan = json.loads((media / "build-plan.json").read_text(encoding="utf-8"))
    bash = Path("C:/msys64/usr/bin/bash.exe")
    if not bash.is_file():
        raise RuntimeError("MSYS2 bash/make are required to build the pinned Android media libraries")
    if plan["build_stage"] != attempt.record["build_stage"]:
        raise RuntimeError("Prepared Android media belongs to another build stage")
    script = ascii_path(Path(plan["build_script"])).as_posix()
    if len(script) > 2 and script[1] == ":":
        script = "/" + script[0].lower() + script[2:]
    media_env = env.copy()
    media_env["LANG"] = "C.UTF-8"
    # GCC's cc1.exe lives outside bin and loads libwinpthread via PATH.
    # The NDK ships an incompatible DLL with the same name.
    media_env["PATH"] = os.pathsep.join([
        str(Path(plan["host_c_compiler"]).parent), str(bash.parent),
        media_env.get("PATH", ""),
    ])
    attempt.run("build-media", [str(bash), "--noprofile", "--norc", script], env=media_env, cwd=ROOT)
    prefix = Path(plan["install_prefix"])
    libraries = staging / "media-libs/arm64-v8a"
    libraries.mkdir(parents=True, exist_ok=True)
    for name in plan["runtime_libraries"]:
        source = prefix / "lib" / name
        if not source.is_file():
            raise RuntimeError(f"Android media build is missing {name}")
        shutil.copy2(source.resolve(), libraries / name)
    notices = staging / "media-notices/third-party/ffmpeg"
    notices.mkdir(parents=True, exist_ok=True)
    for name in ["COPYING.LGPLv2.1", "LICENSE.md", "RELEASE", "build-plan.json", "SOURCE-AND-LICENSE.txt"]:
        shutil.copy2(prefix / "notices" / name, notices / name)
    # Keep exact source, adapter and rebuild instructions beside the APK.
    shutil.make_archive(str(output / "cs2-player-media-source"), "zip", prefix / "notices")
    return libraries.parent, notices.parents[1]


def build(sdk: Path, integration: bool = False) -> Path:
    output = new_build_directory("android", integration)
    attempt = BuildAttempt(output, integration)
    try:
        return build_apk(sdk, output, attempt)
    except Exception as error:
        attempt.fail(error)
        raise


def build_apk(sdk: Path, output: Path, attempt: BuildAttempt) -> Path:
    ndk = ascii_path(sdk / "ndk/28.2.13676358/toolchains/llvm/prebuilt/windows-x86_64")
    linker = ndk / "bin/aarch64-linux-android26-clang.cmd"
    ar = ndk / "bin/llvm-ar.exe"
    if not linker.is_file() or not ar.is_file():
        raise RuntimeError("Required Android NDK clang/llvm-ar are unavailable")
    env = attempt.environment(os.environ)
    host_cc = host_c_compiler()
    env["CC_x86_64_pc_windows_gnu"] = str(host_cc)
    env["CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER"] = str(host_cc)
    env["ANDROID_HOME"] = str(ascii_path(sdk))
    env["ANDROID_SDK_ROOT"] = str(ascii_path(sdk))
    cache = build_cache_directory("android", attempt.integration)
    target = cache / "target"
    staging = cache / "attempts" / output.name
    staging.mkdir(parents=True, exist_ok=False)
    # Gradle/CMake canonicalize project and imported-library paths too. Build
    # from a small source copy on physical ASCII storage, not the inward link.
    project = staging / "project"
    shutil.copytree(SIMULATOR / "android", project, ignore=shutil.ignore_patterns(
        "build", ".gradle", ".cxx", ".externalNativeBuild", "local.properties"))
    attempt.record["android_paths"] = {
        "physical_cache": str(cache), "staging": str(staging), "project": str(project),
        "archive_output": str(output),
    }
    attempt.write()
    env["CARGO_TARGET_DIR"] = str(target)
    env["CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER"] = str(linker)
    env["CC_aarch64_linux_android"] = str(linker)
    env["AR_aarch64_linux_android"] = str(ar)
    env["RUSTFLAGS"] = (env.get("RUSTFLAGS", "") + " -C link-arg=-Wl,-z,max-page-size=16384").strip()
    libraries = staging / "rust-libs/arm64-v8a"
    libraries.mkdir(parents=True, exist_ok=True)
    env["CS2_ANDROID_RUST_LIBS"] = ascii_path(libraries.parent).as_posix()
    env["CS2_ANDROID_BUILD_ROOT"] = (staging / "app").as_posix()
    env["PATH"] = os.pathsep.join([str(host_cc.parent), str(Path.home() / ".cargo/bin"),
                                  str(ndk / "bin"), env.get("PATH", "")])
    media_libraries, media_notices = build_media(sdk, output, env, attempt, staging)
    general_notices = media_notices / "third-party"
    shutil.copy2(SIMULATOR / "crates/runtime/src/javascript_LICENSE-MIT.txt",
                 general_notices / "Boa-LICENSE-MIT.txt")
    shutil.copy2(SIMULATOR / "THIRD_PARTY.md", general_notices / "CS2-THIRD-PARTY.md")
    shutil.copy2(SIMULATOR / "vendor/ttf-parser-0.21.1/LICENSE-MIT",
                 general_notices / "ttf-parser-LICENSE-MIT.txt")
    env["CS2_ANDROID_MEDIA_LIBS"] = ascii_path(media_libraries).as_posix()
    env["CS2_ANDROID_MEDIA_NOTICES"] = ascii_path(media_notices).as_posix()
    profile = "android-integration" if attempt.integration else "release"
    attempt.record["cargo_profile"] = profile
    commands = [
        [str(Path.home() / ".cargo/bin/cargo.exe"), "build", "--locked", "--profile", profile,
         "--target", "aarch64-linux-android", "-p", "cs2-android"],
        [str(gradle_executable()), "--no-daemon", "--project-cache-dir",
         str(cache / "gradle-cache"), "-p", str(project), ":app:assembleDebug"],
    ]
    for index, command in enumerate(commands):
        attempt.run(f"build-{index}", command, env=env)
        if index == 0:
            shutil.copy2(target / "aarch64-linux-android" / profile / "libcs2_android.so",
                         libraries / "libcs2_android.so")
    built_apk = staging / "app/outputs/apk/debug/app-debug.apk"
    if not built_apk.is_file():
        raise RuntimeError("Gradle returned success without the expected APK")
    apk = output / "cs2-player.apk"
    shutil.copy2(built_apk, apk)
    attempt.finish([apk, output / "cs2-player-media-source.zip",
                    libraries / "libcs2_android.so", *sorted((media_libraries / "arm64-v8a").glob("*.so"))])
    build_report_path(attempt.integration).write_text(json.dumps({
        "apk": str(apk), "sha256": hashlib.sha256(apk.read_bytes()).hexdigest(),
        "source_tree_sha256": attempt.source["sha256"], "abi": "arm64-v8a",
        "build_stage": attempt.record["build_stage"],
        "implementation_completion_verified": not attempt.integration,
        "source_consistent": attempt.record["source_consistent"], "provenance": str(output / "build.json"),
        "installed": False, "tested": False,
        "media_libraries": {
            path.name: hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted((media_libraries / "arm64-v8a").glob("*.so"))
        },
        "media_source": str(output / "cs2-player-media-source.zip"),
    }, indent=2) + "\n", encoding="utf-8")
    return apk


def install(sdk: Path, serial: str | None, integration: bool = False) -> None:
    source = source_identity()
    if not integration:
        require_complete(source)
    report_path = build_report_path(integration)
    report = json.loads(report_path.read_text(encoding="utf-8"))
    provenance = json.loads(Path(report["provenance"]).read_text(encoding="utf-8"))
    expected_stage = "integration" if integration else "implementation-complete"
    apk = Path(report["apk"])
    if (
        report.get("build_stage") != expected_stage or provenance.get("build_stage") != expected_stage
        or provenance.get("status") != "built" or provenance.get("source_consistent") is not True
        or report.get("source_consistent") is not True or report["source_tree_sha256"] != source
        or provenance["source_before"]["sha256"] != source
        or hashlib.sha256(apk.read_bytes()).hexdigest() != report["sha256"]
        or provenance.get("artifacts", {}).get(str(apk), {}).get("sha256") != report["sha256"]
    ):
        raise RuntimeError("APK stage, source or artifact provenance is stale/inconsistent; rebuild this stage before installing")
    device = inventory(sdk, serial)
    if not device["device"]:
        raise RuntimeError("Select the connected tablet explicitly with --serial")
    if source_identity() != source:
        raise RuntimeError("Source changed before installation; rebuild before installing")
    print(run([device["adb"], "-s", device["device"]["serial"], "install", "-r", str(apk)], timeout=300))
    report.update(installed=True, device_serial=device["device"]["serial"],
                  installed_at=BuildAttempt.now(), installation_source_current=source_identity() == source)
    report_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    (Path(report["provenance"]).parent / "install.json").write_text(
        json.dumps(report, indent=2) + "\n", encoding="utf-8")
    if not report["installation_source_current"]:
        raise RuntimeError("Recorded APK was installed, but workspace source changed during installation; rebuild before validating new source")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("phase", choices=["inventory", "build", "install"])
    parser.add_argument("--sdk", type=Path, default=DEFAULT_SDK)
    parser.add_argument("--serial")
    parser.add_argument("--integration", action="store_true",
                        help="Build/install unfinished integration artifacts; does not assert implementation completion.")
    args = parser.parse_args()
    if args.integration and args.phase == "inventory":
        parser.error("--integration applies only to build/install")
    if args.phase in {"inventory", "build"}:
        (ROOT / "reports").mkdir(exist_ok=True)
    if args.phase == "inventory":
        print(json.dumps(inventory(args.sdk, args.serial), ensure_ascii=True, indent=2))
        return
    if args.phase == "build":
        print(build(args.sdk, args.integration))
        return
    install(args.sdk, args.serial, args.integration)


if __name__ == "__main__":
    main()
