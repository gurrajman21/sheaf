"""Generate synthetic .docx files, one per Word feature, into corpus/docx/synthetic/.

Written as raw WordprocessingML (no python-docx) so every feature can be covered:
footnotes, comments, tracked changes, fields, content controls, equations, sections.

Files whose page breaks are fully explicit (hard page breaks / section breaks on short
pages) record their known page count and the text each page starts with in
corpus/docx/synthetic/expected.json. The layout score uses those as ground truth for
synthetic files, the way Word's cached hints are used for real files.

Usage: python gen_synthetic_docx.py
"""
from __future__ import annotations

import io
import json
import shutil
import zipfile
from pathlib import Path
from xml.sax.saxutils import escape

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "corpus" / "docx" / "synthetic"

W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
NS = (
    'xmlns:wpc="http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas" '
    'xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" '
    'xmlns:o="urn:schemas-microsoft-com:office:office" '
    'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" '
    'xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math" '
    'xmlns:v="urn:schemas-microsoft-com:vml" '
    'xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" '
    'xmlns:w10="urn:schemas-microsoft-com:office:word" '
    f'{W} '
    'xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" '
    'xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" '
    'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" '
    'xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture" '
    'mc:Ignorable="w14"'
)
REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
LOREM = (
    "The quarterly review covers revenue, staffing and the product roadmap. Each section "
    "summarises what changed since the last report, what is planned next, and which risks "
    "need a decision from the leadership team before the end of the month."
)


def run(text: str, rpr: str = "") -> str:
    rp = f"<w:rPr>{rpr}</w:rPr>" if rpr else ""
    return f'<w:r>{rp}<w:t xml:space="preserve">{escape(text)}</w:t></w:r>'


def para(content: str = "", ppr: str = "", text: str | None = None) -> str:
    if text is not None:
        content = run(text) + content
    pp = f"<w:pPr>{ppr}</w:pPr>" if ppr else ""
    return f"<w:p>{pp}{content}</w:p>"


def styled(style: str, text: str) -> str:
    return para(ppr=f'<w:pStyle w:val="{style}"/>', text=text)


PAGE_BREAK = '<w:r><w:br w:type="page"/></w:r>'
SECT_LETTER = ('<w:sectPr><w:pgSz w:w="12240" w:h="15840"/>'
               '<w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" '
               'w:header="720" w:footer="720" w:gutter="0"/><w:cols w:space="720"/></w:sectPr>')

STYLES = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles {W}>
<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="Calibri" w:cs="Times New Roman"/>
<w:sz w:val="22"/><w:szCs w:val="22"/><w:lang w:val="en-US"/></w:rPr></w:rPrDefault>
<w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style>
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/>
<w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before="240" w:after="0"/><w:outlineLvl w:val="0"/></w:pPr>
<w:rPr><w:rFonts w:ascii="Calibri Light" w:hAnsi="Calibri Light"/><w:color w:val="2F5496"/><w:sz w:val="32"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/>
<w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before="40" w:after="0"/><w:outlineLvl w:val="1"/></w:pPr>
<w:rPr><w:color w:val="2F5496"/><w:sz w:val="26"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/>
<w:rPr><w:rFonts w:ascii="Calibri Light" w:hAnsi="Calibri Light"/><w:sz w:val="56"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:pPr><w:ind w:left="720"/></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="TOC1"><w:name w:val="toc 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:spacing w:after="100"/></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="Header"><w:name w:val="header"/><w:basedOn w:val="Normal"/><w:pPr><w:tabs><w:tab w:val="center" w:pos="4680"/><w:tab w:val="right" w:pos="9360"/></w:tabs><w:spacing w:after="0"/></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="Footer"><w:name w:val="footer"/><w:basedOn w:val="Normal"/><w:pPr><w:tabs><w:tab w:val="center" w:pos="4680"/><w:tab w:val="right" w:pos="9360"/></w:tabs><w:spacing w:after="0"/></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="FootnoteText"><w:name w:val="footnote text"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:after="0"/></w:pPr><w:rPr><w:sz w:val="20"/></w:rPr></w:style>
<w:style w:type="character" w:styleId="FootnoteReference"><w:name w:val="footnote reference"/><w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="EndnoteText"><w:name w:val="endnote text"/><w:basedOn w:val="Normal"/><w:rPr><w:sz w:val="20"/></w:rPr></w:style>
<w:style w:type="character" w:styleId="EndnoteReference"><w:name w:val="endnote reference"/><w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="CommentText"><w:name w:val="annotation text"/><w:basedOn w:val="Normal"/><w:rPr><w:sz w:val="20"/></w:rPr></w:style>
<w:style w:type="character" w:styleId="Hyperlink"><w:name w:val="Hyperlink"/><w:rPr><w:color w:val="0563C1"/><w:u w:val="single"/></w:rPr></w:style>
<w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/><w:tblPr><w:tblInd w:w="0" w:type="dxa"/>
<w:tblCellMar><w:top w:w="0" w:type="dxa"/><w:left w:w="108" w:type="dxa"/><w:bottom w:w="0" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style>
<w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:basedOn w:val="TableNormal"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr>
<w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:left w:val="single" w:sz="4" w:space="0" w:color="auto"/>
<w:bottom w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:right w:val="single" w:sz="4" w:space="0" w:color="auto"/>
<w:insideH w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="auto"/></w:tblBorders></w:tblPr></w:style>
</w:styles>"""

NUMBERING = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering {W}>
<w:abstractNum w:abstractNumId="0"><w:multiLevelType w:val="hybridMultilevel"/>
<w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="•"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr><w:rPr><w:rFonts w:ascii="Symbol" w:hAnsi="Symbol" w:hint="default"/></w:rPr></w:lvl>
<w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="o"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="1440" w:hanging="360"/></w:pPr><w:rPr><w:rFonts w:ascii="Courier New" w:hAnsi="Courier New" w:hint="default"/></w:rPr></w:lvl>
</w:abstractNum>
<w:abstractNum w:abstractNumId="1"><w:multiLevelType w:val="multilevel"/>
<w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl>
<w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%2)"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="1440" w:hanging="360"/></w:pPr></w:lvl>
<w:lvl w:ilvl="2"><w:start w:val="1"/><w:numFmt w:val="lowerRoman"/><w:lvlText w:val="%3."/><w:lvlJc w:val="right"/><w:pPr><w:ind w:left="2160" w:hanging="180"/></w:pPr></w:lvl>
</w:abstractNum>
<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
<w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>
</w:numbering>"""

SETTINGS = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:settings {W}><w:zoom w:percent="100"/><w:defaultTabStop w:val="720"/><w:characterSpacingControl w:val="doNotCompress"/>
<w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/></w:compat></w:settings>"""

APP = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>fidelity-gen</Application></Properties>"""
CORE = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>Synthetic</dc:title><dc:creator>fidelity</dc:creator></cp:coreProperties>"""


class Doc:
    """Collects body XML plus optional parts, then writes a package."""

    def __init__(self):
        self.body: list[str] = []
        self.sect = SECT_LETTER
        self.parts: dict[str, tuple[str, str | bytes, str | None]] = {}  # path -> (ctype, data, reltype)
        self.footnotes: list[str] = []
        self.endnotes: list[str] = []
        self.comments: list[str] = []
        self.extra_rels: list[tuple[str, str, str, str]] = []  # id, type, target, mode
        self.defaults: set[str] = set()

    def add(self, *xml: str):
        self.body.extend(xml)

    def rel(self, rtype: str, target: str, external: bool = False) -> str:
        rid = f"rIdX{len(self.extra_rels) + 1}"
        self.extra_rels.append((rid, rtype, target, "External" if external else ""))
        return rid

    def part(self, path: str, ctype: str, data, rtype: str | None = None) -> str | None:
        self.parts[path] = (ctype, data, rtype)
        if rtype:
            return self.rel(rtype, path.removeprefix("word/"))
        return None

    def footnote(self, text: str) -> str:
        fid = len(self.footnotes) + 1
        self.footnotes.append(
            f'<w:footnote w:id="{fid}"><w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr>'
            f'<w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteRef/></w:r>{run(" " + text)}</w:p></w:footnote>')
        return f'<w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteReference w:id="{fid}"/></w:r>'

    def endnote(self, text: str) -> str:
        eid = len(self.endnotes) + 1
        self.endnotes.append(
            f'<w:endnote w:id="{eid}"><w:p><w:pPr><w:pStyle w:val="EndnoteText"/></w:pPr>'
            f'<w:r><w:rPr><w:rStyle w:val="EndnoteReference"/></w:rPr><w:endnoteRef/></w:r>{run(" " + text)}</w:p></w:endnote>')
        return f'<w:r><w:rPr><w:rStyle w:val="EndnoteReference"/></w:rPr><w:endnoteReference w:id="{eid}"/></w:r>'

    def comment(self, text: str, author: str = "Reviewer") -> int:
        cid = len(self.comments)
        self.comments.append(
            f'<w:comment w:id="{cid}" w:author="{author}" w:date="2026-01-15T10:00:00Z" w:initials="R">'
            f'<w:p><w:pPr><w:pStyle w:val="CommentText"/></w:pPr><w:r><w:annotationRef/></w:r>{run(text)}</w:p></w:comment>')
        return cid

    def write(self, path: Path):
        ct = {
            "/word/document.xml": "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
            "/word/styles.xml": "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml",
            "/word/settings.xml": "application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml",
            "/word/numbering.xml": "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml",
            "/docProps/app.xml": "application/vnd.openxmlformats-officedocument.extended-properties+xml",
            "/docProps/core.xml": "application/vnd.openxmlformats-package.core-properties+xml",
        }
        rels = [
            ("rId1", f"{REL}/styles", "styles.xml", ""),
            ("rId2", f"{REL}/settings", "settings.xml", ""),
            ("rId3", f"{REL}/numbering", "numbering.xml", ""),
        ]
        sep = ('<w:{k} w:type="separator" w:id="-1"><w:p><w:pPr><w:spacing w:after="0"/></w:pPr><w:r><w:separator/></w:r></w:p></w:{k}>'
               '<w:{k} w:type="continuationSeparator" w:id="0"><w:p><w:pPr><w:spacing w:after="0"/></w:pPr><w:r><w:continuationSeparator/></w:r></w:p></w:{k}>')
        if self.footnotes:
            self.part("word/footnotes.xml", "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml",
                      f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:footnotes {NS}>{sep.format(k="footnote")}{"".join(self.footnotes)}</w:footnotes>',
                      f"{REL}/footnotes")
        if self.endnotes:
            self.part("word/endnotes.xml", "application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml",
                      f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:endnotes {NS}>{sep.format(k="endnote")}{"".join(self.endnotes)}</w:endnotes>',
                      f"{REL}/endnotes")
        if self.comments:
            self.part("word/comments.xml", "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml",
                      f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:comments {NS}>{"".join(self.comments)}</w:comments>',
                      f"{REL}/comments")
        defaults = {"png": "image/png", "jpeg": "image/jpeg"}
        used_defaults = {p.rsplit(".", 1)[-1] for p in self.parts if p.rsplit(".", 1)[-1] in defaults}
        overrides = dict(ct)
        for p, (ctype, _, _) in self.parts.items():
            if p.rsplit(".", 1)[-1] not in defaults:
                overrides["/" + p] = ctype
        types = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
                 '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
                 '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
                 '<Default Extension="xml" ContentType="application/xml"/>'
                 + "".join(f'<Default Extension="{e}" ContentType="{defaults[e]}"/>' for e in sorted(used_defaults))
                 + "".join(f'<Override PartName="{p}" ContentType="{c}"/>' for p, c in overrides.items())
                 + "</Types>")
        rel_xml = "".join(
            f'<Relationship Id="{i}" Type="{t}" Target="{escape(tg)}"' + (f' TargetMode="{m}"' if m else "") + "/>"
            for i, t, tg, m in rels + self.extra_rels)
        doc = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {NS}><w:body>'
               + "".join(self.body) + self.sect + "</w:body></w:document>")
        with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
            z.writestr("[Content_Types].xml", types)
            z.writestr("_rels/.rels",
                       '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
                       f'<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
                       f'<Relationship Id="rId1" Type="{REL}/officeDocument" Target="word/document.xml"/>'
                       '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>'
                       f'<Relationship Id="rId3" Type="{REL}/extended-properties" Target="docProps/app.xml"/></Relationships>')
            z.writestr("word/document.xml", doc)
            z.writestr("word/_rels/document.xml.rels",
                       '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
                       f'<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rel_xml}</Relationships>')
            z.writestr("word/styles.xml", STYLES)
            z.writestr("word/settings.xml", SETTINGS)
            z.writestr("word/numbering.xml", NUMBERING)
            z.writestr("docProps/app.xml", APP)
            z.writestr("docProps/core.xml", CORE)
            for p, (_, data, _) in self.parts.items():
                z.writestr(p, data)


def png(w: int, h: int, color: str, label: str) -> bytes:
    img = Image.new("RGB", (w, h), color)
    d = ImageDraw.Draw(img)
    d.rectangle([4, 4, w - 5, h - 5], outline="white", width=4)
    d.text((16, h // 2 - 6), label, fill="white")
    buf = io.BytesIO()
    img.save(buf, "PNG")
    return buf.getvalue()


def drawing(rid: str, cx_px: int, cy_px: int, pid: int, floating: bool = False) -> str:
    cx, cy = cx_px * 9525, cy_px * 9525
    graphic = (f'<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">'
               f'<pic:pic><pic:nvPicPr><pic:cNvPr id="{pid}" name="Picture {pid}"/><pic:cNvPicPr/></pic:nvPicPr>'
               f'<pic:blipFill><a:blip r:embed="{rid}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>'
               f'<pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr>'
               f'</pic:pic></a:graphicData></a:graphic>')
    if not floating:
        return (f'<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/>'
                f'<wp:docPr id="{pid}" name="Picture {pid}"/>{graphic}</wp:inline></w:drawing></w:r>')
    return (f'<w:r><w:drawing><wp:anchor distT="0" distB="0" distL="114300" distR="114300" simplePos="0" relativeHeight="2" '
            f'behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/>'
            f'<wp:positionH relativeFrom="column"><wp:align>right</wp:align></wp:positionH>'
            f'<wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV>'
            f'<wp:extent cx="{cx}" cy="{cy}"/><wp:wrapSquare wrapText="bothSides"/>'
            f'<wp:docPr id="{pid}" name="Picture {pid}"/>{graphic}</wp:anchor></w:drawing></w:r>')


def table(rows: list[list[str]], header: bool = False, widths: list[int] | None = None, extra_tblpr: str = "") -> str:
    n = len(rows[0])
    widths = widths or [9360 // n] * n
    grid = "".join(f'<w:gridCol w:w="{w}"/>' for w in widths)
    out = [f'<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="0" w:type="auto"/>{extra_tblpr}</w:tblPr><w:tblGrid>{grid}</w:tblGrid>']
    for i, r in enumerate(rows):
        trpr = "<w:trPr><w:tblHeader/></w:trPr>" if header and i == 0 else ""
        cells = "".join(f'<w:tc><w:tcPr><w:tcW w:w="{widths[j]}" w:type="dxa"/></w:tcPr>{para(text=c)}</w:tc>' for j, c in enumerate(r))
        out.append(f"<w:tr>{trpr}{cells}</w:tr>")
    out.append("</w:tbl>")
    return "".join(out)


def hf_part(kind: str, body: str) -> str:
    tag = "hdr" if kind == "header" else "ftr"
    return f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:{tag} {NS}>{body}</w:{tag}>'


def field(instr: str, result: str) -> str:
    return (f'<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> {escape(instr)} </w:instrText></w:r>'
            f'<w:r><w:fldChar w:fldCharType="separate"/></w:r>{run(result)}<w:r><w:fldChar w:fldCharType="end"/></w:r>')


# --- feature builders: each returns (Doc, expected | None) ---------------------------------------

def f_basic_text():
    d = Doc()
    d.add(styled("Title", "Basic text"), para(text=LOREM),
          para(run("Bold ", "<w:b/>") + run("italic ", "<w:i/>") + run("underline ", '<w:u w:val="single"/>')
               + run("strike ", "<w:strike/>") + run("red ", '<w:color w:val="C00000"/>') + run("big", '<w:sz w:val="36"/>')
               + run(" highlight", '<w:highlight w:val="yellow"/>') + run(" Times", '<w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman"/>')),
          para(ppr='<w:jc w:val="center"/>', text="Centered paragraph."),
          para(ppr='<w:jc w:val="right"/>', text="Right aligned paragraph."),
          para(ppr='<w:jc w:val="both"/>', text=LOREM * 2),
          para(ppr='<w:ind w:left="1440" w:firstLine="720"/>', text=LOREM),
          para(ppr='<w:spacing w:line="480" w:lineRule="auto"/>', text=LOREM))
    return d, None


def f_headings():
    d = Doc()
    for i in range(1, 6):
        d.add(styled("Heading1", f"Chapter {i}"), para(text=LOREM), styled("Heading2", f"Section {i}.1"), para(text=LOREM))
    return d, None


def f_lists():
    d = Doc()
    d.add(styled("Heading1", "Lists"))
    for i, t in enumerate(["Apples", "Oranges", "Pears"]):
        d.add(para(ppr='<w:pStyle w:val="ListParagraph"/><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr>', text=t))
        if i == 1:
            d.add(para(ppr='<w:pStyle w:val="ListParagraph"/><w:numPr><w:ilvl w:val="1"/><w:numId w:val="1"/></w:numPr>', text="Blood oranges"))
    for lvl, t in [(0, "First step"), (1, "Detail a"), (1, "Detail b"), (2, "Sub-detail"), (0, "Second step"), (0, "Third step")]:
        d.add(para(ppr=f'<w:pStyle w:val="ListParagraph"/><w:numPr><w:ilvl w:val="{lvl}"/><w:numId w:val="2"/></w:numPr>', text=t))
    return d, None


def f_tables():
    d = Doc()
    d.add(styled("Heading1", "Tables"), table([["Region", "Q1", "Q2", "Q3"], ["North", "120", "140", "155"], ["South", "98", "102", "110"]]))
    d.add(para(text="Merged cells:"))
    d.add('<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="0" w:type="auto"/></w:tblPr>'
          '<w:tblGrid><w:gridCol w:w="3120"/><w:gridCol w:w="3120"/><w:gridCol w:w="3120"/></w:tblGrid>'
          '<w:tr><w:tc><w:tcPr><w:tcW w:w="6240" w:type="dxa"/><w:gridSpan w:val="2"/><w:shd w:val="clear" w:color="auto" w:fill="D9E2F3"/></w:tcPr>' + para(text="Spans two columns") + '</w:tc>'
          '<w:tc><w:tcPr><w:tcW w:w="3120" w:type="dxa"/><w:vMerge w:val="restart"/><w:vAlign w:val="center"/></w:tcPr>' + para(text="Spans two rows") + '</w:tc></w:tr>'
          '<w:tr><w:tc><w:tcPr><w:tcW w:w="3120" w:type="dxa"/></w:tcPr>' + para(text="A") + '</w:tc>'
          '<w:tc><w:tcPr><w:tcW w:w="3120" w:type="dxa"/></w:tcPr>' + para(text="B") + '</w:tc>'
          '<w:tc><w:tcPr><w:tcW w:w="3120" w:type="dxa"/><w:vMerge/></w:tcPr><w:p/></w:tc></w:tr></w:tbl>')
    d.add(para(text="Nested table:"))
    inner = table([["x", "y"], ["1", "2"]], widths=[1500, 1500])
    d.add('<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="0" w:type="auto"/></w:tblPr><w:tblGrid><w:gridCol w:w="4680"/><w:gridCol w:w="4680"/></w:tblGrid>'
          f'<w:tr><w:tc><w:tcPr><w:tcW w:w="4680" w:type="dxa"/></w:tcPr>{para(text="Outer cell")}</w:tc>'
          f'<w:tc><w:tcPr><w:tcW w:w="4680" w:type="dxa"/></w:tcPr>{inner}<w:p/></w:tc></w:tr></w:tbl>')
    d.add(para())
    return d, None


def f_table_long():
    d = Doc()
    rows = [["#", "Item", "Owner", "Status"]] + [[str(i), f"Task number {i}", ["Ana", "Ben", "Chen"][i % 3], ["Open", "Done"][i % 2]] for i in range(1, 121)]
    d.add(styled("Heading1", "Long table with repeating header"), table(rows, header=True), para())
    return d, None


def f_images():
    d = Doc()
    r1 = d.part("word/media/image1.png", "image/png", png(400, 200, "#2F5496", "inline"), f"{REL}/image")
    r2 = d.part("word/media/image2.png", "image/png", png(200, 200, "#C55A11", "float"), f"{REL}/image")
    d.add(styled("Heading1", "Images"), para(drawing(r1, 400, 200, 1)), para(text="Text after the inline picture."),
          para(drawing(r2, 200, 200, 2, floating=True), text=LOREM * 3))
    return d, None


def f_sections():
    d = Doc()
    d.add(styled("Heading1", "Portrait section"), para(text=LOREM))
    d.add(para(ppr='<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr>'))
    d.add(styled("Heading1", "Landscape section"), para(text=LOREM))
    d.add(para(ppr='<w:sectPr><w:type w:val="nextPage"/><w:pgSz w:w="15840" w:h="12240" w:orient="landscape"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr>'))
    d.add(styled("Heading1", "Two columns"), *[para(text=LOREM) for _ in range(4)])
    d.sect = ('<w:sectPr><w:type w:val="nextPage"/><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" '
              'w:header="720" w:footer="720" w:gutter="0"/><w:cols w:num="2" w:space="720"/></w:sectPr>')
    return d, {"pages": 3, "starts": ["Portrait section", "Landscape section", "Two columns"]}


def f_page_breaks():
    d = Doc()
    starts = []
    for i in range(1, 6):
        t = f"Page {i} heading"
        starts.append(t)
        d.add(styled("Heading1", t), para(text=LOREM))
        if i < 5:
            d.add(para(PAGE_BREAK))
    return d, {"pages": 5, "starts": starts}


def f_page_break_before():
    d = Doc()
    d.add(styled("Heading1", "Intro"), para(text=LOREM),
          para(ppr="<w:pageBreakBefore/>", text="Starts on page two"), para(text=LOREM),
          para(ppr="<w:pageBreakBefore/>", text="Starts on page three"))
    return d, {"pages": 3, "starts": ["Intro", "Starts on page two", "Starts on page three"]}


def f_long_flow():
    d = Doc()
    d.add(styled("Title", "Long flowing document"))
    for i in range(1, 31):
        d.add(styled("Heading2", f"Topic {i}"), para(text=LOREM * 2), para(text=LOREM))
    return d, None


def f_headers_footers():
    d = Doc()
    hr = d.part("word/header1.xml", "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml",
                hf_part("header", para(ppr='<w:pStyle w:val="Header"/>', content=run("Quarterly report") + run("\tConfidential"))), f"{REL}/header")
    fr = d.part("word/footer1.xml", "application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml",
                hf_part("footer", para(ppr='<w:pStyle w:val="Footer"/><w:jc w:val="center"/>', content=run("Page ") + field("PAGE", "1") + run(" of ") + field("NUMPAGES", "3"))), f"{REL}/footer")
    hf = d.part("word/header2.xml", "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml",
                hf_part("header", para(ppr='<w:pStyle w:val="Header"/>', text="First page header")), f"{REL}/header")
    starts = []
    for i in range(1, 4):
        starts.append(f"Part {i}")
        d.add(styled("Heading1", f"Part {i}"), para(text=LOREM))
        if i < 3:
            d.add(para(PAGE_BREAK))
    d.sect = (f'<w:sectPr><w:headerReference w:type="default" r:id="{hr}"/><w:headerReference w:type="first" r:id="{hf}"/>'
              f'<w:footerReference w:type="default" r:id="{fr}"/><w:pgSz w:w="12240" w:h="15840"/>'
              '<w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/><w:titlePg/></w:sectPr>')
    return d, {"pages": 3, "starts": starts}


def f_footnotes():
    d = Doc()
    d.add(styled("Heading1", "Footnotes and endnotes"))
    for i in range(1, 6):
        d.add(para(run(f"Claim number {i} needs a source.") + d.footnote(f"Source for claim {i}, page {i * 10}.") + run(" " + LOREM)))
    d.add(para(run("This sentence has an endnote.") + d.endnote("The endnote text.")))
    return d, None


def f_toc():
    d = Doc()
    toc = ['<w:sdt><w:sdtPr><w:docPartObj><w:docPartGallery w:val="Table of Contents"/><w:docPartUnique/></w:docPartObj></w:sdtPr><w:sdtContent>',
           para(ppr='<w:pStyle w:val="TOC1"/><w:tabs><w:tab w:val="right" w:leader="dot" w:pos="9350"/></w:tabs>',
                content='<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> TOC \\o "1-3" \\h \\z \\u </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>'
                + run("Introduction") + "<w:r><w:tab/></w:r>" + run("2")),
           para(ppr='<w:pStyle w:val="TOC1"/><w:tabs><w:tab w:val="right" w:leader="dot" w:pos="9350"/></w:tabs>', content=run("Results") + "<w:r><w:tab/></w:r>" + run("3")),
           para(content='<w:r><w:fldChar w:fldCharType="end"/></w:r>'), "</w:sdtContent></w:sdt>"]
    d.add(styled("Title", "Report with contents"), *toc, para(PAGE_BREAK),
          styled("Heading1", "Introduction"), para(text=LOREM), para(PAGE_BREAK), styled("Heading1", "Results"), para(text=LOREM))
    return d, {"pages": 3, "starts": ["Report with contents", "Introduction", "Results"]}


def f_tracked_changes():
    d = Doc()
    ins = '<w:ins w:id="1" w:author="Ana" w:date="2026-01-10T09:00:00Z">' + run("newly inserted ") + "</w:ins>"
    dele = ('<w:del w:id="2" w:author="Ben" w:date="2026-01-11T09:00:00Z"><w:r><w:delText xml:space="preserve">removed words </w:delText></w:r></w:del>')
    fmt = ('<w:r><w:rPr><w:b/><w:rPrChange w:id="3" w:author="Ana" w:date="2026-01-12T09:00:00Z"><w:rPr/></w:rPrChange></w:rPr>'
           '<w:t xml:space="preserve">now bold</w:t></w:r>')
    d.add(styled("Heading1", "Tracked changes"),
          para(run("This sentence has ") + ins + run("text and ") + dele + run("a formatting change: ") + fmt + run(".")),
          '<w:p><w:pPr><w:rPr><w:ins w:id="4" w:author="Ana" w:date="2026-01-10T09:05:00Z"/></w:rPr></w:pPr>'
          '<w:ins w:id="5" w:author="Ana" w:date="2026-01-10T09:05:00Z">' + run("A whole inserted paragraph.") + "</w:ins></w:p>",
          para(text=LOREM))
    return d, None


def f_comments():
    d = Doc()
    c0 = d.comment("Is this number right?")
    c1 = d.comment("Please rephrase.", "Editor")
    d.add(styled("Heading1", "Comments"),
          para(run("Revenue grew ") + f'<w:commentRangeStart w:id="{c0}"/>' + run("12 percent") + f'<w:commentRangeEnd w:id="{c0}"/>'
               + f'<w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:commentReference w:id="{c0}"/></w:r>' + run(" this year.")),
          para(f'<w:commentRangeStart w:id="{c1}"/>' + run(LOREM) + f'<w:commentRangeEnd w:id="{c1}"/>'
               + f'<w:r><w:commentReference w:id="{c1}"/></w:r>'))
    return d, None


def f_hyperlinks_bookmarks():
    d = Doc()
    rid = d.rel(f"{REL}/hyperlink", "https://example.com/report", external=True)
    d.add(styled("Heading1", "Links"),
          para(run("Visit ") + f'<w:hyperlink r:id="{rid}" w:history="1">' + run("the website", '<w:rStyle w:val="Hyperlink"/>') + "</w:hyperlink>" + run(".")),
          para('<w:bookmarkStart w:id="0" w:name="target"/>' + run("Bookmarked text") + '<w:bookmarkEnd w:id="0"/>'),
          para('<w:hyperlink w:anchor="target">' + run("Jump to bookmark", '<w:rStyle w:val="Hyperlink"/>') + "</w:hyperlink>"),
          para(run("Cross reference: see ") + field("REF target \\h", "Bookmarked text")))
    return d, None


def f_content_controls():
    d = Doc()
    d.add(styled("Heading1", "Content controls"),
          '<w:sdt><w:sdtPr><w:alias w:val="Client"/><w:tag w:val="client"/><w:id w:val="1001"/><w:text/></w:sdtPr><w:sdtContent>'
          + para(text="Acme Corporation") + "</w:sdtContent></w:sdt>",
          para('<w:sdt><w:sdtPr><w:id w:val="1002"/><w:date w:fullDate="2026-03-01T00:00:00Z"><w:dateFormat w:val="M/d/yyyy"/></w:date></w:sdtPr><w:sdtContent>'
               + run("3/1/2026") + "</w:sdtContent></w:sdt>"),
          para('<w:sdt><w:sdtPr><w:id w:val="1003"/><w:dropDownList><w:listItem w:displayText="Draft" w:value="draft"/>'
               '<w:listItem w:displayText="Final" w:value="final"/></w:dropDownList></w:sdtPr><w:sdtContent>' + run("Draft") + "</w:sdtContent></w:sdt>"))
    return d, None


def f_equations():
    d = Doc()
    d.add(styled("Heading1", "Equations"),
          para('<m:oMathPara><m:oMath><m:r><m:t>E</m:t></m:r><m:r><m:t>=</m:t></m:r><m:r><m:t>m</m:t></m:r>'
               '<m:sSup><m:e><m:r><m:t>c</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup></m:oMath></m:oMathPara>'),
          para(run("Inline fraction ") + '<m:oMath><m:f><m:num><m:r><m:t>a</m:t></m:r></m:num><m:den><m:r><m:t>b</m:t></m:r></m:den></m:f></m:oMath>' + run(" in text.")))
    return d, None


def f_rtl_cjk():
    d = Doc()
    d.add(styled("Heading1", "Scripts"),
          para(ppr="<w:bidi/>", content=run("مرحبا بالعالم، هذا نص عربي.", "<w:rtl/>")),
          para(ppr="<w:bidi/>", content=run("שלום עולם, זה טקסט בעברית.", "<w:rtl/>")),
          para(run("日本語のテキストです。行の折り返しを確認します。" * 4, '<w:rFonts w:eastAsia="MS Mincho"/><w:lang w:eastAsia="ja-JP"/>')),
          para(run("中文文本，用于测试换行。" * 5, '<w:rFonts w:eastAsia="SimSun"/><w:lang w:eastAsia="zh-CN"/>')))
    return d, None


def f_textbox():
    d = Doc()
    box = ('<w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing><wp:anchor distT="0" distB="0" distL="114300" distR="114300" simplePos="0" '
           'relativeHeight="1" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/>'
           '<wp:positionH relativeFrom="margin"><wp:posOffset>3200400</wp:posOffset></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV>'
           '<wp:extent cx="2286000" cy="914400"/><wp:wrapSquare wrapText="bothSides"/><wp:docPr id="10" name="Text Box 10"/>'
           '<a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp><wps:cNvSpPr txBox="1"/>'
           '<wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2286000" cy="914400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom>'
           '<a:solidFill><a:srgbClr val="FFF2CC"/></a:solidFill><a:ln w="9525"><a:solidFill><a:srgbClr val="BF9000"/></a:solidFill></a:ln></wps:spPr>'
           f'<wps:txbx><w:txbxContent>{para(text="Callout: key figure is 42.")}</w:txbxContent></wps:txbx><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic>'
           '</wp:anchor></w:drawing></mc:Choice><mc:Fallback><w:pict/></mc:Fallback></mc:AlternateContent></w:r>')
    d.add(styled("Heading1", "Text box"), para(box, text=LOREM * 3))
    return d, None


def f_styles_heavy():
    d = Doc()
    d.add(styled("Title", "Mixed formatting"),
          para(ppr='<w:shd w:val="clear" w:color="auto" w:fill="E2EFDA"/><w:pBdr><w:top w:val="single" w:sz="8" w:space="4" w:color="70AD47"/>'
               '<w:bottom w:val="single" w:sz="8" w:space="4" w:color="70AD47"/></w:pBdr>', text="Shaded paragraph with borders."),
          para(ppr='<w:tabs><w:tab w:val="left" w:pos="2880"/><w:tab w:val="decimal" w:pos="7200"/></w:tabs>',
               content=run("Item") + "<w:r><w:tab/></w:r>" + run("Category") + "<w:r><w:tab/></w:r>" + run("1,234.50")),
          para(run("Small caps", "<w:smallCaps/>") + run(" ALL CAPS", "<w:caps/>") + run(" super", '<w:vertAlign w:val="superscript"/>')
               + run(" sub", '<w:vertAlign w:val="subscript"/>') + run(" spaced", '<w:spacing w:val="40"/>')),
          para(ppr='<w:keepNext/>', text="Keep with next: this line stays with the table below."),
          table([["A", "B"], ["1", "2"]]), para(ppr='<w:widowControl w:val="0"/>', text=LOREM * 3))
    return d, None


def f_empty():
    d = Doc()
    d.add(para())
    return d, {"pages": 1, "starts": [""]}


FEATURES = {k[2:]: v for k, v in globals().items() if k.startswith("f_")}


def main():
    shutil.rmtree(OUT, ignore_errors=True)
    OUT.mkdir(parents=True)
    expected = {}
    for name, fn in FEATURES.items():
        d, exp = fn()
        d.write(OUT / f"{name}.docx")
        if exp:
            expected[f"{name}.docx"] = exp
    (OUT / "expected.json").write_text(json.dumps(expected, indent=2, ensure_ascii=False) + "\n")
    print(f"wrote {len(FEATURES)} files to {OUT} ({len(expected)} with known page breaks)")


if __name__ == "__main__":
    main()
