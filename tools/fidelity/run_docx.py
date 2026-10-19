"""Phase D0 engine test for .docx: every corpus file through rdocx, Oxi and LibreOffice.

Per file and engine:
  1. layout a *blind* copy (Word's cached page breaks removed, so engines can't copy them)
     and record page count + the text each page starts with
  2. save a round-trip copy (no edits) and score what survived
  3. apply the same scripted edits (bold the first run of a paragraph, append text to it,
     insert a paragraph after it, retype the first table cell), save, check each edit landed
     and that everything else survived
Layout truth: Word's cached page count and page breaks (Word-saved files) or the known
breaks of synthetic files (corpus/docx/synthetic/expected.json).

Usage:
  python run_docx.py                                   # everything, all engines
  python run_docx.py --corpus synthetic --engines rdocx,oxi
  python run_docx.py --limit 100 --jobs 8
Results: results/docx-<timestamp>/results.jsonl, then: python report_docx.py <folder>
"""
from __future__ import annotations

import argparse
import json
import multiprocessing as mp
import os
import re
import subprocess
import sys
import time
import traceback
import zipfile
from concurrent.futures import ProcessPoolExecutor, as_completed
from multiprocessing.util import Finalize
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))

import docx_compare as dc  # noqa: E402
import docx_xml as dx  # noqa: E402
import run as xlsx_run  # noqa: E402  (LibreOffice job-server driver)

CORPUS = ROOT / "corpus" / "docx"
CORPORA = ["synthetic", "poi", "libreoffice", "python-docx", "real"]
EXTRA = ["debug"]
ENGINES = ["rdocx", "oxi", "libreoffice"]
BENCH = HERE / "docx_bench" / "target" / "release" / "docx_bench"
TIMEOUT = 180


class LibreOfficeWriter(xlsx_run.LibreOffice):
    SCRIPT = "lo_docx_runner.py"


_LO: LibreOfficeWriter | None = None


def lo() -> LibreOfficeWriter:
    global _LO
    if _LO is None:
        _LO = LibreOfficeWriter()
        Finalize(_LO, _LO.stop, kwargs={"final": True}, exitpriority=10)
    return _LO


def run_bench(engine: str, job: dict, work: Path) -> dict:
    if not BENCH.exists():
        return {"ok": False, "crash": "no_start", "error": "docx_bench not built: cargo build --release in docx_bench/"}
    jf = work / f"{engine}.job.json"
    jf.write_text(json.dumps({**job, "engine": engine.split("+")[0]}))
    try:
        p = subprocess.run([str(BENCH), str(jf)], capture_output=True, text=True, timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        return {"ok": False, "crash": "timeout", "error": f"timeout after {TIMEOUT}s"}
    res = xlsx_run._parse(p.stdout)
    if res is None:
        return {"ok": False, "crash": "crash", "error": f"crash (exit {p.returncode}): {p.stderr[-600:]}"}
    return res


def blind_copy(src: Path, dst: Path):
    """Same package with every <w:lastRenderedPageBreak/> removed from document.xml."""
    with zipfile.ZipFile(src) as zin, zipfile.ZipFile(dst, "w", zipfile.ZIP_DEFLATED) as zout:
        for info in zin.infolist():
            data = zin.read(info.filename)
            if info.filename == "word/document.xml":
                data = re.sub(rb"<w:lastRenderedPageBreak\s*/>", b"", data)
            zout.writestr(info, data)


def score(task: dict, engine: str, res: dict, truth: dict, target: dict | None, orig: dict, work: Path) -> dict:
    rec = {k: res.get(k) for k in ("ok", "error", "crash", "stages", "pages", "edit_errors")}
    rec["opened"] = bool(res.get("stages", {}).get("open", {}).get("ok"))
    starts = res.get("starts") or []
    exp_pages, exp_keys = truth.get("expected_pages"), truth.get("expected_keys")
    if exp_pages is not None and res.get("pages") is not None:
        rec["page_count_match"] = res["pages"] == exp_pages
        rec["page_count_diff"] = res["pages"] - exp_pages
    if exp_keys and starts:
        rec["page_starts"] = dc.page_match(exp_keys, starts)
    rec["starts_sample"] = [s[:40] for s in starts[:3]]
    for k in ("revisions_before", "revisions_after", "resolved"):
        if k in res:
            rec[k] = res[k]
    for kind in ("roundtrip", "edit"):
        out = work / f"{engine}.{kind}.docx"
        if not res.get("stages", {}).get(kind, {}).get("ok") or not out.exists():
            continue
        try:
            new = dx.read_parts(out)
            rec[f"{kind}_survival"] = dc.survival(orig, new, edited=(kind == "edit"))
            if kind == "edit" and engine in ("sheaf-accept", "sheaf-reject"):
                rec["revisions"] = dc.check_revisions(orig, new, engine == "sheaf-accept")
            elif kind == "edit" and engine == "sheaf-stories":
                rec["stories"] = dc.check_stories(orig, new, res.get("stories") or {})
            elif kind == "edit" and target:
                rec["edits"] = dc.check_edits(new["word/document.xml"], target)
        except Exception as e:
            rec[f"{kind}_error"] = f"{type(e).__name__}: {e}"[:300]
    return rec


def process(task: dict) -> dict:
    src = Path(task["path"])
    work = Path(task["out_dir"]) / "work" / re.sub(r"[^\w.-]", "_", task["rel"])
    work.mkdir(parents=True, exist_ok=True)
    rec: dict = {"file": task["rel"], "source": task["source"], "engines": {}}
    try:
        orig = dx.read_parts(src)
        t = dx.truth(src)
    except Exception as e:
        rec["unreadable"] = f"{type(e).__name__}: {e}"[:300]
        return rec
    exp = task.get("expected")
    if exp:  # synthetic files with known breaks
        t["expected_pages"] = exp["pages"]
        t["expected_keys"] = [dx.norm_key(s) for s in exp["starts"]]
    elif t["word_saved"] and t["pages_app"]:
        t["expected_pages"] = t["pages_app"]
        if t["trusted"]:
            t["expected_keys"] = t["starts"]
    rec["truth"] = {k: t.get(k) for k in ("application", "word_saved", "pages_app", "lrpb", "trusted", "expected_pages", "chars")}
    rec["inventory"] = dx.inventory(orig)
    target = dc.edit_target(orig["word/document.xml"])
    rec["edit_target"] = bool(target)
    blind = work / "blind.docx"
    blind_copy(src, blind)
    has_revisions = dc.revision_count(orig) > 0
    for engine in task["engines"]:
        if engine in ("sheaf-accept", "sheaf-reject") and not has_revisions:
            continue
        if engine == "sheaf-stories" and not target:
            continue
        hints = engine.endswith("+hints")  # layout of the original, Word's cached breaks included
        job = {"src": str(src), "layout_src": str(src if hints else blind),
               "roundtrip_dst": str(work / f"{engine}.roundtrip.docx"),
               "edit_dst": None if hints else str(work / f"{engine}.edit.docx"),
               "edits": None if hints else target}
        t0 = time.time()
        if engine == "libreoffice":
            res = lo().run(["docx", str(src), "", job])
        else:
            res = run_bench(engine, job, work)
        r = score(task, engine, res, t, target, orig, work)
        edited = work / f"{engine}.edit.docx"
        if engine.startswith("sheaf") and edited.exists():
            # Does LibreOffice still open what we wrote?
            rl = lo().run(["reload", str(edited), ""])
            r["lo_reload"] = bool(rl.get("ok"))
        r["seconds"] = round(time.time() - t0, 3)
        rec["engines"][engine] = r
    if not task.get("keep"):
        for f in work.glob("*"):
            f.unlink()
        work.rmdir()
    return rec


def safe_process(task):
    try:
        return process(task)
    except Exception:
        return {"file": task["rel"], "source": task["source"], "fatal": traceback.format_exc()[-1500:]}


def collect(corpora: list[str]) -> list[dict]:
    expected_path = CORPUS / "synthetic" / "expected.json"
    expected = json.loads(expected_path.read_text()) if expected_path.exists() else {}
    tasks = []
    for name in corpora:
        base = CORPUS / (name if name in ("synthetic", "real", "debug") else f"public/{name}")
        if not base.exists():
            continue
        for f in sorted(base.rglob("*")):
            if f.suffix.lower() in (".docx", ".docm") and f.is_file():
                tasks.append({"path": str(f), "source": name, "rel": f"{name}/{f.relative_to(base)}",
                              "expected": expected.get(f.name) if name == "synthetic" else None})
    return tasks


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", default=",".join(CORPORA))
    ap.add_argument("--engines", default=",".join(ENGINES + ["oxi+hints"]))
    ap.add_argument("--jobs", type=int, default=max(1, min(8, (os.cpu_count() or 4) - 2)))
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--keep", action="store_true", help="keep each engine's output files")
    ap.add_argument("--out", default="")
    args = ap.parse_args()

    engines = [e.strip() for e in args.engines.split(",") if e.strip()]
    tasks = collect([c.strip() for c in args.corpus.split(",") if c.strip()])
    if args.limit:
        tasks = tasks[: args.limit]
    out_dir = Path(args.out) if args.out else ROOT / "results" / time.strftime("docx-%Y%m%d-%H%M%S")
    out_dir.mkdir(parents=True, exist_ok=True)
    for t in tasks:
        t.update(out_dir=str(out_dir), engines=engines, keep=args.keep)
    (out_dir / "run.json").write_text(json.dumps({"engines": engines, "corpus": args.corpus, "files": len(tasks),
                                                  "started": time.ctime()}, indent=2))
    print(f"{len(tasks)} files x {engines} with {args.jobs} workers -> {out_dir}", flush=True)
    t0 = time.time()
    done = 0
    with open(out_dir / "results.jsonl", "w") as fh, \
            ProcessPoolExecutor(args.jobs, mp_context=mp.get_context("spawn")) as pool:
        for fut in as_completed([pool.submit(safe_process, t) for t in tasks]):
            fh.write(json.dumps(fut.result(), default=str) + "\n")
            fh.flush()
            done += 1
            if done % 50 == 0 or done == len(tasks):
                print(f"  {done}/{len(tasks)}  {time.time() - t0:.0f}s", flush=True)
    subprocess.run(["pkill", "-f", "fidelity_lo_"], capture_output=True)
    print(f"done in {time.time() - t0:.0f}s. Next: python report_docx.py {out_dir}")


if __name__ == "__main__":
    main()
