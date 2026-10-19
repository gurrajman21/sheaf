"""LibreOffice Writer runner for the .docx harness, executed as a macro *inside* soffice.

Same job-folder protocol as lo_runner.py (see there for why it runs in-process).
Job spec: {"id", "mode": "docx"|"render"|"ping"|"quit", "src", "dst", "edits": {...}}
where for mode "docx" `edits` carries the whole docx job:
  {"layout_src", "roundtrip_dst", "edit_dst", "edits": {"target", "has_table", ...}}

docx: load layout_src -> page count + first text of each page (view cursor)
      load src -> save round-trip copy -> apply scripted edits -> save edited copy
render: load src -> export PDF to dst
"""
import json
import os
import re
import time

import uno
from com.sun.star.beans import PropertyValue

APPEND = " [EDIT-APPEND]"
NEW_PARA = "[EDIT-NEW-PARAGRAPH]"
CELL = "[EDIT-CELL]"
PARAGRAPH_BREAK = 0  # com.sun.star.text.ControlCharacter.PARAGRAPH_BREAK
MAX_PAGES = 400


def props(**kw):
    return tuple(PropertyValue(Name=k, Value=v) for k, v in kw.items())


def squash(s):
    return re.sub(r"\s+", "", s)


def load(dt, path):
    url = uno.systemPathToFileUrl(os.path.abspath(path))
    doc = dt.loadComponentFromURL(url, "_blank", 0, props(Hidden=True, MacroExecutionMode=0, UpdateDocMode=0))
    if doc is None:
        raise RuntimeError("loadComponentFromURL returned None")
    if not hasattr(doc, "Text"):
        doc.close(True)
        raise RuntimeError("not loaded as a text document")
    return doc


def page_starts(doc):
    ctrl = doc.CurrentController
    vc = ctrl.ViewCursor
    n = ctrl.PageCount
    starts = []
    for p in range(1, min(n, MAX_PAGES) + 1):
        vc.jumpToPage(p)
        vc.jumpToStartOfPage()
        start = vc.getStart()
        vc.goRight(160, True)
        got = vc.String
        vc.collapseToStart()
        if not got.strip():
            # Page starts inside a table cell or frame: read from the cell's own text.
            try:
                cur = start.Text.createTextCursorByRange(start)
                cur.goRight(160, True)
                got = cur.String
            except Exception:
                pass
        starts.append(got[:160])
    return n, starts


def apply_edits(doc, edits, errors):
    text = doc.Text
    target = edits.get("target", "")
    para = table = None
    enum = text.createEnumeration()
    while enum.hasMoreElements():
        el = enum.nextElement()
        if el.supportsService("com.sun.star.text.TextTable"):
            if table is None:
                table = el
        elif para is None and el.supportsService("com.sun.star.text.Paragraph") and squash(el.String) == target:
            para = el
    if para is None:
        errors.append("target paragraph not found")
    else:
        portions = para.createEnumeration()
        bolded = False
        while portions.hasMoreElements():
            por = portions.nextElement()
            if por.TextPortionType == "Text" and por.String:
                por.CharWeight = 150.0  # com.sun.star.awt.FontWeight.BOLD
                bolded = True
                break
        if not bolded:
            errors.append("bold: no text portion")
        text.insertString(para.End, APPEND, False)
        cur = text.createTextCursorByRange(para.End)
        text.insertControlCharacter(cur, PARAGRAPH_BREAK, False)
        text.insertString(cur, NEW_PARA, False)
    if edits.get("has_table"):
        if table is None:
            errors.append("cell: no top-level table")
        else:
            names = table.CellNames
            if names:
                table.getCellByName(names[0]).setString(CELL)


def handle(dt, spec):
    out = {"engine": "libreoffice", "stages": {}}
    docs = []

    def stage(name, fn):
        t = time.perf_counter()
        try:
            result = fn()
        except Exception as e:
            out["stages"][name] = {"ok": False, "ms": round((time.perf_counter() - t) * 1000, 1)}
            raise RuntimeError(("%s: %s: %s" % (name, type(e).__name__, e))[:800])
        out["stages"][name] = {"ok": True, "ms": round((time.perf_counter() - t) * 1000, 1)}
        return result

    def opened(path):
        d = load(dt, path)
        docs.append(d)
        return d

    try:
        mode = spec["mode"]
        if mode == "ping":
            out["ok"] = True
        elif mode == "reload":
            doc = stage("open", lambda: opened(spec["src"]))
            out["pages"] = doc.CurrentController.PageCount
            out["ok"] = True
        elif mode == "render":
            doc = stage("open", lambda: opened(spec["src"]))
            url = uno.systemPathToFileUrl(os.path.abspath(spec["dst"]))
            stage("render", lambda: doc.storeToURL(url, props(FilterName="writer_pdf_Export", Overwrite=True)))
            out["ok"] = True
        elif mode == "docx":
            job = spec["edits"]
            ldoc = stage("layout_open", lambda: opened(job.get("layout_src") or spec["src"]))
            n, starts = stage("layout", lambda: page_starts(ldoc))
            out["pages"], out["starts"] = n, starts
            doc = stage("open", lambda: opened(spec["src"]))
            filt = props(FilterName="MS Word 2007 XML", Overwrite=True)
            rt = uno.systemPathToFileUrl(os.path.abspath(job["roundtrip_dst"]))
            stage("roundtrip", lambda: doc.storeToURL(rt, filt))
            if job.get("edit_dst") and job.get("edits"):
                errors = []
                ed = uno.systemPathToFileUrl(os.path.abspath(job["edit_dst"]))

                def edit():
                    apply_edits(doc, job["edits"], errors)
                    doc.storeToURL(ed, filt)
                stage("edit", edit)
                if errors:
                    out["edit_errors"] = errors
            out["ok"] = True
        else:
            raise RuntimeError("unknown mode %s" % mode)
    except Exception as e:
        out["ok"] = False
        out["error"] = str(e)[:800]
    finally:
        for d in docs:
            try:
                d.close(True)
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
