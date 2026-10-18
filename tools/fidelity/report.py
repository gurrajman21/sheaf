"""Summarize one or more result folders into summary.md (+ summary.json).

Usage: python report.py results/<run> [results/<other-run> ...]
Several folders are merged (e.g. an IronCalc run and a LibreOffice run); the
report is written into the first one.
"""
from __future__ import annotations

import json
import statistics
import sys
from collections import Counter, defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))  # -I drops the script dir
from compare import FEATURE_KEYS  # noqa: E402

STYLE_PARTS = ("numfmt", "font", "fill", "border", "alignment")
LAYOUT_PARTS = ("merged", "col_widths", "row_heights", "hidden_rows", "hidden_cols", "frozen", "sheet_states")


def load(dirs: list[Path]) -> dict[str, dict]:
    files: dict[str, dict] = {}
    for d in dirs:
        for line in open(d / "results.jsonl"):
            rec = json.loads(line)
            cur = files.setdefault(rec["file"], {k: v for k, v in rec.items() if k != "engines"} | {"engines": {}})
            cur["engines"].update(rec.get("engines", {}))
    return files


def pct(a, b) -> str:
    return f"{100 * a / b:.1f}%" if b else "–"


def frac(pair) -> str:
    a, b = pair
    return f"{pct(a, b)} ({a:,}/{b:,})" if b else "–"


def add(acc, pair):
    acc[0] += pair[0]
    acc[1] += pair[1]


def engine_stats(files: dict, eng: str, subset=None) -> dict:
    s = defaultdict(lambda: [0, 0])
    walls, sims = [], []
    errors = Counter()
    funcs = defaultdict(lambda: [0, 0])
    feats = defaultdict(lambda: [0, 0, 0, 0])  # files, full, partial, lost
    per_file = []
    for name, rec in files.items():
        if subset and not subset(rec):
            continue
        e = rec["engines"].get(eng)
        if e is None:
            continue
        s["files"][1] += 1
        loaded = (e.get("stages") or {}).get("load", {}).get("ok", False)
        s["loaded"] = [s["loaded"][0] + loaded, s["loaded"][1] + 1]
        s["saved"] = [s["saved"][0] + bool(e.get("roundtrip")), s["saved"][1] + 1]
        if e.get("crash"):
            s[e["crash"]][0] += 1
        if not e.get("ok"):
            errors[_norm_err(e.get("error"))] += 1
        walls.append(e.get("wall_ms", 0))
        f = e.get("formulas")
        if f and f["expected"]:
            add(s["formula_cells"], [f["matched"], f["expected"]])
            add(s["formula_files"], [int(f["matched"] == f["expected"]), 1])
            for fn, (tot, bad) in f["functions"].items():
                funcs[fn][0] += tot
                funcs[fn][1] += bad
        ed = e.get("edits")
        if ed:
            add(s["edit_ops"], [ed["ok"], ed["total"]])
            add(s["edit_files"], [int(ed["ok"] == ed["total"]), 1])
        fid = e.get("fidelity")
        score = None
        if fid:
            checks = {"values": fid["values"], "formulas_kept": fid["formulas_kept"],
                      **{"style_" + k: fid["styles"][k] for k in STYLE_PARTS},
                      **{"layout_" + k: fid["layout"][k] for k in LAYOUT_PARTS}}
            for k, (a, b) in checks.items():
                if b:
                    add(s["perfile_" + k], [int(a == b), 1])
            add(s["values"], fid["values"])
            add(s["formulas_kept"], fid["formulas_kept"])
            for k in STYLE_PARTS:
                add(s["style_" + k], fid["styles"][k])
            for k in LAYOUT_PARTS:
                add(s["layout_" + k], fid["layout"][k])
            add(s["sheet_names"], [int(fid["sheet_names_kept"]), 1])
            o, r = fid.get("integrity", {}).get("shared_formula_orphans", [0, 0])
            add(s["integrity_ok"], [int(r <= o), 1])
            add(s["defined_names"], fid["defined_names"])
            for k, (rt, orig) in fid["features"].items():
                feats[k][0] += 1
                feats[k][1 if rt >= orig else 2 if rt > 0 else 3] += 1
            parts = [fid["values"], fid["formulas_kept"], *fid["styles"].values()]
            tot = sum(p[1] for p in parts)
            score = sum(p[0] for p in parts) / tot if tot else 1.0
            lost = [k for k, (rt, o) in fid["features"].items() if rt == 0]
        else:
            lost = []
        if e.get("openpyxl_reopen") is not None:
            add(s["reopen"], [int(e["openpyxl_reopen"] is True), 1])
        v = e.get("visual") or {}
        if "similarity" in v:
            sims.append(v["similarity"])
        fscore = (f["matched"] / f["expected"]) if f and f["expected"] else None
        per_file.append({"file": name, "ok": e.get("ok"), "error": e.get("error"), "fidelity": score,
                         "formulas": fscore, "lost": lost})
    s = dict(s)
    s["wall_median"] = statistics.median(walls) if walls else 0
    s["wall_p95"] = sorted(walls)[int(len(walls) * 0.95) - 1] if walls else 0
    s["visual_median"] = statistics.median(sims) if sims else None
    s["errors"] = errors
    s["functions"] = funcs
    s["features"] = feats
    s["per_file"] = per_file
    return s


def _norm_err(err) -> str:
    if not err:
        return "(none)"
    err = str(err).split("\n")[0]
    for cut in (" at ", " in file", "/Users/"):
        if cut in err:
            err = err.split(cut)[0]
    return err[:110]


def main():
    dirs = [Path(a) for a in sys.argv[1:]]
    files = load(dirs)
    order = ["ironcalc", "ironcalc-patch", "libreoffice"]
    engines = sorted({e for r in files.values() for e in r["engines"]}, key=lambda e: (order.index(e) if e in order else 9, e))
    stats = {e: engine_stats(files, e) for e in engines}
    out = []
    w = out.append

    sources = Counter(r["source"] for r in files.values())
    origins = Counter(r.get("origin", "?") for r in files.values())
    trusted = sum(1 for r in files.values() if r.get("trust_cached"))
    w("# Phase 0: engine test results\n")
    w(f"**Files:** {len(files)} ({', '.join(f'{k} {v}' for k, v in sorted(sources.items()))})  ")
    w(f"**Written by:** {', '.join(f'{k} {v}' for k, v in sorted(origins.items()))}  ")
    w(f"**Formula ground truth:** {trusted} files (Excel-written with cached values, or synthetic with known answers)  ")
    w(f"**Engines:** {', '.join(engines)}\n")

    def row(label, fn):
        w(f"| {label} | " + " | ".join(fn(stats[e]) for e in engines) + " |")

    w("## Scorecard\n")
    w("| Metric | " + " | ".join(engines) + " |")
    w("|---|" + "---|" * len(engines))
    row("Files opened", lambda s: frac(s.get("loaded", [0, 0])))
    row("Files saved", lambda s: frac(s.get("saved", [0, 0])))
    row("Crashes / timeouts", lambda s: f"{s.get('crash', [0])[0]} / {s.get('timeout', [0])[0]}")
    if any(s.get("edit_ops") for s in stats.values()):
        row("**Edits applied correctly**", lambda s: frac(s.get("edit_ops", [0, 0])))
        row("Files with every edit correct", lambda s: frac(s.get("edit_files", [0, 0])))
    row("**Formula cells correct**", lambda s: frac(s.get("formula_cells", [0, 0])))
    row("Files with every formula correct", lambda s: frac(s.get("formula_files", [0, 0])))
    row("Cell values kept after save", lambda s: frac(s.get("values", [0, 0])))
    row("Formulas kept after save", lambda s: frac(s.get("formulas_kept", [0, 0])))
    for k in STYLE_PARTS:
        row(f"Style kept: {k}", lambda s, k=k: frac(s.get("style_" + k, [0, 0])))
    for k in LAYOUT_PARTS:
        row(f"Layout kept: {k.replace('_', ' ')}", lambda s, k=k: frac(s.get("layout_" + k, [0, 0])))
    row("Sheet names/order kept (files)", lambda s: frac(s.get("sheet_names", [0, 0])))
    row("Defined names kept", lambda s: frac(s.get("defined_names", [0, 0])))
    row("Saved file re-opens in openpyxl", lambda s: frac(s.get("reopen", [0, 0])))
    row("No broken shared formulas introduced", lambda s: frac(s.get("integrity_ok", [0, 0])))
    row("Visual similarity (median)", lambda s: f"{s['visual_median']:.1%}" if s["visual_median"] is not None else "–")
    row("Time per file (median / p95)", lambda s: f"{s['wall_median'] / 1000:.2f}s / {s['wall_p95'] / 1000:.2f}s")
    w("\n_Timing is wall-clock per file including process start-up (LibreOffice also includes UNO round-trips)._\n")

    both = lambda r: all((r["engines"].get(e) or {}).get("roundtrip") for e in engines)
    common = {e: engine_stats(files, e, both) for e in engines}
    n_common = sum(1 for r in files.values() if both(r))
    w(f"## Head-to-head on the {n_common} files every engine opened and saved\n")
    w("| Metric | " + " | ".join(engines) + " |")
    w("|---|" + "---|" * len(engines))
    def crow(label, fn):
        w(f"| {label} | " + " | ".join(fn(common[e]) for e in engines) + " |")
    if any(s.get("edit_ops") for s in common.values()):
        crow("**Edits applied correctly**", lambda s: frac(s.get("edit_ops", [0, 0])))
    crow("**Formula cells correct**", lambda s: frac(s.get("formula_cells", [0, 0])))
    crow("Files with every formula correct", lambda s: frac(s.get("formula_files", [0, 0])))
    crow("Visual similarity (median)", lambda s: f"{s['visual_median']:.1%}" if s["visual_median"] is not None else "–")
    w("")
    w("**Per file:** share of files (that have the thing) where 100% of it survived the save. "
      "Cell-weighted totals above are skewed by a few stress-test files (e.g. one with 50,000 merges).\n")
    w("| Kept 100% | " + " | ".join(engines) + " |")
    w("|---|" + "---|" * len(engines))
    for k, label in [("values", "cell values"), ("formulas_kept", "formulas"),
                     *[("style_" + p, f"style: {p}") for p in STYLE_PARTS],
                     *[("layout_" + p, f"layout: {p.replace('_', ' ')}") for p in LAYOUT_PARTS]]:
        crow(label, lambda s, k=k: frac(s.get("perfile_" + k, [0, 0])))
    w("")

    w("## Feature survival on save\n")
    w("Files containing each feature, and how many kept all of it / some of it / none of it.\n")
    w("| Feature | " + " | ".join(f"{e}: full / partial / lost" for e in engines) + " |")
    w("|---|" + "---|" * len(engines))
    for k in FEATURE_KEYS:
        cells = []
        n = 0
        for e in engines:
            f = stats[e]["features"].get(k)
            if f:
                n = max(n, f[0])
                cells.append(f"{pct(f[1], f[0])} / {pct(f[2], f[0])} / **{pct(f[3], f[0])}**")
            else:
                cells.append("–")
        if n:
            w(f"| {k.replace('_', ' ')} ({n} files) | " + " | ".join(cells) + " |")

    for e in engines:
        s = stats[e]
        w(f"\n## {e}: formula functions with the most wrong results\n")
        funcs = sorted(((fn, t, b) for fn, (t, b) in s["functions"].items() if b), key=lambda x: (-x[2], x[0]))[:30]
        if funcs:
            w("| Function | Wrong / total cells | Accuracy |")
            w("|---|---|---|")
            for fn, t, b in funcs:
                w(f"| `{fn}` | {b:,} / {t:,} | {pct(t - b, t)} |")
        else:
            w("No wrong formula results.")
        w(f"\n### {e}: most common failures\n")
        errs = [(m, c) for m, c in s["errors"].most_common(12) if m != "(none)"]
        if errs:
            w("| Count | Error |")
            w("|---|---|")
            for m, c in errs:
                w(f"| {c} | `{m.replace('|', '/')}` |")
        else:
            w("No failures.")
        w(f"\n### {e}: lowest-fidelity files\n")
        worst = sorted((p for p in s["per_file"] if p["fidelity"] is not None), key=lambda p: p["fidelity"])[:12]
        w("| File | Fidelity | Formulas | Features lost |")
        w("|---|---|---|---|")
        for p in worst:
            fz = f"{p['formulas']:.0%}" if p["formulas"] is not None else "–"
            w(f"| {p['file']} | {p['fidelity']:.1%} | {fz} | {', '.join(p['lost']) or '–'} |")

    w("\n## Breakdown by source\n")
    w("| Source | Engine | Opened | Formula cells correct | Values kept | Formulas kept | Font kept | Numfmt kept |")
    w("|---|---|---|---|---|---|---|---|")
    for src in sorted(sources):
        for e in engines:
            s = engine_stats(files, e, lambda r, src=src: r["source"] == src)
            w(f"| {src} | {e} | {frac(s.get('loaded', [0, 0]))} | {frac(s.get('formula_cells', [0, 0]))} | "
              f"{frac(s.get('values', [0, 0]))} | {frac(s.get('formulas_kept', [0, 0]))} | "
              f"{frac(s.get('style_font', [0, 0]))} | {frac(s.get('style_numfmt', [0, 0]))} |")

    synth = sorted(n for n, r in files.items() if r["source"] == "synthetic")
    if synth:
        w("\n## Synthetic feature files\n")
        w("| File | " + " | ".join(engines) + " |")
        w("|---|" + "---|" * len(engines))
        for n in synth:
            cells = []
            for e in engines:
                er = files[n]["engines"].get(e) or {}
                if not er.get("ok"):
                    cells.append(f"FAILED: {_norm_err(er.get('error'))[:60]}")
                    continue
                bits = []
                f = er.get("formulas")
                if f and f["expected"]:
                    bits.append(f"formulas {f['matched']}/{f['expected']}")
                fid = er.get("fidelity") or {}
                lost = [k for k, (rt, o) in fid.get("features", {}).items() if rt < o]
                styles = fid.get("styles", {})
                weak = [k for k, (a, b) in styles.items() if b and a / b < 0.98]
                lay = [k for k, (a, b) in fid.get("layout", {}).items() if b and a < b]
                if lost:
                    bits.append("lost/partial: " + ", ".join(lost))
                if weak:
                    bits.append("style drift: " + ", ".join(weak))
                if lay:
                    bits.append("layout drift: " + ", ".join(lay))
                cells.append("; ".join(bits) or "✓ all checks kept")
            w(f"| {n.split('/', 1)[1]} | " + " | ".join(cells) + " |")

    md = "\n".join(out) + "\n"
    (dirs[0] / "summary.md").write_text(md)
    slim = {e: {k: v for k, v in s.items() if k not in ("per_file",)} for e, s in stats.items()}
    (dirs[0] / "summary.json").write_text(json.dumps(slim, indent=1, default=lambda o: dict(o)))
    print(md)


if __name__ == "__main__":
    main()
