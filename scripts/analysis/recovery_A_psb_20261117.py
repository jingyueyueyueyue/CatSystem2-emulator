"""Independent bounded PSB structural probe, derived from format facts.

FreeMote is research-only CC-BY-NC-SA; no source code copied. Primary MIT
psbfile source/actual license pinned in archive/2026-11/20261117_psbfile_*.
Only metadata summaries and selected sample structures are exported.
"""
from __future__ import annotations

import argparse
from collections import Counter
from datetime import datetime, timezone
import json
from pathlib import Path
import struct
import zlib

from recovery_A_20261116 import ROOT, sha
from audit_runtime_evidence import kif_entries, blowfish_le_decrypt

MASK = 0xFFFFFFFF


def header_bytes(data):
    version = struct.unpack_from("<H", data, 4)[0]
    size = {2:40, 3:44, 4:56}.get(version)
    if size is None:
        raise ValueError("PSB version")
    encrypted = struct.unpack_from("<I", data, 8)[0]
    if encrypted == size:
        return data, None
    # First keystream word is known from the validated header-length field.
    a = (123456789 ^ (123456789 << 11)) & MASK
    y = (encrypted ^ size ^ a ^ (a >> 8)) & MASK
    key = y ^ (y >> 19)
    states, current = [123456789, 362436069, 521288629, key], 0
    out = bytearray(data)
    for off in range(8, size):
        if current == 0:
            a = (states[0] ^ (states[0] << 11)) & MASK
            b = states[3]
            current = (a ^ b ^ ((a ^ (b >> 11)) >> 8)) & MASK
            states = [states[1], states[2], b, current]
        out[off] ^= current & 255
        current >>= 8
    header = list(struct.unpack_from("<" + "I"*((size-8)//4), out, 8))
    assert header[0] == size and header[1] == size
    assert all(0 <= x <= len(data) for x in header[1:8])
    if version >= 3:
        checksum = zlib.adler32(out[8:40]+(out[44:56] if version == 4 else b"")) & MASK
        assert header[8] == checksum, (hex(header[8]), hex(checksum))
    return bytes(out), key


class Psb:
    def __init__(self, data):
        self.data, self.key = header_bytes(data)
        h = struct.unpack_from("<12I", self.data, 8)
        self.h = h
        base, pos = self.array(h[1])
        parent, pos = self.array(pos)
        indexes, pos = self.array(pos)
        self.names = []
        for index in indexes:
            cursor, text, seen = parent[index], [], set()
            while cursor:
                if cursor in seen or len(text) > 65536:
                    raise ValueError("name trie cycle")
                seen.add(cursor)
                previous = parent[cursor]
                text.append((cursor-base[previous]) & 255)
                cursor = previous
            self.names.append(bytes(reversed(text)).decode("utf-8"))
        offsets, _ = self.array(h[2])
        self.strings = []
        for off in offsets:
            start = h[3]+off
            end = self.data.index(0, start)
            self.strings.append(self.data[start:end].decode("utf-8"))
        self.offsets, _ = self.array(h[4])
        self.lengths, _ = self.array(h[5])
        self.extra_offsets, _ = self.array(h[9])
        self.extra_lengths, _ = self.array(h[10])
        self.nodes = 0
        self.active = set()
        self.root = self.value(h[7])

    def uint(self, pos, size):
        if not 0 <= pos <= len(self.data)-size:
            raise ValueError("integer bounds")
        return int.from_bytes(self.data[pos:pos+size], "little")

    def array(self, pos):
        size = self.data[pos]-12
        if not 1 <= size <= 4:
            raise ValueError(("array tag", pos, self.data[pos]))
        count = self.uint(pos+1, size)
        if count > 4_000_000:
            raise ValueError("array budget")
        pos += 1+size
        width = self.data[pos]-12
        pos += 1
        if not 1 <= width <= 4 or count*width > len(self.data)-pos:
            raise ValueError("array width/bounds")
        return [self.uint(pos+n*width, width) for n in range(count)], pos+count*width

    def value(self, pos, depth=0):
        self.nodes += 1
        if self.nodes > 2_000_000 or depth > 128 or pos in self.active:
            raise ValueError("object budget/cycle")
        original = pos
        typ = self.data[pos]
        pos += 1
        if typ in (0,1):return None
        if typ in (2,3):return typ == 3
        if 4 <= typ <= 12:
            count = typ-4
            return int.from_bytes(self.data[pos:pos+count], "little", signed=True) if count else 0
        if 13 <= typ <= 20:return self.array(pos-1)[0]
        if 21 <= typ <= 24:return self.strings[self.uint(pos, typ-20)]
        if 25 <= typ <= 28:return {"$chunk":self.uint(pos,typ-24)}
        if typ == 29:return 0.0
        if typ == 30:return struct.unpack_from("<f",self.data,pos)[0]
        if typ == 31:return struct.unpack_from("<d",self.data,pos)[0]
        if 34 <= typ <= 37:return {"$extra":self.uint(pos,typ-33)}
        self.active.add(pos-1)
        if typ == 32:
            offsets, base = self.array(pos)
            value = [self.value(base+off,depth+1) for off in offsets]
        elif typ == 33:
            names, pos = self.array(pos)
            offsets, base = self.array(pos)
            assert len(names)==len(offsets)
            value = {self.names[name]: self.value(base+off,depth+1) for name,off in zip(names,offsets)}
        else:raise ValueError(("unknown tag",typ,pos-1))
        self.active.remove(original)
        return value

    def chunk(self, value):
        extra = "$extra" in value
        index = value["$extra" if extra else "$chunk"]
        offs = self.extra_offsets if extra else self.offsets
        lengths = self.extra_lengths if extra else self.lengths
        base = self.h[11] if extra else self.h[6]
        start, size = base+offs[index], lengths[index]
        assert start+size <= len(self.data)
        return self.data[start:start+size]


def trimmed(value, depth=0):
    if depth > 9:return {"$truncated":type(value).__name__}
    if isinstance(value,dict):return {k:trimmed(v,depth+1) for k,v in value.items()}
    if isinstance(value,list):return [trimmed(x,depth+1) for x in value[:5]]+[{"$remaining":len(value)-5}] if len(value)>5 else [trimmed(x,depth+1) for x in value]
    return value


def load_sample(index=0):
    profile = json.loads((ROOT/"reports/recovery_A_20261116_feature_profiles.json").read_text(encoding="utf-8"))
    p = next(p for p in profile["profiles"] if "NEKOPARA" in p["binary_paths"][0])
    archive = ROOT/Path(p["binary_paths"][0]).parent/"psz.int"
    entries, _, cipher = kif_entries(archive,0x0F6BB0AE)
    e = entries[index]
    with archive.open("rb") as h:
        h.seek(e["offset"]); data=h.read(e["size"])
    data=blowfish_le_decrypt(cipher,data)
    return e["name"].decode("cp932"), zlib.decompress(data[16:])


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("--index",type=int,default=0)
    parser.add_argument("--report",action="store_true")
    args=parser.parse_args()
    name,data=load_sample(args.index)
    psb=Psb(data)
    out={"resource":name,"PSB_sha256":sha(data),"key":psb.key,"header":list(psb.h),
         "names":psb.names,"string_count":len(psb.strings),"chunk_lengths":psb.lengths,
         "extra_chunk_lengths":psb.extra_lengths,"nodes":psb.nodes,
         "root_structure":trimmed(psb.root)}
    path=ROOT/f"archive/2026-11/20261117_psb_probe_{args.index}.json"
    path.write_text(json.dumps(out,ensure_ascii=False,indent=2)+"\n",encoding="utf-8")
    print("resource",name,"key",hex(psb.key or 0),"nodes",psb.nodes,"root",list(psb.root))
    print("source",str(trimmed(psb.root.get("source")))[:1700])
    print("object",str(trimmed(psb.root.get("object")))[:2500])
    print("metadata",str(trimmed(psb.root.get("metadata")))[:1800])
    if args.report:
        write_backend_report(name,data,psb)


def write_backend_report(name,data,psb):
    sources=[
        "simulator/crates/runtime/src/emote.rs",
        "simulator/crates/runtime/src/emote_psb.rs",
        "simulator/crates/runtime/src/emote_cpu.rs",
        "simulator/crates/desktop/src/emote_native.rs",
        "scripts/analysis/recovery_A_psb_20261117.py",
        "scripts/analysis/recovery_A_psb_render_20261117.py",
    ]
    previews=[]
    for suffix in ["neutral","head_ud20"]:
        path=ROOT/f"archive/2026-11/20261117_psb_pixels_0_{suffix}.json"
        if path.is_file():
            info=json.loads(path.read_text(encoding="utf-8"))
            info["evidence_JSON"]=str(path.relative_to(ROOT))
            previews.append(info)
    license_files=[
        "resources/reference/FreeMote/LICENSE",
        "resources/reference/FreeMote/FreeMote/FreeMote.LICENSE.txt",
        "archive/2026-11/20261117_psbfile_LICENSE",
        "archive/2026-11/20261117_psbfile_src_psb.cpp",
    ]
    objects=psb.root["object"]
    motion_count=sum(len(o["motion"]) for o in objects.values())
    parsed={
        "resource":name,"PSB_sha256":sha(data),"version":4,
        "validated_header_key":psb.key,"header_fields":list(psb.h),
        "header_adler32_verified":True,"value_nodes":psb.nodes,
        "object_count":len(objects),"motion_count":motion_count,
        "chunk_bytes":sum(psb.lengths),"texture_platform":psb.root["spec"],
        "texture_formats":[source["texture"]["type"] for source in psb.root["source"].values()
                           if "texture"in source],
    }
    limitations=[
        "No Rust compilation, cargo tests, native execution, game or DLL/JS loading.",
        "Observed pixels come from the independent Python analysis renderer, not execution of Rust source.",
        "The default NEKO model preview is incomplete: face/mask portions missing.",
        "Strict Rust render_rgba rejects all warnings before desktop pixel submission.",
        "Diagnostic previews skip unsupported stencil/composite and some layer types; never count as successful native draw.",
        "Cubic bp deformation, linear type3 interpolation and transform composition are provisional until original-frame comparison.",
        "Skin cc coefficients,3D perspective,non-normal blending,color multipliers,physics,automatic eye/lip and general easing are incomplete.",
        "Most36 complex natives remain ABI-planned but CPU backend returns explicit Unsupported. Full load-spec resolution and native manager semantics are incomplete.",
        "PSB parser accepts canonical2/3/4 headers and first-word key recovery only if section checks and checksums pass; generalized shells/body encryption are not claimed.",
    ]
    payload={
        "schema":"cs2-recovery-A-psb-cpu-batch/v1",
        "task_date":"2026-11-17",
        "generated_at_utc":datetime.now(timezone.utc).isoformat(),
        "status":"independent-parser-and-partial-CPU-backend-source",
        "format_sources":{
            "FreeMote":{"commit":"d87be1f3b8458c81822825fd6d56123d4df860b4",
                        "license":"CC-BY-NC-SA-4.0; research-only; no implementation copied"},
            "psbfile":{"commit":"1f7a6f14a698f160b99198d985f9fa0938c5b910",
                       "repository":"number201724/psbfile","license":"actual root MIT license archived",
                       "use":"format facts and original-byte crosscheck; implementation independently written"},
            "license_evidence":[{"path":p,"sha256":sha((ROOT/p).read_bytes())}
                                for p in license_files],
        },
        "source_files":[{"path":p,"sha256":sha((ROOT/p).read_bytes())} for p in sources],
        "resource_observation":parsed,"diagnostic_previews":previews,
        "APIs":{
            "PSZ":"EmoteResource::decode",
            "PSB":"PsbDocument::parse/from_resource/resource",
            "model":"CpuMotionModel::load/motions",
            "player":"CpuPlayer::new/select_motion/set_variable/render_preview/render_strict",
            "backend":"CpuEmoteBackend::new/set_resources/player/player_mut + EmoteBackend implementation",
            "archive_load":"EmoteArchiveResources implements CpuEmoteResources through ResourceIndex::read_category (host-configured category)",
            "native_bridge":"EmoteNativeServices::call/call_batch/draw; requires real task resolution",
            "pixels":"desktop emote_native::submit_rgba clips/blends actual backend RGBA",
        },
        "pipeline_implemented_in_source":[
            "PSZ bounded zlib; PSB canonical header key recovery and Adler32",
            "name trie,string tables,normal/extra resource chunk bounds,typed recursive objects",
            "RGBA8 win BGRA/common RGBA and pixel-aligned RL decoding",
            "motion.layer preorder flattening separated from priority draw order",
            "frameList sample,parameter range->frame mapping,object/motion references",
            "hierarchical affine transforms,icon atlas rectangles,normalized bp lattice deformation",
            "triangle rasterization,bilinear texture sample,alpha compositing,real RGBA",
            "strict diagnostic refusal instead of missing-feature silent success",
        ],
        "limitations":limitations,"NEKO_compatibility_claim":False,
        "integrated_completed_native_count":0,
    }
    prefix=ROOT/"reports/recovery_A_20261117_psb_backend"
    prefix.with_suffix(".json").write_text(json.dumps(payload,ensure_ascii=False,indent=2)+"\n",
                                          encoding="utf-8")
    lines=["# A: PSB objects / partial CPU backend batch","",
           "Generated "+payload["generated_at_utc"]+".",
           "No compilation or tests; no original game, native DLL, SDK or JS execution.","",
           "## Concrete deliverables",""]
    lines += ["- "+p for p in sources]
    lines += ["","## Available APIs","",
              "| Layer | API |","|---|---|"]
    lines += [f"| {k} | `{v}` |" for k,v in payload["APIs"].items()]
    lines += ["","runtime/emote.rs includes the two sibling modules by path; runtime/lib.rs and desktop/lib.rs are left for the parent.",
              "The runtime uses its existing flate2 dependency. No FreeMote code, SDK/native library or game asset is included in production sources.",
              "EmoteArchiveResources reads actual bytes through ResourceIndex::read_category with the host's configured category. Empty,nonzero-mode and compound colon/comma load specs remain explicit diagnostics.","",
              "## Real resource observation","",
              f"- `{name}`: PSB4; header key`0x{psb.key:08x}` independently inferred and Adler verified.",
              f"- {psb.nodes} typed values, {len(objects)} objects, {motion_count} motions; {sum(psb.lengths)} bytes of texture chunk data.",
              "- RGBA8 texture4096x4096, platformwin. The original contains real bp mesh lattices, parameterized frame lists,priority maps and reference motions.","",
              "## Offline diagnostic pixel observations","",
              "| Preview | Sprites / triangles | Nonzero-alpha pixels | Pixel SHA256 |",
              "|---|---:|---:|---|"]
    for p in previews:
        lines.append(f"| `{p['preview_PNG']}` | {p['counts'].get('sprites',0)} / {p['counts'].get('triangles',0)} | "
                     f"{p['nonzero_alpha_pixels']} | `{p['pixel_sha256']}` |")
    lines += ["","The second preview changes head_UD/move_UD. Pixels change through real frame sampling and mesh/transform traversal, not a fixed portrait.",
              "These are Python analysis observations. Rust source was not compiled/executed. The partial character preview has missing face/mask parts and is not a compatibility result.","",
              "## Unresolved / strict behavior",""]
    lines += ["- "+text for text in limitations]
    lines += ["","## License/source boundary","",
              "FreeMote fixed commitd87be1f3b8458c81822825fd6d56123d4df860b4 has rootCC-BY-NC-SA4.0. Psb.cs mentions only some MIT-derived format portions; no whole-file MIT assumption or source copying.",
              "Original number201724/psbfile commit1f7a6f14a698f160b99198d985f9fa0938c5b910 has a real MIT root license saved under archive/2026-11/20261117_psbfile_LICENSE. Only format facts and byte-level behavior informed this independent implementation.","",
              "## Classification / archive","",
              "Functional scripts remain scripts/analysis; durable evidence reports/recovery_A_20261117_psb_backend.*. Download manifests/license/format snapshots,structural probes and diagnostic PNG/JSON go directly to archive/2026-11/20261117_*.",
              "No root README,localization,host constructors,game backup or runtime/lib.rs edit in this batch."]
    prefix.with_suffix(".md").write_text("\n".join(lines)+"\n",encoding="utf-8")
    prior=ROOT/"reports/recovery_A_20261116_emote.md"
    if prior.is_file():
        text=prior.read_text(encoding="utf-8").split("\n## PSB backend follow-up")[0]
        text+="\n## PSB backend follow-up\n\n"
        text+="The encrypted-header barrier in this historical snapshot has been resolved for the first real resource through independent key recovery,Adler validation and full object parsing. See reports/recovery_A_20261117_psb_backend.md/json for current PSB/CPU source and actual diagnostic pixel observations. The full native manager,stencil,physics and complete NEKO rendering remain unresolved; no compatibility claim.\n"
        prior.write_text(text,encoding="utf-8")


if __name__=="__main__":main()
