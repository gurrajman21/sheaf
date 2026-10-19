"""Raw .docx reading shared by the docx harness: text in reading order, Word's cached
layout hints, a feature inventory and canonical XML for survival checks.

Engine-independent: everything here reads the package XML directly with lxml.
"""
from __future__ import annotations

import re
import zipfile
from collections import Counter
from pathlib import Path

from lxml import etree

W_NS = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
MC_NS = "http://schemas.openxmlformats.org/markup-compatibility/2006"
W = "{%s}" % W_NS
KEY_LEN = 16

PARSER = etree.XMLParser(resolve_entities=False, no_network=True, huge_tree=True, remove_blank_text=False)


def read_parts(path: str | Path) -> dict[str, bytes]:
    with zipfile.ZipFile(path) as z:
        return {n: z.read(n) for n in z.namelist() if not n.endswith("/")}


def parse(data: bytes):
    return etree.fromstring(data, PARSER)


def local(tag) -> str:
    return tag.rsplit("}", 1)[-1] if isinstance(tag, str) else ""


def norm_key(text: str, n: int = KEY_LEN) -> str:
    """Comparison key for 'where a page starts': first n non-space characters, case-folded."""
    return re.sub(r"\s+", "", text).casefold()[:n]


# --------------------------------------------------------------------------- text stream

SKIP_SUBTREES = {"delText", "instrText", "txbxContent", "Fallback", "footnoteReference", "endnoteReference",
                 "commentReference", "delInstrText", "pict", "object"}


def stream(document_xml: bytes, view: str | None = None) -> list:
    """Body content in reading order as a list of str chunks and markers.

    Markers: ("lrpb",) Word's lastRenderedPageBreak; ("pb",) explicit page break.
    Deleted text, field codes, text boxes and mc:Fallback copies are skipped.
    """
    root = parse(document_xml)
    body = root.find(W + "body")
    out: list = []
    if body is None:
        return out
    skip = set(SKIP_SUBTREES)
    if view == "final":  # tracked changes accepted
        skip |= {"del", "moveFrom"}
    elif view == "original":  # tracked changes rejected
        skip = (skip - {"delText"}) | {"ins", "moveTo"}
    skip_depth = 0
    for event, el in etree.iterwalk(body, events=("start", "end")):
        name = local(el.tag)
        if event == "start":
            if skip_depth or name in skip or (name == "tr" and view and _row_hidden(el, view)):
                skip_depth += 1
                continue
            if name in ("t", "delText") and el.text:
                out.append(el.text)
            elif name in ("tab", "ptab"):
                out.append(" ")
            elif name == "br":
                if el.get(W + "type") == "page":
                    out.append(("pb",))
                else:
                    out.append(" ")
            elif name == "lastRenderedPageBreak":
                out.append(("lrpb",))
            elif name == "sym":
                out.append("*")
        else:
            if skip_depth:
                skip_depth -= 1
                continue
            if name == "p" or name == "tc":
                out.append("\n")
    return out


def _row_hidden(tr, view: str) -> bool:
    """A tracked row insertion vanishes in the original view, a row deletion in the final view."""
    trpr = tr.find(W + "trPr")
    if trpr is None:
        return False
    marker = trpr.find(W + ("ins" if view == "original" else "del"))
    return marker is not None and marker.get(W + "author") is not None


def page_start_keys(chunks: list, marker: str = "lrpb") -> list[str]:
    """Key of the text at the start of the document and after every `marker`."""
    starts = [0]
    text = []
    pos = 0
    for c in chunks:
        if isinstance(c, tuple):
            if c[0] == marker:
                starts.append(pos)
        else:
            text.append(c)
            pos += len(c)
    full = "".join(text)
    return [norm_key(full[s:s + 400]) for s in starts]


def plain_text(chunks: list) -> str:
    return "".join(c for c in chunks if isinstance(c, str))


# --------------------------------------------------------------------------- truth + inventory

def app_props(parts: dict[str, bytes]) -> dict:
    data = parts.get("docProps/app.xml")
    if not data:
        return {}
    try:
        root = parse(data)
    except etree.XMLSyntaxError:
        return {}
    out = {}
    for el in root:
        n = local(el.tag)
        if n in ("Application", "AppVersion", "Pages") and el.text:
            out[n] = el.text.strip()
    return out


INVENTORY_TAGS = ["p", "tbl", "tr", "tc", "drawing", "pict", "txbxContent", "sdt", "fldChar", "fldSimple", "hyperlink",
                  "bookmarkStart", "footnoteReference", "endnoteReference", "commentReference", "commentRangeStart",
                  "ins", "del", "moveFrom", "moveTo", "rPrChange", "pPrChange", "sectPr", "oMath", "object",
                  "AlternateContent", "numPr", "lastRenderedPageBreak", "customXml", "smartTag"]

PART_KINDS = {
    "header": r"word/header\d*\.xml", "footer": r"word/footer\d*\.xml", "footnotes": r"word/footnotes\.xml",
    "endnotes": r"word/endnotes\.xml", "comments": r"word/comments\.xml", "commentsExtended": r"word/commentsExtended\.xml",
    "numbering": r"word/numbering\.xml", "styles": r"word/styles\.xml", "theme": r"word/theme/.*",
    "media": r"word/media/.*", "embeddings": r"word/embeddings/.*", "charts": r"word/charts/.*",
    "glossary": r"word/glossary/.*", "customXml": r"customXml/.*", "vba": r"word/vbaProject\.bin",
    "fontTable": r"word/fontTable\.xml", "people": r"word/people\.xml", "diagrams": r"word/diagrams/.*",
}


def inventory(parts: dict[str, bytes]) -> dict:
    inv: dict[str, int] = {}
    doc = parts.get("word/document.xml")
    if doc:
        c = Counter(local(el.tag) for el in parse(doc).iter())
        for t in INVENTORY_TAGS:
            if c.get(t):
                inv[t] = c[t]
    for kind, pat in PART_KINDS.items():
        n = sum(1 for p in parts if re.fullmatch(pat, p))
        if n:
            inv["part:" + kind] = n
    return inv


def truth(path: str | Path) -> dict:
    """Word's own layout hints for a file, and whether they can be trusted."""
    parts = read_parts(path)
    app = app_props(parts)
    chunks = stream(parts["word/document.xml"])
    lrpb = sum(1 for c in chunks if c == ("lrpb",))
    application = app.get("Application", "")
    word = "Microsoft Office Word" in application or application.startswith("Microsoft Macintosh Word")
    pages = int(app["Pages"]) if app.get("Pages", "").isdigit() else None
    return {
        "application": application, "app_version": app.get("AppVersion"), "word_saved": word,
        "pages_app": pages, "lrpb": lrpb,
        # Both hints agree only when Word rendered the whole document right before saving.
        "trusted": bool(word and pages and lrpb and pages == lrpb + 1),
        "starts": page_start_keys(chunks) if lrpb else [],
        "chars": len(plain_text(chunks)),
    }


# --------------------------------------------------------------------------- canonical XML for survival

RSID = re.compile(r"\{%s\}rsid" % re.escape(W_NS))


def canonical(el) -> bytes:
    """C14N of an element without rsids and cached page breaks (editors legitimately rewrite
    those; Word regenerates both)."""
    el = etree.fromstring(etree.tostring(el), PARSER)
    for e in list(el.iter()):
        for a in list(e.attrib):
            if RSID.match(a):
                del e.attrib[a]
        if local(e.tag) in ("rsid", "rsidRoot", "lastRenderedPageBreak", "proofErr") and e.getparent() is not None:
            e.getparent().remove(e)
    return etree.tostring(el, method="c14n", exclusive=True)


def body_children(document_xml: bytes) -> list:
    body = parse(document_xml).find(W + "body")
    return [] if body is None else [c for c in body if isinstance(c.tag, str)]


def element_text(el) -> str:
    return plain_text(stream_element(el))


def stream_element(el) -> list:
    wrapper = etree.Element(W + "document", nsmap={"w": W_NS})
    body = etree.SubElement(wrapper, W + "body")
    body.append(etree.fromstring(etree.tostring(el), PARSER))
    return stream(etree.tostring(wrapper))
