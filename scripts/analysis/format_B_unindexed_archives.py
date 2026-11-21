"""Bounded audit follow-up: three named files and one patch DLL, read-only.

Inspects directory/header bytes and disassembles fixed loader ranges. Does not
decode images, execute target code, or rescan game/PE inventories.
"""
from pathlib import Path
import hashlib
import json
import struct

import capstone
import pefile

ROOT = Path(__file__).resolve().parents[2]
AUDIT = ROOT / "reports/all_sample_content_audit.json"
OUT = ROOT / "archive/2026-11/20261117_format_B_unindexed_archives.json"
PACK_SHA = "45e3f1840c6ef15826d85446f6f6f8142be3fd52ddc5c1e87389f625973162a3"
DLL_SHA = "782674f1eac817b1842f6d0f0d416c17e86fadfb8fb640d335a1da3ce6cabb27"


def sha(data):
    return hashlib.sha256(data).hexdigest()


def main():
    audit_bytes = AUDIT.read_bytes()
    audit = json.loads(audit_bytes)
    result = {
        "audit": AUDIT.relative_to(ROOT).as_posix(), "audit_sha256": sha(audit_bytes),
        "compiled": False, "type_checked": False, "tests_run": False,
        "target_code_executed": False, "image_decoders_executed": False,
        "archive_totals": {key: sum(g["archives"][key] for g in audit["games"])
            for key in ("total", "indexed", "non_kif", "malformed", "encrypted_without_seed")},
        "files": [],
        "hg_cst_old_diagnostics": [],
    }
    for game in audit["games"]:
        if game["archives"]["non_kif"]:
            root = Path(game["root"])
            path = root / ("cnimage.int" if root.name == "ISLAND" else "fcxzzd.int")
            raw = path.read_bytes()
            row = {"path": path.relative_to(ROOT).as_posix(), "size": len(raw),
                   "sha256": sha(raw), "head24": raw[:24].hex(),
                   "original_audit_status": "NotAnArchive/non_kif", "original_audit_error": None}
            if root.name != "ISLAND":
                text = raw[:4096].decode("cp936")
                row.update({"classification": "non-resource CP936 prose",
                    "heading": text.splitlines()[0],
                    "nonempty_lines": [x for x in text.splitlines()[1:] if x][:4]})
            else:
                if sha(raw) != PACK_SHA:
                    raise ValueError("cnimage.int identity changed")
                table_at, table_size = struct.unpack_from("<II", raw, 16)
                count, remainder = divmod(table_size, 20)
                records = [struct.unpack_from("<5I", raw, table_at + 20*i) for i in range(count)]
                row.update({"classification": "runtime translation PNG archive FUYINPAK",
                    "directory_offset": table_at, "directory_size": table_size,
                    "directory_remainder": remainder, "entry_count": count,
                    "directory_ends_at_eof": table_at + table_size == len(raw),
                    "payload_end": max(x[2]+x[3] for x in records),
                    "names_start": min(x[0] for x in records),
                    "unequal_size_entries": sum(x[3] != x[4] for x in records),
                    "entries": []})
                for name_at, name_size, offset, stored, auxiliary in records:
                    name = raw[name_at:name_at+name_size]
                    payload = raw[offset:offset+stored]
                    row["entries"].append({"name_hex": name.hex(),
                        "name": name.rstrip(b"\0").decode("ascii"),
                        "name_offset": name_at, "name_size": name_size,
                        "offset": offset, "stored": stored, "auxiliary_size_field": auxiliary,
                        "payload_sha256": sha(payload), "png_header": payload[:8].hex(),
                        "ihdr_width_height": struct.unpack_from(">II", payload, 16)})
                dll = (root / "Accelerator.dll").read_bytes()
                if sha(dll) != DLL_SHA:
                    raise ValueError("Accelerator.dll identity changed")
                pe = pefile.PE(data=dll, fast_load=True)
                md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
                routines = {}
                for label, address, size in (
                    ("directory_loader", 0x100093d0, 0x1d0),
                    ("decoded_bgra_lookup", 0x10008150, 0x228),
                    ("attach_loads_cnimage", 0x10008470, 0xa0),
                ):
                    routines[label] = [{"va": hex(i.address), "bytes": i.bytes.hex(),
                        "mnemonic": i.mnemonic, "operands": i.op_str}
                        for i in md.disasm(pe.get_data(address-pe.OPTIONAL_HEADER.ImageBase, size), address)]
                row["runtime_source"] = {"path": (root/"Accelerator.dll").relative_to(ROOT).as_posix(),
                    "sha256": sha(dll), "image_base": hex(pe.OPTIONAL_HEADER.ImageBase),
                    "routines": routines}
            result["files"].append(row)
        grouped = {}
        for diagnostic in game["diagnostics"]:
            if diagnostic["resource"].lower().endswith((".cst", ".hg2", ".hg3")):
                grouped.setdefault(diagnostic["message"], []).append(diagnostic["resource"])
        if grouped:
            result["hg_cst_old_diagnostics"].append({"root": Path(game["root"]).relative_to(ROOT).as_posix(),
                "groups": [{"error": error, "count": len(names), "first": names[:2]}
                           for error, names in grouped.items()]})
    OUT.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"output": OUT.relative_to(ROOT).as_posix(), "files": len(result["files"]),
                      "entries": sum(x.get("entry_count", 0) for x in result["files"])}))


if __name__ == "__main__":
    main()
