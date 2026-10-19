"""Scoring for the .docx harness: layout vs Word's hints, edits applied, what survived a save."""
from __future__ import annotations

import re
from collections import Counter

from lxml import etree

import docx_xml as dx

APPEND = "[EDIT-APPEND]"
NEW_PARA = "[EDIT-NEW-PARAGRAPH]"
CELL = "[EDIT-CELL]"
MATCH_LEN = 12

# Parts every editor legitimately rewrites on save; their content isn't the document.
VOLATILE = re.compile(r"(docProps/.*|\[Content_Types\]\.xml|.*\.rels|word/settings\.xml|word/webSettings\.xml|word/fontTable\.xml)$")


# --------------------------------------------------------------------------- layout

def page_match(truth_keys: list[str], engine_starts: list[str]) -> dict:
    """Pages whose start text matches Word's. truth_keys come from docx_xml.page_start_keys."""
    checked = matched = 0
    for i, key in enumerate(truth_keys):
        if len(key) < 4:  # page starts with a picture, table border or empty paragraph
            continue
        checked += 1
        if i < len(engine_starts):
            got = dx.norm_key(engine_starts[i], 48)
            # Engines may prefix list labels, or stop at a table cell's end.
            if key[:MATCH_LEN] in got or (len(got) >= 4 and key.startswith(got[:MATCH_LEN])):
                matched += 1
    return {"checked": checked, "matched": matched}


# --------------------------------------------------------------------------- edit target

def edit_target(document_xml: bytes) -> dict | None:
    """First top-level plain paragraph with unique, non-trivial text: the paragraph every engine edits."""
    kids = dx.body_children(document_xml)
    texts = Counter()
    info = []
    p_i = pt_i = 0
    for el in kids:
        name = dx.local(el.tag)
        if name in ("p", "tbl"):
            t = re.sub(r"\s+", "", dx.element_text(el)) if name == "p" else ""
            info.append((el, name, p_i if name == "p" else None, pt_i, t))
            texts[t] += 1
            pt_i += 1
            if name == "p":
                p_i += 1
    has_table = any(n == "tbl" for _, n, *_ in info)
    for el, name, p, pt, t in info:
        if name != "p" or len(t) < 10 or texts[t] != 1:
            continue
        local_names = {dx.local(e.tag) for e in el.iter()}
        if local_names & {"fldChar", "fldSimple", "ins", "del", "sdt", "moveFrom", "moveTo", "drawing", "pict",
                          "AlternateContent", "commentRangeStart", "footnoteReference", "hyperlink", "customXml", "smartTag",
                          "oMath", "oMathPara", "object", "endnoteReference", "bookmarkStart", "sym", "ruby"}:
            continue
        return {"target": t, "p_index": p, "pt_index": pt, "has_table": has_table}
    return None


def check_edits(new_doc: bytes, target: dict) -> dict:
    """Did each scripted edit land? Returns per-edit booleans."""
    kids = dx.body_children(new_doc)
    res = {"append": False, "bold": False, "new_paragraph": False}
    for i, el in enumerate(kids):
        if dx.local(el.tag) != "p":
            continue
        t = re.sub(r"\s+", "", dx.element_text(el))
        if t == target["target"] + APPEND:
            res["append"] = True
            for r in el.iter(dx.W + "r"):
                if r.find(dx.W + "t") is None:
                    continue
                b = r.find(f"{dx.W}rPr/{dx.W}b")
                res["bold"] = b is not None and b.get(dx.W + "val", "true") not in ("0", "false", "off")
                break
            nxt = next((k for k in kids[i + 1:] if dx.local(k.tag) in ("p", "tbl")), None)
            res["new_paragraph"] = nxt is not None and re.sub(r"\s+", "", dx.element_text(nxt)) == NEW_PARA
            break
    if target.get("has_table"):
        tbl = next((k for k in kids if dx.local(k.tag) == "tbl"), None)
        tc = tbl.find(f".//{dx.W}tc") if tbl is not None else None
        res["cell"] = tc is not None and re.sub(r"\s+", "", dx.element_text(tc)) == CELL
    return res


# --------------------------------------------------------------------------- survival

def _rels(data: bytes) -> set:
    try:
        root = dx.parse(data)
    except etree.XMLSyntaxError:
        return set()
    return {(r.get("Type", "").rsplit("/", 1)[-1], r.get("Target", "")) for r in root}


def _canon_part(data: bytes) -> bytes | None:
    try:
        return dx.canonical(dx.parse(data))
    except etree.XMLSyntaxError:
        return None


def survival(orig: dict[str, bytes], new: dict[str, bytes], edited: bool) -> dict:
    """What of the original survived in `new`.

    With `edited`, the edit target paragraph, the paragraph inserted after it and the first
    table are excluded from the body comparison (they are supposed to change).
    """
    out: dict = {}
    # Body: every original top-level element should come back unchanged.
    o_kids = dx.body_children(orig["word/document.xml"])
    n_kids = dx.body_children(new.get("word/document.xml", b"<w:document xmlns:w='%s'/>" % dx.W_NS.encode()))
    skip: set[int] = set()
    if edited:
        tgt = edit_target(orig["word/document.xml"])
        if tgt:
            p_seen = 0
            for i, el in enumerate(o_kids):
                if dx.local(el.tag) == "p":
                    if p_seen == tgt["p_index"]:
                        skip.add(i)
                    p_seen += 1
            if tgt["has_table"]:
                skip.add(next(i for i, el in enumerate(o_kids) if dx.local(el.tag) == "tbl"))
    o_canon = Counter(dx.canonical(el) for i, el in enumerate(o_kids) if i not in skip)
    n_canon = Counter(dx.canonical(el) for el in n_kids)
    kept = sum(min(c, n_canon[k]) for k, c in o_canon.items())
    total = sum(o_canon.values())
    out["body_total"], out["body_kept"] = total, kept
    # Text: the whole body text (minus edits) should still be there.
    o_text = re.sub(r"\s+", "", dx.plain_text(dx.stream(orig["word/document.xml"])))
    n_text = re.sub(r"\s+", "", dx.plain_text(dx.stream(new["word/document.xml"]))) if "word/document.xml" in new else ""
    n_text = n_text.replace(APPEND, "").replace(NEW_PARA, "").replace(CELL, "")
    out["text_equal"] = o_text == n_text or (edited and abs(len(o_text) - len(n_text)) < 200 and o_text[:2000] == n_text[:2000])
    # Other parts.
    parts = {"identical": 0, "equivalent": 0, "changed": [], "missing": []}
    for name, data in orig.items():
        if name == "word/document.xml" or VOLATILE.match(name):
            continue
        if name not in new:
            parts["missing"].append(name)
        elif new[name] == data:
            parts["identical"] += 1
        elif name.endswith(".xml") and (c := _canon_part(data)) is not None and c == _canon_part(new[name]):
            parts["equivalent"] += 1
        else:
            parts["changed"].append(name)
    out["parts"] = parts
    # Relationships that disappeared (by type + target).
    lost = []
    for name, data in orig.items():
        if name.endswith(".rels"):
            gone = _rels(data) - _rels(new.get(name, b""))
            lost += [f"{name}:{t}->{tg}" for t, tg in sorted(gone) if t != "customXml"][:10]
    out["rels_lost"] = lost
    # Feature inventory.
    o_inv, n_inv = dx.inventory(orig), dx.inventory(new)
    feats = {}
    for k, v in o_inv.items():
        if k in ("p", "lastRenderedPageBreak", "tr", "tc"):
            continue
        feats[k] = {"orig": v, "new": n_inv.get(k, 0)}
    out["features"] = feats
    return out


# --------------------------------------------------------------------------- revisions and stories

REV_TAGS = {"ins", "del", "moveFrom", "moveTo", "rPrChange", "pPrChange", "sectPrChange", "tblPrChange",
            "trPrChange", "tcPrChange", "tblGridChange", "numberingChange", "cellIns", "cellDel", "cellMerge"}
STORY = re.compile(r"word/(document|header\d*|footer\d*|footnotes|endnotes|comments)\.xml$")


def revision_count(parts: dict[str, bytes]) -> int:
    n = 0
    for name, data in parts.items():
        if STORY.match(name):
            try:
                root = dx.parse(data)
            except etree.XMLSyntaxError:
                continue
            n += sum(1 for el in root.iter() if dx.local(el.tag) in REV_TAGS and el.get(dx.W + "author") is not None)
    return n


def check_revisions(orig: dict[str, bytes], new: dict[str, bytes], accept: bool) -> dict:
    """Accept/reject all: no revision may remain, and the text must read like Word's
    'No Markup' (accept) or 'Original' (reject) view of the original."""
    want = re.sub(r"\s+", "", dx.plain_text(dx.stream(orig["word/document.xml"], "final" if accept else "original")))
    got = re.sub(r"\s+", "", dx.plain_text(dx.stream(new["word/document.xml"])))
    return {"left": revision_count(new), "text_ok": want == got}


def check_stories(orig: dict[str, bytes], new: dict[str, bytes], info: dict) -> dict:
    res = {}
    doc = new.get("word/document.xml", b"").decode("utf-8", "replace")
    if info.get("header"):
        res["header"] = "[EDIT-HDR]" in new.get(info["header"], b"").decode("utf-8", "replace")
    if "footnote" in info:
        notes = new.get("word/footnotes.xml", b"").decode("utf-8", "replace")
        res["footnote"] = "[EDIT-FN]" in notes and f'footnoteReference w:id="{info["footnote"]}"' in doc
    if "num_id" in info:
        num = new.get("word/numbering.xml", b"").decode("utf-8", "replace")
        res["list"] = f'w:numId="{info["num_id"]}"' in num and f'<w:numId w:val="{info["num_id"]}"/>' in doc
    if "image_rel" in info:
        rels = new.get("word/_rels/document.xml.rels", b"").decode("utf-8", "replace")
        res["picture"] = f'r:embed="{info["image_rel"]}"' in doc and f'Id="{info["image_rel"]}"' in rels \
            and any(n.startswith("word/media/sheaf_image") for n in new)
    return res
