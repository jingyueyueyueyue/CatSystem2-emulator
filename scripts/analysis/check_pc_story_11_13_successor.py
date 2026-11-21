"""Check a guarded 11/13 body successor or adjacent replay route."""

import argparse
from dataclasses import asdict, dataclass
import json
import math
from pathlib import Path
import re

from check_pc_body_successor import BODY_GENERATION, DOWN, ENTER, RECEIPT, TASK_CREATED, WAIT, WINDOW
from check_pc_story_11_13_sources import MOVIE, NATIVE, json_file, require
from run_playthrough_batch import identify_player


CONFIG = Path(__file__).with_name("pc_story_11_13_successor_observe.json")
CONTINUATION = Path(__file__).with_name("pc_story_11_13_continuation.json")
NEXT = Path(__file__).with_name("pc_story_10_13_15_next.json")
NEXT_WAITS = Path(__file__).with_name("pc_story_10_11_13_15_next_waits.json")
SHELL = re.compile(
    r'body_wait_shell\(window,owner,scene,loaded,marked,group,record\)='
    r'Some\(\("([^"]+)", (\d+), "([^"]+)", (\d+), (\d+), (\d+), (\d+)\)\)'
)
BODY_PAGE = re.compile(
    r'body_page\(context,sources,exact_match,composited\)='
    r'Some\(\((\d+), Some\(\[(.+?)\]\), true, true\)\)'
)
BODY_SOURCE = re.compile(
    r'CstBodySource \{ context_handle: (\d+), load_id: ([1-9]\d*), '
    r'group: (\d+), absolute_record: (\d+) \}'
)
BINDING = re.compile(r'\blegacy_11_13_enter=Some\(\(([1-9]\d*), ([1-9]\d*)\)\)')
RADIO_BINDING = re.compile(r'\bradio_r94_enter=Some\(\(([1-9]\d*), ([1-9]\d*)\)\)')
RADIO_TEXT_GENERATION = re.compile(
    BODY_GENERATION.pattern.removesuffix(r", true\)\)") + r", (?:true|false)\)\)"
)
EDEN_GENERATION = re.compile(
    r"body_generation\(owner,generation,current_committed\)=Some\(\((\d+), "
    r"Some\(StoryBodyGeneration \{ source_epoch: ([1-9]\d*), source_frame: (\d+), "
    r"natural_complete: None, committed_epoch: Some\(\2\), remaining: 0, "
    r"remaining_segment: 0, pending_text: false, drawing: false, skip_pending: false, forced: false, "
    r"projection: Some\(\(true, [1-9]\d*\)\) \}\), true\)\)"
)
EDEN_LAYOUT = re.compile(r"\beden_layout_complete=Some\(\(([1-9]\d*), (\d+)\)\)")
EDEN_ENTER = re.compile(r"\beden_live_enter=Some\(\(([1-9]\d*), ([1-9]\d*)\)\)")
SOURCE_EVENT = re.compile(
    r'frame=(\d+) cst-source-record CstSourceRecordObservation \{ source: (.+?), '
    r'frame: \1, probe: (\w+), native: (\d+), kind: 32, stored_codec: (\w+), output_codec: (\w+), '
    r'raw_bytes: ([1-9]\d*), output_bytes: ([1-9]\d*), body_tagged: true, localized: false \}'
)
RESUME = re.compile(r'frame=(\d+) message-resume (.*)')
EDEN_OP_LOAD = re.compile(
    r'frame=(\d+) native-call index=265 .* movie_load handle=(\d+) resource="movie.int/blaop1.mpg"'
)


@dataclass(frozen=True)
class Page:
    frame: int
    owner: int
    context: int
    load: int
    loaded: int
    marked: int
    epoch: int
    source_frame: int
    complete_frame: int
    region: int
    receiver: int
    body: tuple[int, ...]
    source_reads: tuple[int, ...]


def read_run(run, sample):
    summary = json_file(run / f"{sample}.json")
    identity = json_file(run / "build_identity.json")
    require(summary["index"] == int(sample) and summary["exit_code"] == 0
            and not summary["first_engine_error"] and not summary["timed_out"]
            and not summary["captures"], "unsuccessful run or screenshots present")
    player = Path(identity["player"])
    require(Path(summary["command"][0]) == player
            and identify_player(player, require_integration=True) == identity, "build identity mismatch")
    streams = []
    for key, extension in (("stderr", "host"), ("stdout", "events")):
        path = Path(summary[key]).resolve()
        require(path.is_relative_to(run.resolve()) and path.name == f"{sample}.{extension}.log",
                f"summary references a different {extension} log")
        streams.append(path.read_text(encoding="utf-8").splitlines())
    return identity, *streams


def source_reads(events, profile):
    source = profile["source"]
    signature = source["probe"], str(source["native"]), source["stored"], source["output"]
    reads = {}
    for line in events:
        if (match := SOURCE_EVENT.fullmatch(line)) and match.groups()[2:6] == signature:
            if source["stored"] == source["output"] and match[7] != match[8]:
                continue
            key = BODY_SOURCE.fullmatch(match[2])
            if key:
                reads.setdefault(tuple(map(int, key.groups())), []).append(int(match[1]))
    return reads


def page_at(line, profile, point, events, reads, earliest):
    wait = WAIT.fullmatch(line)
    if wait is None:
        return None
    frame, status = int(wait[1]), wait[2]
    enter = RADIO_BINDING if point.get("radio_binding") else BINDING
    shell, generation, page, binding = (pattern.search(status)
                                        for pattern in (SHELL, BODY_GENERATION, BODY_PAGE, enter))
    eden_layout = None
    if generation is None and profile["profile"] == "Legacy13Observation":
        generation = EDEN_GENERATION.search(status)
        eden_layout = EDEN_LAYOUT.search(status)
        eden_enter = EDEN_ENTER.search(status)
        if not all((generation, eden_layout, eden_enter, binding)) or eden_enter.groups() != binding.groups():
            return None
    if not all((shell, generation, page, binding)):
        return None
    if (shell[1], shell[3]) != (profile["window"], profile["scene"]):
        return None
    owner, loaded, marked, group, record = map(int, (shell[2], *shell.groups()[3:]))
    if (group, record) != (point["group"], point["wait"]):
        return None
    gen_owner, epoch, source_frame = map(int, generation.groups()[:3])
    if eden_layout:
        if int(eden_layout[1]) != epoch:
            return None
        complete_frame = int(eden_layout[2])
    else:
        complete_frame = int(generation[4])
    if owner != gen_owner or not (earliest <= marked < frame and loaded <= marked
                                  and max(loaded, earliest) <= source_frame <= complete_frame <= frame):
        return None
    address = int(profile["root_wait"], 16)
    prefix = (f'trace_enabled=true profile=Some({profile["profile"]}) pe_sha256=Some("{profile["pe"]}") '
              f'startup=Some("kcs.int/main.kcs") frame={frame} ')
    quiet = (f"focused=true paused=false message_wait=Some({address}) "
             "vm_waiting_for_message=true pending_messages=0 movies=")
    movie_counts = (0, 1) if point.get("radio_stopped") else (profile["movies"],)
    windows_text = status.partition(" windows(")[2].partition(" body_wait_shell(")[0]
    windows = WINDOW.findall(windows_text)
    bodies = [item for item in windows if item[1].removesuffix(".fes") == profile["window"]]
    if (not status.startswith(prefix) or not any(f"{quiet}{count} " in status for count in movie_counts)
            or windows_text.count("Some((\"") != 2
            or len(windows) != 2 or len(bodies) != 1
            or (bodies[0][0], *bodies[0][2:]) != (str(owner), "true", "MES_SHOW", str(frame))
            or not any(item[1].removesuffix(".fes") == "flow" and item[2:] == (
                "false", profile["flow"], str(frame)) for item in windows)
            or f'frame={frame} input-wait native 71 address={profile["root_wait"]}' not in events
            or not any(event.startswith(f"frame={marked} ") and event.endswith('debug="[input]"')
                       for event in events)):
        return None
    # Split between complete Rust Debug structs, including each closing brace.
    sources = [BODY_SOURCE.fullmatch(item) for item in re.split(r"(?<=\}), ", page[2])]
    if not sources or any(item is None for item in sources):
        return None
    items = [tuple(map(int, item.groups())) for item in sources]
    context, load = int(page[1]), items[0][1]
    if items != [(context, load, group, record) for record in point["body"]]:
        return None
    # A current load can prefetch a successor before the previous Enter.
    # The wait marker and the generation above still have to follow that Enter.
    checked_reads = []
    for item in items:
        frames = [at for at in reads.get(item, []) if loaded <= at <= source_frame]
        if not frames:
            return None
        checked_reads.append(max(frames))
    if checked_reads != sorted(checked_reads):
        return None
    if eden_layout:
        if len(items) != 1 or not {"body_bytes", "body_sha256"} <= point.keys():
            return None
        body = items[0][3]
        source = (f"CstBodySource {{ context_handle: {context}, load_id: {load}, "
                  f"group: {group}, absolute_record: {body} }}")
        skip = re.compile(
            rf"frame=(\d+) fes-text-eden-body-op owner={owner} block=MES_SCROLLSPD offset=0x6809 "
            rf"op=skip epoch={epoch} source_frame={source_frame} source={re.escape(source)} "
            rf"body_bytes={point['body_bytes']} body_sha256={point['body_sha256']} "
            rf"visible=false skip=true draw=(?:None|Some\(\d+\)) layout_skip=true"
        )
        skips = [int(match[1]) for event in events if (match := skip.fullmatch(event))
                 and source_frame <= int(match[1]) <= complete_frame]
        if not skips:
            return None
    region, receiver = map(int, binding.groups())
    if receiver == owner:
        return None
    return Page(frame, owner, context, load, loaded, marked, epoch, source_frame, complete_frame,
                region, receiver, tuple(point["body"]), tuple(checked_reads))


def movie_evidence(sample, host, events, first, after):
    if sample == "13":
        loads = [match for line in events if (match := EDEN_OP_LOAD.fullmatch(line))
                 and int(match[1]) < first.loaded]
        require(len(loads) == 1 and any(line.startswith(f"frame={first.loaded} ")
                                      and line.endswith('debug="sscript loaded > BLAOP1_END"')
                                      for line in events), "missing BLAOP1 playback/load sequence")
        op_frame, handle = map(int, loads[0].groups())
        handle_le = handle.to_bytes(4, "little").hex()
        calls = [(int(match[1]), match[2], match[4]) for line in events if (match := NATIVE.fullmatch(line))
                 and match[4].startswith(handle_le) and op_frame <= int(match[1]) <= first.loaded]
        play = [(at, packet) for at, native, packet in calls if native == "266"]
        require(len(play) == 1 and play[0][1][8:] == "01000000", "missing single non-looping BLAOP1 play")
        finished = [int(match[1]) for line in events if (match := re.fullmatch(
            rf"frame=(\d+) story-op-finished handle={handle} playback=[1-9]\d*", line
        )) and play[0][0] < int(match[1]) <= first.loaded]
        require(finished and all(at >= finished[0] for at, native, _ in calls if native == "267"),
                "missing natural BLAOP1 completion before the post-OP scene")
        return {"op_load_frame": op_frame, "op_handle": handle, "play_frame": play[0][0],
                "op_finished_frame": finished[0], "scene_loaded": first.loaded, "active_at_wait": 0}

    latest = first.frame if after is None else after.frame
    samples = [(int(match[1]), match.groups()[1:5], float(match[7])) for line in host
               if (match := MOVIE.fullmatch(line)) and first.marked <= int(match[1]) <= latest
               and match[3] == "31" and match[4] == "movie.int/radio_wave.mpg"
               and math.isfinite(float(match[6])) and math.isfinite(float(match[7]))]
    require(len({identity for _, identity, _ in samples}) == 1
            and {first.frame, latest} <= {at for at, _, _ in samples}
            and len({pts for _, _, pts in samples}) >= (1 if after is None else 2),
            "missing a single looping radio_wave instance with scene composition")
    handle, target, resource, playback = samples[0][1]
    handle_le = int(handle).to_bytes(4, "little").hex()
    require(not any(first.frame < int(match[1]) <= latest and match[2] in ("266", "267", "268")
                    and match[4].startswith(handle_le)
                    for line in events if (match := NATIVE.fullmatch(line))),
            "radio_wave was restarted, stopped, or unloaded")
    return {"resource": resource, "handle": int(handle), "target": int(target), "playback": int(playback),
            "composited_pts": [[frame, pts] for frame, _, pts in samples]}


def verify_radio_stop(host, events, before, after, movie):
    handle = movie["handle"].to_bytes(4, "little").hex()
    calls = [(int(match[1]), int(match[2])) for line in events
             if (match := NATIVE.fullmatch(line)) and before.frame < int(match[1]) <= after.frame
             and match[2] in ("266", "267", "268") and match[4].startswith(handle)]
    require(calls and all(native in (267, 268) for _, native in calls)
            and len({native for _, native in calls}) == len(calls), "missing stop of the original radio_wave handle")
    terminal_wait = next(match[2] for line in host if (match := WAIT.fullmatch(line))
                         and int(match[1]) == after.frame)
    terminal_movies = [line for line in host if line.startswith(f"frame {after.frame}: movie ")]
    inactive = (f'frame {after.frame}: movie handle={movie["handle"]} target=Some(31) '
                f'resource=Some("{movie["resource"]}") playback={movie["playback"]} '
                "active=false looped=None decoded_pts=None composited_pts=None")
    require((" movies=0 " in terminal_wait and not terminal_movies)
            or (" movies=1 " in terminal_wait and terminal_movies == [inactive]),
            "radio_wave is still active or composited at 3/94")
    require(any(before.frame < int(line.split(" ", 1)[0][6:]) <= calls[0][0]
                for line in events if line.startswith("frame=") and line.endswith('debug="[command] pl 1 stop"')),
            "missing CST pl 1 stop before the host movie stop")
    return [{"frame": frame, "native": native} for frame, native in calls]


def radio_after_r94(host, events, r90, r94, after, movie):
    handle = movie["handle"].to_bytes(4, "little").hex()
    calls = [(int(match[1]), int(match[2])) for line in events
             if (match := NATIVE.fullmatch(line)) and r94.frame < int(match[1]) <= after.frame
             and match[2] in ("266", "267", "268") and match[4].startswith(handle)]
    require(any(r90.frame < int(line.split(" ", 1)[0][6:]) <= r94.frame
                for line in events if line.startswith("frame=") and line.endswith('debug="[command] pl 1 stop"')),
            "missing script stop command before r94")
    status = next(match[2] for line in host if (match := WAIT.fullmatch(line)) and int(match[1]) == after.frame)
    terminal = [line for line in host if line.startswith(f"frame {after.frame}: movie ")]
    active = [match for line in terminal if (match := MOVIE.fullmatch(line))]
    same = (str(movie["handle"]), "31", movie["resource"], str(movie["playback"]))
    if not calls and len(active) == len(terminal) == 1 and active[0].groups()[1:5] == same:
        require(" movies=1 " in status, "active movie count changed")
        return {"status": "r97_verified_radio_still_looping", "calls": []}
    inactive = (f'frame {after.frame}: movie handle={movie["handle"]} target=Some(31) '
                f'resource=Some("{movie["resource"]}") playback={movie["playback"]} '
                "active=false looped=None decoded_pts=None composited_pts=None")
    require([native for _, native in calls] in ([267], [267, 268])
            and ((" movies=0 " in status and not terminal)
                 or (" movies=1 " in status and terminal == [inactive])),
            "original loop lacks a script stop and inactive composition receipt after r94")
    return {"status": "r97_verified_radio_stopped", "calls": calls}


def verify_eden_rdraw(events, before, after, duration):
    commands = [(int(line.split(" ", 1)[0][6:]), line.rsplit('debug="[command] ', 1)[1])
                for line in events if line.startswith("frame=") and 'debug="[command] ' in line]
    draws = [frame for frame, command in commands if before.frame < frame < after.marked
             and command == f'rdraw {duration}"']
    require(len(draws) == 1 and after.marked >= draws[0] + duration
            and any(draws[0] <= frame < after.marked and command == 'wait"' for frame, command in commands),
            "missing elapsed script rdraw/wait before the next Eden input marker")
    return {"draw_frame": draws[0], "input_marker": after.marked, "duration": duration}


def verify_radio_blocker(profile, host, events):
    downs = [(int(match[1]), match[2]) for line in host if (match := DOWN.fullmatch(line))]
    require([button for _, button in downs] == profile["entry_buttons"] + ["Keyboard(13)"] * 3,
            "unexpected input timeline")
    require([line for line in events if line.startswith("replay_status=")]
            == ["replay_status=frame_limit"], "radio probe did not stop at the frame limit")
    gates = [frame for frame, _ in downs[-3:]]
    reads = source_reads(events, profile)
    before = next((page for line in host if (page := page_at(
        line, profile, {"group": 2, "wait": 90, "body": [89]}, events, reads, gates[1] + 1
    )) and page.frame == gates[-1]), None)
    require(before, "missing verified 2/90 before the final Enter")
    input_frame = gates[-1] + 1
    consumers = [match.groups() for line in host for match in RECEIPT.finditer(line)
                 if int(match[1]) >= input_frame and int(match[5]) and match[9] == "0"]
    require(consumers == [(str(input_frame), "2", "0", "1", "1", "false",
                           str(before.region), str(before.receiver), "0", ENTER)]
            and f"frame={input_frame} message-resume message=[2, 0, 0, 0]" in events,
            "missing unique last Enter consumption and VM resume")
    require(any(re.fullmatch(r"story frame=\d+ state=active scene=00_001 group=3 record=94", line)
                for line in events), "missing r94 story position")
    terminal = next((line.removeprefix("replay wait: ") for line in reversed(host)
                     if line.startswith("replay wait: ")), "")
    terminal_wait = re.search(r"\bframe=(\d+) state=active scene=00_001 group=3 record=94 ", terminal)
    generation = RADIO_TEXT_GENERATION.search(terminal)
    require(terminal_wait and " movies=1 " in terminal
            and f"message_wait=Some({int(profile['root_wait'], 16)}) vm_waiting_for_message=true" in terminal
            and "legacy_11_13_enter=None" in terminal and generation
            and int(generation[1]) == before.owner and int(generation[2]) > before.epoch
            and input_frame <= int(generation[3]) <= int(generation[4]) <= int(terminal_wait[1]),
            "missing r94 with a completed text generation and one registered playback")
    movie = movie_evidence("11", host, events, before, None)
    handle = movie["handle"].to_bytes(4, "little").hex()
    require(any(line.startswith(f"frame={input_frame} native-call index=1040 ")
                and line.endswith('debug="[command] pl 1 stop"') for line in events)
            and not any(match[2] in ("267", "268") and int(match[1]) >= input_frame
                        and match[4].startswith(handle)
                        for line in events if (match := NATIVE.fullmatch(line))),
            "missing stop-command reproduction or a movie stop was recorded")
    samples = [line for line in host if line.startswith(f"frame {terminal_wait[1]}: movie ")]
    return {"status": "radio_stop_blocker_reproduced", "input_frame": input_frame,
            "body_epoch": int(generation[2]), "body_complete_frame": int(generation[4]),
            "movie": movie, "terminal_movie_samples": samples}


def verify_eden_blocker(profile, host, events):
    require([match[2] for line in host if (match := DOWN.fullmatch(line))]
            == profile["entry_buttons"], "Eden input was sent after entry or entry did not complete")
    require([line for line in events if line.startswith("replay_status=")]
            == ["replay_status=frame_limit"], "Eden did not stop at the frame limit")
    terminal = next((line.removeprefix("replay wait: ") for line in reversed(host)
                     if line.startswith("replay wait: ")), "")
    shell = SHELL.search(terminal)
    page = re.search(r'body_page\(context,sources,exact_match,composited\)='
                     r'Some\(\((\d+), Some\(\[(.+?)\]\), true, false\)\)', terminal)
    live = EDEN_ENTER.search(terminal)
    require(shell and shell[1] == "meswnd" and shell[3] == "BLAOP1_END" and shell.groups()[-2:] == ("4", "103")
            and page and live and "eden_layout_complete=None" in terminal
            and "forced: true" in terminal, "missing live 4/103 with forced, uncommitted body")
    origin = BODY_SOURCE.fullmatch(page[2])
    require(origin and origin[1] == page[1] and origin.groups()[-2:] == ("4", "102"),
            "missing exact r102 source")
    skip = re.compile(rf'frame=(\d+) fes-text-eden-body-op owner={shell[2]} block=MES_SCROLLSPD '
                      rf'offset=0x6809 op=skip .* source={re.escape(page[2])} .* layout_skip=false')
    first = next((int(match[1]) for line in events if (match := skip.fullmatch(line))), None)
    require(first is not None, "missing rejected MES_SCROLLSPD layout skip for r102")
    return {"status": "eden_scroll_blocker_reproduced", "mark_frame": int(shell[5]),
            "first_rejected_skip_frame": first, "region": int(live[1]), "receiver": int(live[2])}


def verify_task_lifetime(events, first, last_frame):
    lifetimes = {}
    tracked = {first.owner, first.receiver, first.region}
    for line in events:
        frame_text, separator, detail = line.partition(" ")
        if not separator or not frame_text.startswith("frame=") or not (task := TASK_CREATED.match(detail)):
            continue
        task_id, kind, parent = map(int, task.groups()[:3])
        frame = int(frame_text[6:])
        if task_id in tracked and frame <= last_frame:
            require(frame <= first.frame, "body owner or Enter route task was recreated after the first wait")
            lifetimes[task_id] = (kind, parent, task[4])
    owner = lifetimes.get(first.owner)
    receiver = lifetimes.get(first.receiver)
    require(owner and owner[:2] == (34, first.receiver) and receiver and receiver[0] == 0
            and lifetimes.get(first.region) == (6, first.receiver, "bottom button"),
            "missing live body/window and bottom-button parent evidence")


def verify_august_speaker_queue(events, first, successor, offset=9):
    start, end = first["frame"], successor["frame"]
    receiver = first["receiver"]
    owner_le = first["owner"].to_bytes(4, "little").hex()
    scratch = (0x8000176C + offset).to_bytes(4, "little").hex()
    copied = re.compile(
        rf"frame=(\d+) cst-text-sourced-copy task={receiver} native=13 next_pc=0x1476 "
        rf"scratch_offset={offset} bytes=9 source=sscript-speaker/Utf8"
    )
    for at, line in enumerate(events):
        match = copied.fullmatch(line)
        if not match or at == 0 or not start < int(match[1]) <= end:
            continue
        native_copy = NATIVE.fullmatch(events[at - 1])
        if not native_copy or native_copy.groups()[:3] != (match[1], "13", "8"):
            continue
        target, source = native_copy[4][:8], native_copy[4][8:]
        if source != scratch:
            continue
        frame = int(match[1])
        frame_prefix = f"frame={frame} "
        published = notified = False
        for following in events[at + 1:]:
            if following.startswith("frame=") and not following.startswith(frame_prefix):
                break
            native = NATIVE.fullmatch(following)
            if native and native[2] == "276" and native[4].startswith("e9030000"):
                published = native[4] == "e9030000" + target
                notified = False
            elif native and native[2] == "630" and native[4].startswith(owner_le + "af820700"):
                notified = published
            elif notified and following == (
                frame_prefix + "fes-text-sourced-queued bytes=9 first=U+FF1F fes=Cp932 cst=[Utf8]"
            ):
                return {"frame": frame, "scratch_offset": offset, "shared_id": 1001, "owner": first["owner"]}
    raise ValueError(f"missing AUGUST +{offset} speaker copy, $str1001 publication and cinema MES_SETNAME UTF-8 queue")


def verify(sample, profile, host, events, observe_only, continuation=(), radio_stop=False, radio_r94=False):
    downs = [(int(match[1]), match[2]) for line in host if (match := DOWN.fullmatch(line))]
    entry = profile["entry_buttons"]
    targets = [] if observe_only else [profile["next"], *continuation]
    buttons = entry + ["Keyboard(13)"] * len(targets)
    require([button for _, button in downs] == buttons, "unexpected input timeline")
    reads = source_reads(events, profile)
    gates = [at for at, _ in downs[len(entry):]]
    gate = gates[0] if gates else None
    before = [page for line in host if (page := page_at(line, profile, profile["first"], events, reads,
                                                       downs[len(entry) - 1][0] + 1))
              and (gate is None or page.frame == gate)]
    require(before, "missing first wait with exact committed sources and live Enter binding")
    first = before[-1]
    require(not any(at >= first.marked for at, _ in downs[:len(entry)]),
            "entry input overlaps the first body wait")
    if observe_only:
        verify_task_lifetime(events, first, first.frame)
        return {"status": "first_wait_verified", "first": asdict(first),
                "movie": movie_evidence(sample, host, events, first, None)}

    statuses = [line for line in events if line.startswith("replay_status=")]
    require(statuses == ["replay_status=completed"], "guarded route did not complete")
    pages = [first]
    resume_frames = [at + 1 for at in gates]
    for point, resumed, next_gate in zip(targets, resume_frames, [*gates[1:], None]):
        after = next((page for line in host if (page := page_at(
            line, profile, point, events, reads, resumed)) and page.frame > resumed
            and (next_gate is None or page.frame == next_gate)), None)
        require(after is not None
                and (after.owner, after.context, after.load, after.loaded, after.region, after.receiver)
                == (first.owner, first.context, first.load, first.loaded, first.region, first.receiver)
                and after.epoch > pages[-1].epoch,
                f'{point["group"]}/{point["wait"]}: successor has missing or stale body, task, CST or release proof')
        pages.append(after)
    verify_task_lifetime(events, first, after.frame)
    require(not any((wait := WAIT.fullmatch(line)) and int(wait[1]) > after.frame for line in host),
            "route did not stop at the successor")
    receipts = [match.groups() for line in host for match in RECEIPT.finditer(line)
                if int(match[1]) > gate and int(match[5]) and match[9] == "0"]
    expected = [(str(at), "2", "0", "1", "1", "false", str(first.region), str(first.receiver), "0", ENTER)
                for at in resume_frames]
    require(receipts == expected, "physical Enter did not have a unique consuming receiver")
    resumes = [match.groups() for line in events if (match := RESUME.fullmatch(line)) and int(match[1]) > gate]
    require(resumes == [(str(at), "message=[2, 0, 0, 0]") for at in resume_frames]
            and all(f'frame={at} input-wait native 71 address={profile["root_wait"]}' in events
                    for at in resume_frames),
            "Enter consumption lacks matching same-frame VM wait/resume")
    receiver_receipts = [{"frame": at, "region": first.region, "receiver": first.receiver}
                         for at in resume_frames]
    movie_pages = pages[:-1] if radio_stop or radio_r94 else pages
    movie = movie_evidence(sample, host, events, first, movie_pages[-1])
    if radio_stop:
        movie["stop_calls"] = verify_radio_stop(host, events, pages[-2], after, movie)
    radio = radio_after_r94(host, events, pages[-3], pages[-2], after, movie) if radio_r94 else None
    if continuation:
        releases = [tuple(map(int, match.groups())) for line in host if (match := re.fullmatch(
            r"frame (\d+): 11/13 Enter released after frame (\d+)", line
        ))]
        require(releases == [(page.frame, gate) for page, gate in zip(pages[1:], gates)]
                and all(frame > gate + 3 for frame, gate in releases),
                "missing scanned release between adjacent body waits")
        speaker_queues = []
        if sample == "11":
            require({page.frame for page in movie_pages} <= {at for at, _ in movie["composited_pts"]},
                    "radio_wave composition is missing at an intermediate wait")
            speaker_queues = [verify_august_speaker_queue(events, asdict(first), asdict(after), offset)
                              for first, after, offset in zip(pages, pages[1:], (9, 12))]
        waits = [verify_eden_rdraw(events, before, successor, point["rdraw"])
                 for point, before, successor in zip(targets, pages, pages[1:]) if "rdraw" in point]
        return {"status": radio["status"] if radio else "continuation_verified",
                "pages": [asdict(page) for page in pages], "script_waits": waits, "radio": radio,
                "receiver_receipts": receiver_receipts, "movie": movie, "speaker_queues": speaker_queues}
    return {"status": "successor_verified", "first": asdict(first), "successor": asdict(after),
            "receiver_receipt": receiver_receipts[0], "movie": movie}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sample", choices=("11", "13"))
    parser.add_argument("run", type=Path, help="batch directory with build_identity.json and sample summary")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--observe-only", action="store_true", help="validate the first wait with no body input")
    mode.add_argument("--continuation", action="store_true", help="validate the route to 11 2/90 or 13 11/121")
    mode.add_argument("--radio-stop", action="store_true", help="validate the 11 route through radio_wave stop at 3/94")
    mode.add_argument("--radio-blocker", action="store_true", help="identify the 11 r94 stop-command blocker")
    mode.add_argument("--eden-blocker", action="store_true", help="identify the 13 r103 layout-completion blocker")
    mode.add_argument("--eden-r139", action="store_true", help="validate the 13 route through 12/139")
    mode.add_argument("--eden-r188", action="store_true", help="validate the 13 route through 17/188")
    mode.add_argument("--radio-r94-enter", action="store_true", help="validate the fourth 11 Enter and stop evidence")
    parser.add_argument("--require-august-speaker-copy", action="store_true",
                        help="also require the 11 speaker copy, publication and FES queue after Enter")
    args = parser.parse_args()
    if (args.eden_blocker or args.eden_r139 or args.eden_r188) and args.sample != "13":
        parser.error("Eden modes require sample 13")
    if (args.radio_stop or args.radio_blocker or args.radio_r94_enter) and args.sample != "11":
        parser.error("radio modes require sample 11")
    if args.require_august_speaker_copy and (args.sample != "11" or args.observe_only or args.continuation
                                             or args.radio_stop or args.radio_blocker or args.radio_r94_enter):
        parser.error("--require-august-speaker-copy requires the single-Enter 11 route")
    try:
        profile = json_file(CONFIG)["acceptance"][args.sample]
        identity, host, events = read_run(args.run, args.sample)
        continuation = (json_file(CONTINUATION)["after_first_successor"][args.sample]
                        if (args.continuation or args.radio_stop or args.radio_r94_enter
                            or args.eden_r139 or args.eden_r188) else [])
        if args.radio_stop:
            continuation.append({"group": 3, "wait": 94, "body": [93], "radio_stopped": True})
        if args.radio_r94_enter:
            continuation.extend([
                {"group": 3, "wait": 94, "body": [93], "radio_binding": True},
                {"group": 4, "wait": 97, "body": [96], "radio_binding": True, "radio_stopped": True},
            ])
        if args.eden_r139 or args.eden_r188:
            continuation.append(json_file(NEXT)["eden_r139"])
        if args.eden_r188:
            continuation.extend(json_file(NEXT_WAITS)["eden_r188"])
        receipt = (verify_eden_blocker(profile, host, events) if args.eden_blocker
                   else verify_radio_blocker(profile, host, events) if args.radio_blocker
                   else verify(args.sample, profile, host, events, args.observe_only, continuation,
                               args.radio_stop, args.radio_r94_enter))
        if args.require_august_speaker_copy:
            receipt["speaker_queue"] = verify_august_speaker_queue(events, receipt["first"], receipt["successor"])
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(json.dumps({"sample": args.sample, "status": "unverified", "reason": str(error)}))
        return 1
    print(json.dumps({"sample": args.sample, "build": identity, **receipt}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
