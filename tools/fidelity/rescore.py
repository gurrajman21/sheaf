"""Recompute round-trip fidelity for an existing run without re-running the engines.

Use after changing compare.py. Formula scores are kept (they need engine output
that isn't stored). Usage: python rescore.py results/<run>
"""
import json
import sys
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))  # -I drops the script dir
import compare  # noqa: E402
import ooxml  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
CORPUS = {"synthetic": "corpus/synthetic", "poi": "corpus/public/poi",
          "libreoffice": "corpus/public/libreoffice", "real": "corpus/real"}


def rescore(args):
    line, out_dir = args
    rec = json.loads(line)
    if "fatal" in rec or rec.get("orig_error"):
        return rec
    src = ROOT / CORPUS[rec["source"]] / rec["file"].split("/", 1)[1]
    orig = ooxml.read_package(str(src))
    script = rec.get("edit_script")
    for er in rec["engines"].values():
        if er.get("roundtrip") and "fidelity" in er:
            rt = ooxml.read_package(str(out_dir / er["roundtrip"]))
            er["fidelity"] = compare.compare_packages(orig, rt, compare.edited_cells(script))
            if script:
                er["edits"] = compare.check_edits(rt, script)
    return rec


def main():
    out_dir = Path(sys.argv[1]).resolve()
    lines = (out_dir / "results.jsonl").read_text().splitlines()
    with ProcessPoolExecutor() as pool:
        recs = list(pool.map(rescore, [(l, out_dir) for l in lines], chunksize=4))
    (out_dir / "results.jsonl").write_text("".join(json.dumps(r, default=str) + "\n" for r in recs))
    print(f"rescored {len(recs)} files")


if __name__ == "__main__":
    main()
