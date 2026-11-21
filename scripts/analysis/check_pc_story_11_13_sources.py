"""Validate archived 11/13 replay evidence and print a text-free JSON receipt."""

import argparse
from dataclasses import dataclass
import hashlib
import json
import math
from pathlib import Path
import re
import struct
import zlib

from audit_runtime_evidence import blowfish_le_decrypt, kif_entries
from check_pc_body_successor import BODY, WINDOW, read_host
from cst_variants import expand_plain_cst
from run_playthrough_batch import file_sha256, identify_player


HERE = Path(__file__).resolve().parent
LOG_ROOT = Path("F:/CS2-build-cache/playthrough/2026-11")
MOVIE = re.compile(
    r'^frame (\d+): movie handle=(\d+) target=Some\((\d+)\) resource=Some\("([^"]+)"\) '
    r'playback=(\d+) active=true looped=Some\(true\) decoded_pts=Some\(([^)]+)\) '
    r'composited_pts=Some\(([^)]+)\)$'
)
NATIVE = re.compile(r"frame=(\d+) native-call index=(\d+) arguments=(\d+) bytes \[([0-9a-f]*)\]")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def json_file(path):
    return json.loads(path.read_text(encoding="utf-8"))


@dataclass(frozen=True)
class Replay:
    sample: str
    root: Path
    summary: dict
    identity: dict
    events: list[str]
    host: list[str]

    @classmethod
    def open(cls, root, sample):
        summary = json_file(root / f"{sample}.json")
        identity = json_file(root / "build_identity.json")
        require(
            summary["index"] == int(sample) and summary["exit_code"] == 0
            and not summary["first_engine_error"] and not summary["timed_out"]
            and Path(summary["command"][0]) == Path(identity["player"]),
            f"{root.name}: unsuccessful run or mismatched player",
        )
        require(identify_player(Path(identity["player"]), require_integration=True) == identity,
                f"{root.name}: build identity mismatch")
        events = (root / f"{sample}.events.log").read_text(encoding="utf-8").splitlines()
        host = (root / f"{sample}.host.log").read_text(encoding="utf-8").splitlines()
        return cls(sample, root, summary, identity, events, host)

    @property
    def game(self):
        return Path(self.summary["game"])

    def metadata(self):
        return {
            "run": str(self.root),
            "build": self.identity,
            "logs_sha256": {
                stream: file_sha256(self.root / f"{self.sample}.{stream}.log")
                for stream in ("events", "host")
            },
        }

    def native(self, line, frame, index, *words):
        actual = NATIVE.fullmatch(self.events[line - 1])
        packet = struct.pack(f"<{len(words)}I", *words)
        require(actual is not None and actual.groups() == (
            str(frame), str(index), str(len(packet)), packet.hex()
        ), f"{self.sample}.events.log:{line}: native {index} packet mismatch")

    def marker(self, line, frame, suffix):
        text = self.events[line - 1]
        require(text.startswith(f"frame={frame} ") and text.endswith(suffix),
                f"{self.sample}.events.log:{line}: marker mismatch")

    def text_matches(self, line, frame, tag, expected):
        event = self.events[line - 1]
        prefix, separator, literal = event.partition(" debug=")
        require(separator and prefix.startswith(f"frame={frame} native-call "),
                f"{self.sample}.events.log:{line}: missing text debug event")
        # Rust's Debug escapes some characters as \u{...}, which JSON spells differently.
        literal = re.sub(r"\\u\{([0-9a-f]+)\}", lambda match: json.dumps(
            chr(int(match[1], 16)), ensure_ascii=True)[1:-1], literal)
        require(json.loads(literal) == f"[{tag}] {expected}",
                f"{self.sample}.events.log:{line}: source and debug text differ")

    def settled(self, line, frame, owner, window, wait_address, movies, last_input):
        states, _, presses = read_host(self.host)
        state = next((state for state in states if state[0] == frame), None)
        require(presses and max(pressed_at for pressed_at, _ in presses) == last_input,
                f"{self.sample}: entry input timeline differs")
        require(state and self.host[line - 1] == f"frame {frame}: wait {state[1]}",
                f"{self.sample}.host.log:{line}: no frame inspection")
        _, status, layers = state
        windows = WINDOW.findall(status.partition(" windows(")[2].partition(" body_wait_shell(")[0])
        owners = [item for item in windows if item[1] == window]
        quiet = (
            f"focused=true paused=false message_wait=Some({wait_address}) "
            f"vm_waiting_for_message=true pending_messages=0 movies={movies} "
        )
        alpha = layers.get(owner, {}).get(BODY, 0)
        require(owners == [(str(owner), window, "true", "MES_SHOW", str(frame))]
                and quiet in status and alpha > 0,
                f"{self.sample}.host.log:{line}: no settled body-window evidence")
        require(any(event == f"frame={frame} input-wait native 71 address={wait_address:#x}"
                    for event in self.events), f"{self.sample}: missing native 71 at frame {frame}")
        require("trace_enabled=false" in status and "body_generation(owner,generation,current_committed)=None" in status,
                f"{self.sample}: archived body-generation diagnostics differ")
        return {"frame": frame, "host_line": line, "owner": owner, "window": window,
                "vm_wait": hex(wait_address), "movies": movies, "body_alpha": alpha}


def archive_member(game, name, seed, digest):
    archive_name, member = name.split("::")
    archive = game / archive_name
    entries, _, cipher = kif_entries(archive, seed)
    entry = next((row for row in entries if row["name"].lower() == member.lower().encode("ascii")), None)
    require(entry is not None, f"missing member {name}")
    with archive.open("rb") as stream:
        stream.seek(entry["offset"])
        data = stream.read(entry["size"])
    if cipher is not None:
        data = blowfish_le_decrypt(cipher, data)
    require(len(data) == entry["size"] and sha256(data) == digest, f"{name}: member hash mismatch")
    return data, {"resource": name, "sha256": digest, "member_offset": entry["offset"],
                  "member_bytes": entry["size"]}


def read_cst(game, source, seed, digest):
    data, member = archive_member(game, source, seed, digest)
    _, name = source.split("::")
    spellings = {name.casefold(), name.casefold() + "l"}
    loose = game / "scene"
    require(not loose.is_dir() or not any(file.name.casefold() in spellings for file in loose.iterdir()),
            f"{source}: loose scene or CSTL candidate")
    for archive in game.glob("*.int"):
        if archive.name.casefold().startswith("update") or archive.name.casefold() == "scene.int":
            entries, _, _ = kif_entries(archive, seed)
            alternatives = spellings if archive.name.casefold() != "scene.int" else {name.casefold() + "l"}
            names = {item.encode("ascii") for item in alternatives}
            require(not any(row["name"].lower() in names for row in entries),
                    f"{source}: additional scene or CSTL candidate in {archive.name}")
    body, _ = expand_plain_cst(data)
    _, count, offsets, strings = struct.unpack_from("<4I", body)
    records = []
    for group in range(count):
        size, first = struct.unpack_from("<II", body, 16 + 8 * group)
        for ordinal in range(size):
            relative, = struct.unpack_from("<I", body, 16 + offsets + 4 * (first + ordinal))
            start = 16 + strings + relative
            records.append((group, ordinal, body[start + 1], body[start + 2:body.index(b"\0", start)]))
    return records, member


def following_waits(records, first_wait, expected):
    require(records[first_wait][2] == 0x02, "first wait is not CST opcode 0x02")
    body, controls, waits = [], [], []
    for record, (group, _, kind, data) in enumerate(records[first_wait + 1:], first_wait + 1):
        if kind == 0x20 and data:
            body.append(record)
        elif kind == 0x30:
            controls.append({"record": record, "command": data.decode("ascii").strip()})
        elif kind in (0x02, 0x03):
            waits.append({"group": group, "wait_record": record, "body_records": body,
                          "opcode": f"{kind:#04x}", "controls": controls})
            if len(waits) == len(expected):
                break
            body, controls = [], []
    actual = [{key: row[key] for key in ("group", "wait_record", "body_records")} for row in waits]
    require(actual == expected and all(row["opcode"] == "0x02" for row in waits),
            "adjacent CST waits differ from the scenario")
    return waits


def text_receipt(record, data, codec, log_lines):
    utf8 = data.decode(codec).encode("utf-8")
    return {"record": record, "stored_codec": codec, "raw_bytes": len(data), "raw_sha256": sha256(data),
            "utf8_bytes": len(utf8), "utf8_sha256": sha256(utf8), "events_lines": log_lines}


def check_11(log_root):
    config = json_file(HERE / "pc_story_11_radio_wave_loop_probe.json")["probe_anchors"]
    source, movie = config["source_observation"], config["movie_composition"]
    runtime = Replay.open((log_root / source["events_log"]).parent, "11")
    media = Replay.open((log_root / movie["host_log"]).parent, "11")
    require(runtime.game == media.game, "11: text and movie runs use different game roots")
    seed = int(config["archive_name_seed"], 16)
    records, member = read_cst(runtime.game, config["scene"], seed, config["scene_member_sha256"])
    require(runtime.events[source["scene_load_line"] - 1].endswith('debug="sscript loaded > 00_001"'),
            "11: missing scene-load receipt")
    frame, handle, scratch = source["frame"], source["context_handle"], int(source["scratch"], 16)
    text = []
    for row in source["records"]:
        record = row["absolute_record"]
        group, ordinal, kind, data = records[record]
        read, expand, debug = row["read_line"], row["expand_line"], row["debug_line"]
        receipt = text_receipt(record, data, source["stored_codec"], [read, read + 1, expand, debug])
        require(group == source["group"] and kind == int(row["kind"], 16) and b"$str" not in data
                and (len(data), receipt["utf8_bytes"]) == (row["raw_bytes"], row["utf8_bytes"])
                and (expand, debug) == (read + 2, read + 3),
                f"11: record {record} does not meet the no-expansion case")
        runtime.native(read, frame, 1037, handle, scratch, group, ordinal)
        runtime.native(read + 1, frame, 336, handle, 0xffffffff, 0, scratch)
        runtime.native(expand, frame, 278, scratch)
        runtime.text_matches(debug, frame, "name" if kind == 0x21 else "message", data.decode("cp932"))
        text.append(receipt)
    first = config["records"]["first_body_wait"]
    runtime.native(source["wait_kind_line"], frame, 326, handle, *records[first][:2])
    runtime.marker(source["input_marker_line"], frame, 'debug="[input]"')

    play = movie["play_line"]
    play_frame = movie["play_frame"]
    media.marker(play - 2, play_frame, 'debug="[command] pl 1 vcom play -1"')
    media.native(play, play_frame, 266, movie["handle"], 0xffffffff)
    resource = config["movie_resource"].replace("::", "/")
    identity = (str(movie["handle"]), str(movie["target"]), resource, str(movie["playback"]))
    samples = []
    for row in movie["samples"]:
        match = MOVIE.fullmatch(media.host[row["line"] - 1])
        require(match and match.groups()[1:5] == identity and int(match[1]) == row["frame"]
                and math.isfinite(float(match[6])) and float(match[7]) == row["composited_pts"],
                "11: movie identity or PTS mismatch")
        samples.append({**row, "decoded_pts": float(match[6])})
    frames = [row["frame"] for row in samples]
    require(frames == sorted(set(frames)) and play_frame < frames[0]
            and len({row["composited_pts"] for row in samples}) >= 2,
            "11: missing two distinct samples of a single playback")
    handle_hex = struct.pack("<I", movie["handle"]).hex()
    for event in media.events[play:]:
        call = NATIVE.fullmatch(event)
        if call and call[2] == "266" and call[4].startswith(handle_hex):
            require(int(call[1]) > frames[-1], "11: playback restarted within the sampled interval")
    release = source["draw_release"]
    runtime.native(release["events_line"], release["frame"], 528, release["target"], 0)
    settled = media.settled(movie["stable_wait_line"], movie["stable_frame"], movie["body_owner"],
                            "cinema.fes", 0xd6d, 1, config["last_input_frame"])
    waits = following_waits(records, first, config["following_waits"])
    return {"sample": 11, "source_run": runtime.metadata(), "media_run": media.metadata(),
            "scene": member, "first_wait": {"group": 0, "record": first, "events_line": source["wait_kind_line"]},
            "text": {"context": handle, "scratch": hex(scratch), "read_native": 1037, "expand_native": 278,
                     "observed_output": "Utf8", "comparison": "exact", "records": text},
            "movie": {"resource": resource, "handle": movie["handle"], "target": movie["target"],
                      "playback": movie["playback"], "looped": movie["looped"],
                      "play_frame": play_frame, "play_line": play, "samples": samples},
            "draw_release": release, "settled_observation": settled, "next_three_cst_waits": waits}


def check_13(log_root):
    config = json_file(HERE / "pc_story_13_blaop1_end_provenance_probe.json")["13"]["review"]
    observed = config["runtime_observation"]
    runtime = Replay.open(log_root / observed["run"], "13")
    scene = config["member"]
    seed = int(scene["archive_name_seed"], 16)
    records, member = read_cst(runtime.game, scene["source"], seed, scene["sha256"])
    load = observed["scene_loaded"]
    runtime.marker(load["events_line"], load["frame"], 'debug="sscript loaded > BLAOP1_END"')
    op = observed["op_movie_load"]
    runtime.marker(op["events_line"], op["frame"], 'movie_load handle=274 resource="movie.int/blaop1.mpg"')
    controls = observed["pre_body_controls"]
    for row in controls:
        runtime.marker(row["events_line"], row["frame"], 'debug="[command] ' + row["command"] + '"')
    shared, debug = observed["shared_publication"], observed["legacy_debug"]
    frame, handle, scratch = shared["frame"], shared["context_handle"], int(shared["scratch"], 16)
    text = []
    for label, tag, opcode in (("speaker", "name", 0x21), ("body", "message", 0x20)):
        row = scene[label]
        record = row["absolute_record"]
        group, ordinal, kind, data = records[record]
        read, publish, logged = shared[f"{label}_read_line"], shared[f"{label}_publish_line"], debug[f"{label}_line"]
        string_id = row["shared_string"]
        receipt = text_receipt(record, data, scene["codec"].lower(), [read, publish, logged])
        require((group, ordinal, kind) == (scene["group"], row["ordinal"], opcode)
                and (len(data), receipt["utf8_bytes"]) == (row["bytes"], row["utf8_bytes"])
                and publish == read + 1, f"13: record {record} read/publication mismatch")
        runtime.native(read, frame, 327, handle, scratch, group, ordinal)
        runtime.native(publish, frame, 276, string_id, scratch)
        for line in runtime.events[publish:logged - 1]:
            call = NATIVE.fullmatch(line)
            if call and call[2] == "276":
                require(int.from_bytes(bytes.fromhex(call[4][:8]), "little") != string_id,
                        f"13: shared string {string_id} overwritten before the debug marker")
        runtime.text_matches(logged, debug["frame"], tag, data.decode("utf-8", errors="replace"))
        text.append({**receipt, "shared_string": string_id})
    runtime.marker(debug["input_line"], debug["frame"], 'debug="[input]"')

    fes = config["fes"]
    archives = []
    for name in fes["matching_archive_members"]:
        fes_bytes, archive = archive_member(runtime.game, name, seed, fes["sha256"])
        require(fes_bytes.startswith(b"FES\0"), f"{name}: invalid FES signature")
        document = zlib.decompress(fes_bytes[16:])
        for label in ("speaker", "body"):
            statement = fes[label]
            offset = int(statement["statement_offset"], 16)
            template = document[offset:document.index(b"\n", offset)]
            sections = re.findall(rb"^\s*#([A-Z_0-9]+)", document[:offset], flags=re.MULTILINE)
            variable = rb"\$str" + str(statement["shared_string"]).encode("ascii") + rb"(?![0-9])"
            require(sections and sections[-1] == statement["block"].encode("ascii")
                    and re.search(variable, template), f"{name}: {label} FES template mismatch")
        archives.append(archive)
    settled = observed["settled"]
    waits_at = [runtime.settled(line, frame, settled["owner"], "meswnd.fes", 0xad0, 0, 361)
                for line, frame in zip(settled["host_lines"], settled["frames"], strict=True)]
    first = scene["wait"]["absolute_record"]
    waits = following_waits(records, first, config["following_waits"])
    return {"sample": 13, "source_run": runtime.metadata(), "scene": member,
            "first_wait": {"group": 4, "record": first, "input_marker_line": debug["input_line"]},
            "text": {"context": handle, "scratch": hex(scratch), "read_native": 327, "publish_native": 276,
                     "comparison": "raw bytes with logger's lossy UTF-8 conversion", "records": text},
            "fes_members": archives, "fes_statements": {label: fes[label] for label in ("speaker", "body")},
            "movie_load": op, "scene_load": load, "pre_body_controls": controls,
            "settled_observations": waits_at, "next_three_cst_waits": waits}


def check_eden_publication(run):
    runtime = Replay.open(run, "13")
    require(not runtime.summary["captures"], "unexpected screenshots")
    config = json_file(HERE / "pc_story_13_blaop1_end_provenance_probe.json")["13"]["review"]["member"]
    seed = int(config["archive_name_seed"], 16)
    records, member = read_cst(runtime.game, config["source"], seed, config["sha256"])
    body = config["body"]
    record = body["absolute_record"]
    group, ordinal, kind, data = records[record]
    require((group, ordinal, kind, len(data)) == (config["group"], body["ordinal"], 0x20, body["bytes"]),
            "Eden body record mismatch")
    source_pattern = re.compile(
        rf"frame=(\d+) cst-source-record CstSourceRecordObservation \{{ source: CstBodySource "
        rf"\{{ context_handle: (\d+), load_id: ([1-9]\d*), group: {group}, absolute_record: {record} \}}, "
        rf"frame: \1, probe: Eden, native: 327, kind: 32, stored_codec: Gbk, output_codec: Gbk, "
        rf"raw_bytes: {len(data)}, output_bytes: {len(data)}, body_tagged: true, localized: false \}}"
    )
    sources = [(number, *map(int, match.groups())) for number, line in enumerate(runtime.events, 1)
               if (match := source_pattern.fullmatch(line))]
    require(len(sources) == 1, "missing unique live GBK CST source")
    source_line, source_frame, handle, load = sources[0]
    publication = []
    first_offset = max(0, source_line - 2)
    for offset, line in enumerate(runtime.events[first_offset:-2], first_offset):
        read = NATIVE.fullmatch(line)
        if not read or read.groups()[1:3] != ("327", "16"):
            continue
        context, scratch, input_group, input_ordinal = struct.unpack("<4I", bytes.fromhex(read[4]))
        frame = int(read[1])
        if frame < source_frame or (context, input_group, input_ordinal) != (handle, group, ordinal):
            continue
        # The body read itself can emit the first source observation.
        publish_offset = offset + 1 + int(offset + 2 == source_line)
        if publish_offset + 1 >= len(runtime.events):
            continue
        publish = NATIVE.fullmatch(runtime.events[publish_offset])
        if publish and publish.groups() == (
            str(frame), "276", "8", struct.pack("<2I", body["shared_string"], scratch).hex()
        ):
            publication.append((frame, offset, publish_offset, scratch))
    require(len(publication) == 1, "missing unique adjacent 327/276 body publication")
    frame, read_offset, publish_offset, scratch = publication[0]
    first = ord(data.decode("gbk")[0])
    queued_offset = publish_offset + 1
    if (dispatch := NATIVE.fullmatch(runtime.events[queued_offset])) and dispatch.groups()[:3] == (
        str(frame), "630", "16"
    ):
        queued_offset += 1
    require(runtime.events[queued_offset] ==
            f"frame={frame} fes-text-sourced-queued bytes={len(data)} first=U+{first:04X} fes=Cp932 cst=[Gbk]",
            "body was not queued through the GBK sourced FES path")
    read_packet = struct.pack("<4I", handle, scratch, group, ordinal).hex()
    debug = []
    for index in range(publish_offset + 2, len(runtime.events) - 2):
        read = NATIVE.fullmatch(runtime.events[index])
        if (read and read.groups()[1:] == ("327", "16", read_packet)
                and runtime.events[index + 2].startswith(f"frame={read[1]} ")
                and runtime.events[index + 2].endswith('debug="[input]"')):
            debug.append((int(read[1]), index + 2))
    require(len(debug) == 1, "missing unique r102 reread at the game wait")
    debug_frame, debug_line = debug[0]
    runtime.text_matches(debug_line, debug_frame, "message", data.decode("utf-8", errors="replace"))
    return {
        "sample": "13", "status": "eden_publication_verified", "source_run": runtime.metadata(),
        "member": member, "context": handle, "load": load, "group": group, "ordinal": ordinal,
        "source_frame": source_frame, "source_line": source_line, "publication_frame": frame,
        "scratch": hex(scratch), "shared_string": body["shared_string"], "debug_frame": debug_frame,
        "text": text_receipt(record, data, "gbk", [read_offset + 1, publish_offset + 1, publish_offset + 2, debug_line]),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sample", choices=("11", "13", "both"), default="both")
    parser.add_argument("--log-root", type=Path, default=LOG_ROOT)
    parser.add_argument("--eden-publication-run", type=Path, help="verify r102 in a batch directory")
    args = parser.parse_args()
    checks = {"11": check_11, "13": check_13}
    try:
        if args.eden_publication_run:
            print(json.dumps(check_eden_publication(args.eden_publication_run), indent=2))
            return
        receipts = [check(args.log_root) for sample, check in checks.items() if args.sample in (sample, "both")]
    except (OSError, ValueError, IndexError, KeyError, struct.error, zlib.error) as error:
        parser.exit(1, f"source receipt failed: {error}\n")
    for receipt in receipts:
        receipt["status"] = "archived_runtime_source_verified"
        receipt["fes_current_generation"] = None
        receipt["next_waits_observed_at_runtime"] = False
    print(json.dumps({"schema": "cs2-replay-source-receipt/v1", "samples": receipts}, indent=2))


if __name__ == "__main__":
    main()
