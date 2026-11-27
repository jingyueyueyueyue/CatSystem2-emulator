"""Run the Windows system-feature regressions with private save profiles."""

import argparse
import os
from pathlib import Path
import subprocess
import sys

from run_playthrough_batch import ROOT


AUTOSAVE_READ = "pc_system_features_autosave_read_regression.json"

# Each replay batch may contain one scenario per game. The Newton dictionary
# and quick save scenarios use fast text; the backlog uses its traced timeline.
BATCHES = [
    ((0, 1, 2, 5, 6, 7, 8, 10, 14, 16, 18, 19, 20), ("pc_button_titles.json",), ()),
    ((3, 4, 6, 9, 11, 12, 13, 15, 17), ("pc_system_features_regression.json",), ()),
    ((2, 3, 6, 9), ("pc_system_features_character_gallery_regression.json",
                         "pc_system_features_platform_regression.json",
                         "pc_system_features_rich_regression.json"), ()),
    ((6, 9, 18), ("pc_system_features_save_exchange_regression.json",
                   "pc_system_features_cg_variation_regression.json", AUTOSAVE_READ), ()),
    ((3, 9), ("pc_system_features_island_backlog_voice_regression.json",
              "pc_system_features_music_regression.json"), ()),
    ((9, 18), ("pc_system_features_scene_regression.json",
               "pc_system_features_campanella_state_tree_regression.json"), ()),
    ((5, 8), ("pc_system_features_legacy_layout_regression.json",), ()),
    ((5,), ("pc_system_features_utate_cold_save_regression.json",), ()),
    ((16,), ("pc_system_features_dictionary_regression.json",), ("--fast-text",)),
    ((16,), ("pc_system_features_quick_save_regression.json",), ("--fast-text",)),
    ((16,), ("pc_backlog_text_regression.json",), ()),
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--player", type=Path, required=True)
    parser.add_argument("--jobs", type=int, default=2)
    parser.add_argument("--cache-root", type=Path, help="absolute directory for the playthrough cache")
    parser.add_argument("--cpu-replay", action="store_true", help="use the CPU replay path for diagnosis")
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error("--jobs must be positive")
    if args.cache_root is not None and not args.cache_root.is_absolute():
        parser.error("--cache-root must be an absolute path")
    try:
        player = args.player.resolve(strict=True)
    except OSError as error:
        parser.error(str(error))
    here = Path(__file__).resolve().parent
    prefix = [sys.executable, "-B", "-X", "utf8"]
    player_options = ["--player", str(player)]
    if args.cache_root is not None:
        player_options.extend(("--cache-root", str(args.cache_root)))
    env = dict(os.environ, CS2_REPLAY_PERF="0" if args.cpu_replay else "1")
    result = 0
    for indices, manifests, flags in BATCHES:
        command = [str(here / "run_playthrough_batch.py"), *player_options,
                   "--button-probe", "--strict", "--jobs", str(args.jobs), "--admission-wait-seconds", "120",
                   "--indices", *map(str, indices), *flags]
        for manifest in manifests:
            command.extend(("--scenarios", str(here / manifest)))
        print(", ".join(manifests), flush=True)
        if subprocess.run([*prefix, *command], cwd=ROOT, env=env).returncode:
            result = 1

    for script, options in [
        ("run_pc_autosave_roundtrip.py", []),
        ("run_pc_ordinary_save_roundtrip.py", ["--index", "6"]),
        ("run_pc_ordinary_save_roundtrip.py", ["--index", "6", "--overwrite-seed",
          "archive/2026-11/20261126_06_system_features_seed_save0000.dat", "--deny-directory-writes"]),
        ("run_island_save_comment_roundtrip.py", []),
        ("run_pc_settings_roundtrip.py", []),
    ]:
        print(" ".join([script, *options]), flush=True)
        command = [*prefix, str(here / script), *player_options, *options]
        if subprocess.run(command, cwd=ROOT, env=env).returncode:
            result = 1
    return result


if __name__ == "__main__":
    raise SystemExit(main())
