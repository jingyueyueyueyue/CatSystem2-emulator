"""Prepare fixed Open JTalk sources/data and deferred Windows/Android recipes.

This script never invokes configure, a compiler, make, Cargo, Gradle or ADB.
The emitted recipe is gated until the later implementation-complete phase.
All source/data downloads are from the project's official SourceForge releases.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shlex
import shutil
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
DEFAULT = ROOT / "archive/2026-11/20261116_D_speech_sources"
PACKAGES = {
    "open_jtalk-1.11": (
        "https://downloads.sourceforge.net/open-jtalk/open_jtalk-1.11.tar.gz",
        "20fdc6aeb6c757866034abc175820573db43e4284707c866fcd02c8ec18de71f",
    ),
    "hts_engine_API-1.10": (
        "https://downloads.sourceforge.net/hts-engine/hts_engine_API-1.10.tar.gz",
        "e2132be5860d8fb4a460be766454cfd7c3e21cf67b509c48e1804feab14968f7",
    ),
    "open_jtalk_dic_utf_8-1.11": (
        "https://downloads.sourceforge.net/open-jtalk/open_jtalk_dic_utf_8-1.11.tar.gz",
        "33e9cd251bc41aa2bd7ca36f57abbf61eae3543ca25ca892ae345e394cb10549",
    ),
    "hts_voice_nitech_jp_atr503_m001-1.05": (
        "https://downloads.sourceforge.net/open-jtalk/hts_voice_nitech_jp_atr503_m001-1.05.tar.gz",
        "2e555c88482267b2931c7dbc7ecc0e3df140d6f68fc913aa4822f336c9e0adfc",
    ),
}
COMPONENTS = (
    "text2mecab", "mecab/src", "mecab2njd", "njd", "njd_set_pronunciation",
    "njd_set_digit", "njd_set_accent_phrase", "njd_set_accent_type",
    "njd_set_unvoiced_vowel", "njd_set_long_vowel", "njd2jpcommon", "jpcommon",
)


def shell(path: Path) -> str:
    value = path.resolve().as_posix()
    if len(value) > 2 and value[1] == ":":
        value = f"/{value[0].lower()}{value[2:]}"
    return shlex.quote(value)


def prepare(output: Path) -> None:
    output.mkdir(parents=True, exist_ok=True)
    manifest = []
    for name, (url, expected) in PACKAGES.items():
        archive = output / f"{name}.tar.gz"
        if not archive.exists():
            with urllib.request.urlopen(url, timeout=60) as response:
                data = response.read(80 * 1024 * 1024 + 1)
            if len(data) > 80 * 1024 * 1024:
                raise ValueError("speech archive exceeds80 MiB")
            archive.write_bytes(data)
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        if digest != expected:
            raise ValueError(f"source hash mismatch: {name}")
        if not (output / name).is_dir():
            with tarfile.open(archive) as source:
                for member in source.getmembers():
                    if not (output / member.name).resolve().is_relative_to(output.resolve()):
                        raise ValueError("speech archive path escape")
                source.extractall(output, filter="data")
        manifest.append({"package": name, "url": url, "sha256": digest})

    package = output / "package"
    data = package / "speech-data"
    data.mkdir(parents=True, exist_ok=True)
    dictionary = output / "open_jtalk_dic_utf_8-1.11"
    shutil.copytree(dictionary, data / "dictionary", dirs_exist_ok=True)
    voice = output / "hts_voice_nitech_jp_atr503_m001-1.05"
    shutil.copy2(voice / "nitech_jp_atr503_m001.htsvoice", data)
    licenses = package / "licenses"
    licenses.mkdir(exist_ok=True)
    for name in PACKAGES:
        shutil.copy2(output / name / "COPYING", licenses / f"{name}.COPYING")
    shutil.copy2(output / "open_jtalk-1.11/mecab/COPYING", licenses / "MeCab.COPYING")
    notice = (
        "Independent cs2_speech adapter: MIT, source in cs2-desktop.\n"
        "Open JTalk1.11 / HTS engine API1.10 / MeCab: modified BSD licenses, retained here.\n"
        "UTF-8 dictionary: NAIST/UniDic/Open JTalk attribution, retained here.\n"
        "Voice: NIT ATR503 M001, Nagoya Institute of Technology/HTS Working Group,\n"
        "HTS voice version1.05, Creative Commons Attribution3.0.\n"
        "https://creativecommons.org/licenses/by/3.0/\n"
        "No endorsement by the original projects, universities or contributors is implied.\n"
        "The packaged voice is an independent host default, not the original game's voice.\n"
    )
    (licenses / "NOTICE.txt").write_text(notice, encoding="utf-8")
    (output / "speech-sources.json").write_text(json.dumps({
        "scope": "source/data preparation only; no compiler, synthesis, playback or tests",
        "bridge_abi": 2,
        "packages": manifest,
        "data_directory": str(data),
        "source_archives": [str(output / f"{name}.tar.gz") for name in PACKAGES],
        "licenses": str(licenses),
        "adapter_sha256": {
            name: hashlib.sha256((ROOT / "simulator/crates/desktop/src" / name).read_bytes()).hexdigest()
            for name in ["cs2_speech.c", "cs2_speech.h", "cs2_speech_utf8_io.h"]
        },
    }, indent=2), encoding="utf-8")

    adapter = ROOT / "simulator/crates/desktop/src"
    components = " ".join(shlex.quote(name) for name in COMPONENTS)
    includes = " ".join(f'-I"$JT/{name}"' for name in COMPONENTS)
    archives = " ".join(
        f'"$JT/{name}/lib{"mecab" if name == "mecab/src" else name}.a"'
        for name in COMPONENTS
    )
    recipe = f"""#!/usr/bin/env bash
set -euo pipefail
: "${{CS2_IMPLEMENTATION_COMPLETE:?Set to1 only after the main implementation audit}}"
test "$CS2_IMPLEMENTATION_COMPLETE" = 1
BASE={shell(output)}
ADAPTER={shell(adapter)}
TARGET="${{1:?Use windows-x64 or android-arm64}}"
BUILD="$BASE/build/$TARGET"
PREFIX="$BUILD/prefix"
DEST="$BASE/package/$TARGET"
mkdir -p "$BUILD" "$PREFIX/lib" "$PREFIX/include" "$DEST"
case "$TARGET" in
 windows-x64)
  HOST=x86_64-w64-mingw32
  export CC="${{CS2_SPEECH_CC:-gcc}}" CXX="${{CS2_SPEECH_CXX:-g++}}"
  export AR="${{CS2_SPEECH_AR:-ar}}" RANLIB="${{CS2_SPEECH_RANLIB:-ranlib}}"
  OUTPUT=cs2_speech.dll
  TARGET_FLAGS=""
  EXTRA="-lwinmm"
  ;;
 android-arm64)
  : "${{ANDROID_NDK_HOME:?Use the installed NDK28.2 path in MSYS/POSIX syntax}}"
  TOOL="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/${{CS2_NDK_HOST:-windows-x86_64}}/bin"
  HOST=aarch64-linux-android
  SUFFIX=""
  if test -f "$TOOL/clang.exe"; then SUFFIX=.exe; fi
  export CC="$TOOL/clang$SUFFIX" CXX="$TOOL/clang++$SUFFIX"
  export AR="$TOOL/llvm-ar$SUFFIX" RANLIB="$TOOL/llvm-ranlib$SUFFIX"
  TARGET_FLAGS="--target=aarch64-linux-android26"
  OUTPUT=libcs2_speech.so
  EXTRA="-Wl,-z,max-page-size=16384 -Wl,-soname,libcs2_speech.so"
  ;;
 *) printf '%s\\n' "unsupported speech target" >&2; exit1 ;;
esac
export CFLAGS="$TARGET_FLAGS -O2 -fPIC -fvisibility=hidden"
export CXXFLAGS="$TARGET_FLAGS -O2 -fPIC -fvisibility=hidden" LDFLAGS=""
if test ! -f "$BUILD/hts/configure"; then cp -a "$BASE/hts_engine_API-1.10" "$BUILD/hts"; fi
if test ! -f "$BUILD/jtalk/configure"; then cp -a "$BASE/open_jtalk-1.11" "$BUILD/jtalk"; fi
HTS="$BUILD/hts"
JT="$BUILD/jtalk"
cd "$HTS"
bash ./configure --host="$HOST" --prefix="$PREFIX"
make -C lib -j"${{CS2_BUILD_JOBS:-4}}" \\
 CFLAGS="$CFLAGS -include \\"$ADAPTER/cs2_speech_utf8_io.h\\"" libHTSEngine.a
cp lib/libHTSEngine.a "$PREFIX/lib/"
cp include/HTS_engine.h "$PREFIX/include/"
cd "$JT"
bash ./configure --host="$HOST" --with-charset=UTF-8 \\
 --with-hts-engine-header-path="$PREFIX/include" --with-hts-engine-library-path="$PREFIX/lib"
for PART in {components}; do
 case "$PART" in mecab/src) ARCHIVE=libmecab.a ;; *) ARCHIVE="lib$PART.a" ;; esac
 make -C "$PART" -j"${{CS2_BUILD_JOBS:-4}}" "$ARCHIVE"
done
"$CC" $TARGET_FLAGS -std=c11 -O2 -fPIC -I"$ADAPTER" -I"$PREFIX/include" {includes} \\
 -c "$ADAPTER/cs2_speech.c" -o "$BUILD/cs2_speech.o"
"$CXX" $TARGET_FLAGS -shared -static-libstdc++ -static-libgcc "$BUILD/cs2_speech.o" \\
 -Wl,--start-group {archives} "$PREFIX/lib/libHTSEngine.a" -Wl,--end-group \\
 -lm $EXTRA -o "$DEST/$OUTPUT"
printf '%s\\n' "Speech library built. Copy package/speech-data and licenses alongside the app."
"""
    # Recipe is data until the separately authorized build phase.
    (output / "build-speech-deferred.sh").write_text(recipe.replace("exit1", "exit 1"), encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepare", action="store_true")
    parser.add_argument("--output", type=Path, default=DEFAULT)
    args = parser.parse_args()
    if args.prepare:
        prepare(args.output.resolve())
    else:
        print(json.dumps({"packages": PACKAGES, "output": str(args.output),
                          "targets": ["windows-x64", "android-arm64"],
                          "builds_executed": False}, indent=2))


if __name__ == "__main__":
    main()
