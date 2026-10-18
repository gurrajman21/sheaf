"""Score an engine's run of one file: formula correctness and round-trip fidelity."""
from __future__ import annotations

import math
import re
from collections import Counter

import ooxml
from ooxml import Package, num_to_col

VOLATILE = re.compile(r"\b(NOW|TODAY|RAND|RANDBETWEEN|RANDARRAY|CELL|INFO)\s*\(", re.I)
FUNC = re.compile(r"(?:_xlfn\.|_xlws\.|_xludf\.)*([A-Z][A-Z0-9.]*)\s*\(", re.I)
SAMPLES = 25

# inventory keys judged as "features" (cells/sheets are scored separately)
FEATURE_KEYS = [
    "charts", "chartsheets", "images", "drawings", "pivot_tables", "pivot_caches", "tables",
    "comments", "threaded_comments", "vba_project", "external_links", "slicers", "timelines",
    "form_controls", "activex", "connections", "query_tables", "cf_rules", "data_validations",
    "hyperlinks", "autofilters", "sheet_protection", "workbook_protection", "sparklines",
    "page_breaks", "header_footer", "print_settings_names", "tab_colors", "outline_settings",
    "array_formulas", "defined_names", "hidden_sheets", "frozen_panes", "merged_ranges",
]


def functions_in(formula: str) -> set[str]:
    return {m.upper() for m in FUNC.findall(formula or "")}


def same_value(exp: tuple, got: dict) -> bool:
    kind, v = exp
    t, g = got.get("t"), got.get("v")
    empty = t == "empty" or (t == "s" and g == "")
    if kind == "n":
        if t == "n" and isinstance(g, (int, float)):
            return math.isclose(v, g, rel_tol=1e-7, abs_tol=1e-9)
        if t == "b":
            return float(bool(g)) == v
        return empty and v == 0  # Excel caches 0 for formulas pointing at empty cells
    if kind == "s":
        return (t == "s" and v == g) or (empty and v == "")
    if kind == "b":
        if t == "b":
            return bool(g) == v
        return t == "n" and isinstance(g, (int, float)) and float(v) == g  # LibreOffice has no bool type
    if kind == "e":
        return t == "e" and str(g).strip().upper() == v.upper()
    return False


def score_formulas(orig: Package, values: dict, skip: set[str]) -> dict:
    res = {"expected": 0, "matched": 0, "mismatched": 0, "missing": 0, "skipped": 0,
           "samples": [], "functions": {}}
    per_func: dict[str, list[int]] = {}
    for sh in orig.sheets:
        for (r, c), cell in sh.cells.items():
            if cell.formula is None or cell.value is None:
                continue
            ref = f"{sh.name}!{num_to_col(c)}{r}"
            # A cached #NAME? means the app that saved the file lacked the function: not ground truth.
            if ref in skip or VOLATILE.search(cell.formula) or cell.value[0] in ("d", "?") \
                    or cell.value == ("e", "#NAME?"):
                res["skipped"] += 1
                continue
            res["expected"] += 1
            funcs = functions_in(cell.formula)
            got = values.get(ref)
            if got is None:
                res["missing"] += 1
                status = "missing"
            elif same_value(cell.value, got):
                res["matched"] += 1
                status = "ok"
            else:
                res["mismatched"] += 1
                status = "wrong"
            for fn in funcs or {"(no function)"}:
                pf = per_func.setdefault(fn, [0, 0])
                pf[0] += 1
                pf[1] += status != "ok"
            if status != "ok" and len(res["samples"]) < SAMPLES:
                res["samples"].append({"cell": ref, "formula": cell.formula[:200], "status": status,
                                       "expected": list(cell.value), "got": got})
    res["functions"] = per_func
    return res


def _ratio(kept: int, total: int) -> list[int]:
    return [kept, total]


def make_edits(pkg: Package) -> dict | None:
    """A deterministic edit script for one file: what a user might do in a quick session.

    Changes a number, a text cell (preferring one below the header row) and a cell's
    font, then adds a formula that depends on the changed number and a new text
    cell below the data. Cells inside merged ranges are avoided.
    """
    for sh in pkg.sheets:
        if sh.state != "visible":
            continue
        covered = set()
        for ref in sh.merged:
            if ":" in ref:
                (r1, c1), (r2, c2) = (ooxml.parse_ref(x) for x in ref.split(":"))
                covered |= {(r, c) for r in range(r1, r2 + 1) for c in range(c1, c2 + 1)} - {(r1, c1)}
        consts = sorted(p for p, c in sh.cells.items()
                        if c.formula is None and c.value is not None and p not in covered)
        if not consts:
            continue
        val = lambda p: sh.cells[p].value
        num = next((p for p in consts if val(p)[0] == "n"), None)
        texts = [p for p in consts if val(p)[0] == "s" and p != num]
        txt = next((p for p in texts if p[0] > 1), texts[0] if texts else None)
        bold = next((p for p in consts if p not in (num, txt)), None)
        new_row = max(r for r, _ in sh.cells) + 2
        overwrite = _formula_to_overwrite(pkg)
        ref = lambda p: f"{num_to_col(p[1])}{p[0]}"
        edits = []
        if num:
            edits.append({"op": "number", "cell": ref(num), "value": 4242.25})
        if txt:
            edits.append({"op": "text", "cell": ref(txt), "value": "edited ✓ 編集"})
        if bold:
            edits.append({"op": "bold", "cell": ref(bold), "was": list(val(bold))})
        edits.append({"op": "formula", "cell": f"A{new_row}", "value": f"={ref(num)}*2" if num else "=1+1",
                      "expected": 8484.5 if num else 2})
        edits.append({"op": "text", "cell": f"B{new_row}", "value": "new cell"})
        for e in edits:
            e["sheet"] = sh.name
        if overwrite:
            # typing over a formula; prefers a shared-formula master, the riskiest case
            edits.append({"op": "number", "sheet": overwrite[0], "cell": overwrite[1], "value": 77.0})
        return {"sheet": sh.name, "edits": edits}
    return None


def _formula_to_overwrite(pkg: Package) -> tuple[str, str] | None:
    fallback = None
    for sh in pkg.sheets:
        if sh.state != "visible":
            continue
        for (r, c), cell in sorted(sh.cells.items()):
            if cell.formula is None or cell.ftype in ("array", "dataTable"):
                continue
            if cell.ftype == "shared" and cell.formula.strip():
                return sh.name, f"{num_to_col(c)}{r}"
            if fallback is None and cell.formula.strip():
                fallback = (sh.name, f"{num_to_col(c)}{r}")
    return fallback


def edited_cells(script: dict | None) -> set:
    if not script:
        return set()
    return {(e["sheet"], *ooxml.parse_ref(e["cell"])) for e in script["edits"]}


def check_edits(rt: Package, script: dict) -> dict:
    ok, fails = 0, []
    for e in script["edits"]:
        sh = rt.sheet(e["sheet"])
        cell = sh.cells.get(ooxml.parse_ref(e["cell"])) if sh else None
        op = e["op"]
        if cell is None:
            good = False
        elif op == "number":
            good = cell.formula is None and cell.value == ("n", e["value"])
        elif op == "text":
            good = cell.formula is None and cell.value == ("s", e["value"])
        elif op == "bold":
            good = dict(rt.styles.signature(cell.style)["font"]).get("b") is True and \
                cell.value is not None and list(cell.value) == e["was"]
        else:  # formula: same formula and a correct cached value
            norm = lambda f: (f or "").replace(" ", "").lstrip("=").upper()
            good = cell.formula is not None and norm(cell.formula) == norm(e["value"]) and \
                cell.value is not None and cell.value[0] == "n" and math.isclose(cell.value[1], e["expected"])
        ok += good
        if not good:
            fails.append({"op": op, "cell": e["cell"],
                          "got": [cell.formula, list(cell.value) if cell.value else None] if cell else None})
    return {"ok": ok, "total": len(script["edits"]), "failures": fails}


def compare_packages(orig: Package, rt: Package, exclude: set | None = None) -> dict:
    exclude = exclude or set()
    values = [0, 0]
    formulas = [0, 0]
    style_parts = ("numfmt", "font", "fill", "border", "alignment")
    styles = {k: [0, 0] for k in style_parts}
    style_diffs: Counter = Counter()
    value_samples = []
    sig_o: dict[int, dict] = {}
    sig_r: dict[int, dict] = {}
    layout = {k: [0, 0] for k in ("merged", "col_widths", "row_heights", "hidden_rows", "hidden_cols",
                                 "frozen", "sheet_states")}

    for sh in orig.sheets:
        rs = rt.sheet(sh.name)
        rcells = rs.cells if rs else {}
        for pos, cell in sh.cells.items():
            if (sh.name, *pos) in exclude:
                continue
            rc = rcells.get(pos)
            if cell.formula is None and cell.value is not None and not sh.in_array_output(pos):
                values[1] += 1
                ok = _kept_constant(cell.value, rc)
                values[0] += ok
                if not ok and len(value_samples) < SAMPLES:
                    value_samples.append({"cell": f"{sh.name}!{num_to_col(pos[1])}{pos[0]}",
                                          "expected": list(cell.value),
                                          "got": list(rc.value) if rc and rc.value else None})
            if cell.formula is not None:
                formulas[1] += 1
                formulas[0] += rc is not None and rc.formula is not None
            if cell.value is None and cell.formula is None:
                continue  # styled-but-empty cells: skip, writers legitimately drop/merge them
            so = sig_o.setdefault(cell.style, orig.styles.signature(cell.style))
            rstyle = rc.style if rc else 0
            sr = sig_r.setdefault(rstyle, rt.styles.signature(rstyle))
            for k in style_parts:
                styles[k][1] += 1
                if _same_style(k, so[k], sr[k]):
                    styles[k][0] += 1
                else:
                    style_diffs[(k, str(so[k])[:120], str(sr[k])[:120])] += 1

        # layout
        rm = rs.merged if rs else set()
        layout["merged"] = _add(layout["merged"], len(sh.merged & rm), len(sh.merged))
        rw = rs.col_widths if rs else {}
        layout["col_widths"] = _add(layout["col_widths"],
                                    sum(1 for c, w in sh.col_widths.items() if c in rw and abs(rw[c] - w) <= 0.5),
                                    len(sh.col_widths))
        rh = rs.row_heights if rs else {}
        layout["row_heights"] = _add(layout["row_heights"],
                                     sum(1 for r, h in sh.row_heights.items() if r in rh and abs(rh[r] - h) <= 0.5),
                                     len(sh.row_heights))
        layout["hidden_rows"] = _add(layout["hidden_rows"], len(sh.hidden_rows & (rs.hidden_rows if rs else set())),
                                     len(sh.hidden_rows))
        layout["hidden_cols"] = _add(layout["hidden_cols"], len(sh.hidden_cols & (rs.hidden_cols if rs else set())),
                                     len(sh.hidden_cols))
        if sh.frozen:
            layout["frozen"] = _add(layout["frozen"], int(rs is not None and rs.frozen == sh.frozen), 1)
        layout["sheet_states"] = _add(layout["sheet_states"], int(rs is not None and rs.state == sh.state), 1)

    features = {}
    for k in FEATURE_KEYS:
        o = orig.inventory.get(k, 0)
        if o:
            features[k] = [rt.inventory.get(k, 0), o]

    return {
        "values": values,
        "value_samples": value_samples,
        "formulas_kept": formulas,
        "styles": styles,
        "style_diffs": [{"part": k, "orig": a, "rt": b, "cells": n} for (k, a, b), n in style_diffs.most_common(10)],
        "layout": layout,
        "sheet_names_kept": [s.name for s in orig.sheets] == [s.name for s in rt.sheets],
        "defined_names": [len(orig.defined_names & rt.defined_names), len(orig.defined_names)],
        "features": features,
        "integrity": {"shared_formula_orphans": [orig.inventory.get("shared_formula_orphans", 0),
                                                 rt.inventory.get("shared_formula_orphans", 0)]},
    }


def _add(acc, kept, total):
    return [acc[0] + kept, acc[1] + total]


_BOOL_FORMULA = {"TRUE()", "FALSE()", "TRUE", "FALSE"}
_DATE_BUILTINS = {"mm-dd-yy", "d-mmm-yy", "d-mmm", "mmm-yy", "m/d/yy h:mm"}  # locale-dependent ids 14-17, 22


def _kept_constant(value: tuple, rc) -> bool:
    if rc is None:
        return value == ("s", "")  # dropping an empty-string cell changes nothing visible
    if rc.formula is not None:
        # LibreOffice writes boolean constants as TRUE()/FALSE() formulas; same value, same display
        return value[0] == "b" and rc.formula.strip().upper() in _BOOL_FORMULA and \
            rc.value is not None and bool(rc.value[1]) == value[1]
    return _same_constant(value, rc.value)


def _date_tokens(code: str) -> str:
    return "".join(sorted(set(ch for ch in code if ch in "dmyh")))


def _same_constant(a: tuple, b: tuple | None) -> bool:
    if b is None:
        return False
    if a[0] == "n" and b[0] == "n":
        return math.isclose(a[1], b[1], rel_tol=1e-12, abs_tol=1e-12)
    return a == b


def _same_style(part: str, a: tuple, b: tuple) -> bool:
    if part == "numfmt":
        # "general" and "@"-less general variants are equivalent
        if a == b or {a[0], b[0]} <= {"general", ""}:
            return True
        # built-in short dates render per locale; an explicit spelling of the same fields is equivalent
        return a[0] in _DATE_BUILTINS and _date_tokens(a[0]) == _date_tokens(b[0])
    if part == "font":
        da, db = dict(a or ()), dict(b or ())
        return all(da.get(k) == db.get(k) for k in ("b", "i", "u", "strike", "color")) and \
            abs((da.get("sz") or 11) - (db.get("sz") or 11)) < 0.5 and da.get("name") == db.get("name")
    if part == "fill":
        none = {(), ("none",)}
        return a == b or (a in none and b in none)
    return a == b
