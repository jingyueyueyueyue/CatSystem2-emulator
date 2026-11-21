"""Record fixed select FES and its real motion producers, without executing KCS."""
from pathlib import Path
import hashlib
import json
import struct
import zlib

ROOT = Path(__file__).resolve().parents[2]
source = "cs2_full_v401/system/fes/select.fes"
raw = (ROOT / source).read_bytes()
digest = hashlib.sha256(raw).hexdigest()
if digest != "4174bcc532c687f9547269dd0081acef2e3ad06b370e5ffa60b210874c186646":
    raise ValueError("fixed select FES changed")
if raw[:4] != b"FES\0":
    raise ValueError("not FES")
packed, expanded = struct.unpack_from("<II", raw, 4)
body = zlib.decompress(raw[16:16 + packed])
if len(body) != expanded:
    raise ValueError("FES expansion differs")
calls, declarations, bindings = [], [], []
verbs = {b"base", b"rot", b"mrot", b"blend", b"fade", b"getsize", b"pos", b"str", b"draw"}
for number, line in enumerate(body.splitlines(), 1):
    words = line.split(b"//", 1)[0].split()
    if not words:
        continue
    row = {"line": number, "text": line.decode("ascii", "backslashreplace")}
    if words[0] == b"STRING":
        declarations.append(row)
    if words[0] == b"oneshot" and len(words) > 2 and words[1] in verbs:
        bindings.append(row)
    if words[0].split(b"[", 1)[0] == b"str" and len(words) > 1 and words[1] in verbs:
        calls.append(row)

contracts = json.loads((ROOT / "reports/css_task_576_F_contracts_20261117.json").read_text(encoding="utf-8"))
legacy = contracts["legacy_material_receiver"]
audit = json.loads((ROOT / "reports/all_sample_content_audit.json").read_text(encoding="utf-8"))
legacy_root = Path(legacy["path"]).parent
game = next(row for row in audit["games"]
            if Path(row["root"]).resolve() == (ROOT / legacy_root).resolve())
if game["archives"]["total"] != game["archives"]["indexed"]:
    raise ValueError("legacy inventory has unindexed archives")
if any(game["archives"]["entries_by_extension"].get(extension, 0)
       for extension in ("dae", "kxs", "shd", "mdl")):
    raise ValueError("legacy inventory now has actual model resources")
# Keep the already indexed inventory proof; do not rescan all games/resources.
legacy_proof = {"sha256": "72c9c1e7335a92bc2f1c731275a8da34d638d1730b9d5003fb6a8e85fe5b15c8",
                "path": legacy["path"], "audit": "reports/all_sample_content_audit.json",
                "model_extensions": ["dae", "kxs", "shd", "mdl"],
                "actual_model_resources": 0,
                "inventory": game["archives"]}
result = {"mode": "fixed-resource-static-read-only", "games": 1, "resources": 1,
          "source": source, "sha256": digest, "expanded_sha256": hashlib.sha256(body).hexdigest(),
          "declarations": declarations, "bindings": bindings, "calls": calls,
          "rotation_instructions": "archive/2026-11/20261117_kcs_f_select_prot_static.json",
          "legacy_no_model_producer": legacy_proof,
          "compiled": False, "type_checked": False, "tests_run": False, "vm_executed": False}
target = ROOT / "reports/fes_text_motion_F_evidence_20261117.json"
target.write_text(json.dumps(result, ensure_ascii=True, indent=2) + "\n", encoding="utf-8")
print(json.dumps({"report": target.relative_to(ROOT).as_posix(),
                  "games": 1, "resources": 1, "actual_calls": len(calls)}))
