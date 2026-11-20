"""Prepare a pinned LGPL FFmpeg Android build without executing a build.

Default: print a command plan, establishing only verified path mappings when
needed. --prepare downloads/verifies the official source, extracts it in archive
or its workspace-owned physical ASCII cache, and writes a deferred shell script.
No configure/compiler/make/Gradle/Cargo/ADB subprocess is run by this program.
The default emitted build script retains its implementation-complete gate.
--integration emits a separate recipe requiring CS2_BUILD_STAGE=integration
and no implementation-complete flag. Source-bound build/install provenance is
recorded by android_workspace.py, not by this preparation-only step.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import tarfile
import tempfile
import urllib.request

from android_workspace import ascii_path, build_cache_directory, default_sdk, host_c_compiler

ROOT = Path(__file__).resolve().parents[2]
VERSION = "8.0.2"
SOURCE_URL = f"https://ffmpeg.org/releases/ffmpeg-{VERSION}.tar.xz"
SOURCE_SHA256 = "5d16962332603c427b3d0887fc12b9166d6ee2cb1108b1865dd2d5eb06a09505"
DEFAULT_OUTPUT = ROOT / "archive/2026-11/20261116_android_ffmpeg"
DECODERS = (
    "mpeg1video,mpeg2video,mpeg4,h264,hevc,vp8,vp9,wmv1,wmv2,wmv3,vc1,"
    "msmpeg4v1,msmpeg4v2,msmpeg4v3,mjpeg,mp1,mp2,mp3,mp3float,aac,ac3,eac3,"
    "vorbis,opus,wmav1,wmav2,wmavoice,wmapro,wmalossless,flac,alac,"
    "pcm_s16le,pcm_s16be,pcm_s8,pcm_u8,pcm_s24le,pcm_s24be,"
    "pcm_s32le,pcm_s32be,pcm_f32le,pcm_f32be"
)
DEMUXERS = "mpegps,mpegvideo,matroska,asf,asf_o,avi,mov"
LIBRARIES = ["libcs2_media.so", "libavformat.so", "libavcodec.so",
             "libavutil.so", "libswscale.so", "libswresample.so"]


def digest(path: Path) -> str:
    result = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def prepare_source_archive(archive: Path, source_archive: Path | None) -> None:
    if source_archive is not None:
        if digest(source_archive) != SOURCE_SHA256:
            raise ValueError("Provided source archive does not match the pinned official SHA256")
        if source_archive.resolve() == archive.resolve():
            return
    elif archive.is_file() and digest(archive) == SOURCE_SHA256:
        return

    # Verify the complete download before publishing it into the shared cache.
    temporary = tempfile.NamedTemporaryFile(dir=archive.parent, prefix=".ffmpeg-", delete=False)
    candidate = Path(temporary.name)
    try:
        with temporary:
            if source_archive is not None:
                with source_archive.open("rb") as source:
                    shutil.copyfileobj(source, temporary)
            else:
                with urllib.request.urlopen(SOURCE_URL, timeout=45) as source:
                    shutil.copyfileobj(source, temporary, length=1024 * 1024)
        if digest(candidate) != SOURCE_SHA256:
            raise ValueError("Official source archive SHA256 mismatch; refuse to extract")
        candidate.replace(archive)
    finally:
        candidate.unlink(missing_ok=True)


def shell(path: Path) -> str:
    # MSYS bash accepts a native Windows path in /drive/... form; Linux/macOS
    # plans use their actual NDK paths without crossing a Windows shell.
    value = ascii_path(path).as_posix()
    if len(value) > 2 and value[1] == ":":
        return f"/{value[0].lower()}{value[2:]}"
    return value


def plan(ndk: Path, output: Path, api: int, host: str, jobs: int, integration: bool = False) -> dict:
    if api < 26:
        raise ValueError("AAudio and this Android media backend require API26+")
    ndk = ascii_path(ndk)
    output = ascii_path(output)
    tools = ndk / "toolchains/llvm/prebuilt" / host / "bin"
    suffix = ".exe" if host.startswith("windows") else ""
    executable = lambda name: shell(tools / (name + suffix))
    host_cc = host_c_compiler()
    host_command = shell(host_cc)
    if any(character.isspace() for character in host_command):
        # FFmpeg expands this command as shell words. The generated recipe
        # puts this exact compiler's directory first on PATH.
        host_command = host_cc.name
    source = output / f"ffmpeg-{VERSION}"
    prefix = output / "install/arm64-v8a"
    build = output / "build/arm64-v8a"
    original_shim = ROOT / "simulator/android/app/src/main/cpp/cs2_media.c"
    shim = output / "adapter/cs2_media.c"
    flags = f"--target=aarch64-linux-android{api} -fPIC"
    linker = f"--target=aarch64-linux-android{api} -Wl,-z,max-page-size=16384"
    configure = [
        shell(source / "configure"), "--target-os=android", "--arch=aarch64",
        "--cpu=armv8-a", "--enable-cross-compile", "--enable-pic",
        f"--sysroot={shell(tools.parent / 'sysroot')}", f"--cc={executable('clang')}",
        f"--cxx={executable('clang++')}",
        f"--host-cc={host_command}", f"--host-ld={host_command}",
        f"--ld={executable('clang')}", f"--ar={executable('llvm-ar')}",
        f"--nm={executable('llvm-nm')}",
        f"--ranlib={executable('llvm-ranlib')}", f"--strip={executable('llvm-strip')}",
        f"--prefix={shell(prefix)}", "--disable-autodetect", "--disable-gpl",
        "--disable-nonfree", "--disable-version3", "--disable-static", "--enable-shared",
        "--disable-programs", "--disable-doc", "--disable-debug", "--disable-everything",
        "--enable-avformat", "--enable-avcodec", "--enable-avutil",
        "--enable-swscale", "--enable-swresample", "--disable-avdevice", "--disable-avfilter",
        "--enable-pthreads", "--enable-parsers", f"--enable-demuxer={DEMUXERS}",
        f"--enable-decoder={DECODERS}", f"--extra-cflags={flags}",
        f"--extra-ldflags={linker}",
    ]
    shim_command = [
        executable("clang"), f"--target=aarch64-linux-android{api}", "-std=c11",
        "-fPIC", "-shared", "-O2", "-Wall", "-Wextra", "-Werror",
        "-Wl,--no-undefined", "-Wl,-z,max-page-size=16384", "-Wl,-soname,libcs2_media.so",
        f"-I{shell(prefix / 'include')}", shell(shim),
        f"-L{shell(prefix / 'lib')}", "-lavformat", "-lavcodec", "-lswscale",
        "-lswresample", "-lavutil", "-lm", "-o", shell(prefix / "lib/libcs2_media.so"),
    ]
    return {
        "scope": "preparation only; no build, dependency compilation, APK or device execution",
        "build_stage": "integration" if integration else "implementation-complete",
        "completion_audit_required": not integration,
        "build_script": str(output / ("build-integration.sh" if integration else "build-after-implementation.sh")),
        "source": SOURCE_URL, "version": VERSION, "sha256": SOURCE_SHA256,
        "license": "FFmpeg LGPL-2.1-or-later shared build; new shim MIT",
        "abi": "arm64-v8a", "api": api, "host": host, "ndk": str(ndk),
        "host_c_compiler": str(host_cc),
        "output": str(output), "jobs": jobs, "configure": configure,
        "shim_command": shim_command, "build_directory": str(build),
        "shim_build_source": str(shim),
        "shim_source_sha256": digest(original_shim),
        "shim_header_sha256": digest(original_shim.with_suffix(".h")),
        "install_prefix": str(prefix), "runtime_libraries": LIBRARIES,
        "jni_integration": [
            "Package all listed shared libraries in arm64-v8a jniLibs; keep their original SONAMEs.",
            "Rust loads libcs2_media.so with dlopen and validates ABI/record sizes; no JNI ABI changes.",
            "Main CMake/Gradle integration is deliberately not edited by this preparation script.",
            "Use a minimum SDK >=26 and retain 16KiB ELF segment alignment.",
        ],
    }


def configure_commands(report: dict) -> list[str]:
    command = shlex.join(report["configure"])
    config = Path(report["build_directory"]) / "ffbuild/config.mak"
    try:
        lines = config.read_text(encoding="utf-8").splitlines()
        values = dict(line.split("=", 1) for line in lines if "=" in line)
        saved = values["FFMPEG_CONFIGURATION"]
        source = report["configure"][0].removesuffix("/configure")
        if (shlex.split(saved) != report["configure"][1:]
                or values.get("SRC_PATH") != source
                or "endif # FFMPEG_CONFIG_MAK" not in lines):
            return [command]
    except (OSError, UnicodeError, KeyError, ValueError):
        return [command]

    # Recheck at execution too: preparation must not authorize a stale cache.
    # The pinned source archive has already been verified by prepare().
    checks = [
        "[[ -s config.h && -s config_components.h && -f ffbuild/.config ]]",
        "grep -Fqx -- " + shlex.quote("FFMPEG_CONFIGURATION=" + saved) + " ffbuild/config.mak",
        "grep -Fqx -- " + shlex.quote("SRC_PATH=" + source) + " ffbuild/config.mak",
        '[[ -z "${CC:-}${CXX:-}${CFLAGS:-}${CPPFLAGS:-}${CXXFLAGS:-}'
        '${LDFLAGS:-}${ASFLAGS:-}${HOSTCC:-}${HOSTCFLAGS:-}${HOSTLDFLAGS:-}" ]]',
    ]
    ndk = Path(report["ndk"])
    clang = ndk / "toolchains/llvm/prebuilt" / report["host"] / "bin" / (
        "clang.exe" if report["host"].startswith("windows") else "clang")
    for path in [Path(report["output"]) / f"ffmpeg-{report['version']}/configure",
                 ndk / "source.properties", clang, Path(report["host_c_compiler"])]:
        quoted = shlex.quote(shell(path))
        checks.append(f"[[ -f {quoted} && ! {quoted} -nt ffbuild/config.mak ]]")
    for name in report["runtime_libraries"]:
        library = shlex.quote(shell(Path(report["install_prefix"]) / "lib" / name))
        checks.append(f"[[ -s {library} && ! ffbuild/config.mak -nt {library} ]]")
    return [
        "if " + " &&\n   ".join(checks) + "; then",
        "  echo 'Reusing matching installed FFmpeg configuration.'",
        "else", "  " + command, "fi",
    ]


def prepare(report: dict, output: Path, source_archive: Path | None) -> None:
    output.mkdir(parents=True, exist_ok=True)
    adapter = output / "adapter"
    adapter.mkdir(exist_ok=True)
    for name, key in [("cs2_media.c", "shim_source_sha256"), ("cs2_media.h", "shim_header_sha256")]:
        content = (ROOT / "simulator/android/app/src/main/cpp" / name).read_bytes()
        (adapter / name).write_bytes(content)
        # These exact copied bytes are compiled and distributed. The outer
        # attempt separately marks concurrent source edits diagnostic-only.
        report[key] = hashlib.sha256(content).hexdigest()
    archive = output / f"ffmpeg-{VERSION}.tar.xz"
    prepare_source_archive(archive, source_archive)
    source = output / f"ffmpeg-{VERSION}"
    if not source.exists():
        with tarfile.open(archive) as package:
            # Python's data filter rejects paths/links outside the destination,
            # special devices and unsafe archive metadata.
            package.extractall(output, filter="data")
    if (source / "RELEASE").read_text(encoding="ascii").strip() != VERSION:
        raise ValueError("Prepared source RELEASE differs from the pin")
    build = Path(report["build_directory"])
    prefix = Path(report["install_prefix"])
    if report["build_stage"] == "integration":
        gate = [
            'if [[ "${CS2_BUILD_STAGE:-}" != "integration" || -n "${CS2_IMPLEMENTATION_COMPLETE:-}" ]]; then',
            "  echo 'Integration media requires CS2_BUILD_STAGE=integration and no implementation-complete assertion.' >&2",
            "  exit 2", "fi",
        ]
    else:
        gate = [
            'if [[ "${CS2_IMPLEMENTATION_COMPLETE:-}" != "1" || "${CS2_BUILD_STAGE:-implementation-complete}" != "implementation-complete" ]]; then',
            "  echo 'Formal media build requires the main implementation-completion audit.' >&2",
            "  exit 2", "fi",
        ]
    script = Path(report["build_script"])
    lines = [
        "#!/usr/bin/env bash", "set -euo pipefail", *gate,
        # Keep GCC subprocess DLL lookup consistent when running this recipe
        # directly too: NDK's libwinpthread lacks MinGW GCC's clock_gettime64.
        f'export PATH={shlex.quote(shell(Path(report["host_c_compiler"]).parent))}:"$PATH"',
        f"mkdir -p {shlex.quote(shell(build))}",
        f"cd {shlex.quote(shell(build))}",
        *configure_commands(report), f"make -j {report['jobs']}", "make install",
        shlex.join(report["shim_command"]),
        f"mkdir -p {shlex.quote(shell(prefix / 'notices'))}",
    ]
    for name in ["COPYING.LGPLv2.1", "LICENSE.md", "RELEASE"]:
        lines.append(f"cp {shlex.quote(shell(source / name))} {shlex.quote(shell(prefix / 'notices' / name))}")
    for name in ["cs2_media.c", "cs2_media.h"]:
        original = adapter / name
        lines.append(f"cp {shlex.quote(shell(original))} {shlex.quote(shell(prefix / 'notices' / name))}")
    lines += [
        f"cp {shlex.quote(shell(archive))} {shlex.quote(shell(prefix / 'notices' / archive.name))}",
        f"cp {shlex.quote(shell(output / 'build-plan.json'))} {shlex.quote(shell(prefix / 'notices/build-plan.json'))}",
        f"cp {shlex.quote(shell(script))} {shlex.quote(shell(prefix / 'notices' / script.name))}",
        f"cp {shlex.quote(shell(output / 'SOURCE-AND-LICENSE.txt'))} {shlex.quote(shell(prefix / 'notices/SOURCE-AND-LICENSE.txt'))}",
        "echo 'Built native shared libraries only. APK packaging/test remains a separate authorized main-agent step.'",
    ]
    script.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    (output / "build-plan.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    notice = (
        f"FFmpeg {VERSION}, original unmodified source from {SOURCE_URL}\n"
        f"SHA256 {SOURCE_SHA256}\n"
        "FFmpeg is separately licensed LGPL-2.1-or-later in this non-GPL/nonfree-disabled shared build.\n"
        "The independent cs2_media.c/.h bridge is MIT. No FFmpeg implementation is copied into it.\n"
        "Distribute the exact source archive, configuration plan, notices and relinking instructions with releases.\n"
        "Keep FFmpeg dynamically replaceable; do not remove user rights to debug/relink their modified libraries.\n"
        "No FFmpeg shared libraries or APK have been compiled by this preparation step.\n"
        f"Recipe stage: {report['build_stage']}; this plan is not implementation-completion evidence.\n"
    )
    (output / "SOURCE-AND-LICENSE.txt").write_text(notice, encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ndk", type=Path, default=default_sdk() / "ndk/28.2.13676358")
    parser.add_argument("--host", choices=["windows-x86_64", "linux-x86_64", "darwin-x86_64"],
                        default="windows-x86_64" if os.name == "nt" else "linux-x86_64")
    parser.add_argument("--api", type=int, default=26)
    parser.add_argument("--jobs", type=int, default=4)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--source-archive", type=Path)
    parser.add_argument("--prepare", action="store_true")
    parser.add_argument("--integration", action="store_true",
                        help="Prepare an unfinished integration recipe without asserting implementation completion.")
    args = parser.parse_args()
    if args.jobs < 1 or args.jobs > 64:
        parser.error("--jobs must be in1..64")
    output = (args.output or (DEFAULT_OUTPUT.with_name(DEFAULT_OUTPUT.name + "_integration")
                             if args.integration else DEFAULT_OUTPUT)).resolve()
    if not output.is_relative_to((ROOT / "archive").resolve()):
        owned_media = (build_cache_directory("android", args.integration) / "ffmpeg").resolve()
        if output != owned_media:
            parser.error("Media output must stay in archive or the verified workspace-owned FFmpeg cache")
    report = plan(args.ndk.resolve(), output, args.api, args.host, args.jobs, args.integration)
    if args.prepare:
        prepare(report, output, args.source_archive)
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
