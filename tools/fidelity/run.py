"""Phase 0 engine test: run every corpus file through each engine and score it.

For each file and engine:
  1. load -> recalculate -> read formula results -> save a round-trip copy
  2. formula score: recalculated values vs. the values Excel cached in the file
     (only for files Excel wrote, or synthetic files with known answers)
  3. fidelity: original vs. round-trip, compared at the XML level
     (cell values, formulas, styles, layout, charts/pivots/CF/...)
  4. optional (--visual): both rendered to PDF by LibreOffice and pixel-diffed

Usage:
  python run.py                                  # all corpora, both engines
  python run.py --corpus synthetic --engines ironcalc
  python run.py --limit 50 --jobs 6 --visual
Results: results/<timestamp>/results.jsonl, then run report.py on that folder.
"""
from __future__ import annotations

import argparse
import json
import multiprocessing as mp
import os
import shutil
import subprocess
import sys
import time
import traceback
import uuid
from concurrent.futures import ProcessPoolExecutor, as_completed
from multiprocessing.util import Finalize
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))

import compare  # noqa: E402
import ooxml  # noqa: E402

LO_HOME = Path(os.environ.get("LO_HOME", "/Applications/LibreOffice.app/Contents"))
SOFFICE = LO_HOME / "MacOS" / "soffice"
CORPORA = ["synthetic", "poi", "libreoffice", "real"]
TIMEOUT = {"ironcalc": 120, "libreoffice": 180}
ENGINES = ["ironcalc", "ironcalc-patch", "libreoffice"]


# --------------------------------------------------------------------------- engines

def _parse(proc_out: str) -> dict | None:
    for line in reversed(proc_out.strip().splitlines()):
        if line.startswith("{"):
            try:
                return json.loads(line)
            except json.JSONDecodeError:
                return None
    return None


RUNNER = ROOT / "target" / "release" / "ooxml_runner"


def run_ironcalc(src: Path, dst: Path, mode: str, edits: Path | None) -> dict:
    """IronCalc via the Rust runner: `native` = IronCalc's own writer, `patch` = patch-in-place."""
    if not RUNNER.exists():
        return {"ok": False, "error": "ooxml_runner not built: run `cargo build --release`", "crash": "no_start"}
    cmd = [str(RUNNER), str(src), str(dst), "--mode", mode] + (["--edits", str(edits)] if edits else []) \
        + (["--reload-check"] if mode == "patch" else [])
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=TIMEOUT["ironcalc"])
    except subprocess.TimeoutExpired:
        return {"ok": False, "error": f"timeout after {TIMEOUT['ironcalc']}s", "crash": "timeout"}
    res = _parse(p.stdout)
    if res is None:
        return {"ok": False, "error": f"crash (exit {p.returncode}): {p.stderr[-600:]}", "crash": "crash"}
    return res


class LibreOffice:
    """One headless soffice per worker process, with its own throwaway profile.

    LibreOffice's bundled Python may only be launched by LibreOffice itself (macOS
    launch constraint), so engines/lo_runner.py runs as an in-process macro that
    serves jobs from a folder. See lo_runner.py for the protocol.
    """
    SCRIPT = "lo_runner.py"

    def __init__(self):
        self.MACRO = f"vnd.sun.star.script:{self.SCRIPT}$serve?language=Python&location=user"
        self.name = f"fidelity_lo_{os.getpid()}_{uuid.uuid4().hex[:6]}"
        self.profile = HERE / ".lo-profile" / self.name
        self.jobs = self.profile / "jobs"
        self.proc = None
        self.prewarmed = False
        self.broken: str | None = None

    def _cmd(self, *extra):
        return [str(SOFFICE), "--headless", "--invisible", "--nologo", "--norestore", "--nodefault",
                "--nolockcheck", f"-env:UserInstallation={self.profile.as_uri()}", *extra]

    def start(self):
        if not self.prewarmed:  # a fresh profile is initialised (and may restart soffice) on first launch
            self.profile.mkdir(parents=True, exist_ok=True)
            subprocess.run(self._cmd("--terminate_after_init"), capture_output=True, timeout=180)
            self.prewarmed = True
        scripts = self.profile / "user" / "Scripts" / "python"
        scripts.mkdir(parents=True, exist_ok=True)
        shutil.copy2(HERE / "engines" / self.SCRIPT, scripts / self.SCRIPT)
        shutil.rmtree(self.jobs, ignore_errors=True)
        self.jobs.mkdir(parents=True)
        self.proc = subprocess.Popen(self._cmd(self.MACRO), env={**os.environ, "FIDELITY_JOBS": str(self.jobs)},
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        deadline = time.time() + 90
        while time.time() < deadline:
            if (self.jobs / "ready").exists():
                return
            if self.proc.poll() is not None:
                raise RuntimeError(f"soffice exited ({self.proc.returncode}) before the runner macro started")
            time.sleep(0.2)
        raise RuntimeError("runner macro did not start within 90s")

    def stop(self, final: bool = False):
        if self.proc and self.proc.poll() is None:
            self.proc.kill()  # the macro owns soffice's main thread, so SIGTERM wouldn't be handled
            self.proc.wait(10)
        self.proc = None
        if final:
            shutil.rmtree(self.profile, ignore_errors=True)

    def _call(self, args, timeout) -> dict:
        jid = uuid.uuid4().hex
        spec = {"id": jid, "mode": args[0], "src": args[1] if len(args) > 1 else None,
                "dst": args[2] if len(args) > 2 else None, "edits": args[3] if len(args) > 3 else None}
        tmp = self.jobs / f"{jid}.tmp-job"
        tmp.write_text(json.dumps(spec))
        tmp.rename(self.jobs / f"{jid}.job")
        result = self.jobs / f"{jid}.json"
        deadline = time.time() + timeout
        while time.time() < deadline:
            if result.exists():
                data = json.loads(result.read_text())
                result.unlink()
                return data
            if self.proc.poll() is not None:
                return {"ok": False, "crash": "crash", "error": f"soffice crashed (exit {self.proc.returncode})"}
            time.sleep(0.02)
        return {"ok": False, "crash": "timeout", "error": f"timeout after {timeout}s"}

    def run(self, args) -> dict:
        if self.broken:
            return {"ok": False, "crash": "no_start", "error": self.broken}
        if self.proc is None or self.proc.poll() is not None:
            self.stop()
            try:
                self.start()
            except (RuntimeError, subprocess.TimeoutExpired) as e:
                self.broken = str(e)  # don't spend 90s per file retrying a dead install
                return {"ok": False, "crash": "no_start", "error": self.broken}
        res = self._call(args, TIMEOUT["libreoffice"])
        if res.get("crash"):  # hang or crash: restart soffice for the next file
            self.stop()
        return res


_LO: LibreOffice | None = None


def lo() -> LibreOffice:
    global _LO
    if _LO is None:
        _LO = LibreOffice()
        Finalize(_LO, _LO.stop, kwargs={"final": True}, exitpriority=10)  # runs when the pool worker exits
    return _LO


# --------------------------------------------------------------------------- visual

def visual_similarity(pdf_a: Path, pdf_b: Path, max_pages: int = 5) -> dict:
    import fitz  # pymupdf
    from PIL import Image, ImageChops

    def pages(p):
        out = []
        with fitz.open(p) as doc:
            for i, page in enumerate(doc):
                if i >= max_pages:
                    break
                pix = page.get_pixmap(dpi=40, colorspace=fitz.csGRAY)
                out.append(Image.frombytes("L", (pix.width, pix.height), pix.samples))
            return out, doc.page_count

    a, na = pages(pdf_a)
    b, nb = pages(pdf_b)
    sims = []
    for i in range(max(len(a), len(b))):
        if i >= len(a) or i >= len(b):
            sims.append(0.0)
            continue
        pb = b[i].resize(a[i].size) if b[i].size != a[i].size else b[i]
        diff = ImageChops.difference(a[i], pb).point(lambda v: 255 if v > 40 else 0)
        sims.append(1 - diff.histogram()[255] / (a[i].size[0] * a[i].size[1]))
    return {"pages": [na, nb], "similarity": round(sum(sims) / len(sims), 4) if sims else 1.0}


# --------------------------------------------------------------------------- per file

def process(task: dict) -> dict:
    src = Path(task["path"])
    out_dir = Path(task["out_dir"])
    rec = {"file": task["rel"], "source": task["source"], "size": src.stat().st_size, "engines": {}}
    orig = None
    try:
        orig = ooxml.read_package(str(src))
        rec["origin"] = "synthetic" if task["source"] == "synthetic" else ooxml.origin(orig)
        rec["features"] = orig.inventory
    except Exception as e:
        rec["origin"] = "unknown"
        rec["orig_error"] = f"{type(e).__name__}: {e}"[:400]
    meta = task.get("meta") or {}
    trust = meta.get("trust_cached", rec.get("origin") == "excel")
    rec["trust_cached"] = trust

    script = compare.make_edits(orig, task.get("insert_rows", False)) if task["edits"] and orig is not None else None
    edits_path = None
    if script:
        rec["edit_script"] = script
        edits_path = out_dir / "edits" / (task["rel"].replace("/", "__") + ".json")
        edits_path.parent.mkdir(parents=True, exist_ok=True)
        edits_path.write_text(json.dumps(script["edits"]))
    exclude = compare.edited_cells(script)

    orig_pdf = None
    if task["visual"]:
        orig_pdf = out_dir / "pdf" / "original" / (task["rel"].replace("/", "__") + ".pdf")
        orig_pdf.parent.mkdir(parents=True, exist_ok=True)
        r = lo().run(["render", str(src), str(orig_pdf)])
        if not r.get("ok"):
            orig_pdf = None

    for eng in task["engines"]:
        ext = src.suffix.lower() if eng == "libreoffice" else ".xlsx"
        dst = out_dir / eng / (task["rel"].replace("/", "__").rsplit(".", 1)[0] + ext)
        dst.parent.mkdir(parents=True, exist_ok=True)
        dst.unlink(missing_ok=True)
        t = time.perf_counter()
        if eng == "libreoffice":
            res = lo().run(["roundtrip", str(src), str(dst), script["edits"] if script else None])
        else:
            res = run_ironcalc(src, dst, "patch" if eng == "ironcalc-patch" else "native", edits_path)
        er = {k: res.get(k) for k in ("ok", "error", "crash", "stages", "version", "patch", "edit_errors", "reload")}
        er["wall_ms"] = round((time.perf_counter() - t) * 1000)
        values = res.get("values") or {}
        er["formula_cells_reported"] = len(values)
        # after edits, cached values are no longer ground truth
        if orig is not None and trust and not script and res.get("stages", {}).get("read", {}).get("ok"):
            er["formulas"] = compare.score_formulas(orig, values, set(meta.get("skip_cells", [])))
        if dst.exists():
            er["roundtrip"] = str(dst.relative_to(out_dir))
            try:
                rt = ooxml.read_package(str(dst))
                if orig is not None:
                    er["fidelity"] = compare.compare_packages(orig, rt, exclude, compare.row_shifts(script))
                if script:
                    er["edits"] = compare.check_edits(rt, script)
            except Exception as e:
                er["roundtrip_error"] = f"{type(e).__name__}: {e}"[:400]
            er["openpyxl_reopen"] = _openpyxl_ok(dst)
            if orig_pdf is not None:
                pdf = out_dir / "pdf" / eng / (dst.name + ".pdf")
                pdf.parent.mkdir(parents=True, exist_ok=True)
                r = lo().run(["render", str(dst), str(pdf)])
                er["visual"] = visual_similarity(orig_pdf, pdf) if r.get("ok") else {"error": r.get("error")}
        rec["engines"][eng] = er
    return rec


def _openpyxl_ok(path: Path):
    import warnings
    import openpyxl
    try:
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            wb = openpyxl.load_workbook(path, read_only=True)
            for ws in wb.worksheets:
                for _ in ws.iter_rows(values_only=True):
                    pass
            wb.close()
        return True
    except Exception as e:
        return f"{type(e).__name__}: {e}"[:300]


def safe_process(task):
    try:
        return process(task)
    except Exception:
        return {"file": task["rel"], "source": task["source"], "fatal": traceback.format_exc()[-1500:]}


# --------------------------------------------------------------------------- main

def collect(corpora: list[str]) -> list[dict]:
    tasks = []
    manifest_path = ROOT / "corpus" / "synthetic" / "manifest.json"
    manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else {}
    for name in corpora:
        base = ROOT / "corpus" / ("synthetic" if name == "synthetic" else "real" if name == "real" else f"public/{name}")
        for f in sorted(base.rglob("*")):
            if f.suffix.lower() in (".xlsx", ".xlsm") and f.is_file() and ".src" not in f.parts:
                tasks.append({"path": str(f), "source": name, "rel": f"{name}/{f.relative_to(base)}",
                              "meta": manifest.get(f.name) if name == "synthetic" else None})
    return tasks


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", default=",".join(CORPORA))
    ap.add_argument("--engines", default=",".join(ENGINES))
    ap.add_argument("--jobs", type=int, default=max(1, min(6, (os.cpu_count() or 4) - 2)))
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--visual", action="store_true", help="render PDFs with LibreOffice and pixel-diff")
    ap.add_argument("--edits", action="store_true", help="apply the same scripted edits through every engine")
    ap.add_argument("--insert-rows", action="store_true", help="with --edits: start the script with a row insert")
    ap.add_argument("--out", default="")
    args = ap.parse_args()

    engines = [e.strip() for e in args.engines.split(",") if e.strip()]
    tasks = collect([c.strip() for c in args.corpus.split(",") if c.strip()])
    if args.limit:
        tasks = tasks[: args.limit]
    out_dir = Path(args.out) if args.out else ROOT / "results" / time.strftime("%Y%m%d-%H%M%S")
    out_dir.mkdir(parents=True, exist_ok=True)
    for t in tasks:
        t.update(out_dir=str(out_dir), engines=engines, visual=args.visual, edits=args.edits,
                 insert_rows=args.insert_rows)
    (out_dir / "run.json").write_text(json.dumps({"engines": engines, "corpus": args.corpus, "files": len(tasks),
                                                  "visual": args.visual, "edits": args.edits,
                                                  "insert_rows": args.insert_rows, "started": time.ctime()}, indent=2))
    print(f"{len(tasks)} files x {engines} with {args.jobs} workers -> {out_dir}", flush=True)

    t0 = time.time()
    done = 0
    with open(out_dir / "results.jsonl", "w") as fh, \
            ProcessPoolExecutor(args.jobs, mp_context=mp.get_context("spawn")) as pool:
        futures = [pool.submit(safe_process, t) for t in tasks]
        for fut in as_completed(futures):
            rec = fut.result()
            fh.write(json.dumps(rec, default=str) + "\n")
            fh.flush()
            done += 1
            if done % 25 == 0 or done == len(tasks):
                print(f"  {done}/{len(tasks)}  {time.time() - t0:.0f}s", flush=True)
    subprocess.run(["pkill", "-f", "fidelity_lo_"], capture_output=True)  # stray soffice safety net
    print(f"done in {time.time() - t0:.0f}s. Next: python report.py {out_dir}")


if __name__ == "__main__":
    main()
