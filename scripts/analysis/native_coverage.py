"""Reconcile immediate native sightings with actual Rust dispatch, read-only.

Run: python -B scripts/analysis/native_coverage.py
Reports and deduplicated source-evidence blobs are written. No Rust compiler, original executable,
game VM, network, or third-party implementation code is executed/imported.
The Rust reader is a balanced-delimiter source inspector, not a Rust compiler.
Source paths/line numbers, hashes, guards and helper evidence remain inspectable.
"""

from __future__ import annotations

import argparse
import ast
import csv
import hashlib
import io
import json
import re
import struct
import sys
from array import array
from collections import Counter, defaultdict
from dataclasses import dataclass
from datetime import datetime, timezone
from functools import lru_cache
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
DESKTOP = "simulator/crates/desktop/src/lib.rs"
DATA_SERVICES = "simulator/crates/desktop/src/data_services.rs"
ORDINARY_SERVICES = "simulator/crates/desktop/src/ordinary_native.rs"
LANGUAGE_SERVICES = "simulator/crates/desktop/src/language_native.rs"
MESSAGE_SERVICES = "simulator/crates/desktop/src/message_native.rs"
AUDIT = "reports/all_sample_content_audit.json"
LEDGER = "reports/runtime_function_ledger_all.json"
REF = "resources/reference/KatzeSystem2/docs/"
REPORT = "reports/native_coverage_20261116"
VERSION_CHANGED = set(range(906, 934)) | {
    965, 966, 1025, 1027, 1028, 1029, 1030, 1047, 1048
}

def archive_source_blobs(root: Path, blobs: dict[str, bytes], hashes: dict[str, str]) -> dict[str, str]:
    """Retain the actual bytes inspected while agents keep editing live files.

    Content identity deduplicates unchanged inputs across rolling batches.
    This is evidence storage, not a current-checkout or completion claim.
    """
    folder = root / "archive/2026-11/source_blobs"
    folder.mkdir(parents=True, exist_ok=True)
    archived = {}
    for name, data in blobs.items():
        path = folder / f"20261117_{hashes[name]}_{Path(name).name}"
        if path.exists():
            if hashlib.sha256(path.read_bytes()).hexdigest() != hashes[name]:
                raise ValueError(f"source evidence blob is corrupt: {path.name}")
        else:
            path.write_bytes(data)
        archived[name] = path.relative_to(root).as_posix()
    return archived


def rust_mask(text: str) -> str:
    """Blank comments/literals, preserving offsets and newlines, including r#""#."""
    result = list(text)
    pos = 0
    while pos < len(text):
        end = pos
        if text.startswith("//", pos):
            end = text.find("\n", pos)
            if end < 0:
                end = len(text)
        elif text.startswith("/*", pos):
            depth, end = 1, pos + 2
            while end < len(text) and depth:
                if text.startswith("/*", end):
                    depth, end = depth + 1, end + 2
                elif text.startswith("*/", end):
                    depth, end = depth - 1, end + 2
                else:
                    end += 1
            if depth:
                raise ValueError("unterminated Rust block comment")
        else:
            raw = (re.match(r'(?:br|r)(#{0,255})"', text[pos:pos + 260])
                   if text[pos] in "br" else None)
            if raw:
                finish = '"' + raw.group(1)
                close = text.find(finish, pos + raw.end())
                if close < 0:
                    raise ValueError("unterminated Rust raw string")
                end = close + len(finish)
            elif text[pos] == '"':
                end = pos + 1
                while end < len(text):
                    if text[end] == "\\":
                        end += 2
                    elif text[end] == '"':
                        end += 1
                        break
                    else:
                        end += 1
            elif text[pos] == "'":
                char = re.match(r"'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'", text[pos:])
                if char:
                    end = pos + char.end()
        if end > pos:
            for index in range(pos, end):
                if result[index] != "\n":
                    result[index] = " "
            pos = end
        else:
            pos += 1
    return "".join(result)


def matching(mask: str, start: int) -> int:
    pairs = {"(": ")", "[": "]", "{": "}"}
    stack = [pairs[mask[start]]]
    pos = start + 1
    while pos < len(mask):
        char = mask[pos]
        if char in pairs:
            stack.append(pairs[char])
        elif char in ")]}":
            if char != stack.pop():
                raise ValueError(f"unbalanced Rust delimiter at {pos}")
            if not stack:
                return pos
        pos += 1
    raise ValueError(f"unclosed Rust delimiter at {start}")


def numbers(pattern: str) -> set[int] | None:
    pattern = pattern.strip()
    if pattern == "_":
        return None
    values: set[int] = set()
    for part in pattern.split("|"):
        part = part.strip()
        found = re.fullmatch(r"(\d+)\s*\.\.=\s*(\d+)", part)
        if found:
            values.update(range(int(found[1]), int(found[2]) + 1))
        elif part.isdecimal():
            values.add(int(part))
        else:
            raise ValueError(f"unsupported numeric Rust pattern: {pattern!r}")
    return values


@dataclass
class Arm:
    pattern: str
    guard: str
    values: set[int] | None
    body: str
    line: int
    body_line: int
    source: str = DESKTOP

    def evidence(self) -> dict:
        return {
            "source": self.source,
            "line": self.line,
            "body_line": self.body_line,
            "pattern": self.pattern,
            "guard": self.guard or None,
            "return_variants": sorted(set(re.findall(r"NativeReturn::(\w+)", self.body))),
        }


class RustSource:
    def __init__(self, text: str, path: str = DESKTOP, owner: str | None = None,
                 require_dispatch: bool = True, call_function: str = "call"):
        self.text = text
        self.path = path
        self.mask = rust_mask(text)
        self.functions: dict[str, tuple[int, int]] = {}
        self.batch_contracts: dict[int, dict] = {}
        self.call_function = call_function
        for found in re.finditer(r"\bfn\s+([a-zA-Z_]\w*)\s*\(", self.mask):
            arg_start = self.mask.index("(", found.start())
            arg_end = matching(self.mask, arg_start)
            brace = self.mask.find("{", arg_end)
            semicolon = self.mask.find(";", arg_end)
            if brace < 0 or (0 <= semicolon < brace):
                continue
            self.functions[found[1]] = (brace, matching(self.mask, brace))
        if not require_dispatch:
            self.arms = []
            return
        impl = (self.mask.index("impl Host for LoggingHost" if owner is None else f"impl {owner}")
                if call_function == "call" else 0)
        call = re.search(r"\bfn\s+" + re.escape(call_function) + r"\s*\(", self.mask[impl:])
        if not call:
            raise ValueError(f"{path}: native call method not found")
        start = impl + call.start()
        self.call_brace = self.mask.index("{", start)
        self.call_end = matching(self.mask, self.call_brace)
        matches = list(re.finditer(r"\bmatch\s+index\s*\{", self.mask[self.call_brace:self.call_end]))
        self.wrapper_forwarding = None
        if not matches and call_function != "call":
            helper_name = call_function + "_inner"
            forwarding = re.search(
                r"\bself\." + re.escape(helper_name) + r"\s*\(\s*index\s*,\s*args\s*\)",
                self.mask[self.call_brace:self.call_end],
            )
            if forwarding and helper_name in self.functions:
                self.wrapper_forwarding = {
                    "function": call_function, "line": self.line(self.call_brace),
                    "receiver": helper_name, "source": path,
                }
                self.call_brace, self.call_end = self.functions[helper_name]
                matches = list(re.finditer(r"\bmatch\s+index\s*\{", self.mask[self.call_brace:self.call_end]))
        # Message adapters first validate argument counts, then dispatch the
        # actual object operation. Count that semantic dispatch, not the size
        # table. Other modules retain the outer numeric dispatcher.
        found = matches[1] if owner in {
            "MessageNativeServices", "LegacyTextNativeServices",
        } and len(matches) > 1 else (matches[0] if matches else None)
        if not found:
            # A descriptor batch maps ordinary slot -> DWORD prefix -> object
            # command. Require the actual table lookup and receiver match;
            # merely listing numbers is never implementation evidence.
            table = re.search(
                r"let\s*&\s*\(\s*_\s*,\s*words\s*,\s*command\s*\)\s*=\s*(\w+)"
                r"\s*\.iter\s*\(\s*\)\s*\.find\s*\(\s*\|\s*spec\s*\|\s*spec\.0\s*==\s*index\s*\)",
                self.mask[self.call_brace:self.call_end],
            )
            receiver = re.search(r"\bmatch\s+command\s*\{", self.mask[self.call_brace:self.call_end])
            if not table or not receiver:
                raise ValueError(f"{path}: native dispatch/descriptor receiver not found")
            table_name = table[1]
            declaration = re.search(
                r"\bconst\s+" + re.escape(table_name) + r"\s*:[^=]+=\s*&\s*\[", self.mask
            )
            if not declaration:
                raise ValueError(f"{path}: descriptor table {table_name} is not inspectable")
            opening = declaration.end() - 1
            closing = matching(self.mask, opening)
            payload = self.mask[opening + 1:closing]
            tuples = list(re.finditer(r"\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*\)", payload))
            residue = re.sub(r"\(\s*\d+\s*,\s*\d+\s*,\s*\d+\s*\)", "", payload)
            if residue.replace(",", "").strip() or not tuples:
                raise ValueError(f"{path}: descriptor table contains unsupported expressions")
            self.call_match = self.call_brace + receiver.end() - 1
            receiver_arms = [
                arm
                for match in re.finditer(
                    r"\bmatch\s+command\s*\{", self.mask[self.call_brace:self.call_end]
                )
                for arm in self.match_arms(self.call_brace + match.end() - 1)
            ]
            self.arms = []
            for item in tuples:
                index, words, command = map(int, item.groups())
                receiver_arm = next((arm for arm in reversed(receiver_arms)
                                     if arm.values and command in arm.values), None)
                if receiver_arm is None and owner == "LegacyTextNativeServices" and command == 102:
                    body = self.text[self.call_brace:self.call_end + 1]
                    body_mask = rust_mask(body)
                    packaged = re.search(
                        r"\blet\s+configuration\s*=\s*if\s+command\s*==\s*102\s*\{",
                        body_mask,
                    )
                    accepted = re.search(
                        r"\bif\s+let\s+Some\s*\(\s*configuration\s*\)\s*=\s*configuration\s*\{",
                        body_mask,
                    )
                    accepted_body = ""
                    if accepted:
                        opening = accepted.end() - 1
                        accepted_body = body_mask[opening:matching(body_mask, opening) + 1]
                    direct_old = index == 123 and all(token in body_mask for token in (
                        "if index == 123", "LegacyTextObject::new", "self.objects.insert",
                    ))
                    actual_configuration = packaged and all(token in accepted_body for token in (
                        "LegacyTextObject::new", "self.objects.insert",
                        "NativeReturn::Continue",
                    ))
                    if direct_old or actual_configuration:
                        receiver_arm = Arm(
                            "102", "", {102}, body, self.line(self.call_brace),
                            self.line(self.call_brace), path,
                        )
                if receiver_arm is None:
                    raise ValueError(f"{path}: native {index} has no explicit command {command} receiver")
                self.batch_contracts[index] = {
                    "table": table_name, "words": words, "command": command,
                    "table_line": self.line(opening + 1 + item.start()),
                }
                self.arms.append(Arm(
                    str(index), "", {index}, receiver_arm.body,
                    receiver_arm.line, receiver_arm.body_line, path,
                ))
            return
        self.call_match = self.call_brace + found.end() - 1
        self.arms = self.match_arms(self.call_match)
        if owner == "MessageNativeServices":
            delegated = next((arm for arm in self.arms if arm.values is None), None)
            if delegated and re.search(r"\bobject\.text_command\s*\(", rust_mask(delegated.body)):
                mapping = re.search(
                    r"let\s+command\s*=\s*match\s+index\s*\{",
                    self.mask[self.call_match:self.call_end],
                )
                if not mapping:
                    raise ValueError(f"{path}: message receiver command mapping is absent")
                for arm in self.match_arms(self.call_match + mapping.end() - 1):
                    if arm.values:
                        expression = re.sub(r"\s+", "", arm.body)
                        if not (expression.isdecimal() or re.fullmatch(r"index-\d+", expression)):
                            raise ValueError(f"{path}: message command mapping changed")
                        for index in sorted(arm.values):
                            self.arms.append(Arm(
                                str(index), "", {index}, delegated.body,
                                arm.line, delegated.body_line, path,
                            ))
                # Numeric synthetic branches must be considered before the
                # receiver's original catch-all when checking argument lengths.
                self.arms = [arm for arm in self.arms if arm.values is not None] + [
                    arm for arm in self.arms if arm.values is None
                ]
        if owner == "NativeTaskStore" and self.call_function == "task_values_native":
            # This zero-argument operation runs before the shared-value scope
            # forwards to task_values_native_inner. Retain that real outer
            # branch instead of silently dropping it during wrapper peeling.
            outer_start, outer_end = self.functions["task_values_native"]
            outer = self.mask[outer_start:outer_end]
            step = re.search(r"\bif\s+index\s*==\s*1074\s*\{", outer)
            if step:
                opening = outer_start + step.end() - 1
                closing = matching(self.mask, opening)
                body = self.text[opening:closing + 1]
                if not all(token in rust_mask(body) for token in (
                    "ordinary_native_table.as_ref()", "table.task_clock_supports_single_step()",
                    "self.current_task_id",
                    "self.ensure_task_record", "record.clock.single_step = 1",
                    "task.clock.single_step = 1", "NativeReturn::Continue",
                )):
                    raise ValueError(f"{path}: one-step outer receiver changed")
                line = self.line(opening)
                self.arms.insert(0, Arm("1074", "", {1074}, body, line, line, path))
                self.batch_contracts[1074] = {
                    "table": "REGISTERED_TASK_SINGLE_STEP", "words": 0, "table_line": line,
                    "function": "task_values_native", "exact_words": False,
                }
            prefix = self.mask[self.call_brace:self.call_match]
            early = re.search(
                r"if\s+matches!\s*\(\s*index\s*,\s*609\s*\|\s*610\s*\)\s*\{\s*"
                r"return\s+self\.shared_numeric_backup_native\s*\(\s*index\s*\)",
                prefix,
            )
            if early:
                body, line = self.function("shared_numeric_backup_native")
                if not all(token in rust_mask(body) for token in (
                    "shared_area", ".backups.get_mut", "if index == 609",
                    "self.shared_values", "to_le_bytes", "from_le_bytes",
                )):
                    raise ValueError(f"{path}: shared numeric backup receiver changed")
                self.arms = [
                    Arm(str(index), "", {index}, body, line, line, path) for index in [609, 610]
                ] + self.arms

    def line(self, pos: int) -> int:
        return self.text.count("\n", 0, pos) + 1

    @lru_cache(maxsize=None)
    def function(self, name: str) -> tuple[str, int]:
        start, end = self.functions[name]
        return self.text[start:end + 1], self.line(start)

    def match_arms(self, brace: int) -> list[Arm]:
        end = matching(self.mask, brace)
        pos = brace + 1
        result = []
        while pos < end:
            while pos < end and (self.mask[pos].isspace() or self.mask[pos] == ","):
                pos += 1
            if pos >= end:
                break
            head = pos
            while not self.mask.startswith("=>", pos):
                if self.mask[pos] in "([{":
                    pos = matching(self.mask, pos) + 1
                else:
                    pos += 1
                if pos >= end:
                    raise ValueError("Rust match arm has no arrow")
            pattern = self.mask[head:pos].strip()
            parts = re.split(r"\s+if\s+", pattern, maxsplit=1)
            pattern, guard = parts[0], parts[1] if len(parts) == 2 else ""
            # Identifier bindings (including a guarded descriptor lookup)
            # are not numeric routes until their concrete table/receiver is
            # inspected below. Merely seeing such an arm adds no slots.
            values = None if re.fullmatch(r"[A-Za-z_]\w*", pattern) else numbers(pattern)
            pos += 2
            while self.mask[pos].isspace():
                pos += 1
            body = pos
            if self.mask[pos] == "{":
                pos = matching(self.mask, pos) + 1
            else:
                while pos < end and self.mask[pos] != ",":
                    if self.mask[pos] in "([{":
                        pos = matching(self.mask, pos) + 1
                    else:
                        pos += 1
            result.append(Arm(pattern, guard, values, self.text[body:pos],
                              self.line(head), self.line(body), self.path))
        return result

    def integer_matches(self, name: str) -> list[list[Arm]]:
        start, end = self.functions[name]
        return [
            self.match_arms(start + found.end() - 1)
            for found in re.finditer(r"\bmatch\s+index\s*\{", self.mask[start:end])
        ]


def external_dispatches(root: Path, source: RustSource, blobs: dict[str, bytes]) -> list[dict]:
    """Inspect pre-match forwarding guards, not just numeric match arms."""
    modules = {}
    support_pattern = r"(?:super::)?(\w+)::(\w+)::supports\s*\(\s*index\s*\)"

    def load_module(module_name: str, owner: str) -> RustSource:
        module_path = f"simulator/crates/desktop/src/{module_name}.rs"
        if module_path not in blobs:
            raise ValueError(f"uninspected external native module: {module_path}")
        key = (module_name, owner)
        if key not in modules:
            text = blobs[module_path].decode("utf-8-sig")
            if module_name == "native_tasks" and owner == "NativeTaskStore":
                child = "simulator/crates/desktop/src/task_commands.rs"
                if child not in blobs or not re.search(
                    r'#\s*\[\s*path\s*=\s*"task_commands\.rs"\s*\]\s*mod\s+task_commands\s*;', text
                ):
                    raise ValueError("task numeric area adapter has no declared child source")
                modules[key] = RustSource(
                    blobs[child].decode("utf-8-sig"), child, owner=owner,
                    call_function="task_values_native",
                )
            else:
                modules[key] = RustSource(text, module_path, owner=owner)
        return modules[key]

    def supported_indices(module: RustSource, active: frozenset = frozenset()) -> set[int]:
        if module.path in active:
            raise ValueError(f"cyclic supports forwarding: {module.path}")
        active = active | {module.path}
        start, stop = module.functions["supports"]
        indices = set()
        for found in re.finditer(r"matches!\s*\(\s*index\s*,", module.mask[start:stop]):
            opening = module.mask.index("(", start + found.start())
            closing = matching(module.mask, opening)
            pattern = module.mask[start + found.end():closing].strip().rstrip(",")
            indices.update(numbers(pattern) or set())
        for found in re.finditer(support_pattern, module.mask[start:stop]):
            indices.update(supported_indices(load_module(*found.group(1, 2)), active))
        for found in re.finditer(r"\b(\w+)\.contains\s*\(\s*&index\s*\)", module.mask[start:stop]):
            declaration = re.search(
                r"\bconst\s+" + re.escape(found.group(1)) + r"\s*:[^=]+=\s*&?\s*\[",
                module.mask,
            )
            if not declaration:
                raise ValueError(f"cannot inspect supports array {found.group(1)} in {module.path}")
            opening = declaration.end() - 1
            closing = matching(module.mask, opening)
            pattern = "|".join(part.strip() for part in module.mask[opening + 1:closing].split(",") if part.strip())
            indices.update(numbers(pattern) or set())
        for found in re.finditer(
            r"\b(\w+)\.iter\s*\(\s*\)\s*\.any\s*\(\s*\|\s*spec\s*\|\s*spec\.0\s*==\s*index\s*\)",
            module.mask[start:stop],
        ):
            contracts = {index for index, contract in module.batch_contracts.items()
                         if contract["table"] == found[1]}
            if not contracts:
                raise ValueError(f"{module.path}: supports table lacks an inspected receiver")
            indices.update(contracts)
        if module.path == MESSAGE_SERVICES:
            support_body = module.mask[start:stop]
            if "is_display_native(index)" in support_body:
                predicate, _ = module.function("is_display_native")
                match = re.search(r"matches!\s*\(\s*index\s*,([^)]*)\)", rust_mask(predicate))
                if not match:
                    raise ValueError("message display support predicate changed")
                indices.update(numbers(match[1]) or set())
            if "is_body_native(index)" in support_body:
                predicate, _ = module.function("is_body_native")
                if not re.search(r"BODY_NATIVES\.iter\s*\(\s*\)\.any", rust_mask(predicate)):
                    raise ValueError("message body support predicate changed")
                table = re.search(r"\bconst\s+BODY_NATIVES\s*:[^=]+=\s*&\s*\[", module.mask)
                if not table:
                    raise ValueError("message body descriptor is absent")
                opening = table.end() - 1
                closing = matching(module.mask, opening)
                entries = re.findall(r"\(\s*(\d+)\s*,\s*(\d+)\s*,\s*MessageEncoding::(Cp932|Unicode)\s*\)",
                                     module.mask[opening:closing])
                if not entries:
                    raise ValueError("message body descriptor has no recognized entries")
                indices.update(int(row[0]) for row in entries)
        return indices

    def walk(caller: RustSource, allowed: set[int] | None = None, chain: tuple = ()) -> list[dict]:
        prefix = caller.mask[caller.call_brace:caller.call_match]
        result = []
        for guard in re.finditer(r"if\s+" + support_pattern + r"\s*\{", prefix):
            module_name, owner = guard.group(1, 2)
            brace = caller.call_brace + guard.end() - 1
            end = matching(caller.mask, brace)
            if module_name == "native_tasks" and owner == "ObjectGraphReceiver":
                # This explicit host adapter serializes a portable subset. No
                # game creates/installs it, and it deliberately rejects the
                # original opaque state-tree format. Keep its evidence without
                # counting688/689 as implemented original runtime routes.
                path = "simulator/crates/desktop/src/object_graph.rs"
                module = RustSource(blobs[path].decode("utf-8-sig"), path, require_dispatch=False)
                support, support_line = module.function("supports")
                native, native_line = module.function("portable_object_graph_native")
                install, _ = module.function("install_portable_object_graph_receiver")
                forwarded = re.search(r"return\s+self\.portable_object_graph_native\s*\(\s*index\s*,",
                                      caller.mask[brace:end])
                predicate = re.search(r"matches!\s*\(\s*index\s*,([^)]*)\)", rust_mask(support))
                if not forwarded or not predicate or numbers(predicate[1]) != {688, 689} or not all(
                    token in rust_mask(native) for token in (".graph.receivers.contains", "NativeReturn::Retry")
                ) or ".graph.receivers.insert" not in rust_mask(install):
                    raise ValueError("portable graph adapter changed; review its real producer before classifying")
                result.append({
                    "module": module, "supported": set(), "optional_adapter": True, "descriptor_abis": {},
                    "evidence": {
                        "source": caller.path, "guard_line": caller.line(caller.call_brace + guard.start()),
                        "forwarding": "self.portable_object_graph_native -> explicit host-installed subset",
                        "module_source": path, "function": "portable_object_graph_native", "function_line": native_line,
                        "supports_line": support_line, "supported_indices": [], "supported_count": 0,
                        "excluded_supported_indices": [688, 689],
                        "status": "host-opt-in-only-not-original-state-tree-receiver",
                    },
                })
                continue
            if module_name == "native_tasks" and owner == "HgWindowNative":
                path = "simulator/crates/desktop/src/hg_window_native.rs"
                if path not in blobs or not re.search(
                    r'#\s*\[\s*path\s*=\s*"hg_window_native\.rs"\s*\]\s*mod\s+hg_window_native\s*;',
                    blobs["simulator/crates/desktop/src/native_tasks.rs"].decode("utf-8-sig"),
                ):
                    raise ValueError("HG-window native guard lacks its declared child source")
                module = RustSource(blobs[path].decode("utf-8-sig"), path, require_dispatch=False)
                support, support_line = module.function("supports")
                native, native_line = module.function("hg_window_native")
                compose, _ = module.function("compose")
                blend, _ = module.function("mix_tile")
                if not re.search(r"\bindex\s*==\s*648\b", rust_mask(support)) or not re.search(
                    r"return\s+self\.hg_window_native\s*\(\s*arguments\s*\)", caller.mask[brace:end]
                ) or not all(token in rust_mask(native) for token in (
                    ".ordinary_native_table.as_ref()", "table.hg_window_blend()",
                    "companion.hg_window_blend", ".ordinary.raw_image", "compose(&bytes, size, blend)",
                    "self.register_image_wrapper(image, true)",
                )) or not all(token in rust_mask(compose) for token in (
                    "HgFile::parse", "for id in 0..9", "frame.decode_pixels_rgba8", "mix_tile(",
                    "RgbaFrame::from_rgba8_with_metadata",
                )) or not all(token in rust_mask(blend) for token in (
                    "image_blit::seed_rgb", "image_blit::blend_pixel_integer",
                    "HgWindowBlend::SeedAdjacentRgb", "HgWindowBlend::SkipTransparent",
                )):
                    raise ValueError("HG-window native lost its real source/decode/blend/registration chain")
                body = native + compose + blend
                module.arms = [Arm("648", "", {648}, body, native_line, native_line, path)]
                module.call_function = "hg_window_native"
                module.wrapper_forwarding = None
                module.call_brace, module.call_end = module.functions["hg_window_native"]
                module.call_match = module.call_end
                result.append({
                    "module": module, "supported": {648}, "optional_adapter": True, "descriptor_abis": {},
                    "evidence": {
                        "source": caller.path, "guard_line": caller.line(caller.call_brace + guard.start()),
                        "forwarding": "self.hg_window_native -> raw HG -> nine-piece compose -> owned image wrapper",
                        "module_source": path, "function": "hg_window_native", "function_line": native_line,
                        "supports_line": support_line, "supported_indices": [648], "supported_count": 1,
                        "status": "concrete-HG-source-compose-registration-not-version-equivalence",
                    },
                })
                continue
            module = load_module(module_name, owner)
            if module.path in chain:
                raise ValueError(f"cyclic native call forwarding: {module.path}")
            forwarding = re.search(
                r"return\s+self\s*\.\s*(\w+(?:\s*\.\s*\w+)*)\s*\.\s*call\s*\(\s*index\s*,",
                caller.mask[brace:end],
            )
            forwarding_name = f"self.{forwarding.group(1)}.call" if forwarding else None
            if not forwarding and owner == "MessageNativeServices":
                helper = re.search(
                    r"return\s+self\.(call_message_native)\s*\(\s*index\s*,",
                    caller.mask[brace:end],
                )
                helper_source = "simulator/crates/desktop/src/message_platform.rs"
                if helper and helper_source in blobs and re.search(
                    r"\bdata\s*\.\s*messages\s*\.\s*call\s*\(\s*index\s*,",
                    rust_mask(blobs[helper_source].decode("utf-8-sig")),
                ):
                    forwarding_name = "self.call_message_native -> data.messages.call"
                    forwarding = helper
            if not forwarding and owner == "NativeTaskStore":
                helper = re.search(
                    r"return\s+self\.(task_values_native)\s*\(\s*index\s*,",
                    caller.mask[brace:end],
                )
                if helper and module.call_function == helper[1]:
                    forwarding_name = "self.task_values_native -> declared task_commands receiver"
                    forwarding = helper
            if not forwarding:
                raise ValueError(f"{owner} supports guard has no reviewed call forwarding")
            supported = supported_indices(module)
            if allowed is not None:
                supported &= allowed
            # The first pre-match guard of the child takes precedence over its
            # containing module's numeric match, just as Rust evaluates it.
            children = walk(module, supported, chain + (module.path,))
            result.extend(children)
            child_indices = set().union(*(item["supported"] for item in children)) if children else set()
            declared = set().union(*(arm.values or set() for arm in module.arms))
            start, stop = module.functions["supports"]
            result.append({
                "module": module,
                "supported": supported - child_indices,
                "evidence": {
                    "source": caller.path,
                    "guard_line": caller.line(caller.call_brace + guard.start()),
                    "forwarding": forwarding_name,
                    "module_source": module.path,
                    "supports_line": module.line(start),
                    "supports_pattern": module.text[start:stop + 1],
                    "supported_indices": sorted(supported - child_indices),
                    "supported_count": len(supported - child_indices),
                    "call_declared_indices": sorted(declared),
                    "supports_without_explicit_call_arm": sorted(supported - child_indices - declared),
                    "call_arms_not_exposed_by_supports": sorted(declared - supported),
                    "forwarding_chain": list(chain),
                    "status": "pre-match-guard-and-call-source-evidence",
                    "wrapper_forwarding": module.wrapper_forwarding,
                },
            })
        return result

    result = walk(source)
    prefix = source.mask[source.call_brace:source.call_match]
    for namespace, owner, filename, function, required_tokens in [
        ("native_tasks", "RenderTaskStore", "render_tasks", "render_tasks_native",
         ["render_task_command_bytes", "RenderTaskOperation::Query", "RenderTaskOperation::ConfigureWipe"]),
        ("canvas_native", "CanvasNativeState", "canvas_native", "canvas_native",
         ["ensure_canvas_state", "541", "542", "568"]),
        ("native_tasks", "ExportTaskStore", "export_tasks", "export_task_native",
         ["begin_export_task", "index == 691", ".complete"]),
        ("achievement_native", "AchievementState", "achievement_native", "achievement_native",
         ["has_achievement_family", "read_declarations", ".achievements.initialize", ".achievements.unlock",
          ".achievements.entries", ".achievements.reset", "self.vm_context_native"]),
        ("javascript_native", "JavaScriptStore", "javascript_native", "javascript_native",
         ["self.prepare_javascript_input", ".bind_javascript_context", "self.call_javascript_function",
          "self.load_javascript_task", ".save_services", ".clone_from(&read_u32(args, 4)?"]),
    ]:
        guard = re.search(r"\bif\s+" + re.escape(namespace + "::" + owner)
                          + r"::supports\s*\(\s*index\s*\)", prefix)
        if not guard:
            continue
        begin = source.call_brace + guard.start()
        opening = source.mask.find("{", source.call_brace + guard.end(), source.call_match)
        if opening < 0:
            raise ValueError(f"{owner}: conditional guard has no body")
        closing = matching(source.mask, opening)
        if not re.search(r"return\s+self\." + re.escape(function) + r"\s*\(\s*index\s*,",
                         source.mask[opening:closing]):
            raise ValueError(f"{owner}: conditional guard lacks its concrete forwarding")
        path = f"simulator/crates/desktop/src/{filename}.rs"
        module = RustSource(blobs[path].decode("utf-8-sig"), path, require_dispatch=False)
        support_start, support_stop = module.functions["supports"]
        supported = set()
        support_body = module.mask[support_start:support_stop + 1]
        for predicate in re.finditer(r"matches!\s*\(\s*index\s*,", support_body):
            predicate_open = module.mask.index("(", support_start + predicate.start())
            predicate_close = matching(module.mask, predicate_open)
            supported.update(numbers(module.mask[support_start + predicate.end():predicate_close].strip()) or set())
        body, line = module.function(function)
        body_mask = rust_mask(body)
        if not all(token in body_mask for token in required_tokens):
            raise ValueError(f"{owner}: concrete receiver work changed; review it before counting")
        if owner == "AchievementState":
            for receiver, tokens in [
                ("unlock", ("self.entries.get", "persist(root, &next)", "self.stored = next", ".unlocked = true")),
                ("reset", ("persist(root, &BTreeSet::new())", "self.stored.clear", "entry.unlocked = false")),
                ("initialize_local_achievements", ("has_achievement_family", "read_declarations", ".achievements.initialize")),
                ("persist", ("create_new(true)", "file.write_all", "file.sync_all", "replace(&temporary")),
            ]:
                receiver_body, _ = module.function(receiver)
                if not all(token in rust_mask(receiver_body) for token in tokens):
                    raise ValueError("achievement family lost its local producer/receiver/persistence path")
        if owner == "JavaScriptStore":
            for receiver, tokens in [
                ("load_javascript_task", ("JavaScriptTask::new", "record.engine", "engine.advance", "self.publish_javascript")),
                ("call_javascript_function", ("record.callback", "arguments", "engine.call_function", "self.publish_javascript")),
                ("prepare_javascript_input", ("self.create_task_object", "register_region", ".input_owners")),
            ]:
                receiver_body, _ = module.function(receiver)
                if not all(token in rust_mask(receiver_body) for token in tokens):
                    raise ValueError("JavaScript family lost its live engine/callback/context consumer")
        module.arms = [Arm(str(index), "", {index}, body, line, line, path) for index in supported]
        module.call_function = function
        module.wrapper_forwarding = None
        module.call_brace, module.call_end = module.functions[function]
        module.call_match = module.call_end
        result.insert(0, {
            "module": module, "supported": supported, "optional_adapter": True,
            "descriptor_abis": {},
            "evidence": {
                "source": source.path, "guard_line": source.line(begin),
                "forwarding": f"self.{function} -> concrete state owner",
                "module_source": path, "function": function, "function_line": line,
                "supported_indices": sorted(supported), "supported_count": len(supported),
                "guard_source": source.text[begin:opening],
                "status": "conditional-platform-receiver-source-not-version-equivalence",
                **({"behavior_limit": "Local simulator achievements only; Steam-account state/import/synchronization absent",
                    "registered_receiver_source": "simulator/crates/runtime/src/native_achievements.rs"}
                   if owner == "AchievementState" else {}),
                **({"behavior_limit": "Kind55 context/load/function consumers exist;1006 stores a save-service binding but full JS save consumption remains absent",
                    "conditional_on_task_kind": True, "incomplete_consumer_indices": [1006]}
                   if owner == "JavaScriptStore" else {}),
            },
        })
    # RenderTaskStore owns a second descriptor-backed dispatcher. Follow its
    # actual forwarding, command consumer and composition path; the parent's
    # direct matches! predicate covers only the older 520..532 family.
    render_parent = next((item for item in result
                          if item["module"].path == "simulator/crates/desktop/src/render_tasks.rs"), None)
    if render_parent:
        parent = render_parent["module"]
        support, _ = parent.function("supports")
        wrapper, _ = parent.function("render_tasks_native")
        composition, _ = parent.function("compose")
        child_forward = re.search(
            r"if\s+frame_effects::supports\s*\(\s*index\s*\)\s*\{\s*"
            r"return\s+self\.frame_effect_native\s*\(\s*index\s*,\s*args\s*\)",
            rust_mask(wrapper),
        )
        if "frame_effects::supports(index)" in rust_mask(support):
            if not child_forward or not re.search(
                r'#\[path\s*=\s*"frame_effects\.rs"\]\s*mod\s+frame_effects\s*;', parent.text
            ):
                raise ValueError("frame effect supports lost its declared child/actual forwarding")
            path = "simulator/crates/desktop/src/frame_effects.rs"
            module = RustSource(blobs[path].decode("utf-8-sig"), path, require_dispatch=False)
            predicate, _ = module.function("supports")
            native, native_line = module.function("frame_effect_native")
            receiver, receiver_line = module.function("effect_command")
            compose, compose_line = module.function("compose")
            raw_receiver, _ = module.function("receive_frame_effect_command")
            register, _ = module.function("register_frame_effect")
            for body, required in [
                (predicate, ("DESCRIPTORS.iter().any", "entry.slot == index")),
                (native, ("DESCRIPTORS", ".find", "entry.slot == index", "descriptor.input_words * 4",
                          "kind != descriptor.kind", "descriptor.operation", ".effect_command")),
                (receiver, ("match operation", "Operation::Prepare", "task.words = words",
                            "geometry.configure", "task.captures", "Operation::Play", "task.active = scalar",
                            "Operation::CaptureSecond", "Operation::Flag", "self.effects.tasks.insert")),
                (compose, ("geometry.draw", "pixels::composite")),
                (composition, ("self.effects.compose",)),
                (raw_receiver, (".effect_command", "propagate: descriptor.operation == Operation::Delegate")),
                (register, ("geometry::Geometry::new", "self.effects.tasks.insert")),
            ]:
                if not all(token in rust_mask(body) for token in required):
                    raise ValueError("frame effect descriptor lost its reviewed receiver/composition path")
            if not re.search(r"target\.pixels_mut\(\)\.copy_from_slice|\*target\s*=\s*frame\s*;", rust_mask(compose)):
                raise ValueError("frame effect composition lost its canvas commit")
            declaration = re.search(r"\bconst\s+DESCRIPTORS\s*:[^=]+=\s*&\s*\[", module.mask)
            if not declaration:
                raise ValueError("frame effect descriptor table is absent")
            opening = declaration.end() - 1
            closing = matching(module.mask, opening)
            payload = module.mask[opening + 1:closing]
            record_pattern = (
                r"Descriptor\s*\{\s*slot:\s*(\d+)\s*,\s*kind:\s*(\d+)\s*,"
                r"\s*operation:\s*Operation::(\w+)\s*,\s*input_words:\s*(\d+)\s*,"
                r"\s*record_words:\s*(\d+)\s*,\s*old_message:\s*(\d+)\s*,"
                r"\s*current_message:\s*(\d+)\s*,\s*\}"
            )
            records = list(re.finditer(record_pattern, payload))
            if not records or re.sub(record_pattern, "", payload).replace(",", "").strip():
                raise ValueError("frame effect descriptor has an unreviewed record layout")
            descriptor_abis = {}
            for entry in records:
                index, kind, operation, words, record_words, old, current = entry.groups()
                index, words = int(index), int(words)
                if index in descriptor_abis or f"Operation::{operation}" not in rust_mask(receiver):
                    raise ValueError("frame effect descriptor has no distinct concrete operation")
                descriptor_abis[index] = {
                    "minimum_argument_bytes": words * 4,
                    "receiver_record_bytes": int(record_words) * 4,
                    "task_kind": int(kind), "operation": operation,
                    "old_message": int(old), "current_message": int(current),
                    "descriptor_line": module.line(opening + 1 + entry.start()),
                    "receiver_line": receiver_line, "composition_line": compose_line,
                    "semantic_equivalence": "not-verified",
                }
                module.arms.append(Arm(str(index), "", {index}, native + receiver + compose,
                                       native_line, native_line, path))
            module.call_function = "frame_effect_native"
            module.call_brace, module.call_end = module.functions[module.call_function]
            module.call_match = module.call_end
            module.wrapper_forwarding = None
            result.insert(0, {
                "module": module, "supported": set(descriptor_abis), "optional_adapter": True,
                "descriptor_abis": descriptor_abis,
                "evidence": {
                    "source": source.path, "guard_line": render_parent["evidence"]["guard_line"],
                    "forwarding": "self.render_tasks_native -> self.frame_effect_native -> effect_command -> compose",
                    "module_source": path, "function": module.call_function, "function_line": native_line,
                    "supported_indices": sorted(descriptor_abis), "supported_count": len(descriptor_abis),
                    "parent_source": parent.path,
                    "status": "descriptor-and-concrete-effect-receiver-source-not-version-equivalence",
                },
            })
    for guard in re.finditer(
        r"\bif\s+matches!\s*\(\s*index\s*,\s*([0-9.\s|]+)\)\s*\{\s*"
        r"return\s+self\.(\w+_native)\s*\(\s*index\s*,\s*arguments\s*,\s*vm\s*\)",
        prefix,
    ):
        supported = numbers(guard[1]) or set()
        function = guard[2]
        found_module = None
        for path, raw in blobs.items():
            if not path.startswith("simulator/crates/desktop/src/") or path == DESKTOP:
                continue
            text = raw.decode("utf-8-sig")
            if re.search(r"\bfn\s+" + re.escape(function) + r"\s*\(", rust_mask(text)):
                found_module = RustSource(text, path, call_function=function)
                break
        if found_module is None:
            raise ValueError(f"numeric pre-match guard has no receiver function: {function}")
        declared = set().union(*(arm.values or set() for arm in found_module.arms))
        if supported - declared:
            raise ValueError(f"numeric pre-match guard has unsupported receiver slots: {sorted(supported - declared)}")
        result.insert(0, {
            "module": found_module, "supported": supported, "optional_adapter": True,
            "descriptor_abis": {},
            "evidence": {
                "source": source.path, "guard_line": source.line(source.call_brace + guard.start()),
                "forwarding": f"self.{function} -> explicit numeric receiver",
                "module_source": found_module.path, "function": function,
                "function_line": found_module.line(found_module.call_brace),
                "supported_indices": sorted(supported), "supported_count": len(supported),
                "status": "numeric-pre-match-guard-and-concrete-receiver-source-evidence",
            },
        })
    # The message host performs concrete input operations before forwarding
    # the remaining slots into MessageNativeServices::call. Inspect that real
    # receiver, so a supports declaration alone cannot mask either behavior
    # or an absent implementation in the forwarded module.
    message_parent = next((item for item in result if item["module"].path == MESSAGE_SERVICES), None)
    if message_parent and message_parent["evidence"]["forwarding"].startswith("self.call_message_native"):
        path = "simulator/crates/desktop/src/message_platform.rs"
        module = RustSource(blobs[path].decode("utf-8-sig"), path,
                            call_function="call_message_native")
        start, stop = module.functions["call_message_native"]
        function_mask = module.mask[start:stop + 1]
        for table, helper_name, receiver_name, entry_pattern in [
            ("BUTTON_NATIVES", "input_button_native", "receive_input_button_command",
             r"\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*,\s*(true|false)\s*,\s*(true|false)\s*\)"),
            ("INPUT_REGION_NATIVES", "input_region_native", "receive_input_region_command",
             r"\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*\)"),
            ("SOUND_PREPARE_NATIVES", "legacy_sound_prepare_native", "legacy_sound_prepare_native",
             r"\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(true|false)\s*,\s*(\d+)\s*\)"),
        ]:
            descriptor = next((arm for arm in module.arms if arm.values is None
                and re.search(re.escape(table) + r"\.iter\s*\(\s*\)\s*\.any", arm.guard)
                and f"self.{helper_name}" in rust_mask(arm.body)), None)
            if not descriptor:
                continue
            declaration = re.search(r"\bconst\s+" + re.escape(table) + r"\s*:[^=]+=\s*&\s*\[", module.mask)
            if not declaration:
                raise ValueError(f"{table} native guard has no descriptor table")
            table_start = declaration.end() - 1
            table_end = matching(module.mask, table_start)
            entries = list(re.finditer(entry_pattern, module.mask[table_start:table_end]))
            if not entries:
                raise ValueError(f"{table} descriptor table has no recognized entries")
            helper, _ = module.function(helper_name)
            receiver, _ = module.function(receiver_name)
            if table == "INPUT_REGION_NATIVES":
                # This descriptor reaches the real input task dispatcher.
                # Cursor143/150 are now separate from region127..136, and
                # owned continuations must remain part of the evidence.
                routed, _ = module.function("receive_device_input_buffers_with_budget")
                if not all(token in rust_mask(routed) for token in (
                    "match command.code", "self.receive_input_region_command",
                    "self.advance_input_cursor_command", "self.receive_input_cursor_command",
                )):
                    raise ValueError("input region descriptor lost its actual task dispatch")
                cursor, _ = module.function("advance_input_cursor_command")
                direct_cursor, _ = module.function("receive_input_cursor_command")
                receiver += routed + cursor + direct_cursor
            if table == "SOUND_PREPARE_NATIVES":
                helper_mask = rust_mask(helper)
                root_path = all(token in helper_mask for token in (
                    "code: 35", "self.default_sound_task", "NativeReturn::Retry",
                ))
                concrete_loader = all(token in helper_mask for token in (
                    "self.load_sound_native", "self.load_sound_group_native",
                )) or all(token in helper_mask for token in (
                    "self.dispatch_sound_task_command", "pending.record", "pending.sound",
                    "data1: pending.record.as_deref_mut", "NativeReturn::Value",
                ))
                if not root_path or not concrete_loader:
                    raise ValueError("SOUND_PREPARE descriptor lost its concrete root/loader path")
            if not all(token in rust_mask(helper) for token in (
                table, ".find", "route_task_command_buffers",
            )):
                raise ValueError(f"{table} descriptor does not reach its typed task dispatcher")
            module.arms.remove(descriptor)
            for entry in entries:
                index, words = map(int, entry.group(1, 2))
                command = int(entry.group(4 if table == "SOUND_PREPARE_NATIVES" else 3))
                receiver_mask = rust_mask(receiver)
                literal = re.search(r"\b" + str(command) + r"\b", receiver_mask)
                in_range = any(int(start) <= command <= int(end) for start, end in
                               re.findall(r"\b(\d+)\s*\.\.=\s*(\d+)\b", receiver_mask))
                if not literal and not in_range:
                    raise ValueError(f"{table} descriptor command{command} has no inspected receiver")
                module.batch_contracts[index] = {
                    "table": table, "words": words, "command": command,
                    "table_line": module.line(table_start + entry.start()),
                }
                module.arms.append(Arm(str(index), "", {index}, helper + receiver,
                    descriptor.line, descriptor.body_line, path))
        for found in re.finditer(r"\bif\s+index\s*==\s*(\d+)\s*\{", function_mask):
            index = int(found[1])
            if index not in {69, 70, 92, 285}:
                continue
            opening = start + found.end() - 1
            closing = matching(module.mask, opening)
            body = module.text[opening:closing + 1]
            body_mask = rust_mask(body)
            if index == 285:
                if "self.sample_device_key" not in body_mask or "NativeReturn::Value" not in body_mask:
                    raise ValueError("input285 route lacks its actual device query")
            elif index in {69, 92}:
                helper_name = "message_raw_display_native" if index == 69 else "message_visibility_native"
                if f"return self.{helper_name}(arguments, vm)" not in body_mask:
                    raise ValueError(f"message{index} route lost its display receiver")
                helper, _ = module.function(helper_name)
                if "self.forward_message_plane_display" not in rust_mask(helper):
                    raise ValueError(f"message{index} receiver lost its real display mutation")
                body += helper
            else:
                normalized = re.sub(r"\bself\s*\.\s*", "self.", body_mask)
                if not all(token in normalized for token in (
                    "self.kx_raw_native", "self.input_graphic_raw_native",
                    "self.dispatch_buffered_text_message", "self.task_message_native",
                    "NativeReturn::Value",
                )):
                    raise ValueError("message70 route lost a typed receiver or task fallback")
            module.arms.append(Arm(str(index), "", {index}, body,
                                   module.line(opening), module.line(opening), path))
        if re.search(r"if\s+super::message_native::is_body_native\s*\(\s*index\s*\)", function_mask):
            direct = re.search(r"return\s+super::message_native::call_message_body_native\s*\(\s*index\s*,", function_mask)
            traced = re.search(r"let\s+result\s*=\s*super::message_native::call_message_body_native\s*\(\s*index\s*,[^;]+\?;.+?return\s+Ok\(result\);", function_mask, re.S)
            if not (direct or traced):
                raise ValueError("message body predicate has no concrete wrapper forwarding")
            body_module = message_parent["module"]
            helper, helper_line = body_module.function("call_message_body_native")
            if not all(token in rust_mask(helper) for token in (
                "BODY_NATIVES", ".find", "words * 4", "expand_kcs_format",
                "extract_message_body", "vm.write", "NativeReturn::Value",
            )):
                raise ValueError("message body descriptor lost its actual VM receiver")
            table = re.search(r"\bconst\s+BODY_NATIVES\s*:[^=]+=\s*&\s*\[", body_module.mask)
            opening = table.end() - 1
            closing = matching(body_module.mask, opening)
            entries = re.findall(r"\(\s*(\d+)\s*,\s*(\d+)\s*,\s*MessageEncoding::(Cp932|Unicode)\s*\)",
                                 body_module.mask[opening:closing])
            for index, words, encoding in entries:
                index = int(index)
                body_module.batch_contracts[index] = {
                    "table": "BODY_NATIVES", "words": int(words), "encoding": encoding,
                    "table_line": body_module.line(opening),
                    "function": "call_message_body_native", "exact_words": True,
                }
                body_module.arms.append(Arm(str(index), "", {index}, helper,
                    helper_line, helper_line, body_module.path))
        expected = {69: 16, 70: 16, 92: 8, 150: 12, 285: 4, 403: 8, 404: 0, 937: 0}
        declared = set().union(*(arm.values or set() for arm in module.arms))
        supported = declared & message_parent["supported"]
        if supported:
            result.insert(0, {
                "module": module, "supported": supported, "optional_adapter": True,
                "descriptor_abis": {index: {
                    "exact_argument_bytes": expected.get(index), "source": path,
                    "function": "call_message_native", "numeric_receiver_present": True,
                } for index in supported},
                "evidence": {
                    "source": source.path, "guard_line": message_parent["evidence"]["guard_line"],
                    "forwarding": "self.call_message_native -> concrete input receiver",
                    "module_source": path, "function": "call_message_native",
                    "supported_indices": sorted(supported),
                    "supported_count": len(supported),
                    "function_line": module.line(start),
                    "status": "pre-forwarding-host-receiver-source-evidence",
                },
            })
    # Optional adapters are callable routes only where an inspected predicate
    # owns the numeric entry. The movie adapter additionally intercepts many
    # existing entries for particular task kinds; those partial overrides do
    # not replace the general handler in this accounting.
    for filename, function, owner, full_entries in [
        ("movie_plane_native", "movie_plane_native", None, {701, 702, 988, 989}),
        ("plane_native", "plane_native", "PlaneServices", None),
    ]:
        forwarding = re.search(
            r"if\s+let\s+Some\s*\(\s*result\s*\)\s*=\s*self\."
            + re.escape(function)
            + r"\s*\(\s*index\s*,\s*arguments\s*,\s*vm\s*\)\s*\?\s*\{\s*return\s+Ok\s*\(\s*result\s*\)",
            prefix,
        )
        if not forwarding:
            continue
        path = f"simulator/crates/desktop/src/{filename}.rs"
        module = RustSource(blobs[path].decode("utf-8-sig"), path, require_dispatch=False)
        start, stop = module.functions[function]
        function_mask = module.mask[start:stop + 1]
        descriptor_abis = {}
        descriptor_arms = {}
        if owner:
            support_start, support_stop = module.functions["supports"]
            support_body = module.mask[support_start:support_stop + 1]
            predicate = re.search(r"matches!\s*\(\s*index\s*,", support_body)
            if not predicate:
                raise ValueError(f"{path}: optional supports predicate changed")
            opening = support_start + predicate.end() - 1
            opening = module.mask.rfind("(", support_start, opening + 1)
            closing = matching(module.mask, opening)
            supported = numbers(module.mask[support_start + predicate.end():closing].strip()) or set()
            if not re.search(
                r"if\s*!\s*" + re.escape(owner) + r"::supports\s*\(\s*index\s*\)\s*\{\s*return\s+Ok\s*\(\s*None\s*\)",
                function_mask,
            ):
                raise ValueError(f"{path}: optional adapter no longer gates on supports")
            for lookup in re.finditer(
                r"(\w+)\.iter\s*\(\s*\)\s*\.any\s*\(\s*\|\s*abi\s*\|\s*abi\.slot\s*==\s*index\s*\)",
                support_body,
            ):
                name = lookup[1]
                declaration = re.search(
                    r"\bconst\s+" + re.escape(name) + r"\s*:[^=]+=\s*&\s*\[", module.mask
                )
                if not declaration or not re.search(
                    re.escape(name) + r"\.iter\s*\(\s*\)\s*\.find\s*\(\s*\|\s*abi\s*\|\s*abi\.slot\s*==\s*index\s*\)",
                    function_mask,
                ):
                    raise ValueError(f"{path}: plane ABI table has no reviewed lookup")
                table_start = declaration.end() - 1
                table_end = matching(module.mask, table_start)
                payload = module.mask[table_start + 1:table_end]
                declarations = list(re.finditer(
                    r"abi\s*\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*,\s*(true|false)\s*,\s*(true|false)\s*\)",
                    payload,
                ))
                if not declarations:
                    raise ValueError(f"{path}: empty or unrecognized plane ABI descriptors")
                helper_start, helper_end = module.functions["plane_geometry_native"]
                helper_body = module.mask[helper_start:helper_end + 1]
                length_guard = re.search(r"args\.len\(\)\s*<\s*abi\.argument_bytes", helper_body)
                dispatch = re.search(r"\bmatch\s+abi\.slot\s*\{",
                                     helper_body[length_guard.end():]) if length_guard else None
                if not dispatch:
                    raise ValueError(f"{path}: plane ABI receiver/length validation changed")
                branches = module.match_arms(helper_start + length_guard.end() + dispatch.end() - 1)
                for declaration in declarations:
                    index, command, byte_count = map(int, declaration.group(1, 2, 3))
                    arm = next((arm for arm in branches if arm.values and index in arm.values), None)
                    if arm is None:
                        raise ValueError(f"{path}: plane ABI slot {index} lacks a receiver branch")
                    supported.add(index)
                    descriptor_arms[index] = arm
                    descriptor_abis[index] = {
                        "table": name, "command": command,
                        "minimum_argument_bytes": byte_count,
                        "zero_target_short_form": declaration[4] == "true",
                        "returns_value": declaration[5] == "true",
                        "table_line": module.line(table_start + 1 + declaration.start()),
                    }
        else:
            supported = set(full_entries or ())
            inspected = set()
            for match in re.finditer(r"if\s+matches!\s*\(\s*index\s*,", function_mask):
                opening = start + function_mask.index("(", match.start())
                closing = matching(module.mask, opening)
                brace = module.mask.index("{", closing)
                end = matching(module.mask, brace)
                entries = numbers(module.mask[start + match.end():closing].strip()) or set()
                if re.search(r"return\s+Ok\s*\(\s*Some\s*\(", module.mask[brace:end]):
                    inspected.update(entries)
            if not supported <= inspected:
                raise ValueError(f"{path}: complete movie entry predicate changed")
        if not re.search(r"Ok\s*\(\s*Some\s*\(\s*NativeReturn::", function_mask):
            raise ValueError(f"{path}: adapter lacks a real NativeReturn result")
        module.arms = []
        semantic_arms = [arm for group in module.integer_matches(function) for arm in group]
        for index in sorted(supported):
            selected = descriptor_arms.get(index) or next((
                arm for arm in reversed(semantic_arms) if arm.values and index in arm.values
            ), None)
            module.arms.append(selected or Arm(
                str(index), "", {index}, module.text[start:stop + 1],
                module.line(start), module.line(start), path,
            ))
        result.append({
            "module": module, "supported": supported, "optional_adapter": True,
            "descriptor_abis": descriptor_abis,
            "evidence": {
                "source": source.path,
                "guard_line": source.line(source.call_brace + forwarding.start()),
                "forwarding": f"self.{function} -> Some(NativeReturn)",
                "module_source": path, "function": function,
                "function_line": module.line(start),
                "supported_indices": sorted(supported), "supported_count": len(supported),
                "status": "optional-adapter-predicate-and-result-source-evidence",
                "conditional_on_task_kind": True,
                "descriptor_abis": descriptor_abis,
                "note": (
                    "None falls through to later routes. Movie/camera overrides of "
                    "other existing entries are not counted as complete general routes."
                ),
            },
        })
    result.extend(concrete_service_dispatches(source, blobs))
    return result


def concrete_service_dispatches(source: RustSource, blobs: dict[str, bytes]) -> list[dict]:
    """Account for reviewed owned services behind profile/constant guards.

    A descriptor or wrapper alone is insufficient: keep the resource, state,
    lifecycle and actual platform/pixel consumers in the evidence chain. These
    are partial source routes, never function-equivalence/completion claims.
    """
    prefix = source.mask[source.call_brace:source.call_match]
    result = []

    def load(path: str) -> RustSource:
        return RustSource(blobs[path].decode("utf-8-sig"), path, require_dispatch=False)

    def require(module: RustSource, function: str, tokens: tuple[str, ...]) -> tuple[str, int]:
        body, line = module.function(function)
        if not all(token in rust_mask(body) for token in tokens):
            raise ValueError(f"{module.path}::{function}: concrete service chain changed; review before counting")
        return body, line

    tasks = load("simulator/crates/desktop/src/native_tasks.rs")
    commands = load("simulator/crates/desktop/src/task_commands.rs")
    for filename, function, guard_pattern in [
        ("effekseer_native", "effekseer_native",
         r"\bif\s+effekseer_native::supports\s*\(\s*index\s*\)\s*&&\s*"
         r"effekseer_native::identified\s*\(\s*self\.data_services\.ordinary_native_table\.as_ref\(\)\s*\)"),
        ("tweet_native", "tweet_native", r"\bif\s+matches!\s*\(\s*index\s*,\s*1018\s*\.\.=\s*1023\s*\)"),
    ]:
        guard = re.search(guard_pattern, prefix)
        if not guard:
            continue
        begin = source.call_brace + guard.start()
        opening = source.mask.find("{", source.call_brace + guard.end(), source.call_match)
        closing = matching(source.mask, opening)
        forwarding = source.mask[opening:closing]
        if not re.search(r"return\s+self\." + function + r"\s*\(\s*index\s*,\s*arguments\s*,\s*vm\s*\)", forwarding):
            raise ValueError(f"{function}: service guard lost its actual forwarding")
        module = load(f"simulator/crates/desktop/src/{filename}.rs")
        inspected = [module.path, tasks.path, commands.path, source.path]
        descriptors = {}
        if filename == "effekseer_native":
            runtime = load("simulator/crates/runtime/src/effekseer.rs")
            format_source = load("simulator/crates/runtime/src/effekseer_format.rs")
            table = re.search(r"\bconst\s+NATIVE_COMMANDS\s*:[^=]+?=\s*\[([^;]+)\]\s*;", runtime.mask)
            if not table:
                raise ValueError("Effekseer has no inspected native descriptor table")
            rows = [tuple(map(int, row)) for row in re.findall(r"\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*\)", table[1])]
            if rows != [(1014, 578, 8), (1015, 579, 4), (1016, 580, 8)]:
                raise ValueError("Effekseer wrapper descriptors changed; review the receivers")
            native_table = load("simulator/crates/runtime/src/native_table.rs")
            require(module, "identified", ("table.is_some_and(OrdinaryNativeTable::has_effekseer_family)",))
            require(module, "supports", ("NATIVE_COMMANDS.iter().any", "d.0 == index"))
            body, line = require(module, function, (
                "NATIVE_COMMANDS", ".find", "args.len() < size", "identified", "OwnedTaskBuffers::new",
                "self.route_task_command_buffers", "NativeReturn::Continue"))
            require(module, "receive", ("578", "579", "580", ".begin_load", "read_resource", ".finish_load", ".unload", ".play"))
            resource, _ = require(module, "read_resource", ("read_category", "Effect::parse", "ImageReader::new", "image.decode", "Texture::new", "Resource::new"))
            if 'b"efk"' not in resource or 'b"efktex"' not in resource:
                raise ValueError("Effekseer lost its effect/texture resource categories")
            require(module, "draw_scene_task", ("self.sync", ".draw_task", "raster", "submitted_pixels"))
            require(module, "raster", ("q.resource", ".textures", "sample(texture", "canvas.pixels_mut", "blend(dest", "q.zwrite", "depth[pixel]"))
            require(runtime, "draw_task", ("self.advance_frame", "self.plays.values", "project", "quads.push"))
            parse_body, _ = require(format_source, "parse", ("r.paths", "textures"))
            if "r.node" in rust_mask(parse_body):
                require(format_source, "node", ("self.node", "Geometry::Sprite", "nodes[index]", "children"))
            elif "r.sprite" not in rust_mask(parse_body):
                raise ValueError("Effekseer lost its concrete node/geometry parser")
            for token in ("self.effekseer_register_task_hook(id)", "self.effekseer_update_task_hook(id)", "self.data_services.effekseer.manager.release(id)"):
                if token not in tasks.mask:
                    raise ValueError("Effekseer lost its actual task lifecycle hooks")
            if not all(token in source.mask for token in (
                "install_buffer_receiver(61, effekseer_native::receive)", "self.data_services.effekseer.draw_scene_task")):
                raise ValueError("Effekseer lost its owned receiver/final Canvas consumer")
            supported = {row[0] for row in rows}
            descriptors = {index: {"table": "NATIVE_COMMANDS", "command": command,
                                   "minimum_argument_bytes": size} for index, command, size in rows}
            inspected.extend([runtime.path, format_source.path, native_table.path])
            limitation = (
                "Requires the Effekseer family in the registered native table. "
                "The receiver implements the supported SKFE node and geometry subset."
            )
        else:
            guard_source = source.mask[begin:opening]
            if "feature_constant.is_none()" not in guard_source or not all(
                token in prefix for token in ("ordinary_native_table.as_ref()", "table.feature_constant(index)")
            ):
                raise ValueError("Tweet lost its registered-table constant precedence")
            body, line = require(module, function, ("args.len() != expected", "583 + index - 1018", "OwnedTaskBuffers::new", "self.route_task_command_buffers", "NativeReturn::Value(output)"))
            require(module, "has_registered_tweet_family", (
                "ordinary_native_table.as_ref()", "OrdinaryNativeTable::has_tweet_family",
            ))
            require(module, "register_tweet_task", ("self.has_registered_tweet_family()", "TweetTask", "AtomicU32::new(8)", "install_buffer_receiver(62, receiver)"))
            require(module, "receive_tweet_command", ("583", "584", "585", "586", "587", "588", "task.config", "task.images", "task.tags", "task.body", "task.status.load", "self.start_tweet_worker"))
            require(module, "start_tweet_worker", ("self.read_tweet_media", "Request", "status.store(0", ".spawn", "crate::tweet_worker::run", "task.worker = Some"))
            require(module, "update_tweet_task", (
                "JoinHandle::is_finished", "worker.join", "outcome.diagnostics", "self.events.push",
            ))
            require(module, "release", ("self.can_release", "self.tasks.remove", "worker.join"))
            worker = load("simulator/crates/desktop/src/tweet_worker.rs")
            platform = load("simulator/crates/desktop/src/tweet_platform.rs")
            host = load("simulator/crates/desktop/src/tweet_host.rs")
            require(worker, "signed", ("credentials.header", "platform::nonce", "backend.request"))
            worker_body, _ = require(worker, "run", ("backend.open_url", "backend.edit", "backend.edit_tweet", "signed", "write_tokens", "wire::api_reply", "status.store(1", "status.store(5"))
            if not all(token in worker_body for token in ("oauth/request_token", "oauth/access_token", "verify_credentials.json", "media/upload.json", "statuses/update.json")):
                raise ValueError("Tweet worker lost its real authorization/verification/media/submission consumer")
            require(platform, "request", ("WinHttpOpen", "WinHttpConnect", "WinHttpSendRequest", "WinHttpReadData"))
            require(host, "request", ("self.bridge", "platform::request", "bridge.issue", "platform::Response"))
            require(host, "open_url", ("self.bridge", "platform::open_url", "bridge.issue"))
            require(host, "edit", ("self.bridge", "platform::edit", "bridge.issue", "String::from_utf8"))
            require(host, "edit_tweet", ("self.bridge", "platform::edit_tweet", "bridge.issue", "platform::EditorResult"))
            require(host, "protect", ("self.bridge", "platform::seal", "platform::unseal", "bridge.issue"))
            require(host, "issue", ("state.pending.insert", "state.closed", "wake.wait", "state.pending.remove"))
            android = load("simulator/crates/android/src/lib.rs")
            open_body, _ = android.function("cs2_android_open")
            if "open_game(game, saves," in rust_mask(open_body):
                profiled_body, _ = android.function("cs2_android_open_profiled_v4")
                if "cs2_android_open_profiled_v5(game, saves," in rust_mask(profiled_body):
                    require(android, "cs2_android_open_profiled_v5", ("open_game(game, saves,",))
                else:
                    require(android, "cs2_android_open_profiled_v4", ("open_game(game, saves,",))
                require(android, "open_game", ("player.enable_host_tweet_queue", "player.load_startup"))
            else:
                require(android, "cs2_android_open", ("player.enable_host_tweet_queue", "player.load_startup"))
            require(android, "cs2_android_tweet", ("player.take_host_tweet_request", "sessions.tweet"))
            require(android, "cs2_android_tweet_result", (".complete_host_tweet_request", "sessions.players.get_mut"))
            require(android, "cs2_android_tweet_cancel", (".cancel_host_tweet_requests", "sessions.players.get_mut"))
            drop = re.search(r"\bimpl\s+Drop\s+for\s+TweetStore\s*\{", module.mask)
            if not drop or "bridge.cancel" not in module.mask[drop.end():matching(module.mask, drop.end() - 1)]:
                raise ValueError("Tweet store lost cancellation before joining owned workers")
            android_sources = {
                "simulator/android/app/src/main/cpp/bridge.c": (
                    "Java_org_catsystem2_player_AndroidTweetHost_nativeTake",
                    "Java_org_catsystem2_player_AndroidTweetHost_nativeComplete",
                    "Java_org_catsystem2_player_AndroidTweetHost_nativeCancel",
                    "cs2_android_tweet_result", "cs2_android_tweet_cancel"),
                "simulator/android/app/src/main/java/org/catsystem2/player/AndroidTweetHost.java": (
                    "nativeTake(session)", "nativeComplete(session", "nativeCancel(session)",
                    "HttpsURLConnection", "connection.getResponseCode()", "connection.getInputStream()",
                    "Intent.ACTION_VIEW", "TweetActivity.class", "engine.post", "io.shutdownNow()",
                    "AndroidKeyStore", "AES/GCM/NoPadding", "foregroundWork"),
                "simulator/android/app/src/main/java/org/catsystem2/player/TweetActivity.java": (
                    "editor.finish(true", "editor.finish(false", "editor.forget()",
                    "onSaveInstanceState", "AndroidTweetHost.nativeCount", "editor.draft"),
                "simulator/android/app/src/main/AndroidManifest.xml": (
                    "android.permission.INTERNET", ".TweetActivity"),
            }
            engine_activity = "simulator/android/app/src/main/java/org/catsystem2/player/PlayerEngineActivity.java"
            if engine_activity in blobs:
                android_sources[engine_activity] = (
                    "new AndroidTweetHost", "AndroidTweetHost host = tweetHost", "host.close()",
                    "tweetHost.paused()", "tweetHost.resumed()",
                )
            else:
                android_sources["simulator/android/app/src/main/java/org/catsystem2/player/PlayerActivity.java"] = (
                    "new AndroidTweetHost", "tweetHost.close()", "tweetHost.paused()", "tweetHost.resumed()",
                )
            for path, tokens in android_sources.items():
                text = blobs[path].decode("utf-8-sig")
                if not all(token in text for token in tokens):
                    raise ValueError(f"{path}: Tweet Android consumer changed; review before counting")
            if not all(token in platform.mask for token in ("DialogBoxIndirectParamW", "CryptProtectData", "CryptUnprotectData")):
                raise ValueError("Tweet lost its actual Windows UI/credential consumer")
            for token in ("self.register_tweet_task(id)", "self.update_tweet_task(id)", "self.data_services.tweets.can_release(id)", "self.data_services.tweets.release(id)"):
                if token not in tasks.mask:
                    raise ValueError("Tweet lost its live task lifecycle hooks")
            if "self.receive_tweet_command" not in commands.mask:
                raise ValueError("Tweet has no addressed command receiver")
            supported = set(range(1018, 1024))
            inspected.extend([worker.path, platform.path, host.path, android.path,
                              *android_sources, "simulator/crates/desktop/src/tweet_protocol.rs",
                              "simulator/crates/formats/src/tweet_credentials.rs"])
            limitation = "Exact Yukikoi profile only; shared worker with Windows UI/HTTPS and owned Android Java/JNI UI/HTTPS/Keystore consumers. Preview/media conversion, original credential edge cases and object-graph save equivalence remain incomplete. Completion status1 is not HTTP success."
        module.arms = [Arm(str(index), "", {index}, body, line, line, module.path) for index in supported]
        module.call_function = function
        module.call_brace, module.call_end = module.functions[function]
        module.call_match = module.call_end
        module.wrapper_forwarding = None
        result.append({
            "module": module, "supported": supported, "optional_adapter": True,
            "descriptor_abis": descriptors,
            "evidence": {
                "source": source.path, "guard_line": source.line(begin),
                "guard_source": source.text[begin:opening],
                "forwarding": f"self.{function} -> owned receiver -> concrete consumer",
                "module_source": module.path, "function": function, "function_line": line,
                "supported_indices": sorted(supported), "supported_count": len(supported),
                "status": "partial-profile-service-source-chain-not-function-equivalence",
                "conditional_on_task_kind": True, "behavior_limit": limitation,
                "inspected_source_chain": list(dict.fromkeys(inspected)),
                "descriptor_abis": descriptors,
            },
        })
    return result


def external_data_abi(module: RustSource, index: int, arm: Arm) -> dict:
    # Sizes below are the argument(args, n) reads in this independently authored
    # module, including the three sequential output arguments for 596/597.
    sizes = {
        290: 4, 594: 4, 595: 4, 596: 20, 597: 20,
        633: 4, 634: 4, 635: 8, 636: 8, 637: 12, 638: 16,
        833: 4, 834: 4, 835: 16, 836: 12, 900: 8, 901: 8,
    }
    if index in module.batch_contracts:
        contract = module.batch_contracts[index]
        function = contract.get("function", module.call_function)
        return {
            "helper": f"{Path(module.path).stem}::{function}",
            "function_name": function,
            "source": module.path, "line": arm.line,
            "accepted_argument_bytes": [contract["words"] * 4] if contract.get("exact_words") else None,
            "minimum_argument_bytes": contract["words"] * 4,
            "length_validation": "descriptor-exact-DWORD-count" if contract.get("exact_words") else "descriptor-DWORD-prefix-and-alignment",
            "receiver_command": contract.get("command"),
            "encoding": contract.get("encoding"),
            "descriptor_table": contract["table"],
            "descriptor_line": contract["table_line"],
            "evidence_kind": "descriptor-lookup-and-explicit-receiver-source-inspection",
        }
    if module.path != DATA_SERVICES:
        accesses = [
            int(offset) + 4 for offset in re.findall(
                r"\bread_u32\s*\(\s*args\s*,\s*(\d+)\s*\)", rust_mask(arm.body)
            )
        ]
        accesses += [
            int(offset) + 4 for offset in re.findall(
                r"\bargument\s*\(\s*arguments\s*,\s*(\d+)\s*\)", rust_mask(arm.body)
            )
        ]
        exact = None
        if module.path == MESSAGE_SERVICES:
            exact = 12 if index == 621 else (8 if index in {611, 612, 613, 614, 619, 620, 695} else 4)
        return {
            "helper": f"{Path(module.path).stem}::call", "function_name": "call",
            "source": module.path, "line": arm.line,
            "accepted_argument_bytes": [exact] if exact is not None else None,
            "minimum_argument_bytes": max(accesses) if accesses else None,
            "length_validation": "minimum-per-DWORD-read-source-inspection",
            "validation_helper": {"name": "read_u32", "source": DESKTOP,
                                  "line": None},
            "byte_validation_note": (
                "Maximum direct DWORD read in the arm; early returns may require "
                "fewer arguments. Helper-only accesses require separate review."
            ),
            "evidence_kind": "external-module-source-inspection",
        }
    return {
        "helper": "DataServices::call", "function_name": "call",
        "source": module.path, "line": arm.line,
        "accepted_argument_bytes": None,
        "minimum_argument_bytes": sizes.get(index),
        "length_validation": "minimum-per-argument-DWORD-access",
        "validation_helper": {
            "name": "argument", "source": DATA_SERVICES,
            "line": module.function("argument")[1],
        },
        "byte_validation_note": (
            "Normal path data size; null handles, absent rows and resources can "
            "return before later argument accesses. Trailing bytes are allowed."
        ),
        "evidence_kind": "external-module-source-inspection",
    }


def runtime_execution_evidence(source: RustSource, helper_sources: dict[str, RustSource]) -> dict:
    """Record actual execution links without equating them to validation."""
    result = {}
    motion, motion_line = source.function("motion_native")
    advance = source.functions.get("advance_motion_controller")
    if advance:
        advance_body, advance_line = source.function("advance_motion_controller")
    else:
        advance_body, advance_line = "", None
    motion_link = (
        "advance_motion_controller(" in rust_mask(motion)
        and re.search(r"task\.runner\.update\s*\(", rust_mask(advance_body))
        and re.search(r"task\.runner\.finish\s*\(", rust_mask(advance_body))
    )
    result["motion"] = {
        "status": "source-runner-execution-path-present" if motion_link else "execution-link-needs-source-review",
        "source": DESKTOP, "motion_native_line": motion_line,
        "advance_motion_controller_line": advance_line,
        "calls": ["advance_motion_controller", "task.runner.update", "task.runner.finish"] if motion_link else [],
        "verified_behavior": False,
    }
    particle_source = helper_sources.get("load_particle_native", source)
    particle, particle_line = particle_source.function("load_particle_native")
    particle_receiver_source = helper_sources.get("receive_particle_command", particle_source)
    particle_receiver = (particle_receiver_source.function("receive_particle_command")[0]
                         if "receive_particle_command" in particle_receiver_source.functions else "")
    particle_loader_source = helper_sources.get("load_particle_record", particle_source)
    particle_loader = (particle_loader_source.function("load_particle_record")[0]
                       if "load_particle_record" in particle_loader_source.functions else "")
    test_start = re.search(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*mod\s+tests", source.mask)
    production = source.mask[:test_start.start()] if test_start else source.mask
    update_source = helper_sources.get("advance_particle_task", source)
    update_body = (update_source.function("advance_particle_task")[0]
                   if "advance_particle_task" in update_source.functions else production)
    particle_body = re.search(
        r"let\s+Some\s*\(\s*runner\s*\)\s*=\s*particle\.runner\.as_mut\s*\(\s*\)",
        rust_mask(update_body),
    )
    update = (
        re.search(r"\brunner\.update\s*\(", rust_mask(update_body)[particle_body.end():particle_body.end() + 5000])
        if particle_body else None
    )
    update_pos = particle_body.end() + update.start() if particle_body and update else None
    particle_load_path = particle + particle_receiver + particle_loader
    particle_link = (
        "ParticleProgram::parse(" in rust_mask(particle_load_path)
        and "ParticleVm::new(" in rust_mask(particle_load_path)
        and update is not None
    )
    if particle_source.path != DESKTOP:
        task_owner = helper_sources.get("update_ordinary_children")
        task_body = task_owner.text if task_owner else ""
        particle_link = bool(particle_link
                             and "self.route_task_command_buffers" in rust_mask(particle)
                             and "self.load_particle_record" in rust_mask(particle_receiver)
                             and "self.advance_particle_task" in rust_mask(task_body))
    result["particle"] = {
        "status": "source-KCS-VM-execution-path-present" if particle_link else "execution-link-needs-source-review",
        "source": particle_source.path, "load_particle_native_line": particle_line,
        "runner_update_source": update_source.path,
        "runner_update_line": ((update_source.function("advance_particle_task")[1]
                                + update_body[:update_pos].count("\n"))
                               if update_pos is not None and "advance_particle_task" in update_source.functions
                               else source.line(update_pos) if update_pos is not None else None),
        "calls": ["ParticleProgram::parse", "ParticleVm::new", "runner.update"] if particle_link else [],
        "verified_behavior": False,
    }
    movie_source = helper_sources.get("load_movie_native", source)
    control_source = helper_sources.get("control_movie_native", source)
    movie, movie_line = movie_source.function("load_movie_native")
    control, control_line = control_source.function("control_movie_native")
    movie_link = "movie_host" in movie and "movie_host" in control
    result["movie"] = {
        "status": "source-movie-host-path-present" if movie_link else "execution-link-needs-source-review",
        "source": movie_source.path, "load_movie_native_line": movie_line,
        "control_movie_native_line": control_line,
        "verified_behavior": False,
    }
    frame_body, frame_line = source.function("run_frames")
    task_source = helper_sources.get("update_ordinary_children")
    result["ordinary_tasks"] = {
        "status": (
            "source-subtree-update-and-root-resume-present"
            if "update_ordinary_children(" in frame_body and "vm.resume(" in frame_body and task_source
            else "execution-link-needs-source-review"
        ),
        "frame_source": source.path, "frame_line": frame_line,
        "subtree_source": task_source.path if task_source else None,
        "verified_behavior": False,
    }
    return result


def guard_accepts(guard: str, index: int, length: int) -> bool | None:
    if not guard:
        return True
    value = guard.replace("arguments.is_empty()", str(length == 0))
    value = value.replace("arguments.len()", str(length))
    value = re.sub(
        r"matches!\(\s*index\s*,\s*([\d\s.|=]+)\)",
        lambda item: str(index in (numbers(item[1]) or set())), value
    )
    value = re.sub(
        r"word\(\s*(\d+)\s*\)\s*==\s*Some\(\s*\d+\s*\)",
        lambda item: "False" if length < int(item[1]) + 4 else "UNKNOWN",
        value,
    )
    value = value.replace("&&", " and ").replace("||", " or ")
    value = " ".join(re.sub(r"!(?!=)", " not ", value).split())
    try:
        tree = ast.parse(value, mode="eval")
        allowed = (ast.Expression, ast.Constant, ast.Name, ast.Load, ast.BoolOp, ast.And, ast.Or,
                   ast.UnaryOp, ast.Not, ast.Compare, ast.Eq, ast.NotEq,
                   ast.GtE, ast.LtE, ast.Gt, ast.Lt)
        if any(not isinstance(item, allowed) for item in ast.walk(tree)):
            return None

        def interpret(node: ast.AST):
            if isinstance(node, ast.Constant):
                return node.value
            if isinstance(node, ast.Name):
                return None
            if isinstance(node, ast.UnaryOp):
                operand = interpret(node.operand)
                return None if operand is None else not operand
            if isinstance(node, ast.BoolOp):
                operands = [interpret(item) for item in node.values]
                if isinstance(node.op, ast.And):
                    if False in operands:
                        return False
                    return None if None in operands else True
                if True in operands:
                    return True
                return None if None in operands else False
            if isinstance(node, ast.Compare):
                left = interpret(node.left)
                unknown = False
                for operation, right_node in zip(node.ops, node.comparators):
                    right = interpret(right_node)
                    if left is None or right is None:
                        unknown = True
                    else:
                        checks = {
                            ast.Eq: left == right, ast.NotEq: left != right,
                            ast.GtE: left >= right, ast.LtE: left <= right,
                            ast.Gt: left > right, ast.Lt: left < right,
                        }
                        if not checks[type(operation)]:
                            return False
                    left = right
                return None if unknown else True
            return None

        return interpret(tree.body)
    except (SyntaxError, ValueError, TypeError):
        return None


def possible_routes(arms: list[Arm], index: int, length: int) -> list[Arm]:
    result = []
    for arm in arms:
        if arm.values is None or index in arm.values:
            accepts = guard_accepts(arm.guard, index, length)
            if accepts is not False:
                result.append(arm)
            if accepts is True:
                break
    return result


def route_for(arms: list[Arm], index: int, length: int) -> Arm | None:
    candidates = possible_routes(arms, index, length)
    return candidates[0] if candidates else None


def helpers(arm: Arm) -> list[str]:
    return sorted(set(re.findall(
        r"\b(\w+_native|native_scalar|parse_kcs_float_prefix)\s*\(", rust_mask(arm.body)
    )))


@lru_cache(maxsize=None)
def helper_abi(source: RustSource, name: str, index: int) -> dict:
    body, line = source.function(name)
    evidence = {"helper": name, "source": source.path, "line": line,
                "accepted_argument_bytes": None, "length_validation": "unresolved",
                "evidence_kind": "source-inspection"}
    for found in re.finditer(r"let\s+(?:required|expected)\s*=\s*match\s+index\s*\{", rust_mask(body)):
        absolute = source.functions[name][0] + found.end() - 1
        for arm in source.match_arms(absolute):
            if arm.values is None or index in arm.values:
                number = re.fullmatch(r"\s*(\d+)\s*", arm.body)
                if number:
                    evidence.update(accepted_argument_bytes=[int(number[1])],
                                    validation_line=arm.line, length_validation="exact")
                elif re.fullmatch(r"\s*arguments\.len\(\)\s*", arm.body):
                    values = re.search(r"matches!\(arguments\.len\(\),\s*([\d |]+)\)", arm.guard)
                    if values:
                        evidence.update(accepted_argument_bytes=sorted(numbers(values[1]) or []),
                                        validation_line=arm.line, length_validation="exact")
                elif arm.values is None:
                    evidence.update(rejects_index_for_nonzero_handle=True,
                                    validation_line=arm.line)
                break
    # Early returns before the required-byte match above.
    early = {
        ("motion_native", 294): [0],
        ("backlog_native", 696): [0],
        ("image_resource_native", 710): [4],
        ("image_resource_native", 711): [4],
        ("image_resource_native", 733): [8],
    }
    if (name, index) in early:
        evidence.pop("rejects_index_for_nonzero_handle", None)
        evidence["accepted_argument_bytes"] = early[name, index]
        evidence["early_branch"] = True
        if index in (294, 711):
            evidence["byte_validation_note"] = "required data size; helper does not validate exact length"
            evidence["length_validation"] = "ignored" if index == 294 else "minimum"
        else:
            evidence["length_validation"] = "exact"
    if evidence["accepted_argument_bytes"] is None and not evidence.get("rejects_index_for_nonzero_handle"):
        fixed = re.search(r"require_(?:audio|particle)_args\(\s*arguments,\s*(\d+)", body)
        if fixed:
            evidence["accepted_argument_bytes"] = [int(fixed[1])]
            evidence["validation_line"] = line + body.count("\n", 0, fixed.start())
        else:
            exact = sorted(set(int(item) for item in re.findall(r"arguments\.len\(\)\s*!=\s*(\d+)", body)))
            if len(exact) == 1:
                evidence["accepted_argument_bytes"] = exact
                evidence["length_validation"] = "exact"
    # Source-specific conditional expressions. Keep the conditions rather than
    # calling these unconditional full ABI validation.
    derived = {
        "fes_parameter_native": {645: [12], 646: [8], 649: [12], 650: [12]},
        "utf8_text_native": {1045: [8], 1046: [8], 1051: [8]},
        "record_stack_native": {
            863: [8], 864: [4], 865: [8], 866: [8], 867: [4], 868: [8]
        },
        "cross_task_memory_native": {72: [16], 73: [16]},
        "integer_sequence_native": {539: [12]},
        "word_offset_native": {540: [8]},
        "load_movie_native": {265: [4]},
        "control_movie_native": {266: [8], 267: [4], 268: [4], 269: [4]},
    }
    if name in derived and index in derived[name]:
        evidence["accepted_argument_bytes"] = derived[name][index]
        evidence["byte_validation_note"] = (
            "source-derived required data size; inspect top-level guard and helper "
            "conditions for exact/minimum length and null-handle exceptions"
        )
        evidence["length_validation"] = (
            "minimum" if name in ("cross_task_memory_native", "integer_sequence_native",
                                  "word_offset_native", "load_movie_native", "control_movie_native")
            else "exact"
        )
    if name == "record_stack_native" and index != 863:
        evidence["null_handle_short_form_bytes"] = 4
    if name == "image_resource_native":
        evidence["null_handle_short_circuit"] = (
            "zero first DWORD returns before required-byte match except 710/733 "
            "and canvas-target 736"
        )
    # These validation helpers check len < required, not len != required.
    # In particular native 163's ten data bytes allow the observed 12-byte
    # aligned argument block and therefore are not an ABI conflict.
    validator = re.search(r"\brequire_(audio|particle)_args\s*\(", body)
    if validator and evidence["accepted_argument_bytes"]:
        validator_name = f"require_{validator[1]}_args"
        validator_body, validator_line = source.function(validator_name)
        if re.search(r"arguments\.len\(\)\s*<\s*required", validator_body):
            evidence["length_validation"] = "minimum"
            evidence["validation_helper"] = {
                "name": validator_name, "source": DESKTOP, "line": validator_line,
            }
    if evidence["accepted_argument_bytes"] and re.search(
        r"\b(?:args|arguments)\.len\(\)\s*<\s*required", rust_mask(body)
    ):
        evidence["length_validation"] = "minimum"
        evidence["validation_helper"] = {
            "name": "inline minimum-length guard", "source": source.path, "line": line,
        }
    if evidence["length_validation"] == "minimum" and evidence["accepted_argument_bytes"]:
        evidence["minimum_argument_bytes"] = min(evidence["accepted_argument_bytes"])
        evidence["accepted_argument_bytes"] = None
    if evidence["length_validation"] == "ignored":
        evidence["accepted_argument_bytes"] = None
    return evidence


def reference_rows(root: Path) -> dict[int, list[dict]]:
    result = defaultdict(list)
    for name, baseline in (
        ("native-calls.tsv", "2.6.1.90-A"),
        ("native-calls-comparison.tsv", "2.6.1.90-B"),
        ("native-calls-2.6.1.92.tsv", "2.6.1.92-C"),
    ):
        data = (root / REF / name).read_text(encoding="utf-8-sig")
        for line, row in enumerate(csv.DictReader(io.StringIO(data), delimiter="\t"), 2):
            result[int(row["index"])].append({
                "source": REF + name, "line": line, "baseline": baseline,
                "table": row["table"], "program_version": row["program_version"],
                "argument_bytes": int(row["argument_bytes"]),
                "call_sites": int(row["call_sites"]),
                "status": "reference-static-ABI-observation",
            })
    return result


@lru_cache(maxsize=None)
def all_doc_evidence(root: Path) -> dict[int, list[dict]]:
    result = defaultdict(list)
    for name in ("kcs.md", "native-calls.md", "graphics.md", "audio.md",
                 "fes.md", "message.md", "input.md", "save.md", "movie.md",
                 "backlog.md", "cst.md"):
        heading = ""
        host_table = "ordinary"
        native_table = False
        lines = (root / REF / name).read_text(encoding="utf-8").splitlines()
        for offset, text in enumerate(lines):
            line = offset + 1
            if text.startswith("#"):
                heading = text.lstrip("# ").strip()
                if name == "kcs.md":
                    if heading == "运动脚本宿主":
                        host_table = "motion"
                    elif heading == "粒子脚本宿主":
                        host_table = "particle"
                    elif heading in ("本阶段使用的宿主接口", "运动队列与对象更新"):
                        host_table = "ordinary"
            if not text.startswith("|"):
                native_table = False
                continue
            if offset + 1 < len(lines) and re.match(r"^\|[ :|-]+\|?$", lines[offset + 1]):
                first_header = text.split("|")[1].strip()
                native_table = (
                    first_header.lower() == "native" or first_header == "普通接口"
                    or (name in ("kcs.md", "movie.md", "backlog.md")
                        and first_header in ("编号", "编号组"))
                )
                continue
            # Object command numbers, field offsets and arbitrary numeric
            # ranges are not native slots, even when the numbers happen to match.
            if not native_table:
                continue
            if not re.match(r"^\|\s*`?\d", text):
                continue
            first = text.split("|")[1].replace("`", "").strip()
            expanded = re.sub(r"(\d+)\s*[～~–-]\s*(\d+)", r"\1..=\2", first)
            expanded = expanded.replace("／", "|").replace("/", "|").replace("、", "|")
            try:
                matches = numbers(expanded)
            except ValueError:
                matches = set(int(item) for item in re.findall(r"\d+", first))
            for index in matches or set():
                result[index].append({"source": REF + name, "line": line,
                                      "table": host_table, "section": heading,
                                      "description": text.strip()})
    return result


def const_alias_evidence(root: Path) -> dict:
    """Read documented ordinary-table alias sets; no implementation code."""
    path = root / REF / "kcs.md"
    lines = path.read_text(encoding="utf-8").splitlines()
    start = next(i for i, line in enumerate(lines) if line == "### 固定返回入口")
    stop = next(i for i in range(start + 1, len(lines)) if lines[i].startswith("### "))
    continuation, zeros, evidence_lines = set(), set(), []
    for pos in range(start, stop):
        text = lines[pos]
        if text.startswith("|") and text.endswith("|2|"):
            raw = text.split("|")[1].replace("／", "|").replace("～", "..=")
            continuation.update(numbers(raw) or set())
            evidence_lines.append(pos + 1)
        if text.startswith("另有 "):
            raw = text.split("另有 ", 1)[1].split("，", 1)[0]
            zeros.update(numbers(raw.replace("／", "|").replace("～", "..=")) or set())
            evidence_lines.append(pos + 1)
    if len(continuation) != 89 or len(zeros) != 23:
        raise ValueError("documented alias set changed; review evidence parser")
    return {"continue": sorted(continuation), "zero": sorted(zeros),
            "source": REF + "kcs.md", "lines": evidence_lines,
            "baseline": "2.6.1.90 only"}


def startup_evidence(root: Path, source: RustSource) -> dict:
    path = root / "archive/2026-11/20261116_all_samples_cold_start.json"
    archived = json.loads(path.read_text(encoding="utf-8")) if path.exists() else []
    historical = []
    for item in archived:
        if item["Status"] == "ok" and not item.get("Diagnostic"):
            continue
        diagnosis = item["Diagnostic"]
        native = re.search(r"native call (\d+)", diagnosis)
        row = {
            "game": item["Name"], "root": item["Root"],
            "source": path.relative_to(root).as_posix(),
            "evidence_status": "historical-archived-trace-not-rerun",
            "diagnostic": diagnosis,
        }
        if native:
            index = int(native[1])
            route = route_for(source.arms, index, 8)
            row.update(native=index, current_route=source_kind(route),
                       current_route_line=route.line if route else None,
                       conclusion="Current source has a route; resolution is not behaviorally revalidated.")
        else:
            row["conclusion"] = "Entry/resource blocker; native routing alone cannot resolve it."
        historical.append(row)
    return {
        "historical_failures": historical,
        "newly_proven_runtime_blockers": [],
        "policy": "No new startup path was executed. Frequency-ranked slots are potential blockers, not observed startup failures.",
    }


def recover_native_table_evidence(root: Path, report: dict) -> dict:
    """Discover pointer-array candidates using documented alias identities.

    A matching array is still a candidate until its VM registration chain is
    established. Code bytes, aliases and exact-RVA ledger joins are facts;
    calling convention and game behavior are not inferred from table presence.
    """
    ledger_bytes = (root / LEDGER).read_bytes()
    ledger = json.loads(ledger_bytes)
    try:
        import capstone
    except ImportError:
        capstone = None
    alias = report["reserved_alias_documentation"]
    continuation = alias["continue"]
    zeros = alias["zero"]
    observed = [row["index"] for row in report["natives"]]
    result = []
    for binary in ledger["binaries"]:
        if binary.get("runtime_role") != "primary-runtime-executable":
            continue
        path = root / binary["binary_path"]
        blob = path.read_bytes()
        digest = hashlib.sha256(blob).hexdigest()
        record = {
            "sample_id": binary["sample_id"], "binary_path": binary["binary_path"],
            "sha256": digest, "matches_function_ledger_sha256": digest == binary["sha256"],
            "status": "no-candidate-found", "candidates": [],
            "semantically_implemented_count": 0,
        }
        result.append(record)
        if not record["matches_function_ledger_sha256"]:
            record["status"] = "fingerprint-mismatch-no-analysis"
            continue
        meta = binary.get("pe_metadata", {})
        if meta.get("architecture") != "x86":
            record["status"] = "unsupported-static-architecture"
            continue
        image_base = meta["image_base"]
        sections = binary["pe_sections"]
        code = [section for section in sections if section["executable"]]
        executable_ranges = [
            (image_base + section["rva_start"], image_base + section["rva_mapped_end_exclusive"])
            for section in code
        ]

        def executable(va: int) -> bool:
            return any(start <= va < end for start, end in executable_ranges)

        def code_bytes(rva: int, length: int = 64) -> bytes:
            for section in sections:
                delta = rva - section["rva_start"]
                if 0 <= delta < section["raw_size_available"]:
                    start = section["raw_offset"] + delta
                    return blob[start:min(start + length, section["raw_end_exclusive"])]
            return b""

        function_by_rva = {item["rva"]: item for item in binary["functions"]}
        disassembler = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32) if capstone else None
        for section in sections:
            if section["executable"] or section["name"] == ".rsrc":
                continue
            start = section["raw_offset"]
            size = section["raw_size_available"]
            words = array("I")
            words.frombytes(blob[start:start + size - size % 4])
            if sys.byteorder != "little":
                words.byteswap()
            for pos in range(max(0, len(words) - 1080 + 1)):
                first = words[pos]
                # Three documented reserved aliases plus time/identity slots
                # suppress arbitrary RTTI/data arrays before expensive checks.
                if first != words[pos + 2] or first != words[pos + 3]:
                    continue
                if first == words[pos + 1] or not executable(first):
                    continue
                if not all(executable(words[pos + index]) for index in (1, 4, 5, 6, 8, 9)):
                    continue
                zero = words[pos + 171]
                continuing_matches = [index for index in continuation if words[pos + index] == first]
                zero_matches = [index for index in zeros if words[pos + index] == zero]
                score = len(continuing_matches) + len(zero_matches)
                if score < 60 or not executable(zero) or zero == first:
                    continue
                code_count = sum(executable(value) for value in words[pos:pos + 1080])
                if code_count < 1060:
                    continue
                table_rva = section["rva_start"] + pos * 4
                aliases = defaultdict(list)
                for index in range(1080):
                    aliases[words[pos + index] - image_base].append(index)
                slots = []
                for index in observed:
                    if index >= 1080:
                        continue
                    rva = words[pos + index] - image_base
                    target_executable = executable(words[pos + index])
                    body = code_bytes(rva) if target_executable else b""
                    entry = function_by_rva.get(rva)
                    instructions = []
                    if disassembler:
                        for instruction in disassembler.disasm(body, image_base + rva):
                            instructions.append({
                                "rva": instruction.address - image_base,
                                "bytes": instruction.bytes.hex(),
                                "mnemonic": instruction.mnemonic,
                                "operands": instruction.op_str,
                            })
                            if instruction.mnemonic.startswith("ret") or len(instructions) >= 10:
                                break
                    stub = None
                    if body.startswith(bytes.fromhex("b802000000c21000")):
                        stub = {
                            "kind": "fixed-callback-status", "eax_status": 2,
                            "writes_output_pointer": False, "ret_pop_bytes": 16,
                            "evidence": "exact-eight-byte-instruction-prefix",
                        }
                    elif (body.startswith(bytes.fromhex("8b442410c700"))
                          and body[10:18] == bytes.fromhex("b801000000c21000")):
                        stub = {
                            "kind": "fixed-output-and-callback-status", "eax_status": 1,
                            "writes_output_pointer": True,
                            "output_dword": struct.unpack_from("<I", body, 6)[0],
                            "ret_pop_bytes": 16,
                            "evidence": "exact-eighteen-byte-instruction-prefix",
                        }
                    slots.append({
                        "index": index, "entry_rva": rva,
                        "entry_rva_hex": f"0x{rva:08x}",
                        "target_is_executable_section": target_executable,
                        "table_membership_verified": False,
                        "first_64_bytes": body.hex(),
                        "first_64_sha256": hashlib.sha256(body).hexdigest(),
                        "shared_entry_slots": aliases[rva],
                        "candidate_function_exact_entry_match": bool(entry),
                        "existing_ledger_status": entry.get("status") if entry else None,
                        "existing_candidate_size": entry.get("size") if entry else None,
                        "existing_candidate_called_by": entry.get("called_by") if entry else None,
                        "disassembly_prefix": instructions,
                        "decoded_constant_stub_fact": stub,
                        "evidence_status": "pointer-slot-static-candidate-not-registered-ABI-proof",
                        "implemented": False,
                    })
                contiguous = 0
                for value in words[pos:min(len(words), pos + 2048)]:
                    if not executable(value):
                        break
                    contiguous += 1
                raw_references = []
                needle = struct.pack("<I", image_base + table_rva)
                for code_section in code:
                    code_start = code_section["raw_offset"]
                    code_end = code_section["raw_end_exclusive"]
                    position = blob.find(needle, code_start, code_end)
                    while position >= 0:
                        ref_rva = code_section["rva_start"] + position - code_start
                        envelopes = [
                            fn["rva"] for fn in binary["functions"]
                            if fn["rva"] <= ref_rva < fn.get("range_end_rva_exclusive", fn["rva"])
                        ]
                        raw_references.append({
                            "value_rva": ref_rva, "value_rva_hex": f"0x{ref_rva:08x}",
                            "containing_candidate_envelopes": envelopes,
                            "context_start_rva": ref_rva - 8,
                            "context_bytes": blob[max(code_start, position - 8):min(code_end, position + 12)].hex(),
                            "status": "raw-table-VA-occurrence-not-decoded-xref",
                        })
                        position = blob.find(needle, position + 1, code_end)
                record["candidates"].append({
                    "table_rva": table_rva, "table_rva_hex": f"0x{table_rva:08x}",
                    "file_offset": start + pos * 4, "section": section["name"],
                    "status": "ordinary-table-layout-candidate",
                    "registration_chain_verified": False,
                    "alias_agreement": score, "alias_denominator": 112,
                    "continue_alias_agreement": continuing_matches,
                    "zero_alias_agreement": zero_matches,
                    "code_pointer_count_in_first_1080": code_count,
                    "contiguous_code_pointers_not_confirmed_table_length": contiguous,
                    "different_entries_in_first_1080": len(aliases),
                    "raw_table_address_references": raw_references,
                    "distinct_code_entries_in_observed_slots": len({
                        slot["entry_rva"] for slot in slots if slot["target_is_executable_section"]
                    }),
                    "new_indirect_entry_rvas_not_in_function_ledger": sorted({
                        slot["entry_rva"] for slot in slots
                        if slot["target_is_executable_section"]
                        and not slot["candidate_function_exact_entry_match"]
                    }),
                    "slot_evidence": slots,
                })
        if record["candidates"]:
            record["status"] = "static-table-candidate-found"
    return {
        "schema_version": "cs2-runtime-native-table-evidence/1.0",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "status": "static-candidates-only",
        "source_function_ledger": LEDGER,
        "source_function_ledger_sha256": hashlib.sha256(ledger_bytes).hexdigest(),
        "alias_source": alias,
        "capstone_version": capstone.__version__ if capstone else None,
        "scope": {
            "primary_executables_read": len(result),
            "binaries_with_candidates": sum(bool(item["candidates"]) for item in result),
            "table_candidates": sum(len(item["candidates"]) for item in result),
            "new_indirect_entry_seeds_per_binary": sum(
                len(candidate["new_indirect_entry_rvas_not_in_function_ledger"])
                for item in result for candidate in item["candidates"]
            ),
            "semantically_implemented_functions": 0,
        },
        "limitations": [
            "Layout/alias matching does not establish host registration or table bounds.",
            "Only the documented first 1080 ordinary slots are inspected; motion/particle tables need separate registration-chain evidence.",
            "An exact candidate-function RVA match does not validate recovered function bounds or semantics.",
            "Disassembly prefixes are evidence only; no native code was executed.",
            "Non-code targets in the candidate window may mark a table boundary or reserved holes; no function bytes or ABI is claimed for them.",
        ],
        "binaries": result,
    }


def source_kind(arm: Arm | None) -> str:
    if arm is None or (
        "UnimplementedNativeCall" in arm.body
        and not re.search(r"\bNativeReturn::(?:Continue|Value|Retry|YieldVoid|Yield)\b", rust_mask(arm.body))
    ):
        return "explicit-unimplemented" if arm and arm.values is not None else "unmapped"
    compact = re.sub(r"\s+", "", arm.body)
    if compact.startswith("reserved_native::call("):
        # analyze() validates the structural gate and historical sample proofs.
        return "constant-return-only"
    if re.fullmatch(r"Ok\(NativeReturn::(?:Continue|Value\(0\))\)", compact):
        return "constant-return-only"
    if compact.startswith("Err("):
        return "argument-rejected"
    return "source-behavior"


def reserved_constant_bindings(blobs: dict[str, bytes], source: RustSource) -> dict[int, list[dict]]:
    """Validate the reserved dispatch and historical sample constant proofs."""
    path = "simulator/crates/desktop/src/reserved_native.rs"
    table_path = "simulator/crates/runtime/src/native_table.rs"
    report_path = "reports/reserved_native_contracts_20261117.json"
    mask = rust_mask(blobs[path].decode("utf-8-sig"))
    table_source = rust_mask(blobs[table_path].decode("utf-8-sig"))
    if (not all(token in mask for token in ("table.constant(index)", "constant.native_return()",
                                           "ok_or(ScriptError::UnimplementedNativeCall"))
            or not all(token in table_source for token in ("x86_image_base(data)", "starts_with(VM_TABLE_SETTER)",
                                                          "constant_stub(body)"))
            or "OrdinaryNativeTable::from_pe(&bytes)" not in source.mask):
        raise ValueError("reserved native dispatcher lost its registered-table/body gate")
    indices = set().union(*(arm.values or set() for arm in source.arms
        if "reserved_native::call" in rust_mask(arm.body)
        and "ordinary_native_table.as_ref()" in rust_mask(arm.body)))
    if not indices:
        raise ValueError("no reserved native slots forward the registered table")

    evidence = json.loads(blobs[report_path])
    bindings = defaultdict(list)
    for profile in evidence["profiles"]:
        selected = set()
        for row in profile["slots"]:
            constant = row["constant"]
            status, value = constant["callback_status"], constant["output_dword"]
            proof = bytes.fromhex(constant["complete_stub_bytes"])
            template = (bytes.fromhex("b802000000c21000") if (status, value) == (2, None)
                        else bytes.fromhex("8b442410c700") + struct.pack("<I", value)
                        + bytes.fromhex("b801000000c21000") if status == 1 else None)
            if proof != template or hashlib.sha256(proof).hexdigest() != constant["complete_stub_sha256"]:
                raise ValueError("reserved native proof is not a complete recognized function")
            index = row["index"]
            if index in selected:
                raise ValueError("duplicate reserved native sample proof")
            selected.add(index)
            bindings[index].append({
                "pe_sha256": profile["sha256"], "entry_rva": row["entry_rva"],
                "callback_status": status, "output_dword": value,
                "proof_sha256": constant["complete_stub_sha256"],
                "status": "exact-local-constant-bytes-not-full-game-equivalence",
            })
        if selected != indices:
            raise ValueError("reserved native source slots differ from historical sample evidence")
    if not bindings:
        raise ValueError("reserved native dispatcher lacks historical sample evidence")
    return dict(bindings)


def logging_host_source(data: bytes) -> RustSource:
    text = data.decode("utf-8-sig")
    name = "dispatch_kcs_native" if re.search(r"\bfn\s+dispatch_kcs_native\s*\(", text) else "call"
    return RustSource(text, call_function=name)


def analyze(root: Path) -> dict:
    paths = [DESKTOP, AUDIT, LEDGER, REF + "kcs.md", REF + "native-calls.md",
             REF + "native-calls.tsv", REF + "native-calls-comparison.tsv",
             REF + "native-calls-2.6.1.92.tsv"]
    paths += [
        REF + name for name in (
            "graphics.md", "audio.md", "fes.md", "message.md", "input.md",
            "save.md", "movie.md", "backlog.md", "cst.md"
        )
    ]
    paths += [
        name for name in (
            DATA_SERVICES,
            ORDINARY_SERVICES,
            LANGUAGE_SERVICES,
            MESSAGE_SERVICES,
            "simulator/crates/desktop/src/message_platform.rs",
            "simulator/crates/desktop/src/message_backend.rs",
            "simulator/crates/desktop/src/legacy_text_native.rs",
            "simulator/crates/desktop/src/language_platform.rs",
            "simulator/crates/desktop/src/language_adapters.rs",
            "simulator/crates/desktop/src/native_tasks.rs",
            "simulator/crates/desktop/src/fes_tasks.rs",
            "simulator/crates/desktop/src/fes_driver.rs",
            "simulator/crates/desktop/src/fes_task_platform.rs",
            "simulator/crates/desktop/src/fes_media.rs",
            "simulator/crates/runtime/src/native_profile.rs",
            "simulator/crates/desktop/src/movie_native.rs",
            "simulator/crates/desktop/src/movie.rs",
            "simulator/crates/desktop/src/media_input.rs",
            "simulator/crates/desktop/src/media_process.rs",
            "simulator/crates/desktop/src/motion_vm.rs",
            "simulator/crates/desktop/src/particle_vm.rs",
        ) if (root / name).exists()
    ]
    # A moved helper, FES extension or CPU backend must invalidate the snapshot,
    # even when its numeric match lives in lib.rs.
    paths = list(dict.fromkeys(paths + [
        path.relative_to(root).as_posix()
        for path in sorted((root / "simulator/crates").rglob("*"))
        if path.is_file() and path.suffix in {".rs", ".js", ".toml"}
        and not any(part in {"target", "tests"} for part in path.parts)
    ]))
    # Native host routes consume Java/JNI/manifest code outside crates/. Keep
    # those actual bytes in the same immutable evidence snapshot as the Rust.
    paths = list(dict.fromkeys(paths + [
        path.relative_to(root).as_posix()
        for path in sorted((root / "simulator/android/app/src").rglob("*"))
        if path.is_file() and (path.suffix in {".java", ".c", ".h", ".xml"}
                               or path.name == "CMakeLists.txt")
    ]))
    paths += ["simulator/Cargo.toml", "simulator/Cargo.lock", "reports/reserved_native_contracts_20261117.json"]
    blobs = {name: (root / name).read_bytes() for name in paths}
    hashes = {name: hashlib.sha256(data).hexdigest() for name, data in blobs.items()}
    archived_sources = archive_source_blobs(root, blobs, hashes)
    source = logging_host_source(blobs[DESKTOP])
    helper_sources = {name: source for name in source.functions}
    for path in paths:
        if path.startswith("simulator/crates/desktop/src/") and path != DESKTOP:
            helper_source = RustSource(blobs[path].decode("utf-8-sig"), path,
                                       require_dispatch=False)
            for name in helper_source.functions:
                helper_sources.setdefault(name, helper_source)
    overlays = external_dispatches(root, source, blobs)
    reserved_bindings = reserved_constant_bindings(blobs, source)
    execution_evidence = runtime_execution_evidence(source, helper_sources)
    audit = json.loads(blobs[AUDIT])
    ledger = json.loads(blobs[LEDGER])
    refs = reference_rows(root)
    alias_evidence = const_alias_evidence(root)
    continuing = set(alias_evidence["continue"])
    zero_aliases = set(alias_evidence["zero"])
    observations = defaultdict(list)
    for game in audit["games"]:
        for key, value in game["scripts"]["native_calls"].items():
            observations[int(key)].append({
                "game": game["root"], "variant": game["variant"],
                "call_sites": value["call_sites"],
                "argument_bytes": {str(k): v for k, v in value["argument_bytes"].items()},
                "host_table": "not-preserved-by-content-audit",
            })
    rows = []
    for index in sorted(observations):
        observed = observations[index]
        counts = Counter()
        for game in observed:
            counts.update({int(k): v for k, v in game["argument_bytes"].items()})
        overlay = next((item for item in overlays if index in item["supported"]), None)
        dispatch_source = overlay["module"] if overlay else source
        matching_arms = [arm for arm in dispatch_source.arms if arm.values and index in arm.values]
        behavior_arms = [arm for arm in matching_arms if source_kind(arm) not in ("explicit-unimplemented", "argument-rejected")]
        complex_arms = [arm for arm in behavior_arms if source_kind(arm) == "source-behavior"]
        active = (complex_arms[0] if complex_arms else
                  behavior_arms[0] if behavior_arms else
                  matching_arms[0] if matching_arms else None)
        kind = source_kind(active)
        if active and "reserved_native::call" in rust_mask(active.body) and index not in reserved_bindings:
            raise ValueError("reserved native call has no historical sample proof")
        helper_names = helpers(active) if active else []
        if index == 25:
            # The shared 25/26 arm selects integer parsing for 25.
            helper_names = [name for name in helper_names if name != "parse_kcs_float_prefix"]
        helper_info = (
            [{
                "helper": overlay["evidence"]["function"],
                "function_name": overlay["evidence"]["function"],
                "source": dispatch_source.path, "line": active.line,
                "accepted_argument_bytes": None,
                "minimum_argument_bytes": overlay.get("descriptor_abis", {}).get(index, {}).get("minimum_argument_bytes"),
                "descriptor_abi": overlay.get("descriptor_abis", {}).get(index),
                "length_validation": "conditional-adapter-needs-per-branch-ABI-review",
                "evidence_kind": "optional-adapter-source-inspection",
            }] if overlay and overlay.get("optional_adapter") and active else
            [external_data_abi(dispatch_source, index, active)] if overlay and active else
            [helper_abi(
                dispatch_source if name in dispatch_source.functions else helper_sources[name],
                name, index
            ) for name in helper_names
             if name in dispatch_source.functions or name in helper_sources]
        )
        if any(item.get("rejects_index_for_nonzero_handle") for item in helper_info):
            kind = "helper-rejects-index"
        if kind == "constant-return-only" and index in VERSION_CHANGED:
            kind = "version-sensitive-constant"
        notes = []
        if index in {304, 307, 440, 535}:
            notes.append("Motion execution evidence: " + execution_evidence["motion"]["status"]
                         + "; source paths do not constitute behavioral verification.")
        if index in {265, 266, 267, 268, 269}:
            notes.append("Movie host evidence: " + execution_evidence["movie"]["status"]
                         + "; source paths do not constitute behavioral verification.")
        if index in {271, 693}:
            notes.append("Particle execution evidence: " + execution_evidence["particle"]["status"]
                         + "; source paths do not constitute behavioral verification.")
        per_length = []
        for length, sites in sorted(counts.items()):
            alternatives = possible_routes(dispatch_source.arms, index, length)
            selected = alternatives[0] if alternatives else None
            value_dependent = len(alternatives) > 1
            item = {"argument_bytes": length, "call_sites": sites,
                    "route_kind": "value-dependent-dispatch" if value_dependent else source_kind(selected),
                    "selected_source": dispatch_source.path,
                    "selected_line": selected.line if selected and not value_dependent else None,
                    "possible_arms": [arm.evidence() for arm in alternatives],
                    "argument_values_required": value_dependent,
                    "dispatch_origin": "pre-match-external-guard" if overlay else "LoggingHost-match"}
            if helper_info and item["route_kind"] == "source-behavior":
                constraints = [h["accepted_argument_bytes"] for h in helper_info
                               if h["accepted_argument_bytes"] is not None]
                if any(length not in allowed for allowed in constraints):
                    item["route_kind"] = "helper-argument-rejected"
                if any(length < h["minimum_argument_bytes"] for h in helper_info
                       if h.get("minimum_argument_bytes") is not None):
                    item["route_kind"] = (
                        "conditional-argument-short-form-needs-call-values" if overlay
                        else "helper-minimum-argument-rejected"
                    )
                if any(h.get("rejects_index_for_nonzero_handle") for h in helper_info):
                    item["route_kind"] = "helper-rejects-nonzero-handle"
            item["reference_tables_for_same_size"] = sorted({
                ref["table"] for ref in refs[index] if ref["argument_bytes"] == length
            })
            per_length.append(item)
        tables = sorted({item["table"] for item in refs[index]})
        if len(tables) > 1:
            notes.append("Numeric slot occurs in multiple reference host tables. Aggregate observations cannot establish which table was used.")
        if kind == "constant-return-only":
            notes.append("The registered ordinary table and complete callback body determine the outcome. The archived PE bindings are sample evidence."
                         if index in reserved_bindings else
                         "Only a fixed status/value is returned. Baseline reserved/no-op semantics require version-specific evidence.")
        if kind == "version-sensitive-constant":
            notes.append("native-calls.md:63 documents activation in 2.6.1.92; baseline constant behavior cannot be applied globally.")
        abi_conflicts = [
            item for item in per_length
            if "ordinary" in item["reference_tables_for_same_size"]
            and item["route_kind"] in (
                "argument-rejected", "helper-argument-rejected", "helper-minimum-argument-rejected"
            )
        ]
        if abi_conflicts:
            notes.append("Observed length is also documented in the ordinary reference table but the current route/helper rejects it.")
        if index == 8 and active and "800_u32" in active.body and "600_u32" in active.body:
            notes.append("Outputs hardcoded 800x600 rather than the active render context dimensions.")
        if index == 9:
            notes.append("Review the close-request source and desktop consumer together; emitting an event alone does not establish the window side effect.")
        if index == 393:
            notes.append("Review the menu-request source and desktop consumer together; queueing alone does not establish native-menu behavior.")
        semantic_conflicts = []
        if index == 64:
            body, line = source.function("native_scalar")
            if re.search(r"64\s*=>\s*\(first as i32\)\.wrapping_abs\(\) as f32", body):
                kind = "source-semantic-conflict"
                semantic_conflicts.append({
                    "source": DESKTOP, "line": line, "helper": "native_scalar",
                    "fact": "Converts integer absolute value to f32 then returns to_bits().",
                    "reference": REF + "kcs.md:387",
                    "expected": "Return wrapped integer absolute value bits, not f32 encoding.",
                    "example_by_source_evaluation": "input 1 -> 0x3f800000; documented integer output 0x00000001",
                })
        if index == 26:
            body, line = source.function("parse_kcs_float_prefix")
            if "text.parse::<f32>()" in body:
                kind = "source-semantic-conflict"
                semantic_conflicts.append({
                    "source": DESKTOP, "line": line, "helper": "parse_kcs_float_prefix",
                    "fact": "Rust parse::<f32>() consumes the complete trimmed string; a nonnumeric suffix causes zero.",
                    "reference": REF + "kcs.md:217",
                    "expected": "CRT-style numeric prefix conversion ignores suffix; preserve documented special values.",
                    "example_by_source_evaluation": "'1.5rest' -> 0; documented prefix result 1.5f",
                })
        if index == 727:
            body, line = source.function("image_resource_native")
            flip = re.search(
                r"727\s*=>\s*\{.*?if read_u32\(arguments, 4\)\?\s*==\s*0.*?flip_horizontal\(\)",
                body, re.S
            )
            if flip:
                kind = "source-semantic-conflict"
                semantic_conflicts.append({
                    "source": DESKTOP, "line": line + body.count("\n", 0, flip.start()),
                    "helper": "image_resource_native",
                    "fact": "Zero direction selects horizontal flip and nonzero selects vertical.",
                    "reference": REF + "graphics.md:140",
                    "expected": "Documented ordinary 727: nonzero horizontal, zero vertical.",
                })
        return_variants = set()
        if active:
            return_variants.update(re.findall(r"NativeReturn::(\w+)", rust_mask(active.body)))
        for info in helper_info:
            function_name = info.get("function_name", info["helper"])
            if function_name not in dispatch_source.functions:
                continue
            body, _ = dispatch_source.function(function_name)
            return_variants.update(re.findall(r"NativeReturn::(\w+)", rust_mask(body)))
        evidence = all_doc_evidence(root).get(index, [])
        rows.append({
            "index": index,
            "status": kind,
            "implemented_verified": False,
            "games_count": len(observed),
            "call_sites": sum(counts.values()),
            "argument_bytes": {str(k): v for k, v in sorted(counts.items())},
            "observations": observed,
            "rust_route_present": bool(matching_arms),
            "dispatch_origin": "pre-match-external-guard" if overlay else "LoggingHost-match",
            "forwarding_guard": overlay["evidence"] if overlay else None,
            "rust_arms": [arm.evidence() for arm in matching_arms],
            "helpers": helper_info,
            "observed_length_dispatch": per_length,
            "ordinary_reference_ABI_conflicts": abi_conflicts,
            "source_semantic_conflicts": semantic_conflicts,
            "source_return_variants_overapproximation": sorted(return_variants),
            "constant_baseline_evidence": (
                "documented-ordinary-state-2-alias" if index in continuing
                else "documented-ordinary-zero-state-1-alias" if index in zero_aliases
                else "not-established-by-reserved-alias-documentation"
            ) if "constant" in kind else None,
            "exact_constant_PE_bindings": reserved_bindings.get(index, [])
                if active and "reserved_native::call" in rust_mask(active.body) else [],
            "reference_tables": tables,
            "reference_abi": refs[index],
            "semantic_documentation": evidence,
            "notes": notes,
        })
    summary = Counter(row["status"] for row in rows)
    explicit = set().union(*(arm.values or set() for arm in source.arms))
    top_level_explicit = set(explicit)
    external_indices = set().union(*(item["supported"] for item in overlays)) if overlays else set()
    explicit.update(external_indices)
    missing = [row for row in rows if row["status"] in ("unmapped", "explicit-unimplemented", "helper-rejects-index")]
    ready = []
    motion_body, motion_line = source.function("motion_native")
    for index in (388, 440, 586):
        row = next((row for row in rows if row["index"] == index), None)
        ready.append({
            "index": index, "observed": row is not None,
            "status": row["status"] if row else "not-observed",
            "candidate_action": (
                "Add missing LoggingHost::call arm to motion_native after ABI review."
                if row and row["status"] == "unmapped"
                else "Inspect the current route and helper behavior; source may already contain wiring."
            ),
            "helper": "motion_native", "source": DESKTOP, "line": motion_line,
            "abi": helper_abi(source, "motion_native", index),
            "caveat": "Helper source exists but is not reachable through the numeric host route; this is not an implemented native claim.",
        })
    # Same patterns/guards after a previous match are unreachable, even though
    # a line-regex would incorrectly count them as independent implementations.
    shadows = []
    for index in sorted(explicit):
        previous = set()
        for arm in source.arms:
            if arm.values and index in arm.values:
                if arm.guard in previous:
                    shadows.append({"index": index, "source": DESKTOP, "line": arm.line,
                                    "guard": arm.guard, "status": "shadowed-duplicate-arm"})
                previous.add(arm.guard)
    binary_summary = []
    for binary in ledger["binaries"]:
        functions = binary.get("functions", [])
        binary_summary.append({
            "sample_id": binary["sample_id"], "binary_path": binary["binary_path"],
            "sha256": binary["sha256"], "runtime_role": binary.get("runtime_role"),
            "candidate_count": len(functions),
            "entrypoint_reachable_candidates": sum(bool(f.get("reachable_from_entrypoint")) for f in functions),
            "candidate_status_counts": dict(Counter(f.get("status", "unknown") for f in functions)),
            "native_slot_to_rva_binding": "not-established-by-existing-ledger",
            "semantic_implementation_count": 0,
        })
    ranked = sorted(missing, key=lambda row: (-row["call_sites"], row["index"]))
    conflicts = [
        {"index": row["index"], "status": row["status"],
         "helper_constraints": row["helpers"],
         "conflicts": row["ordinary_reference_ABI_conflicts"],
         "reference_evidence": [ref for ref in row["reference_abi"] if ref["table"] == "ordinary"]}
        for row in rows if row["ordinary_reference_ABI_conflicts"]
    ]
    rejection_rows = [
        {"index": row["index"],
         "rejections": [item for item in row["observed_length_dispatch"]
                        if "rejected" in item["route_kind"]],
         "helpers": row["helpers"]}
        for row in rows if any("rejected" in item["route_kind"]
                              for item in row["observed_length_dispatch"])
    ]
    games_summary = []
    for game in audit["games"]:
        selected = [row for row in rows if any(obs["game"] == game["root"] for obs in row["observations"])]
        games_summary.append({
            "root": game["root"], "variant": game["variant"],
            "observed_numeric_slots": len(selected),
            "status_counts": dict(sorted(Counter(row["status"] for row in selected).items())),
            "unmapped_slots": [row["index"] for row in selected if row["status"] == "unmapped"],
            "ordinary_reference_ABI_conflicts": [row["index"] for row in selected if row["ordinary_reference_ABI_conflicts"]],
        })
    return {
        "schema_version": "cs2-native-coverage/1.0",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "report_date": "2026-11-16",
        "status": "static-source-and-ABI-evidence-only",
        "scope": {
            "games": len(audit["games"]), "observed_numeric_slots": len(rows),
            "distinct_explicit_rust_slots": len(explicit),
            "distinct_top_level_match_slots": len(top_level_explicit),
            "external_guard_supported_slots": len(external_indices),
            "observed_slots_with_explicit_arm": sum(row["rust_route_present"] for row in rows),
            "status_counts": dict(sorted(summary.items())),
            "semantically_verified_implementation_count": 0,
            "slots_with_ordinary_reference_ABI_conflicts": len(conflicts),
            "slots_with_source_semantic_conflicts": sum(bool(row["source_semantic_conflicts"]) for row in rows),
        },
        "sources": [{"path": name, "sha256": digest, "archive": archived_sources[name]}
                    for name, digest in hashes.items()],
        "source_evidence_storage": {
            "kind": "content-addressed-inspected-bytes",
            "directory": "archive/2026-11/source_blobs",
            "is_atomic_working_tree_snapshot": False,
            "live_files_may_continue_changing": True,
            "does_not_replace_current_source_completion_gate": True,
        },
        "reference_snapshot": {
            "documented_commit": "3184165d03ad9eac0bb6eba3792c4f2f49e07166",
            "documentation": "reports/reuse_matrix.md:11",
            "license": "LGPL-2.1",
            "verification": "per-file hashes; this script does not verify git commit identity",
            "usage": "documentation and ABI facts only; no reference implementation code copied",
        },
        "source_unchanged_during_analysis": (root / DESKTOP).read_bytes() == blobs[DESKTOP],
        "evidence_policy": {
            "original_executables_started": False, "compiled": False,
            "game_runtime_tests_run": False, "reference_code_copied": False,
            "static_source_path_is_not_behavioral_verification": True,
            "content_audit_loses_host_table_and_program_version": True,
            "counts_are_numeric_slots_not_distinct_native_ABIs": True,
            "reference_ABIs_are_not_proven_for_every_workspace_binary": True,
            "unknown_or_dynamic_calls_excluded_by_input_audit": True,
        },
        "important_corrections": [
            "The older 75-route/523-missing figure uses a line-based extractor and an older source snapshot; it omits multiline groups and includes nested matches.",
            "1063 now routes to cp932_utf8_native; its archived cold-start error is historical, not a current unmapped native.",
            "Pre-match DataServices guards and the module's supports/call intersection are included before the LoggingHost numeric match.",
            "Motion/particle/movie execution paths are inspected from the current source; historical missing-VM findings are not hardcoded.",
            "Motion and particle table numbers must never be treated as ordinary-table slots solely because their numbers match.",
            "37 version-activated slots in the reference cannot be satisfied globally by the baseline constant returns.",
        ],
        "external_dispatches": [item["evidence"] for item in overlays],
        "runtime_execution_evidence": execution_evidence,
        "direct_wiring_candidates": ready,
        "ordinary_reference_ABI_conflicts": conflicts,
        "all_observed_argument_rejections": rejection_rows,
        "source_semantic_conflicts": [
            {"index": row["index"], "evidence": row["source_semantic_conflicts"]}
            for row in rows if row["source_semantic_conflicts"]
        ],
        "reserved_alias_documentation": alias_evidence,
        "startup_evidence": startup_evidence(root, source),
        "game_summary": games_summary,
        "native_return_ABI": {
            "reference": REF + "kcs.md:177-189",
            "original_callback_convention": "x86 stdcall; four pointer/context arguments",
            "original_callback_arguments": ["VM context", "task context", "argument byte buffer", "return-value pointer"],
            "modern_host_rule": "VM addresses are u32 offsets, not native Windows process pointers",
            "status_to_variant": {
                "0": "Yield: no value; yield", "1": "Value: push DWORD; continue",
                "2": "Continue: no value; continue", "3": "YieldValue: push DWORD; yield",
                "4": "YieldVoid: no value; yield", "5": "Retry: restore args; rewind call; yield",
            },
            "per_native_parameter_types_and_effects": "documented subsets only; do not infer types/return values from TSV byte counts",
        },
        "shadowed_duplicate_arms": shadows,
        "highest_frequency_gaps": [
            {"index": row["index"], "status": row["status"],
             "call_sites": row["call_sites"], "games_count": row["games_count"],
             "argument_bytes": row["argument_bytes"],
             "reference_tables": row["reference_tables"]}
            for row in ranked
        ],
        "function_ledger_evidence": {
            "path": LEDGER, "scope": ledger.get("scope"),
            "binaries": binary_summary,
            "policy": "Function candidates and call edges do not bind native slot ABI or prove runtime implementations.",
        },
        "natives": rows,
        "recommended_next_steps": [
            {"priority": 0, "action": "Preserve (host table, program version, resource name, instruction offset) in content audits and dispatch motion/particle through their own hosts."},
            {"priority": 0, "indices": [332], "action": (
                "Correct exact byte count from 12 to the evidenced 8."
                if any(row["index"] == 332 and row["ordinary_reference_ABI_conflicts"] for row in rows)
                else "Preserve the recovered eight-byte query signature; no ordinary-reference length conflict is present for 332."
            )},
            {"priority": 0, "indices": [388, 440, 586], "action": "Review current wiring and runner execution evidence; do not repeat historical missing-VM claims."},
            {"priority": 0, "indices": [26, 64, 727], "action": (
                "Resolve documented source semantic conflicts still listed in this snapshot."
                if any(row["source_semantic_conflicts"] for row in rows)
                else "Preserve recovered numeric-prefix, integer-bit-pattern and flip-direction semantics; historical conflict patterns are absent."
            )},
            {"priority": 1, "indices": [124, 934, 647, 449, 447, 448, 94, 452], "action": "Recover parameter types and task command behavior for high-frequency ordinary gaps; do not add fixed-success placeholders."},
            {"priority": 1, "indices": [299, 301, 302, 697, 698, 700], "action": "Bind sample-only argument lengths to concrete game/runtime versions before changing validation."},
            {"priority": 1, "action": "Promote indirect table targets to new static function seeds after registration/bounds review; keep implementation status unchanged."},
        ],
    }


def markdown(report: dict) -> str:
    scope = report["scope"]
    out = [
        "# Native 实际路由、缺口与 ABI 证据（2026-11-16）", "",
        "这是静态源码与已有样本审计的交叉清单；没有启动游戏、编译或进行实机测试。",
        "有路由、helper 中有代码和经过行为验证是三个不同结论；本报告的已验证实现数为 0。",
        f"源码快照 SHA-256：`{next(item['sha256'] for item in report['sources'] if item['path'] == DESKTOP)}`。",
        "其他代理可能继续修改宿主源码；行号和结论绑定此快照，接线前应重新生成清单。",
        "",
        f"覆盖 {scope['games']} 个样本、{scope['observed_numeric_slots']} 个数字槽位；"
        f"Rust 路由（含 match 前置外部 guard）展开后有 {scope['distinct_explicit_rust_slots']} 个不同槽位，"
        f"其中 {scope['observed_slots_with_explicit_arm']} 个出现在样本审计中。",
        "",
        "|分类|数量|解释|", "|---|---:|---|",
    ]
    labels = {
        "source-behavior": "有实际源码行为；未验证完整原版语义",
        "source-semantic-conflict": "源码行为与明确的参考语义不同；尚未执行验证",
        "partial-behavior": "已确认仅有部分行为，存在执行或宿主连接缺口",
        "constant-return-only": "仅固定返回；必须核对版本是否为保留入口",
        "version-sensitive-constant": "参考明确记载版本启用，现有常量不能覆盖所有版本",
        "unmapped": "落入默认 UnimplementedNativeCall",
        "explicit-unimplemented": "显式返回 UnimplementedNativeCall",
        "helper-rejects-index": "顶层范围命中但 helper 对正常非零句柄拒绝该编号",
        "argument-rejected": "只有参数错误路径",
    }
    for status, count in scope["status_counts"].items():
        out.append(f"|`{status}`|{count}|{labels.get(status, status)}|")
    out += ["", "## 优先结论", ""]
    out += [f"- {text}" for text in report["important_corrections"]]
    out += [
        "", "## 外部模块前置路由", "",
        "|模块|guard 行|supports 行|精确支持编号|数量|",
        "|---|---:|---:|---|---:|",
    ]
    for item in report["external_dispatches"]:
        out.append(f"|{item['module_source']}|{item['guard_line']}|{item.get('supports_line', item.get('function_line'))}|"
                   f"{', '.join(map(str, item['supported_indices']))}|{item['supported_count']}|")
    out += [
        "", "外部模块须有 `supports` guard 与 `call` 转发，或已检查的可选适配器"
        "编号谓词及 `Some(NativeReturn)` 转发，才计入路由。"
        "可选适配器的任务类型条件、None 后续分派与未核对 ABI 保留在 JSON 证据中；"
        "分类继续根据具体源码分支计算。",
        "", "## 当前 VM 与媒体执行连接", "",
        "|范围|当前源码证据|已验证行为|", "|---|---|---|",
    ]
    for name, item in report["runtime_execution_evidence"].items():
        out.append(f"|{name}|{item['status']}|false|")
    out += [
        "", "## 可直接接线的已有 helper", "",
        "|编号|现状|参数字节|现有 helper 证据|",
        "|---:|---|---|---|",
    ]
    for item in report["direct_wiring_candidates"]:
        abi = item["abi"]
        out.append(f"|{item['index']}|{item['status']}|{abi['accepted_argument_bytes']}|"
                   f"`{item['helper']}`，`{DESKTOP}:{abi.get('validation_line', item['line'])}`|")
    out += [
        "", "上表反映当前接线状态；已有路由的编号无需重复接线。"
        "helper 或 runner 路径存在不表示完整行为已经验证。",
        "", "## 已定位的源码语义差异", "",
        "|编号|源码行为|参考要求|证据|",
        "|---:|---|---|---|",
    ]
    for row in report["source_semantic_conflicts"]:
        for item in row["evidence"]:
            out.append(f"|{row['index']}|{item['fact']}|{item['expected']}|"
                       f"`{item['source']}:{item['line']}`；`{item['reference']}`|")
    out += [
        "", "## 已证实的普通表参数长度冲突", "",
        "以下长度同时出现在工作区审计及参考 ordinary TSV 中；现有源码分派或 helper 会拒绝。"
        "这与低编号跨宿主表的歧义分开记录。",
        "", "|编号|样本/参考参数字节|当前 helper 约束|源码证据|",
        "|---:|---|---|---|",
    ]
    for row in report["ordinary_reference_ABI_conflicts"]:
        observed = ", ".join(str(item["argument_bytes"]) for item in row["conflicts"])
        required = ", ".join(
            str(item["accepted_argument_bytes"]) for item in row["helper_constraints"]
        )
        locations = ", ".join(
            f"{item['helper']}:{item.get('validation_line', item['line'])}"
            for item in row["helper_constraints"]
        )
        out.append(f"|{row['index']}|{observed}|{required}|{locations}|")
    out += [
        "",
        "`163` 的 `required=10` 由声音控制分支执行最小长度校验，"
        "因此样本的 12 字节参数合法；脚本明确区分最小长度和精确长度。",
        "",
        "其余样本长度拒绝（含跨表歧义与参考未覆盖的版本差异）：",
        "",
        "|编号|拒绝的参数字节|当前 helper|",
        "|---:|---|---|",
    ]
    for row in report["all_observed_argument_rejections"]:
        observed = ", ".join(str(item["argument_bytes"]) for item in row["rejections"])
        names = ", ".join(item["helper"] for item in row["helpers"])
        out.append(f"|{row['index']}|{observed}|{names}|")
    out += [
        "", "## 启动阻断证据", "",
        "这里只引用已有冷启动归档；本轮未运行启动路径，不能把静态调用频率称为启动阻断。",
        "", "|样本|历史失败|当前静态证据|", "|---|---|---|",
    ]
    for row in report["startup_evidence"]["historical_failures"]:
        diagnosis = row["diagnostic"].replace("|", "\\|").replace("\n", " ")
        conclusion = row["conclusion"]
        if "native" in row:
            conclusion += f" `{DESKTOP}:{row['current_route_line']}`"
        out.append(f"|{row['game']}|{diagnosis}|{conclusion}|")
    out += [
        "", "## 跨表 ABI 与版本风险", "",
        "原 native 回调为 x86 stdcall，四项为 VM、任务、参数字节区和返回值指针。"
        "状态 0/1/2/3/4/5 分别对应 Yield/Value/Continue/YieldValue/YieldVoid/Retry。"
        "VM 的 u32 地址是独立存储偏移；参数字节数不能推导参数类型。",
        "",
        "`7` 在参考里属于 motion 的 atan2（8 字节）和 particle 的 cos（4 字节）；"
        "当前显式未实现不能被解释成普通表缺少一项。"
        "本报告仅按真实普通宿主分派统计数字路由，同时完整保留参考宿主表身份。",
        "",
        "仅固定返回的编号分别记录普通表基线别名证据。"
        "参考 2.6.1.92 启用的 37 个槽位中，有 32 个出现在工作区数字并集中并仍被常量处理。",
        "",
        "## 按样本分类", "",
        "|样本|槽位数|无路由|helper/显式缺口|普通 ABI 冲突|",
        "|---|---:|---:|---:|---|",
    ]
    for game in report["game_summary"]:
        counts = game["status_counts"]
        missing = counts.get("explicit-unimplemented", 0) + counts.get("helper-rejects-index", 0)
        out.append(f"|{Path(game['root']).name}|{game['observed_numeric_slots']}|"
                   f"{counts.get('unmapped', 0)}|{missing}|"
                   f"{', '.join(map(str, game['ordinary_reference_ABI_conflicts']))}|")
    out += [
        "", "## 按静态调用点排序的缺口", "",
        "静态出现次数不等于启动执行次数；样本审计已合并普通、运动、粒子表，以下必须结合表身份处理。",
        "", "|编号|分类|样本数|调用点|参数字节:调用点|参考表|",
        "|---:|---|---:|---:|---|---|",
    ]
    for row in report["highest_frequency_gaps"]:
        sizes = ", ".join(f"{key}:{value}" for key, value in row["argument_bytes"].items())
        out.append(f"|{row['index']}|{row['status']}|{row['games_count']}|"
                   f"{row['call_sites']}|{sizes}|{', '.join(row['reference_tables'])}|")
    out += [
        "", "## 全部 685 个数字槽位", "",
        "JSON 保留逐游戏调用数、逐长度首个实际命中分支、源码行号、helper 参数校验、"
        "三份 TSV 的表/程序版本/参数字节，以及文档语义条目。下面为可扫描索引。",
        "", "|编号|状态|样本数|调用点|参数字节|路由行/helper|参考表|",
        "|---:|---|---:|---:|---|---|---|",
    ]
    for row in report["natives"]:
        loc = ", ".join(str(arm["line"]) for arm in row["rust_arms"])
        helper = ", ".join(item["helper"] for item in row["helpers"])
        out.append(f"|{row['index']}|{row['status']}|{row['games_count']}|{row['call_sites']}|"
                   f"{', '.join(row['argument_bytes'])}|{loc} {helper}|{', '.join(row['reference_tables'])}|")
    out += [
        "", "## 证据边界与下一步", "",
        "- 首先让内容审计保留每份 KCS 的宿主表、程序版本、文件名与调用地址；当前 685 是数字并集，不能当作 685 个独立 ABI。",
        "- 按当前源码连接检查 388/440/586、运动与粒子各自的 Host/runner；"
        "不得把历史缺少 VM 的结论硬编码到新快照。",
        "- 按函数表/引擎版本恢复启用的保留槽位，尤其 925 在参考 2.6.1.92 返回 1，当前返回 0。",
        "- 按当前影片宿主路径核对句柄、播放、停止及释放；源码中存在连接不等于已经完成播放验证。",
        "- 现有运行时函数账本只给出候选边界和调用关系，没有 ordinary/motion/particle 的槽位→RVA 绑定。先恢复表基址、别名关系和返回状态，再提升函数语义状态。",
        "- `main.kcs` 缺失等历史启动问题应保留为历史证据；源码路由的存在不能证明启动成功。", "",
        f"可复现命令：`python -B scripts/analysis/native_coverage.py`。",
        "追加 PE 新证据：`python -B scripts/analysis/native_coverage.py --pe-evidence`；"
        "输出 `reports/runtime_function_native_abi_evidence_20261116.json/md`。",
        "脚本归类到 `scripts/analysis/`，正式报告在 `reports/`；逐文件哈希证据去重保存于"
        "`archive/2026-11/source_blobs/`。源码可能继续变动，归档不是当前源码完成证明。未改译文或游戏。",
        "",
    ]
    return "\n".join(out)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, default=ROOT)
    parser.add_argument(
        "--pe-evidence", action="store_true",
        help="Also read runtime PE bytes to emit runtime_function_native_abi_evidence_20261116.json/md.",
    )
    args = parser.parse_args()
    root = args.workspace.resolve()
    report = analyze(root)
    if args.pe_evidence:
        evidence = recover_native_table_evidence(root, report)
        output = root / "reports/runtime_function_native_abi_evidence_20261116.json"
        output.write_text(json.dumps(evidence, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        lines = [
            "# 运行时 native 函数表静态新证据（2026-11-16）", "",
            "仅以已有文档的 ordinary 别名关系扫描只读 PE 数据；未执行代码，未修改原函数账本。",
            "表布局候选没有经过注册链验证，不能称为已恢复的实际 ABI 或已实现函数。",
            "",
            "|运行时|数据表候选 RVA|别名符合数/112|前1080项代码指针数|",
            "|---|---|---:|---:|",
        ]
        for binary in evidence["binaries"]:
            for candidate in binary["candidates"]:
                lines.append(f"|{binary['binary_path']}|`{candidate['table_rva_hex']}`|"
                             f"{candidate['alias_agreement']}|{candidate['code_pointer_count_in_first_1080']}|")
            if not binary["candidates"]:
                lines.append(f"|{binary['binary_path']}|{binary['status']}|—|—|")
        lines += [
            "", f"在四份候选的样本编号窗口内共发现 "
            f"{evidence['scope']['new_indirect_entry_seeds_per_binary']} 个按二进制分别计数的间接入口种子，"
            "均不在原账本的精确入口集合内。它们是下一轮静态恢复种子，不是新增已实现函数。",
            "",
            "## 925 槽位候选的直接字节证据", "",
            "|运行时|候选入口 RVA|写回 DWORD|EAX 状态|ret 清理字节|",
            "|---|---|---:|---:|---:|",
        ]
        for binary in evidence["binaries"]:
            for candidate in binary["candidates"]:
                for slot in candidate["slot_evidence"]:
                    stub = slot["decoded_constant_stub_fact"]
                    if slot["index"] == 925 and stub and "output_dword" in stub:
                        lines.append(f"|{binary['binary_path']}|`{slot['entry_rva_hex']}`|"
                                     f"{stub['output_dword']}|{stub['eax_status']}|{stub['ret_pop_bytes']}|")
        lines += [
            "",
            "这是候选表下的指令事实，普通 VM 注册链仍需确认。"
            "NEKOPARA 候选写回一，其余三份写回零，不能把常量零作为跨版本统一行为。",
            "", "JSON 保存每个样本槽位的 RVA、64 字节指纹、共享入口关系、反汇编前缀、"
            "原账本精确入口匹配与候选状态；其语义实现标志始终为 false。",
            "下一步从候选表的交叉引用恢复普通 VM 注册链，独立定位运动/粒子表，"
            "并按实际 PE 版本核对新启用槽位。", "",
        ]
        output.with_suffix(".md").write_text("\n".join(lines), encoding="utf-8")
        report["additional_runtime_function_evidence"] = {
            "json": output.relative_to(root).as_posix(),
            "markdown": output.with_suffix(".md").relative_to(root).as_posix(),
            "scope": evidence["scope"],
            "status": "static-candidates-only",
        }
        print(json.dumps({"PE_static_evidence": evidence["scope"]}, ensure_ascii=True))
    source_hash = next(item["sha256"] for item in report["sources"] if item["path"] == DESKTOP)
    report["source_snapshot_matches_at_report_write"] = (
        hashlib.sha256((root / DESKTOP).read_bytes()).hexdigest() == source_hash
    )
    (root / (REPORT + ".json")).write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    (root / (REPORT + ".md")).write_text(markdown(report), encoding="utf-8")
    print(json.dumps({"scope": report["scope"],
                      "direct_wiring_candidates": [item["index"] for item in report["direct_wiring_candidates"]],
                      "top_20_gap_indices": [
                          item["index"] for item in report["highest_frequency_gaps"][:20]
                      ],
                      "source_unchanged_during_analysis": report["source_unchanged_during_analysis"],
                      "source_snapshot_matches_at_report_write": report["source_snapshot_matches_at_report_write"]},
                     ensure_ascii=True, indent=2))


if __name__ == "__main__":
    main()
