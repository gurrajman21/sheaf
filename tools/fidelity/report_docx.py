"""Scorecard for a run_docx.py run: results/<run>/summary.md

Usage: python report_docx.py ../../results/docx-<timestamp>
"""
from __future__ import annotations

import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path

FEATURE_LABELS = {
    "tbl": "tables", "drawing": "pictures/drawings", "pict": "VML pictures", "txbxContent": "text boxes",
    "sdt": "content controls", "fldChar": "fields", "fldSimple": "simple fields", "hyperlink": "hyperlinks",
    "bookmarkStart": "bookmarks", "footnoteReference": "footnotes", "endnoteReference": "endnotes",
    "commentReference": "comments", "ins": "tracked insertions", "del": "tracked deletions",
    "moveFrom": "tracked moves", "rPrChange": "tracked formatting", "pPrChange": "tracked para formatting",
    "sectPr": "sections", "oMath": "equations", "object": "OLE objects", "AlternateContent": "alternate content",
    "numPr": "list paragraphs", "customXml": "custom XML", "smartTag": "smart tags",
    "part:header": "header parts", "part:footer": "footer parts", "part:footnotes": "footnotes part",
    "part:endnotes": "endnotes part", "part:comments": "comments part", "part:commentsExtended": "comment threads",
    "part:numbering": "numbering part", "part:styles": "styles part", "part:theme": "theme",
    "part:media": "media files", "part:embeddings": "embedded files", "part:charts": "charts",
    "part:glossary": "building blocks", "part:customXml": "custom XML parts", "part:vba": "VBA macros",
    "part:people": "people (comment authors)", "part:diagrams": "SmartArt",
}


def pct(n, d):
    return f"{100 * n / d:.1f}%" if d else "n/a"


def main(run_dir: Path):
    recs = [json.loads(l) for l in (run_dir / "results.jsonl").read_text().splitlines() if l.strip()]
    meta = json.loads((run_dir / "run.json").read_text())
    engines = meta["engines"]
    fatal = [r for r in recs if "fatal" in r or "unreadable" in r]
    recs = [r for r in recs if "truth" in r]
    L = [f"# .docx engine scorecard", "", f"Run: `{run_dir.name}` — {len(recs)} files"
         f" ({', '.join(f'{s}: {sum(1 for r in recs if r['source'] == s)}' for s in sorted({r['source'] for r in recs}))})."
         f" Unreadable/harness errors: {len(fatal)}.", ""]
    L += ["Layout is measured on a *blind* copy of each file (Word's cached `lastRenderedPageBreak` markers removed). "
          "`oxi+hints` is Oxi laying out the original file, which uses those markers.", ""]

    def row(label, fn):
        return f"| {label} | " + " | ".join(fn(e) for e in engines) + " |"

    def E(r, e):
        return r["engines"].get(e) or {}

    L += ["## Summary", "", "| | " + " | ".join(engines) + " |", "|---|" + "---|" * len(engines)]
    L.append(row("Files opened", lambda e: pct(sum(E(r, e).get("opened", False) for r in recs), len(recs))))
    L.append(row("Crashes / timeouts", lambda e: f"{sum(E(r, e).get('crash') == 'crash' for r in recs)} / {sum(E(r, e).get('crash') == 'timeout' for r in recs)}"))

    def pc(subset, tol=0):
        def f(e):
            rs = [E(r, e) for r in subset if "page_count_diff" in E(r, e)]
            return f"{pct(sum(abs(x['page_count_diff']) <= tol for x in rs), len(rs))} ({len(rs)})"
        return f
    word = [r for r in recs if r["truth"].get("expected_pages")]
    multi = [r for r in word if r["truth"]["expected_pages"] > 1]
    trusted = [r for r in recs if r["truth"].get("trusted") or (r["source"] == "synthetic" and r["truth"].get("expected_pages"))]
    L.append(row("Page count = Word, all files with a known count", pc(word)))
    L.append(row("Page count = Word, multi-page files", pc(multi)))
    L.append(row("Page count within ±1, multi-page files", pc(multi, 1)))

    def ps(e):
        m = c = files = exact = 0
        for r in trusted:
            x = E(r, e).get("page_starts")
            if x and x["checked"]:
                m += x["matched"]; c += x["checked"]; files += 1; exact += x["matched"] == x["checked"]
        return f"{pct(m, c)} of {c} pages; {pct(exact, files)} of {files} files all match"
    L.append(row("Page starts = Word (trusted files)", ps))

    def surv(kind, what):
        def f(e):
            rs = [E(r, e)[f"{kind}_survival"] for r in recs if E(r, e).get(f"{kind}_survival")]
            if what == "body":
                return pct(sum(s["body_kept"] == s["body_total"] for s in rs), len(rs)) + f" ({len(rs)})"
            if what == "elements":
                return pct(sum(s["body_kept"] for s in rs), sum(s["body_total"] for s in rs))
            if what == "text":
                return pct(sum(bool(s["text_equal"]) for s in rs), len(rs))
            if what == "parts":
                ok = sum(s["parts"]["identical"] + s["parts"]["equivalent"] for s in rs)
                tot = ok + sum(len(s["parts"]["changed"]) + len(s["parts"]["missing"]) for s in rs)
                return pct(ok, tot)
            if what == "missing":
                return pct(sum(not s["parts"]["missing"] for s in rs), len(rs))
            if what == "features":
                return pct(sum(all(v["new"] >= v["orig"] for v in s["features"].values()) for s in rs), len(rs))
            if what == "rels":
                return pct(sum(not s["rels_lost"] for s in rs), len(rs))
        return f
    L.append(row("Round trip: files with every body element unchanged", surv("roundtrip", "body")))
    L.append(row("Round trip: body elements unchanged", surv("roundtrip", "elements")))
    L.append(row("Round trip: text identical", surv("roundtrip", "text")))
    L.append(row("Round trip: other parts unchanged", surv("roundtrip", "parts")))
    L.append(row("Round trip: files with no part dropped", surv("roundtrip", "missing")))
    L.append(row("Round trip: files with every feature kept", surv("roundtrip", "features")))
    L.append(row("Round trip: files with no relationship lost", surv("roundtrip", "rels")))

    def edits(key=None):
        def f(e):
            rs = [r for r in recs if r.get("edit_target") and not e.endswith("+hints")]
            if not rs:
                return "–"
            if key:
                vals = [E(r, e).get("edits", {}).get(key) for r in rs]
                vals = [v for v in vals if v is not None or key != "cell"]
                rel = [r for r in rs if key != "cell" or "cell" in E(r, e).get("edits", {}) or r["inventory"].get("tbl")]
                return pct(sum(bool(E(r, e).get("edits", {}).get(key)) for r in rel), len(rel))
            return pct(sum(bool(E(r, e).get("edits")) and all(E(r, e)["edits"].values()) for r in rs), len(rs)) + f" ({len(rs)})"
        return f
    L.append(row("Edits: files with every edit applied", edits()))
    for k, label in [("bold", "bold first run"), ("append", "append text"), ("new_paragraph", "insert paragraph"), ("cell", "retype table cell")]:
        L.append(row(f"Edits: {label}", edits(k)))
    L.append(row("After edit: files with every other body element unchanged", surv("edit", "body")))
    L.append(row("After edit: body elements unchanged", surv("edit", "elements")))
    L.append(row("After edit: other parts unchanged", surv("edit", "parts")))
    def reload(e):
        rs = [E(r, e) for r in recs if E(r, e).get("stages", {}).get("edit", {}).get("ok")]
        if not e.startswith("sheaf") or not rs:
            return "–"
        bad = sum(any("reload" in x for x in (v.get("edit_errors") or [])) for v in rs)
        lo_bad = sum(v.get("lo_reload") is False for v in rs)
        return f"{pct(len(rs) - bad, len(rs))} rdocx+Oxi; {pct(len(rs) - lo_bad, len(rs))} LibreOffice"
    L.append(row("Edited file reopens", reload))
    L.append(row("After edit: files with every feature kept", surv("edit", "features")))

    def med(stage):
        def f(e):
            def ms(st):
                # LibreOffice lays out while loading, so its layout time includes that load.
                extra = st.get("layout_open", {}).get("ms", 0) if stage == "layout" else 0
                return st[stage]["ms"] + extra
            v = [ms(E(r, e)["stages"]) for r in recs if (E(r, e).get("stages") or {}).get(stage, {}).get("ok")]
            return f"{statistics.median(v):.0f} ms" if v else "–"
        return f
    L.append(row("Median open", med("open")))
    L.append(row("Median layout", med("layout")))
    L.append(row("Median save (round trip)", med("roundtrip")))
    L.append("")

    # Tracked changes and stories (Sheaf's writer).
    special = [e for e in engines if e in ("sheaf-accept", "sheaf-reject", "sheaf-stories")]
    if special:
        L += ["## Tracked changes and stories", ""]
        for e in ("sheaf-accept", "sheaf-reject"):
            if e not in engines:
                continue
            rs = [E(r, e) for r in recs if E(r, e)]
            ok = [x for x in rs if x.get("revisions")]
            L.append(f"**{e}** on {len(rs)} files with tracked changes ({sum(x.get('revisions_before', 0) for x in rs)} changes):")
            L.append(f"- saved: {pct(len(ok), len(rs))}; no tracked change left: {pct(sum(x['revisions']['left'] == 0 for x in ok), len(ok))}; "
                     f"text matches Word's {'No Markup' if e == 'sheaf-accept' else 'Original'} view: {pct(sum(x['revisions']['text_ok'] for x in ok), len(ok))}")
            L.append(f"- reopens: rdocx+Oxi {pct(sum(not any('reload' in y for y in (x.get('edit_errors') or [])) for x in ok), len(ok))}, "
                     f"LibreOffice {pct(sum(x.get('lo_reload') is not False for x in ok), len(ok))}")
            L.append("")
        if "sheaf-stories" in engines:
            rs = [E(r, "sheaf-stories") for r in recs if E(r, "sheaf-stories")]
            ok = [x for x in rs if x.get("stories")]
            L.append(f"**sheaf-stories** on {len(rs)} files: header text, new footnote, new numbered list, inline picture.")
            for k in ("header", "footnote", "list", "picture"):
                vals = [x["stories"][k] for x in ok if k in x["stories"]]
                L.append(f"- {k}: {pct(sum(vals), len(vals))} of {len(vals)}")
            L.append(f"- every other body element unchanged: {pct(sum(1 for x in ok if (s := x.get('edit_survival')) and s['body_kept'] == s['body_total']), len(ok))}")
            L.append(f"- reopens: rdocx+Oxi {pct(sum(not any('reload' in y for y in (x.get('edit_errors') or [])) for x in ok), len(ok))}, "
                     f"LibreOffice {pct(sum(x.get('lo_reload') is not False for x in ok), len(ok))}")
            L.append("")

    # Per source breakdown of the headline numbers.
    L += ["## By source", ""]
    for src in sorted({r["source"] for r in recs}):
        sub = [r for r in recs if r["source"] == src]
        w = [r for r in sub if r["truth"].get("expected_pages") and r["truth"]["expected_pages"] > 1]
        L += [f"### {src} ({len(sub)} files, {len(w)} multi-page with known count)", "", "| | " + " | ".join(engines) + " |", "|---|" + "---|" * len(engines)]
        L.append(row("Opened", lambda e: pct(sum(E(r, e).get("opened", False) for r in sub), len(sub))))
        L.append(row("Page count = Word (multi-page)", pc(w)))
        L.append(row("Round trip: every body element unchanged", lambda e: pct(
            sum(1 for r in sub if (s := E(r, e).get("roundtrip_survival")) and s["body_kept"] == s["body_total"]),
            sum(1 for r in sub if E(r, e).get("roundtrip_survival")))))
        L.append("")

    # Feature survival on round trip.
    L += ["## Feature survival (round trip)", "",
          "Files where the saved copy has at least as many of the feature as the original.", "",
          "| Feature | files | " + " | ".join(engines) + " |", "|---|---|" + "---|" * len(engines)]
    feats = defaultdict(lambda: defaultdict(lambda: [0, 0]))
    for r in recs:
        for e in engines:
            s = E(r, e).get("roundtrip_survival")
            if not s:
                continue
            for k, v in s["features"].items():
                feats[k][e][1] += 1
                feats[k][e][0] += v["new"] >= v["orig"]
    order = sorted(feats, key=lambda k: -max(x[1] for x in feats[k].values()))
    for k in order:
        n = max(x[1] for x in feats[k].values())
        L.append(f"| {FEATURE_LABELS.get(k, k)} | {n} | " + " | ".join(pct(*feats[k][e]) if feats[k][e][1] else "–" for e in engines) + " |")
    L.append("")

    # Most common errors.
    L += ["## Most common errors", ""]
    for e in engines:
        errs = defaultdict(int)
        for r in recs:
            x = E(r, e)
            if x and not x.get("ok"):
                errs[str(x.get("error"))[:90]] += 1
        if errs:
            L.append(f"**{e}**")
            L += [f"- {n} × `{msg}`" for msg, n in sorted(errs.items(), key=lambda t: -t[1])[:6]]
            L.append("")
    if fatal:
        L += ["## Harness errors", ""] + [f"- {r['file']}: {(r.get('fatal') or r.get('unreadable'))[-160:]}" for r in fatal[:15]]
    out = run_dir / "summary.md"
    out.write_text("\n".join(L) + "\n")
    print(out)


if __name__ == "__main__":
    main(Path(sys.argv[1]))
