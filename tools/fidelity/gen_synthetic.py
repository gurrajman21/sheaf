"""Generate one synthetic .xlsx per feature group into corpus/synthetic/.

Files are written with XlsxWriter, which mimics Excel's output closely. XlsxWriter
can't calculate, so every formula in a `trust_cached` file is written with its
expected result computed here in Python. That makes those files a formula oracle.

Usage: python gen_synthetic.py [out_dir]
"""
from __future__ import annotations

import datetime as dt
import io
import json
import math
import statistics
import sys
from pathlib import Path

import xlsxwriter
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[2]
OUT = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "corpus" / "synthetic"
MANIFEST: dict[str, dict] = {}


def serial(d: dt.date) -> int:
    return (d - dt.date(1899, 12, 30)).days


def book(name: str, features: list[str], trust_cached: bool = False, **opts):
    MANIFEST[name] = {"features": features, "trust_cached": trust_cached}
    # Future functions (XLOOKUP, LET, SORT, ...) need _xlfn. prefixes or Excel shows #NAME?.
    return xlsxwriter.Workbook(str(OUT / name), {"use_future_functions": True, **opts})


class Formulas:
    """Writes label | formula | expected rows; asserts every formula has an expected value."""

    def __init__(self, ws, start_row: int = 1):
        self.ws, self.row = ws, start_row

    def add(self, label: str, formula: str, expected):
        self.ws.write(self.row, 0, label)
        self.ws.write_formula(self.row, 1, formula, None, expected)
        self.row += 1


# --------------------------------------------------------------------------- formulas

def formulas_basic():
    wb = book("formulas_basic.xlsx", ["formulas", "defined_names", "cross_sheet"], trust_cached=True)
    data = wb.add_worksheet("Data")
    ws = wb.add_worksheet("Formulas")
    lk = wb.add_worksheet("Lookup")

    nums = [12, 7.5, -3, 42, 0, 19.25, 8, 100, 3.3, 61]
    data.write_row(0, 0, ["n", "label", "date"])
    labels = ["apple", "banana", "cherry", "date", "elder", "fig", "grape", "honeydew", "kiwi", "lemon"]
    dates = [dt.date(2024, 1, 15) + dt.timedelta(days=37 * i) for i in range(10)]
    for i, (n, lab, d) in enumerate(zip(nums, labels, dates)):
        data.write_number(i + 1, 0, n)
        data.write_string(i + 1, 1, lab)
        data.write_number(i + 1, 2, serial(d))
    wb.define_name("Nums", "=Data!$A$2:$A$11")
    wb.define_name("TaxRate", "=0.2")

    ws.write_row(0, 0, ["case", "formula", ""])
    f = Formulas(ws)
    # arithmetic & aggregation
    f.add("sum", "=SUM(Data!A2:A11)", sum(nums))
    f.add("sum named", "=SUM(Nums)", sum(nums))
    f.add("average", "=AVERAGE(Nums)", statistics.mean(nums))
    f.add("min", "=MIN(Nums)", min(nums))
    f.add("max", "=MAX(Nums)", max(nums))
    f.add("count", "=COUNT(Data!A1:C11)", 20)
    f.add("counta", "=COUNTA(Data!A1:C11)", 33)
    f.add("product", "=PRODUCT(2,3.5,4)", 28)
    f.add("precedence", "=2+3*4^2/8-1", 2 + 3 * 4 ** 2 / 8 - 1)
    f.add("named const", "=100*TaxRate", 20)
    f.add("round", "=ROUND(2.675,2)", 2.68)  # Excel rounds half away from zero on decimal repr
    f.add("roundup", "=ROUNDUP(3.14159,3)", 3.142)
    f.add("rounddown", "=ROUNDDOWN(-3.14159,1)", -3.1)
    f.add("int neg", "=INT(-4.3)", -5)
    f.add("trunc", "=TRUNC(-4.7)", -4)
    f.add("mod neg", "=MOD(-7,3)", 2)
    f.add("abs", "=ABS(-12.5)", 12.5)
    f.add("power", "=POWER(2,10)", 1024)
    f.add("sqrt", "=SQRT(144)", 12)
    f.add("pi", "=PI()", math.pi)
    f.add("ln/exp", "=LN(EXP(3))", 3)
    f.add("log10", "=LOG10(1000)", 3)
    f.add("ceiling", "=CEILING(4.2,0.5)", 4.5)
    f.add("floor", "=FLOOR(4.7,0.5)", 4.5)
    f.add("sumproduct", "=SUMPRODUCT({1,2,3},{4,5,6})", 32)
    # logic
    f.add("if", '=IF(Data!A2>10,"big","small")', "big")
    f.add("nested if", '=IF(Data!A4<0,"neg",IF(Data!A4=0,"zero","pos"))', "neg")
    f.add("and/or", "=AND(OR(1>2,3>2),NOT(FALSE))", True)
    f.add("iferror", '=IFERROR(1/0,"div")', "div")
    f.add("ifs", '=IFS(Data!A5>50,"a",Data!A5>40,"b",TRUE,"c")', "b")
    f.add("switch", '=SWITCH(3,1,"one",3,"three","other")', "three")
    # errors
    f.add("div0", "=1/0", "#DIV/0!")
    f.add("na", "=NA()", "#N/A")
    f.add("value err", '=1+"x"', "#VALUE!")
    f.add("iserror", "=ISERROR(1/0)", True)
    f.add("isblank", "=ISBLANK(Data!Z99)", True)
    # text
    f.add("concat &", '=Data!B2&"-"&Data!B3', "apple-banana")
    f.add("concatenate", '=CONCATENATE("a","b","c")', "abc")
    f.add("left", '=LEFT("spreadsheet",6)', "spread")
    f.add("right", '=RIGHT("spreadsheet",5)', "sheet")
    f.add("mid", '=MID("spreadsheet",3,4)', "read")
    f.add("len", '=LEN("hello world")', 11)
    f.add("upper", '=UPPER("MiXeD")', "MIXED")
    f.add("lower", '=LOWER("MiXeD")', "mixed")
    f.add("proper", '=PROPER("hello big world")', "Hello Big World")
    f.add("trim", '=TRIM("  too   many  spaces ")', "too many spaces")
    f.add("substitute", '=SUBSTITUTE("a-b-c","-","+")', "a+b+c")
    f.add("find", '=FIND("s","spreadsheet",2)', 7)
    f.add("search", '=SEARCH("SHEET","spreadsheet")', 7)
    f.add("rept", '=REPT("ab",3)', "ababab")
    f.add("exact", '=EXACT("Abc","abc")', False)
    f.add("text num", '=TEXT(1234.5,"#,##0.00")', "1,234.50")
    f.add("text pct", '=TEXT(0.256,"0.0%")', "25.6%")
    f.add("value", '=VALUE("42.5")', 42.5)
    f.add("textjoin", '=TEXTJOIN(", ",TRUE,"a","","b")', "a, b")
    # dates (serials)
    d0 = dt.date(2024, 2, 29)
    f.add("date", "=DATE(2024,2,29)", serial(d0))
    f.add("year", "=YEAR(DATE(2024,2,29))", 2024)
    f.add("month", "=MONTH(DATE(2024,2,29))", 2)
    f.add("day", "=DAY(DATE(2024,2,29))", 29)
    f.add("date overflow", "=DATE(2024,13,1)", serial(dt.date(2025, 1, 1)))
    f.add("edate", "=EDATE(DATE(2024,1,31),1)", serial(dt.date(2024, 2, 29)))
    f.add("eomonth", "=EOMONTH(DATE(2024,2,10),0)", serial(dt.date(2024, 2, 29)))
    f.add("weekday", "=WEEKDAY(DATE(2024,2,29))", 5)  # Thursday, Sunday=1
    f.add("weekday mon", "=WEEKDAY(DATE(2024,2,29),2)", 4)
    f.add("date diff", "=DATE(2024,12,25)-DATE(2024,1,1)", 359)
    f.add("networkdays", "=NETWORKDAYS(DATE(2024,1,1),DATE(2024,1,31))", 23)
    f.add("datedif m", '=DATEDIF(DATE(2020,5,15),DATE(2024,2,29),"m")', 45)
    f.add("datedif y", '=DATEDIF(DATE(2020,5,15),DATE(2024,2,29),"y")', 3)
    f.add("time", "=TIME(18,30,0)", 18.5 / 24)
    f.add("hour", "=HOUR(0.75)", 18)
    # stats & conditional aggregation
    f.add("median", "=MEDIAN(Nums)", statistics.median(nums))
    f.add("stdev.s", "=STDEV.S(Nums)", statistics.stdev(nums))
    f.add("var.p", "=VAR.P(Nums)", statistics.pvariance(nums))
    f.add("large", "=LARGE(Nums,2)", sorted(nums)[-2])
    f.add("small", "=SMALL(Nums,3)", sorted(nums)[2])
    f.add("rank.eq", "=RANK.EQ(42,Nums)", sorted(nums, reverse=True).index(42) + 1)
    f.add("countif", '=COUNTIF(Nums,">10")', sum(1 for n in nums if n > 10))
    f.add("countif text", '=COUNTIF(Data!B2:B11,"*e*")', sum(1 for s in labels if "e" in s))
    f.add("sumif", '=SUMIF(Nums,">=8")', sum(n for n in nums if n >= 8))
    f.add("sumifs", '=SUMIFS(Data!A2:A11,Data!A2:A11,">0",Data!B2:B11,"<>fig")',
          sum(n for n, s in zip(nums, labels) if n > 0 and s != "fig"))
    f.add("countifs", '=COUNTIFS(Data!A2:A11,">5",Data!A2:A11,"<50")', sum(1 for n in nums if 5 < n < 50))
    f.add("averageif", '=AVERAGEIF(Nums,"<10")', statistics.mean([n for n in nums if n < 10]))
    f.add("maxifs", '=MAXIFS(Data!A2:A11,Data!B2:B11,"<f")', max(n for n, s in zip(nums, labels) if s < "f"))
    # financial (closed forms)
    rate, nper, pv = 0.05 / 12, 360, 300000
    pmt = -pv * rate / (1 - (1 + rate) ** -nper)
    f.add("pmt", "=PMT(0.05/12,360,300000)", pmt)
    f.add("fv", "=FV(0.04,10,-1000)", 1000 * ((1.04 ** 10 - 1) / 0.04))
    f.add("pv", "=PV(0.06,5,-2000)", 2000 * (1 - 1.06 ** -5) / 0.06)
    flows = [-1000, 300, 400, 500]
    f.add("npv", "=NPV(0.1,300,400,500)", sum(c / 1.1 ** (i + 1) for i, c in enumerate(flows[1:])))
    f.add("irr", "=IRR({-1000,300,400,500})", irr(flows))
    # references
    f.add("cross sheet", "=Data!A2*Data!A3", 12 * 7.5)
    f.add("abs ref", "=$B$2*2", sum(nums) * 2)
    f.add("indirect", '=INDIRECT("Data!A5")', 42)
    f.add("offset", "=SUM(OFFSET(Data!A1,1,0,3,1))", 12 + 7.5 - 3)
    f.add("row/col", "=ROW(Data!C7)+COLUMN(Data!C7)", 10)
    f.add("rows", "=ROWS(Nums)", 10)
    f.add("address", "=ADDRESS(3,4)", "$D$3")
    f.add("choose", '=CHOOSE(2,"x","y","z")', "y")

    # lookups
    lk.write_row(0, 0, ["code", "name", "price"])
    table = [("A1", "Widget", 2.5), ("B2", "Gadget", 10.0), ("C3", "Doohickey", 7.25), ("D4", "Gizmo", 99.0)]
    for i, row in enumerate(table):
        lk.write_row(i + 1, 0, row)
    g = Formulas(lk, start_row=7)
    g.add("vlookup exact", '=VLOOKUP("C3",A2:C5,2,FALSE)', "Doohickey")
    g.add("vlookup approx", "=VLOOKUP(8,{0,\"low\";5,\"mid\";50,\"high\"},2,TRUE)", "mid")
    g.add("hlookup", '=HLOOKUP("price",A1:C5,3,FALSE)', 10.0)
    g.add("index/match", '=INDEX(C2:C5,MATCH("Gizmo",B2:B5,0))', 99.0)
    g.add("match approx", "=MATCH(8,{1,5,10,20},1)", 2)
    g.add("xlookup", '=XLOOKUP("B2",A2:A5,C2:C5)', 10.0)
    g.add("xlookup missing", '=XLOOKUP("Z9",A2:A5,C2:C5,"none")', "none")
    g.add("vlookup missing", '=VLOOKUP("Z9",A2:C5,2,FALSE)', "#N/A")
    g.add("3d sum", "=SUM(Data:Formulas!Z1)", 0)
    wb.close()


def irr(flows, lo=-0.99, hi=1.0):
    npv = lambda r: sum(c / (1 + r) ** i for i, c in enumerate(flows))
    for _ in range(200):
        mid = (lo + hi) / 2
        if npv(lo) * npv(mid) <= 0:
            hi = mid
        else:
            lo = mid
    return (lo + hi) / 2


def formulas_dynamic():
    wb = book("formulas_dynamic.xlsx", ["formulas", "dynamic_arrays"], trust_cached=True)
    ws = wb.add_worksheet("Dynamic")
    vals = [5, 3, 8, 3, 1, 8, 2]
    ws.write_column(0, 0, vals)
    # Only the anchor cell carries an expected value; spilled cells aren't stored by XlsxWriter.
    ws.write_dynamic_array_formula("C1:C7", "=SORT(A1:A7)", None, sorted(vals)[0])
    ws.write_dynamic_array_formula("D1:D5", "=UNIQUE(A1:A7)", None, 5)
    ws.write_dynamic_array_formula("E1:E5", "=SEQUENCE(5,1,10,10)", None, 10)
    ws.write_dynamic_array_formula("F1:F2", "=FILTER(A1:A7,A1:A7>5)", None, 8)
    ws.write_formula("H1", "=LET(_xlpm.x,5,_xlpm.y,7,_xlpm.x*_xlpm.y)", None, 35)  # Excel stores params as _xlpm.*
    ws.write_formula("H2", "=SUM(SEQUENCE(10))", None, 55)
    ws.write_formula("H3", "=LAMBDA(_xlpm.a,_xlpm.b,_xlpm.a+_xlpm.b)(2,3)", None, 5)
    ws.write_array_formula("H4:H4", "{=SUM(A1:A7*A1:A7)}", None, sum(v * v for v in vals))
    ws.write_formula("H5", "=COUNTA(UNIQUE(A1:A7))", None, 5)
    wb.close()


# --------------------------------------------------------------------------- formatting

def number_formats():
    wb = book("number_formats.xlsx", ["number_formats"])
    ws = wb.add_worksheet("Formats")
    ws.set_column("A:A", 28)
    ws.set_column("B:B", 22)
    cases = [
        ("General", 1234.5678), ("0", 1234.5678), ("0.00", 1234.5678), ("#,##0", 1234567.89),
        ("#,##0.00", -1234.5), ("0%", 0.256), ("0.00%", 0.25678), ("0.00E+00", 123456789),
        ("# ?/?", 1.25), ("# ??/??", 3.14159), ("$#,##0.00", 1234.5), ("[$€-x-euro2] #,##0.00", 99.5),
        ('_($* #,##0.00_);_($* (#,##0.00);_($* "-"??_);_(@_)', -42.1),
        ("#,##0;[Red]-#,##0", -500), ('[Blue]#,##0;[Red]-#,##0;"zero"', 0),
        ("yyyy-mm-dd", serial(dt.date(2024, 7, 4))), ("dd/mm/yyyy", serial(dt.date(2024, 7, 4))),
        ("mmmm d, yyyy", serial(dt.date(2024, 7, 4))), ("ddd, mmm d", serial(dt.date(2024, 7, 4))),
        ("h:mm AM/PM", 0.6875), ("hh:mm:ss", 0.6875), ("[h]:mm", 1.75), ("yyyy-mm-dd hh:mm", 45477.5),
        ("@", "text as text"), ('0.0 "kg"', 72.35), ("000-00-0000", 123456789), ('#,##0,"K"', 1234567),
        ("[>=1000000]0.0,,\"M\";[>=1000]0.0,\"K\";0", 2500000),
    ]
    for i, (fmt, val) in enumerate(cases):
        ws.write_string(i, 0, fmt)
        cell_fmt = wb.add_format({"num_format": fmt})
        if isinstance(val, str):
            ws.write_string(i, 1, val, cell_fmt)
        else:
            ws.write_number(i, 1, val, cell_fmt)
    wb.close()


def styles():
    wb = book("styles.xlsx", ["fonts", "fills", "borders", "alignment"])
    ws = wb.add_worksheet("Styles")
    ws.set_column("A:F", 18)
    fonts = [
        {"bold": True}, {"italic": True}, {"underline": 1}, {"underline": 2}, {"font_strikeout": True},
        {"font_size": 18}, {"font_name": "Courier New"}, {"font_color": "#C00000"},
        {"font_color": "#1F77B4", "bold": True, "italic": True}, {"font_script": 1}, {"font_script": 2},
        {"font_name": "Georgia", "font_size": 9},
    ]
    for i, f in enumerate(fonts):
        ws.write(i, 0, f"font {i}", wb.add_format(f))
    fills = [
        {"bg_color": "#FFFF00"}, {"bg_color": "#D9E1F2"}, {"pattern": 6, "bg_color": "#FF0000", "fg_color": "#FFFFFF"},
        {"pattern": 18, "fg_color": "#00B050"}, {"bg_color": "#000000", "font_color": "#FFFFFF"},
    ]
    for i, f in enumerate(fills):
        ws.write(i, 1, f"fill {i}", wb.add_format(f))
    borders = [
        {"border": 1}, {"border": 2}, {"border": 5}, {"bottom": 6}, {"top": 3, "bottom": 4},
        {"border": 1, "border_color": "#FF0000"}, {"left": 7, "right": 8}, {"diag_type": 3, "diag_border": 1},
    ]
    for i, f in enumerate(borders):
        ws.write(i * 2, 2, f"border {i}", wb.add_format(f))
    aligns = [
        {"align": "center"}, {"align": "right"}, {"align": "fill"}, {"align": "justify"},
        {"valign": "top"}, {"valign": "vcenter"}, {"text_wrap": True}, {"indent": 2},
        {"rotation": 45}, {"rotation": 90}, {"shrink": True}, {"align": "center_across"},
    ]
    for i, f in enumerate(aligns):
        ws.set_row(i, 36)
        ws.write(i, 3, f"align {i} with some longer text", wb.add_format(f))
    ws.write("E1", "locked off", wb.add_format({"locked": False}))
    ws.write("E2", "hidden formula", wb.add_format({"hidden": True}))
    wb.close()


def layout():
    wb = book("layout.xlsx", ["col_widths", "row_heights", "hidden_rows_cols", "frozen_panes",
                              "merged_cells", "outline", "tab_colors", "hidden_sheets"])
    ws = wb.add_worksheet("Layout")
    ws.set_column("A:A", 30)
    ws.set_column("B:C", 5)
    ws.set_column("D:D", 12, None, {"hidden": True})
    ws.set_row(2, 40)
    ws.set_row(4, None, None, {"hidden": True})
    ws.freeze_panes(1, 1)
    ws.set_tab_color("#FF9900")
    for r in range(30):
        for c in range(8):
            ws.write(r, c, f"r{r}c{c}")
    ws.merge_range("F2:H4", "merged block", wb.add_format({"align": "center", "valign": "vcenter"}))
    ws.merge_range("A20:E20", "merged row")
    for r in range(10, 15):
        ws.set_row(r, None, None, {"level": 1})
    ws.set_row(15, None, None, {"collapsed": True})
    ws.set_column("F:G", 10, None, {"level": 1})
    ws.set_zoom(85)
    ws2 = wb.add_worksheet("NoGrid")
    ws2.hide_gridlines(2)
    ws2.write("A1", "gridlines hidden")
    ws2.split_panes(15, 8.43)
    hidden = wb.add_worksheet("Hidden")
    hidden.write("A1", "secret")
    hidden.hide()
    vh = wb.add_worksheet("VeryHidden")
    vh.very_hidden()
    wb.close()


def conditional_formatting():
    wb = book("conditional_formatting.xlsx", ["conditional_formatting"])
    ws = wb.add_worksheet("CF")
    for r in range(20):
        ws.write_row(r, 0, [(r * 7) % 23, (r * 13) % 31 - 10, r, (r * 5) % 17, r % 4, (r * 3) % 11])
    red = wb.add_format({"bg_color": "#FFC7CE", "font_color": "#9C0006"})
    green = wb.add_format({"bg_color": "#C6EFCE", "font_color": "#006100"})
    ws.conditional_format("A1:A20", {"type": "cell", "criteria": ">", "value": 15, "format": red})
    ws.conditional_format("A1:A20", {"type": "cell", "criteria": "between", "minimum": 3, "maximum": 8, "format": green})
    ws.conditional_format("B1:B20", {"type": "3_color_scale"})
    ws.conditional_format("C1:C20", {"type": "data_bar", "bar_color": "#638EC6"})
    ws.conditional_format("D1:D20", {"type": "icon_set", "icon_style": "3_traffic_lights"})
    ws.conditional_format("E1:E20", {"type": "duplicate", "format": red})
    ws.conditional_format("F1:F20", {"type": "top", "value": 3, "format": green})
    ws.conditional_format("A1:F20", {"type": "formula", "criteria": "=MOD(ROW(),5)=0",
                                     "format": wb.add_format({"bold": True})})
    ws.conditional_format("C1:C20", {"type": "2_color_scale", "min_color": "#FFFFFF", "max_color": "#FF0000"})
    ws.conditional_format("D1:D20", {"type": "average", "criteria": "above", "format": green})
    ws.conditional_format("B1:B20", {"type": "data_bar", "bar_solid": True, "bar_negative_color": "#FF0000"})  # x14 ext
    wb.close()


def data_validation():
    wb = book("data_validation.xlsx", ["data_validation"])
    ws = wb.add_worksheet("Validation")
    ws.write_column("H1", ["Red", "Green", "Blue"])
    ws.data_validation("A1:A10", {"validate": "list", "source": ["open", "closed", "pending"]})
    ws.data_validation("B1:B10", {"validate": "list", "source": "=$H$1:$H$3"})
    ws.data_validation("C1:C10", {"validate": "integer", "criteria": "between", "minimum": 1, "maximum": 10,
                                  "input_title": "Pick 1-10", "input_message": "Whole number please",
                                  "error_title": "Nope", "error_message": "Must be 1-10"})
    ws.data_validation("D1:D10", {"validate": "decimal", "criteria": ">", "value": 0.5})
    ws.data_validation("E1:E10", {"validate": "date", "criteria": ">=", "value": dt.date(2024, 1, 1)})
    ws.data_validation("F1:F10", {"validate": "length", "criteria": "<=", "value": 5})
    ws.data_validation("G1:G10", {"validate": "custom", "value": "=ISNUMBER(G1)", "error_type": "warning"})
    wb.close()


# --------------------------------------------------------------------------- objects

def charts():
    wb = book("charts.xlsx", ["charts"])
    ws = wb.add_worksheet("Data")
    ws.write_row(0, 0, ["Month", "Sales", "Costs", "Units"])
    for i, m in enumerate(["Jan", "Feb", "Mar", "Apr", "May", "Jun"]):
        ws.write_row(i + 1, 0, [m, 100 + i * 17, 80 + (i % 3) * 12, 5 + i])
    def series(chart, col, name):
        chart.add_series({"name": f"=Data!${col}$1", "categories": "=Data!$A$2:$A$7",
                          "values": f"=Data!${col}$2:${col}$7"})
    for i, (typ, sub) in enumerate([("column", None), ("bar", "stacked"), ("line", None), ("pie", None),
                                    ("area", None), ("scatter", "straight_with_markers"), ("doughnut", None),
                                    ("radar", None)]):
        ch = wb.add_chart({"type": typ, **({"subtype": sub} if sub else {})})
        series(ch, "B", "Sales")
        if typ not in ("pie", "doughnut"):
            series(ch, "C", "Costs")
        ch.set_title({"name": f"{typ} chart"})
        ws.insert_chart(1 + (i // 2) * 16, 5 + (i % 2) * 8, ch)
    combo = wb.add_chart({"type": "column"})
    series(combo, "B", "Sales")
    line = wb.add_chart({"type": "line"})
    line.add_series({"name": "Units", "categories": "=Data!$A$2:$A$7", "values": "=Data!$D$2:$D$7", "y2_axis": True})
    combo.combine(line)
    combo.set_title({"name": "combo"})
    ws.insert_chart("F66", combo)
    cs = wb.add_chartsheet("ChartSheet")
    big = wb.add_chart({"type": "column"})
    series(big, "B", "Sales")
    cs.set_chart(big)
    wb.close()


def tables_filters():
    wb = book("tables_filters.xlsx", ["tables", "autofilter", "structured_refs"], trust_cached=True)
    ws = wb.add_worksheet("Table")
    rows = [["North", 120, 3], ["South", 80, 5], ["East", 200, 2], ["West", 150, 4]]
    ws.add_table("A1:D5", {
        "name": "Sales", "data": rows, "total_row": False,
        "columns": [{"header": "Region"}, {"header": "Amount"}, {"header": "Qty"},
                    {"header": "Total", "formula": "=[@Amount]*[@Qty]"}],
        "style": "Table Style Medium 9",
    })
    # Structured-ref column values aren't written by XlsxWriter; F-column formulas carry expected values.
    ws.write_formula("F1", "=SUM(Sales[Amount])", None, 550)
    ws.write_formula("F2", "=SUMPRODUCT(Sales[Amount],Sales[Qty])", None, 120 * 3 + 80 * 5 + 200 * 2 + 150 * 4)
    ws.write_formula("F3", "=ROWS(Sales)", None, 4)
    MANIFEST["tables_filters.xlsx"]["skip_cells"] = ["Table!D2", "Table!D3", "Table!D4", "Table!D5"]
    ws2 = wb.add_worksheet("Filter")
    ws2.write_row(0, 0, ["Name", "Score"])
    for i, (n, s) in enumerate([("a", 5), ("b", 9), ("c", 2), ("d", 7)]):
        ws2.write_row(i + 1, 0, [n, s])
    ws2.autofilter("A1:B5")
    ws2.filter_column("B", "x > 4")
    ws2.set_row(3, None, None, {"hidden": True})
    wb.close()


def comments_links():
    wb = book("comments_links.xlsx", ["comments", "hyperlinks"])
    ws = wb.add_worksheet("Notes")
    ws.write("A1", "has a note")
    ws.write_comment("A1", "This is a note\nwith two lines", {"author": "Tester"})
    ws.write("A3", "visible note")
    ws.write_comment("A3", "Always shown", {"visible": True, "x_scale": 1.5})
    ws.write_url("B1", "https://www.libreoffice.org", string="external link")
    ws.write_url("B2", "internal:'Notes'!A10", string="internal link")
    ws.write_url("B3", "mailto:someone@example.com", string="mail link")
    ws.write_url("B4", "https://example.com/path?q=1#frag", tip="with tooltip")
    wb.close()


def images():
    wb = book("images.xlsx", ["images"])
    ws = wb.add_worksheet("Images")
    img = Image.new("RGB", (240, 120), "#2E75B6")
    ImageDraw.Draw(img).text((20, 50), "test image", fill="white")
    buf = io.BytesIO()
    img.save(buf, "PNG")
    ws.insert_image("B2", "logo.png", {"image_data": io.BytesIO(buf.getvalue())})
    ws.insert_image("F10", "scaled.png", {"image_data": io.BytesIO(buf.getvalue()), "x_scale": 0.5, "y_scale": 2})
    ws.write("A1", "two images below")
    wb.close()


def print_setup():
    wb = book("print_setup.xlsx", ["print_setup"])
    ws = wb.add_worksheet("Print")
    for r in range(120):
        ws.write_row(r, 0, [f"row {r}", r, r * r])
    ws.set_landscape()
    ws.set_paper(9)
    ws.set_margins(left=0.5, right=0.5, top=1, bottom=1)
    ws.set_header("&LLeft header&CPage &P of &N&RDate &D")
    ws.set_footer("&CConfidential")
    ws.print_area("A1:C100")
    ws.repeat_rows(0)
    ws.set_h_pagebreaks([40, 80])
    ws.fit_to_pages(1, 0)
    ws.center_horizontally()
    ws.hide_gridlines(0)  # 0 = show on screen and print
    wb.close()


def protection():
    wb = book("protection.xlsx", ["sheet_protection"])
    ws = wb.add_worksheet("Protected")
    ws.write("A1", "locked")
    ws.write("A2", "editable", wb.add_format({"locked": False}))
    ws.protect("pw", {"format_cells": True, "sort": True})
    wb.add_worksheet("Open").write("A1", "not protected")
    wb.close()


def sparklines():
    wb = book("sparklines.xlsx", ["sparklines"])
    ws = wb.add_worksheet("Spark")
    for r in range(5):
        ws.write_row(r, 0, [(r * 3 + c * 7) % 11 - 3 for c in range(8)])
        ws.add_sparkline(r, 9, {"range": f"Spark!A{r + 1}:H{r + 1}",
                                "type": ["line", "column", "win_loss"][r % 3], "markers": True})
    wb.close()


def rich_text():
    wb = book("rich_text.xlsx", ["rich_text", "unicode"])
    ws = wb.add_worksheet("Text")
    bold = wb.add_format({"bold": True})
    red = wb.add_format({"font_color": "red", "italic": True})
    ws.write_rich_string("A1", "plain ", bold, "bold ", red, "red italic", " end")
    ws.write("A2", "Emoji 🚀📊✅")
    ws.write("A3", "مرحبا بالعالم")  # RTL
    ws.write("A4", "中文 日本語 한국어")
    ws.write("A5", "line one\nline two", wb.add_format({"text_wrap": True}))
    ws.write("A6", "x" * 5000)
    ws.write("A7", "  leading and trailing spaces  ")
    ws.write_string("A8", "=not a formula")
    ws.write_string("A9", "00123")
    ws.write("A10", 'quotes "double" and \'single\' & <xml> chars')
    wb.close()


def large_sheet():
    wb = book("large_sheet.xlsx", ["large", "formulas", "performance"], trust_cached=True, constant_memory=True)
    ws = wb.add_worksheet("Big")
    ws.write_row(0, 0, ["id", "a", "b", "c", "sum", "running"])
    running = 0
    for r in range(1, 50001):
        a, b, c = r % 97, (r * 31) % 89, (r * 7) % 13
        running += a + b + c
        ws.write_row(r, 0, [r, a, b, c])
        ws.write_formula(r, 4, f"=B{r + 1}+C{r + 1}+D{r + 1}", None, a + b + c)
        ws.write_formula(r, 5, f"=E{r + 1}+F{r}" if r > 1 else "=E2", None, running)
    wb.close()


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    for fn in (formulas_basic, formulas_dynamic, number_formats, styles, layout, conditional_formatting,
               data_validation, charts, tables_filters, comments_links, images, print_setup, protection,
               sparklines, rich_text, large_sheet):
        fn()
        print("wrote", fn.__name__)
    (OUT / "manifest.json").write_text(json.dumps(MANIFEST, indent=2))
    print(f"{len(MANIFEST)} files -> {OUT}")


if __name__ == "__main__":
    main()
