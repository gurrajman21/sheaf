"""Engine-independent reader for .xlsx/.xlsm packages.

Reads the raw XML (no openpyxl conversions) so original files and engine
round-trips are judged by the same yardstick:

- every cell: cached value, whether it holds a formula, resolved style
- layout: merged ranges, column widths, frozen panes, hidden rows/cols
- a feature inventory: charts, pivots, tables, comments, CF rules, ...
"""
from __future__ import annotations

import colorsys
import posixpath
import re
import zipfile
from dataclasses import dataclass, field
from xml.etree import ElementTree as ET


def local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def rel_id(el) -> str | None:
    for k, v in el.attrib.items():
        if k.endswith("}id"):
            return v
    return None


def col_to_num(letters: str) -> int:
    n = 0
    for ch in letters:
        n = n * 26 + (ord(ch) - 64)
    return n


def num_to_col(n: int) -> str:
    s = ""
    while n:
        n, r = divmod(n - 1, 26)
        s = chr(65 + r) + s
    return s


_REF = re.compile(r"^\$?([A-Z]{1,3})\$?(\d+)$")


def parse_ref(ref: str) -> tuple[int, int]:
    m = _REF.match(ref.upper())
    if not m:
        raise ValueError(f"bad cell ref {ref!r}")
    return int(m.group(2)), col_to_num(m.group(1))


# --------------------------------------------------------------------------- styles

BUILTIN_NUMFMTS = {
    0: "General", 1: "0", 2: "0.00", 3: "#,##0", 4: "#,##0.00", 9: "0%", 10: "0.00%",
    11: "0.00E+00", 12: "# ?/?", 13: "# ??/??", 14: "mm-dd-yy", 15: "d-mmm-yy", 16: "d-mmm",
    17: "mmm-yy", 18: "h:mm AM/PM", 19: "h:mm:ss AM/PM", 20: "h:mm", 21: "h:mm:ss",
    22: "m/d/yy h:mm", 37: "#,##0 ;(#,##0)", 38: "#,##0 ;[Red](#,##0)",
    39: "#,##0.00;(#,##0.00)", 40: "#,##0.00;[Red](#,##0.00)", 45: "mm:ss", 46: "[h]:mm:ss",
    47: "mmss.0", 48: "##0.0E+0", 49: "@",
}

# Default legacy indexed palette (indices 0-63); 64/65 are system fg/bg.
INDEXED = (
    "000000 FFFFFF FF0000 00FF00 0000FF FFFF00 FF00FF 00FFFF "
    "000000 FFFFFF FF0000 00FF00 0000FF FFFF00 FF00FF 00FFFF "
    "800000 008000 000080 808000 800080 008080 C0C0C0 808080 "
    "9999FF 993366 FFFFCC CCFFFF 660066 FF8080 0066CC CCCCFF "
    "000080 FF00FF FFFF00 00FFFF 800080 800000 008080 0000FF "
    "00CCFF CCFFFF CCFFCC FFFF99 99CCFF FF99CC CC99FF FFCC99 "
    "3366FF 33CCCC 99CC00 FFCC00 FF9900 FF6600 666699 969696 "
    "003366 339966 003300 333300 993300 993366 333399 333333"
).split() + ["000000", "FFFFFF"]

# Excel swaps lt/dk pairs: theme index 0 -> lt1, 1 -> dk1, 2 -> lt2, 3 -> dk2.
_THEME_ORDER = ["lt1", "dk1", "lt2", "dk2", "accent1", "accent2", "accent3",
                "accent4", "accent5", "accent6", "hlink", "folHlink"]


def normalize_numfmt(code: str) -> str:
    return code.replace("\\", "").replace('"', "").lower().strip()


def apply_tint(rgb: str, tint: float) -> str:
    r, g, b = (int(rgb[i:i + 2], 16) / 255 for i in (0, 2, 4))
    h, l, s = colorsys.rgb_to_hls(r, g, b)
    l = l * (1 + tint) if tint < 0 else l + (1 - l) * tint
    r, g, b = colorsys.hls_to_rgb(h, l, s)
    return "".join(f"{round(x * 255):02X}" for x in (r, g, b))


@dataclass
class Styles:
    numfmts: dict[int, str] = field(default_factory=dict)
    fonts: list[tuple] = field(default_factory=list)
    fills: list[tuple] = field(default_factory=list)
    borders: list[tuple] = field(default_factory=list)
    xfs: list[dict] = field(default_factory=list)
    counts: dict[str, int] = field(default_factory=dict)

    def signature(self, s: int) -> dict[str, tuple]:
        """Resolved style of xf index `s`, split into comparable components."""
        xf = self.xfs[s] if 0 <= s < len(self.xfs) else (self.xfs[0] if self.xfs else {})
        num_id = xf.get("numFmtId", 0)
        code = self.numfmts.get(num_id, BUILTIN_NUMFMTS.get(num_id, f"builtin:{num_id}"))

        def pick(lst, i):
            return lst[i] if 0 <= i < len(lst) else (lst[0] if lst else ())

        return {
            "numfmt": (normalize_numfmt(code),),
            "font": pick(self.fonts, xf.get("fontId", 0)),
            "fill": pick(self.fills, xf.get("fillId", 0)),
            "border": pick(self.borders, xf.get("borderId", 0)),
            "alignment": xf.get("alignment", ()),
        }


def _color(el, theme: list[str], default: str | None = None) -> str | None:
    if el is None:
        return default
    a = el.attrib
    rgb = None
    if "rgb" in a:
        rgb = a["rgb"][-6:].upper()
    elif "theme" in a:
        i = int(a["theme"])
        rgb = theme[i] if i < len(theme) else None
    elif "indexed" in a:
        i = int(a["indexed"])
        rgb = INDEXED[i] if i < len(INDEXED) else None
    elif a.get("auto") in ("1", "true"):
        return default
    if rgb and "tint" in a:
        try:
            rgb = apply_tint(rgb, float(a["tint"]))
        except ValueError:
            pass
    return rgb or default


def _child(el, name):
    for c in el:
        if local(c.tag) == name:
            return c
    return None


def _flag(el, name) -> bool:
    c = _child(el, name)
    return c is not None and c.attrib.get("val", "1") not in ("0", "false")


def parse_theme(data: bytes | None) -> list[str]:
    if not data:
        return []
    root = ET.fromstring(data)
    scheme = {}
    for el in root.iter():
        if local(el.tag) == "clrScheme":
            for slot in el:
                name = local(slot.tag)
                for c in slot:
                    t = local(c.tag)
                    if t == "srgbClr":
                        scheme[name] = c.attrib.get("val", "000000").upper()
                    elif t == "sysClr":
                        scheme[name] = c.attrib.get("lastClr", "000000").upper()
            break
    return [scheme.get(n, "000000") for n in _THEME_ORDER]


def parse_styles(data: bytes | None, theme: list[str]) -> Styles:
    st = Styles()
    if not data:
        return st
    root = ET.fromstring(data)
    for sect in root:
        name = local(sect.tag)
        kids = list(sect)
        st.counts[name] = len(kids)
        if name == "numFmts":
            for nf in kids:
                st.numfmts[int(nf.attrib.get("numFmtId", 0))] = nf.attrib.get("formatCode", "")
        elif name == "fonts":
            for f in kids:
                u = _child(f, "u")
                sz = _child(f, "sz")
                nm = _child(f, "name")
                st.fonts.append((
                    ("b", _flag(f, "b")), ("i", _flag(f, "i")), ("strike", _flag(f, "strike")),
                    ("u", (u.attrib.get("val", "single") if u is not None else "none")),
                    ("sz", float(sz.attrib.get("val", 11)) if sz is not None else 11.0),
                    ("name", (nm.attrib.get("val", "") if nm is not None else "").lower()),
                    ("color", _color(_child(f, "color"), theme, "000000")),
                ))
        elif name == "fills":
            for f in kids:
                p = _child(f, "patternFill")
                g = _child(f, "gradientFill")
                if g is not None:
                    st.fills.append(("gradient",))
                elif p is None or p.attrib.get("patternType", "none") == "none":
                    st.fills.append(("none",))
                else:
                    pt = p.attrib.get("patternType", "solid")
                    st.fills.append((pt, _color(_child(p, "fgColor"), theme),
                                     None if pt == "solid" else _color(_child(p, "bgColor"), theme)))
        elif name == "borders":
            for b in kids:
                sides = []
                for side in ("left", "right", "top", "bottom"):
                    e = _child(b, side)  # Element truthiness is child count, so no `or` here
                    if e is None and side in ("left", "right"):
                        e = _child(b, {"left": "start", "right": "end"}[side])
                    style = e.attrib.get("style", "none") if e is not None else "none"
                    color = _color(_child(e, "color"), theme, "000000") if style != "none" else None
                    sides.append((side, style, color))
                st.borders.append(tuple(sides))
        elif name == "cellXfs":
            for xf in kids:
                a = xf.attrib
                al = _child(xf, "alignment")
                align = ()
                if al is not None:
                    keys = ("horizontal", "vertical", "wrapText", "indent", "textRotation", "shrinkToFit")
                    defaults = {"horizontal": "general", "vertical": "bottom", "wrapText": "0",
                                "indent": "0", "textRotation": "0", "shrinkToFit": "0"}
                    norm = lambda k, v: {"true": "1", "false": "0"}.get(v, v)
                    align = tuple((k, norm(k, al.attrib[k])) for k in keys
                                  if k in al.attrib and norm(k, al.attrib[k]) != defaults[k])
                st.xfs.append({
                    "numFmtId": int(a.get("numFmtId", 0)), "fontId": int(a.get("fontId", 0)),
                    "fillId": int(a.get("fillId", 0)), "borderId": int(a.get("borderId", 0)),
                    "alignment": align,
                })
    return st


# --------------------------------------------------------------------------- package

# content-type fragment -> inventory key
CONTENT_TYPES = [
    ("drawingml.chart+xml", "charts"),
    ("spreadsheetml.chartsheet+xml", "chartsheets"),
    ("officedocument.drawing+xml", "drawings"),
    ("spreadsheetml.pivotTable+xml", "pivot_tables"),
    ("pivotCacheDefinition+xml", "pivot_caches"),
    ("spreadsheetml.table+xml", "tables"),
    ("spreadsheetml.comments+xml", "comment_parts"),
    ("threadedcomments+xml", "threaded_comments"),
    ("vbaProject", "vba_project"),
    ("externalLink+xml", "external_links"),
    ("slicer+xml", "slicers"),
    ("slicerCache+xml", "slicer_caches"),
    ("timeline+xml", "timelines"),
    ("controlproperties+xml", "form_controls"),
    ("activeX", "activex"),
    ("connections+xml", "connections"),
    ("queryTable+xml", "query_tables"),
    ("vmlDrawing", "vml_drawings"),
]

# worksheet element local-name -> inventory key (counted across all sheets)
SHEET_ELEMENTS = {
    "mergeCell": "merged_ranges",
    "cfRule": "cf_rules",
    "dataValidation": "data_validations",
    "hyperlink": "hyperlinks",
    "autoFilter": "autofilters",
    "sheetProtection": "sheet_protection",
    "sparkline": "sparklines",
    "tablePart": "table_refs",
    "brk": "page_breaks",
    "headerFooter": "header_footer",
    "tabColor": "tab_colors",
    "outlinePr": "outline_settings",
}


@dataclass
class Cell:
    value: tuple | None          # (kind, value): kind in n/s/b/e ; None if no cached value
    formula: str | None = None   # None = constant; "" = formula text not stored here (shared child)
    ftype: str | None = None     # shared / array / dataTable
    style: int = 0


@dataclass
class Sheet:
    name: str
    state: str = "visible"
    cells: dict[tuple[int, int], Cell] = field(default_factory=dict)
    merged: set[str] = field(default_factory=set)
    col_widths: dict[int, float] = field(default_factory=dict)   # custom widths only
    hidden_cols: set[int] = field(default_factory=set)
    hidden_rows: set[int] = field(default_factory=set)
    row_heights: dict[int, float] = field(default_factory=dict)  # customHeight rows only
    frozen: tuple[int, int] | None = None                         # (cols, rows)
    array_ranges: list[str] = field(default_factory=list)          # refs of array/dynamic formulas

    def in_array_output(self, pos: tuple[int, int]) -> bool:
        """True for cells filled by an array formula (but not its anchor): values, not constants."""
        r, c = pos
        for ref in self.array_ranges:
            if ":" not in ref:
                continue
            (r1, c1), (r2, c2) = (parse_ref(x) for x in ref.split(":"))
            if r1 <= r <= r2 and c1 <= c <= c2 and (r, c) != (r1, c1):
                return True
        return False


@dataclass
class Package:
    path: str
    sheets: list[Sheet]
    styles: Styles
    inventory: dict[str, int]
    defined_names: set[str]
    application: str
    has_calc_chain: bool

    def sheet(self, name: str) -> Sheet | None:
        for s in self.sheets:
            if s.name == name:
                return s
        return None


def _resolve(base_dir: str, target: str) -> str:
    if target.startswith("/"):
        return target[1:]
    return posixpath.normpath(posixpath.join(base_dir, target))


def _rels(z: zipfile.ZipFile, part: str) -> dict[str, tuple[str, str]]:
    d, f = posixpath.split(part)
    rp = posixpath.join(d, "_rels", f + ".rels")
    if rp not in z.namelist():
        return {}
    out = {}
    for r in ET.fromstring(z.read(rp)):
        if r.attrib.get("TargetMode") == "External":
            continue
        out[r.attrib["Id"]] = (r.attrib.get("Type", ""), _resolve(d, r.attrib.get("Target", "")))
    return out


def _shared_strings(data: bytes | None) -> list[str]:
    if not data:
        return []
    out = []
    for si in ET.fromstring(data):
        out.append(_text_of(si))
    return out


def _text_of(el) -> str:
    """Concatenate <t> text, skipping phonetic runs (<rPh>)."""
    parts = []
    for c in el:
        t = local(c.tag)
        if t == "t":
            parts.append(c.text or "")
        elif t == "r":
            for rc in c:
                if local(rc.tag) == "t":
                    parts.append(rc.text or "")
    return "".join(parts)


def _parse_sheet(stream, sheet: Sheet, sst: list[str], inv: dict[str, int]) -> None:
    row = 0
    col = 0
    shared_masters: set[str] = set()
    shared_members: list[str] = []
    for event, el in ET.iterparse(stream, events=("start", "end")):
        tag = local(el.tag)
        if event == "start":
            if tag == "row":
                row = int(el.attrib["r"]) if "r" in el.attrib else row + 1
                col = 0
            continue
        # end events
        if tag == "c":
            ref = el.attrib.get("r")
            if ref:
                r, col = parse_ref(ref)
            else:
                r, col = row, col + 1
            t = el.attrib.get("t", "n")
            v_el = f_el = is_el = None
            for c in el:
                lt = local(c.tag)
                if lt == "v":
                    v_el = c
                elif lt == "f":
                    f_el = c
                elif lt == "is":
                    is_el = c
            value = None
            raw = v_el.text if v_el is not None else None
            try:
                if t == "inlineStr" and is_el is not None:
                    value = ("s", _text_of(is_el))
                elif raw is not None:
                    if t == "s":
                        value = ("s", sst[int(raw)])
                    elif t in ("str", "inlineStr"):
                        value = ("s", raw)
                    elif t == "b":
                        value = ("b", raw.strip() in ("1", "true"))
                    elif t == "e":
                        value = ("e", raw.strip())
                    elif t == "d":
                        value = ("d", raw.strip())
                    else:
                        value = ("n", float(raw))
            except (ValueError, IndexError):
                value = ("?", raw)
            formula = ftype = None
            if f_el is not None:
                formula = f_el.text or ""
                ftype = f_el.attrib.get("t")
                inv["formula_cells"] = inv.get("formula_cells", 0) + 1
                if ftype == "array":
                    inv["array_formulas"] = inv.get("array_formulas", 0) + 1
                    if f_el.attrib.get("ref"):
                        sheet.array_ranges.append(f_el.attrib["ref"].upper())
                elif ftype == "shared":
                    si = f_el.attrib.get("si", "")
                    if f_el.attrib.get("ref") and formula.strip():
                        shared_masters.add(si)
                    else:
                        shared_members.append(si)
            style = int(el.attrib.get("s", 0))
            if value is not None or formula is not None or style:
                sheet.cells[(r, col)] = Cell(value, formula, ftype, style)
            el.clear()
        elif tag == "row":
            a = el.attrib
            if a.get("hidden") in ("1", "true"):
                sheet.hidden_rows.add(row)
            if a.get("customHeight") in ("1", "true") and "ht" in a:
                sheet.row_heights[row] = float(a["ht"])
            el.clear()
        elif tag == "col":
            a = el.attrib
            lo, hi = int(a.get("min", 1)), int(a.get("max", 1))
            for c in range(lo, min(hi, lo + 256) + 1):  # cap whole-row col specs
                if a.get("customWidth") in ("1", "true") and "width" in a:
                    sheet.col_widths[c] = float(a["width"])
                if a.get("hidden") in ("1", "true"):
                    sheet.hidden_cols.add(c)
        elif tag == "mergeCell":
            sheet.merged.add(el.attrib.get("ref", "").upper())
            inv["merged_ranges"] = inv.get("merged_ranges", 0) + 1
        elif tag == "pane":
            if el.attrib.get("state") in ("frozen", "frozenSplit"):
                sheet.frozen = (int(float(el.attrib.get("xSplit", 0))), int(float(el.attrib.get("ySplit", 0))))
                inv["frozen_panes"] = inv.get("frozen_panes", 0) + 1
        elif tag in SHEET_ELEMENTS and tag != "mergeCell":
            key = SHEET_ELEMENTS[tag]
            inv[key] = inv.get(key, 0) + 1
    # integrity: shared-formula members whose master is gone make Excel "repair" the file
    orphans = sum(1 for si in shared_members if si not in shared_masters)
    if orphans:
        inv["shared_formula_orphans"] = inv.get("shared_formula_orphans", 0) + orphans


def read_package(path: str) -> Package:
    with zipfile.ZipFile(path) as z:
        names = set(z.namelist())
        read = lambda p: z.read(p) if p in names else None

        # locate workbook via package rels
        wb_path = "xl/workbook.xml"
        for _, (typ, target) in _rels(z, "").items() if "_rels/.rels" in names else []:
            if typ.endswith("/officeDocument"):
                wb_path = target
        wb_dir = posixpath.dirname(wb_path)
        wb_rels = _rels(z, wb_path)

        theme_xml = sst_xml = styles_xml = None
        for typ, target in wb_rels.values():
            if typ.endswith("/theme"):
                theme_xml = read(target)
            elif typ.endswith("/sharedStrings"):
                sst_xml = read(target)
            elif typ.endswith("/styles"):
                styles_xml = read(target)
        theme = parse_theme(theme_xml)
        styles = parse_styles(styles_xml, theme)
        sst = _shared_strings(sst_xml)

        inv: dict[str, int] = {}
        wb = ET.fromstring(z.read(wb_path))
        sheets: list[Sheet] = []
        defined: set[str] = set()
        for el in wb.iter():
            t = local(el.tag)
            if t == "sheet":
                rid = rel_id(el)
                typ, target = wb_rels.get(rid, ("", ""))
                if not typ.endswith("/worksheet"):
                    continue
                sh = Sheet(el.attrib.get("name", "?"), el.attrib.get("state", "visible"))
                if target in names:
                    with z.open(target) as fh:
                        _parse_sheet(fh, sh, sst, inv)
                sheets.append(sh)
                if sh.state != "visible":
                    inv["hidden_sheets"] = inv.get("hidden_sheets", 0) + 1
            elif t == "definedName":
                if el.attrib.get("name", "").startswith("_xlnm."):
                    inv["print_settings_names"] = inv.get("print_settings_names", 0) + 1
                else:
                    defined.add(f'{el.attrib.get("localSheetId", "")}|{el.attrib.get("name", "").lower()}')
                    inv["defined_names"] = inv.get("defined_names", 0) + 1
            elif t == "workbookProtection":
                inv["workbook_protection"] = 1
        inv["sheets"] = len(sheets)

        # content-type based part counts
        ct = ET.fromstring(z.read("[Content_Types].xml")) if "[Content_Types].xml" in names else None
        defaults, overrides = {}, {}
        if ct is not None:
            for el in ct:
                if local(el.tag) == "Default":
                    defaults[el.attrib.get("Extension", "").lower()] = el.attrib.get("ContentType", "")
                elif local(el.tag) == "Override":
                    overrides[el.attrib.get("PartName", "").lstrip("/")] = el.attrib.get("ContentType", "")
        for n in names:
            ctype = overrides.get(n) or defaults.get(n.rsplit(".", 1)[-1].lower(), "")
            if ctype.startswith("image/") and not n.startswith("docProps/"):
                inv["images"] = inv.get("images", 0) + 1
                continue
            for frag, key in CONTENT_TYPES:
                if frag.lower() in ctype.lower():
                    inv[key] = inv.get(key, 0) + 1
                    break
            if ctype.lower().endswith("spreadsheetml.comments+xml"):
                inv["comments"] = inv.get("comments", 0) + sum(
                    1 for e in ET.fromstring(z.read(n)).iter() if local(e.tag) == "comment")

        app = ""
        if "docProps/app.xml" in names:
            for el in ET.fromstring(z.read("docProps/app.xml")):
                if local(el.tag) == "Application":
                    app = el.text or ""
        has_calc_chain = any(n.lower().endswith("calcchain.xml") for n in names)

    return Package(path, sheets, styles, inv, defined, app, has_calc_chain)


def origin(pkg: Package) -> str:
    """Who wrote this file. Only 'excel' files have cached values we treat as ground truth."""
    a = pkg.application.lower()
    if "libreoffice" in a or "openoffice" in a:
        return "libreoffice"
    if "microsoft" in a and pkg.has_calc_chain:
        return "excel"
    return "other"
