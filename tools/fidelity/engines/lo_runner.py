"""LibreOffice engine runner, executed as a Python macro *inside* soffice.

On macOS, LibreOffice's bundled Python is signed with a launch constraint: only
LibreOffice itself may start it, so an external UNO client script is killed by
AMFI. Instead, the driver copies this file into a throwaway profile
(user/Scripts/python/) and starts soffice with the macro URL

    vnd.sun.star.script:lo_runner.py$serve?language=Python&location=user

`serve` then processes jobs from the folder named by $FIDELITY_JOBS:
  <id>.job   {"id", "mode": ping|roundtrip|render|quit, "src", "dst"}
  <id>.json  result, written atomically when done

Modes:
  roundtrip   load -> recalculate -> read formula results -> save as xlsx/xlsm
  render      load (no recalc, shows cached values) -> export PDF
"""
import json
import os
import time

import uno
from com.sun.star.beans import PropertyValue

FORMULA_CELLS = 16   # com.sun.star.sheet.CellFlags.FORMULA
ERROR_RESULT = 4     # com.sun.star.sheet.FormulaResult.ERROR


def props(**kw):
    return tuple(PropertyValue(Name=k, Value=v) for k, v in kw.items())


def col_name(n):
    s = ""
    while n:
        n, r = divmod(n - 1, 26)
        s = chr(65 + r) + s
    return s


def load(dt, path):
    url = uno.systemPathToFileUrl(os.path.abspath(path))
    # MacroExecutionMode 0 = never run document macros; UpdateDocMode 0 = don't refresh external links
    doc = dt.loadComponentFromURL(url, "_blank", 0, props(Hidden=True, MacroExecutionMode=0, UpdateDocMode=0))
    if doc is None:
        raise RuntimeError("loadComponentFromURL returned None")
    if not hasattr(doc, "Sheets"):
        doc.close(True)
        raise RuntimeError("not loaded as a spreadsheet")
    return doc


def read_values(doc, out):
    sheets = doc.Sheets
    for i in range(sheets.Count):
        sh = sheets.getByIndex(i)
        name = sh.Name
        for a in sh.queryContentCells(FORMULA_CELLS).RangeAddresses:
            rng = sh.getCellRangeByPosition(a.StartColumn, a.StartRow, a.EndColumn, a.EndRow)
            for dr, row in enumerate(rng.DataArray):
                for dc, v in enumerate(row):
                    ref = "%s!%s%d" % (name, col_name(a.StartColumn + dc + 1), a.StartRow + dr + 1)
                    out[ref] = {"t": "n", "v": v} if isinstance(v, float) else {"t": "s", "v": v}
        for a in sh.queryFormulaCells(ERROR_RESULT).RangeAddresses:
            for r in range(a.StartRow, a.EndRow + 1):
                for c in range(a.StartColumn, a.EndColumn + 1):
                    ref = "%s!%s%d" % (name, col_name(c + 1), r + 1)
                    out[ref] = {"t": "e", "v": sh.getCellByPosition(c, r).String}


def apply_edits(doc, edits, out):
    errors = []
    for e in edits:
        try:
            cell = doc.Sheets.getByName(e["sheet"]).getCellRangeByName(e["cell"])
            op = e["op"]
            if op == "number":
                cell.setValue(e["value"])
            elif op == "text":
                cell.setString(e["value"])
            elif op == "formula":
                cell.setFormula(e["value"])
            elif op == "bold":
                cell.CharWeight = 150.0  # com.sun.star.awt.FontWeight.BOLD
            else:
                raise RuntimeError("unknown op %s" % op)
        except Exception as ex:
            errors.append("%s: %s" % (e.get("cell"), ex))
    if errors:
        out["edit_errors"] = errors


def handle(dt, spec):
    out = {"engine": "libreoffice", "stages": {}, "values": {}}
    doc = None

    def stage(name, fn):
        t = time.perf_counter()
        try:
            result = fn()
        except Exception as e:
            out["stages"][name] = {"ok": False, "ms": round((time.perf_counter() - t) * 1000, 1)}
            raise RuntimeError(("%s: %s: %s" % (name, type(e).__name__, e))[:800])
        out["stages"][name] = {"ok": True, "ms": round((time.perf_counter() - t) * 1000, 1)}
        return result

    try:
        mode = spec["mode"]
        if mode == "ping":
            out["ok"] = True
        elif mode in ("roundtrip", "render"):
            doc = stage("load", lambda: load(dt, spec["src"]))
            dst_url = uno.systemPathToFileUrl(os.path.abspath(spec["dst"]))
            if mode == "roundtrip":
                if spec.get("edits"):
                    stage("edits", lambda: apply_edits(doc, spec["edits"], out))
                stage("evaluate", doc.calculateAll)
                stage("read", lambda: read_values(doc, out["values"]))
                filt = ("Calc MS Excel 2007 VBA XML" if spec["dst"].lower().endswith(".xlsm")
                        else "Calc MS Excel 2007 XML")
                stage("save", lambda: doc.storeToURL(dst_url, props(FilterName=filt, Overwrite=True)))
            else:
                stage("render", lambda: doc.storeToURL(dst_url, props(FilterName="calc_pdf_Export", Overwrite=True)))
            out["ok"] = True
        else:
            raise RuntimeError("unknown mode %s" % mode)
    except Exception as e:
        out["ok"] = False
        out["error"] = str(e)[:800]
    finally:
        if doc is not None:
            try:
                doc.close(True)
            except Exception:
                pass
    return out


def serve(*args):
    jobs = os.environ["FIDELITY_JOBS"]
    dt = XSCRIPTCONTEXT.getDesktop()  # noqa: F821 (injected by LibreOffice's script provider)
    with open(os.path.join(jobs, "ready"), "w") as fh:
        fh.write(str(os.getpid()))
    while True:
        pending = sorted(f for f in os.listdir(jobs) if f.endswith(".job"))
        if not pending:
            time.sleep(0.02)
            continue
        path = os.path.join(jobs, pending[0])
        with open(path) as fh:
            spec = json.load(fh)
        os.remove(path)
        if spec.get("mode") == "quit":
            break
        result = handle(dt, spec)
        tmp = os.path.join(jobs, spec["id"] + ".tmp")
        with open(tmp, "w") as fh:
            json.dump(result, fh, default=str)
        os.replace(tmp, os.path.join(jobs, spec["id"] + ".json"))
    dt.terminate()


g_exportedScripts = (serve,)
