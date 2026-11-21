"""CatSystem2 main workflow: static evidence, toolchain and compilation.

No command launches an original game EXE. `snapshot` never invokes Rust, a VM
or acceptance tests. Formal `build` requires completion evidence; explicit
`build --integration` validates unfinished implementation without claiming it
complete. Both retain per-attempt source/artifact provenance.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def executable(name: str, alternatives: list[Path]) -> Path | None:
    configured = shutil.which(name)
    if configured:
        return Path(configured)
    return next((path for path in alternatives if path.is_file()), None)


def toolchain() -> tuple[dict, dict]:
    user = Path.home()
    cargo_bin = user / ".cargo/bin"
    cargo = executable("cargo", [cargo_bin / "cargo.exe"])
    rustc = executable("rustc", [cargo_bin / "rustc.exe"])
    cc = executable("gcc", [Path("C:/msys64/mingw64/bin/gcc.exe")])
    environment = os.environ.copy()
    directories = [str(path.parent) for path in [cargo, rustc, cc] if path]
    environment["PATH"] = os.pathsep.join(dict.fromkeys(directories + [environment.get("PATH", "")]))
    if cc:
        environment["CC_x86_64_pc_windows_gnu"] = str(cc)
        environment["CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER"] = str(cc)
    tools = {}
    for name, path in [("cargo", cargo), ("rustc", rustc), ("gcc", cc)]:
        if path is None:
            tools[name] = {"available": False}
            continue
        process = subprocess.run(
            [str(path), "--version"], capture_output=True, text=True,
            encoding="utf-8", errors="replace", env=environment, timeout=20,
        )
        tools[name] = {
            "available": process.returncode == 0, "path": str(path),
            "version": process.stdout.splitlines()[0] if process.stdout else process.stderr.strip(),
        }
    if tools.get("rustc", {}).get("available"):
        version = tools["rustc"]["version"].split()[1].split(".")
        tools["rustc"]["meets_1_87"] = tuple(int(x) for x in version[:2]) >= (1, 87)
    return tools, environment


def snapshot(refresh_content: bool = False) -> None:
    # Dependencies remain sequential: workload_status consumes fresh coverage.
    workload_path = ROOT / "reports/workload_status_20261116.json"
    previous = json.loads(workload_path.read_text(encoding="utf-8")) if workload_path.is_file() else None
    scripts = []
    for script, report in [
        ("startup_inventory.py", "startup_inventory_20261116.json"),
        ("kcs_call_inventory.py", "kcs_call_inventory_20261116.json"),
    ]:
        if refresh_content or not (ROOT / "reports" / report).is_file():
            scripts.append(script)
    scripts += ["native_coverage.py"]
    if (ROOT / "scripts/analysis/particle_ppi_manifest.py").is_file():
        scripts.append("particle_ppi_manifest.py")
    if (ROOT / "reports/recovery_A_20261116.json").is_file():
        scripts.append("source_function_bindings.py")
    if (ROOT / "scripts/analysis/implementation_gap_markers.py").is_file():
        scripts.append("implementation_gap_markers.py")
    scripts.append("workload_status.py")
    scripts.append("batch_wiring_queue.py")
    for script in scripts:
        process = subprocess.run(
            [sys.executable, "-B", "-X", "utf8", str(ROOT / "scripts/analysis" / script)],
            cwd=ROOT, capture_output=True, encoding="utf-8", errors="replace", timeout=180,
        )
        if process.returncode:
            raise RuntimeError(f"{script} failed:\n{process.stdout}\n{process.stderr}")
        print(f"{script}: updated reports")
    report = json.loads(workload_path.read_text(encoding="utf-8"))
    ordinary = report["domains"]["native_by_host_table"]["ordinary"]
    delta = record_batch_snapshot(previous, report)
    print(json.dumps({
        "ordinary_observed": ordinary["observed"],
        "ordinary_routed": ordinary["routed"],
        "ordinary_unmapped": ordinary["unmapped"],
        "ordinary_extractor_missed_partial_routes": ordinary.get("extractor_missed_partial_indices", []),
        "ordinary_routed_including_reviewed_partial_subsets": ordinary.get(
            "routed_including_reviewed_partial_subsets", ordinary["routed"]),
        "ordinary_source_behavior": ordinary["source_behavior"],
        "particle_ppi_models": report["domains"].get("particle_ppi_models"),
        "source_snapshot_current": report["native_source_current"],
        "complete_game_acceptance": report["domains"]["full_game_acceptance"],
        "newly_routed_since_previous_snapshot": delta["newly_routed"],
        "route_regressions_since_previous_snapshot": delta["route_regressions"],
    }, ensure_ascii=True, indent=2))

def record_batch_snapshot(previous: dict | None, current: dict) -> dict:
    """Retain actual numeric deltas; concurrent deliveries must not be guessed
    from arithmetic on the total or attributed to a single agent."""
    ordinary = current["domains"]["native_by_host_table"]["ordinary"]
    if ordinary["routed"] + ordinary["unmapped"] != ordinary["observed"]:
        raise RuntimeError("Native route/status accounting disagrees; repair the source inspector before publishing progress.")
    before = (previous or {}).get("domains", {}).get("native_by_host_table", {}).get("ordinary")
    old_missing = set(before["missing_indices"]) if before else set(ordinary["missing_indices"])
    missing = set(ordinary["missing_indices"])
    row = {
        "generated_at": current["generated_at"],
        "status": "static-routing-snapshot-not-behavior-equivalence",
        "source_snapshot_current": current["native_source_current"],
        "ordinary_observed": ordinary["observed"],
        "ordinary_routed": ordinary["routed"],
        "ordinary_source_behavior": ordinary["source_behavior"],
        "ordinary_unmapped": ordinary["unmapped"],
        "ordinary_missing_indices": ordinary["missing_indices"],
        "source_accounting_consistent": True,
        "newly_routed": sorted(old_missing - missing),
        "route_regressions": sorted(missing - old_missing),
        "previous_snapshot": previous.get("generated_at") if previous else None,
        "previous_source_snapshot_current": previous.get("native_source_current") if previous else None,
        "previous_accounting_consistent": (
            before["routed"] + before["unmapped"] == before["observed"] if before else None
        ),
        "coverage_sha256": hashlib_file(ROOT / "reports/native_coverage_20261116.json"),
        "full_game_acceptance": current["domains"]["full_game_acceptance"],
    }
    path = ROOT / "reports/native_batch_snapshots.json"
    document = json.loads(path.read_text(encoding="utf-8")) if path.is_file() else {
        "schema": "cs2-native-batch-snapshots/v1", "snapshots": [],
    }
    document["snapshots"].append(row)
    path.write_text(json.dumps(document, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return row


def hashlib_file(path: Path) -> str:
    import hashlib
    return hashlib.sha256(path.read_bytes()).hexdigest()


def build(environment: dict, tools: dict, release: bool, integration: bool = False,
          *, story_replay: bool = False, profile: str | None = None, freeze_source: bool = False) -> None:
    from android_workspace import BuildAttempt, build_cache_directory, new_build_directory, source_snapshot
    archive_root = ROOT / "archive"
    if story_replay and os.environ.get("CS2_BUILD_CACHE_ROOT"):
        archive_root = build_cache_directory("workspace_pipeline", integration).parent / "archive"
    archive = new_build_directory("story_replay" if story_replay else "workspace_pipeline", integration,
                                  archive_root=archive_root)
    source_root = ROOT
    if freeze_source:
        expected = source_snapshot()
        snapshots = build_cache_directory("workspace_pipeline", integration) / "source_snapshots"
        snapshots.mkdir(exist_ok=True)
        source_root = Path(tempfile.mkdtemp(prefix=f"{archive.name}_", dir=snapshots))
        for name in expected["files"]:
            target = source_root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / name, target)
        if source_snapshot(source_root)["sha256"] != expected["sha256"]:
            raise RuntimeError(f"Build inputs changed while staging the snapshot: {source_root}")
    attempt = BuildAttempt(archive, integration, source_root=source_root)
    try:
        build_workspace(environment, tools, release, attempt, story_replay=story_replay, profile=profile)
    except Exception as error:
        attempt.fail(error)
        raise


def story_replay_resource(cache: Path, environment: dict, tools: dict, attempt, simulator: Path) -> Path:
    sibling = Path(tools["gcc"]["path"]).with_name("windres.exe")
    compiler = environment.get("WINDRES") or (str(sibling) if sibling.is_file() else "windres")
    windres = shutil.which(compiler, path=environment["PATH"])
    if not windres:
        raise RuntimeError(f"Windows GNU story_replay requires windres: {compiler}")
    resources = cache / "story_replay_manifest"
    resources.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="windres-", dir=resources) as directory:
        staging = Path(directory)
        manifest = staging / "player.manifest"
        shutil.copyfile(simulator / "crates/cli/windows.manifest", manifest)
        (staging / "player.rc").write_text('1 24 "player.manifest"\n', encoding="ascii")
        attempt.run("windres", [windres, "--target=pe-x86-64", "-i", "player.rc", "-O", "coff",
                                "-o", "player-resource.o"], env=environment, cwd=staging)
        compiled = staging / "player-resource.o"
        resource_sha256 = hashlib_file(compiled)
        resource = resources / f"player-{resource_sha256}.o"
        if not resource.is_file() or hashlib_file(resource) != resource_sha256:
            compiled.replace(resource)
        attempt.record["windows_manifest"] = {
            "source": "simulator/crates/cli/windows.manifest", "sha256": hashlib_file(manifest),
            "resource": str(resource), "resource_sha256": resource_sha256,
        }
    return resource


def build_workspace(environment: dict, tools: dict, release: bool, attempt,
                    *, story_replay: bool = False, profile: str | None = None) -> None:
    from android_workspace import build_cache_directory
    if not tools.get("rustc", {}).get("meets_1_87"):
        raise RuntimeError("Rust 1.87 or later is required")
    if not all(tools.get(name, {}).get("available") for name in ("cargo", "rustc", "gcc")):
        raise RuntimeError("the Windows GNU Rust/C toolchain is incomplete")
    archive = attempt.output
    simulator = attempt.source_root / "simulator"
    environment = attempt.environment(environment)
    cache = build_cache_directory("workspace_pipeline", attempt.integration)
    target = cache / "target"
    environment["CARGO_TARGET_DIR"] = str(target)
    triple = "x86_64-pc-windows-gnu"
    if story_replay:
        command = [tools["cargo"]["path"], "rustc", "--locked", "-p", "cs2-desktop",
                   "--example", "story_replay", "--target", triple]
        artifact = Path("examples/story_replay.exe")
    else:
        command = [tools["cargo"]["path"], "build", "--locked", "--workspace", "--target", triple]
        artifact = Path("cs2-player.exe")
    if profile:
        command.extend(("--profile", profile))
    elif release:
        command.append("--release")
    if story_replay:
        resource = story_replay_resource(cache, environment, tools, attempt, simulator)
        environment["CS2_REPLAY_SOURCE_SHA256"] = attempt.source["sha256"]
        command.extend(("--", f"-Clink-arg={resource}"))
    log_path = archive / "build.log"
    attempt.run("build", command, env=environment, cwd=simulator)
    profile_directory = profile or ("release" if release else "debug")
    binary = archive / artifact.name
    shutil.copy2(target / triple / profile_directory / artifact, binary)
    if story_replay:
        attempt.run("replay_identity", [str(binary), "--source-sha256"],
                    env=environment, cwd=simulator, timeout=30)
        embedded = (archive / "replay_identity.log").read_text(encoding="utf-8").strip()
        if embedded != attempt.source["sha256"]:
            raise RuntimeError(f"Archived story_replay reports source {embedded!r}, expected {attempt.source['sha256']}")
        attempt.record["replay_source_sha256"] = embedded
    attempt.finish([binary])
    print(f"player: {binary}")
    print(f"build log: {log_path}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("phase", choices=["snapshot", "toolchain", "build"])
    profiles = parser.add_mutually_exclusive_group()
    profiles.add_argument("--release", action="store_true")
    profiles.add_argument("--profile", choices=["playthrough"],
                          help="Use the optimized no-LTO profile (requires build --integration --story-replay).")
    parser.add_argument("--integration", action="store_true",
                        help="Compile unfinished implementation for integration; retain separate source-bound artifacts.")
    parser.add_argument("--story-replay", action="store_true",
                        help="Build only the Windows GNU story_replay example (requires build --integration).")
    parser.add_argument("--freeze-source", action="store_true",
                        help="Copy verified build inputs to the cache before compiling story_replay.")
    parser.add_argument("--refresh-content", action="store_true",
                        help="Rescan KCS provenance/startup only for new or changed game resources.")
    args = parser.parse_args()
    if args.story_replay and (args.phase != "build" or not args.integration):
        parser.error("--story-replay requires build --integration")
    if args.profile and not args.story_replay:
        parser.error("--profile playthrough requires build --integration --story-replay")
    if args.freeze_source and not args.story_replay:
        parser.error("--freeze-source requires build --integration --story-replay")
    if args.integration and args.phase != "build":
        parser.error("--integration applies only to build")
    if args.phase == "snapshot":
        snapshot(args.refresh_content)
        return
    tools, environment = toolchain()
    if args.phase == "toolchain":
        path = ROOT / "reports/toolchain_20261116.json"
        path.write_text(json.dumps(tools, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(tools, indent=2))
        return
    build(environment, tools, args.release, args.integration, story_replay=args.story_replay,
          profile=args.profile, freeze_source=args.freeze_source)


if __name__ == "__main__":
    main()
