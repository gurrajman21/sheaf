//! Charts: read what a workbook's charts show and where they sit, and write
//! chart edits (moves, deletes, new charts) back into the package. IronCalc
//! doesn't model charts, so they live here at the package level.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::package::{append_to_root, rels_path, remove_elements, resolve, Package};
use crate::patch::replace_child;
use crate::xml::{self, attr, set_attr, tokens, Kind, Tok};

pub const EMU_PER_PX: i64 = 9525;
const NS_C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const NS_XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
const REL_DRAWING: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing";
const REL_CHART: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart";
const CT_DRAWING: &str = "application/vnd.openxmlformats-officedocument.drawing+xml";
const CT_CHART: &str = "application/vnd.openxmlformats-officedocument.drawingml.chart+xml";
const AFTER_DRAWING: &[&str] = &[
    "legacyDrawing", "legacyDrawingHF", "drawingHF", "picture", "oleObjects", "controls", "webPublishItems", "tableParts", "extLst",
];
pub const PALETTE: [&str; 7] = ["0E7C6B", "4F5BD5", "E8A33D", "D6456A", "2F9BD6", "8B5CF6", "65A30D"];

/// A two-cell anchor: 0-based columns/rows plus offsets in EMU.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub struct Anchor {
    pub from_col: i32,
    pub from_row: i32,
    pub from_dx: i64,
    pub from_dy: i64,
    pub to_col: i32,
    pub to_row: i32,
    pub to_dx: i64,
    pub to_dy: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Series {
    pub name: Option<String>,
    pub name_ref: Option<String>,
    pub cat_ref: Option<String>,
    pub val_ref: Option<String>,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartInfo {
    pub sheet: String,
    pub drawing: String,
    pub index: usize,
    pub chart_part: String,
    /// column | bar | line | area | pie | doughnut | scatter | radar
    pub kind: String,
    pub grouping: Option<String>,
    pub title: Option<String>,
    pub legend: bool,
    pub series: Vec<Series>,
    /// None for one-cell/absolute anchors (shown, but not movable)
    pub anchor: Option<Anchor>,
}

// --------------------------------------------------------------------------- tiny DOM

#[derive(Debug, Default)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
    text: String,
}

impl Node {
    fn local(&self) -> &str {
        xml::local(&self.name)
    }
    fn attr(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(a, _)| xml::local(a) == k).map(|(_, v)| v.as_str())
    }
    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.local() == name)
    }
    fn find(&self, name: &str) -> Option<&Node> {
        if self.local() == name {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(name))
    }
    fn all_text(&self, name: &str, out: &mut String) {
        if self.local() == name {
            out.push_str(&self.text);
        }
        for c in &self.children {
            c.all_text(name, out);
        }
    }
    fn val(&self, name: &str) -> Option<&str> {
        self.child(name).and_then(|c| c.attr("val"))
    }
}

fn parse_tree(src: &str) -> Node {
    let mut stack: Vec<Node> = vec![Node::default()];
    let mut last_end = 0;
    for t in tokens(src) {
        if let Some(top) = stack.last_mut() {
            if t.start > last_end {
                top.text.push_str(&xml::unescape(&src[last_end..t.start]));
            }
        }
        last_end = t.end;
        let tag = &src[t.start..t.end];
        let node = || Node {
            name: t.name.to_string(),
            attrs: xml::attrs(tag).into_iter().map(|(k, v)| (k.to_string(), xml::unescape(v))).collect(),
            ..Default::default()
        };
        match t.kind {
            Kind::Start => stack.push(node()),
            Kind::Empty => stack.last_mut().unwrap().children.push(node()),
            Kind::End => {
                if stack.len() > 1 {
                    let n = stack.pop().unwrap();
                    stack.last_mut().unwrap().children.push(n);
                }
            }
            Kind::Other => {}
        }
    }
    stack.into_iter().next().unwrap_or_default()
}

// --------------------------------------------------------------------------- reading

fn worksheet_parts(pkg: &mut Package) -> Result<(String, Vec<(String, String)>), String> {
    let wb_path = pkg
        .rels("")?
        .into_iter()
        .find(|r| r.kind.ends_with("/officeDocument"))
        .map(|r| r.target)
        .unwrap_or_else(|| "xl/workbook.xml".into());
    let wb_rels = pkg.rels(&wb_path)?;
    let wb_xml = pkg.read(&wb_path)?;
    let mut out = Vec::new();
    for t in tokens(&wb_xml).filter(|t| t.local() == "sheet" && matches!(t.kind, Kind::Empty | Kind::Start)) {
        let tag = &wb_xml[t.start..t.end];
        let Some(rid) = xml::attrs(tag).into_iter().find(|(k, _)| k.ends_with(":id")).map(|(_, v)| v) else { continue };
        if let Some(rel) = wb_rels.iter().find(|r| r.id == rid && r.kind.ends_with("/worksheet")) {
            out.push((xml::unescape(attr(tag, "name").unwrap_or("")), rel.target.clone()));
        }
    }
    Ok((wb_path, out))
}

/// Root children of a drawing that anchor something, with byte ranges.
fn anchors(src: &str) -> Vec<(usize, usize, String)> {
    let toks: Vec<Tok> = tokens(src).collect();
    let mut out = Vec::new();
    let mut depth = 0usize;
    for (i, t) in toks.iter().enumerate() {
        if depth == 1 && matches!(t.kind, Kind::Start | Kind::Empty) && t.local().ends_with("Anchor") {
            out.push((t.start, xml::element_end(&toks, i), t.name.to_string()));
        }
        match t.kind {
            Kind::Start => depth += 1,
            Kind::End => depth -= 1,
            _ => {}
        }
    }
    out
}

fn anchor_of(n: &Node) -> Option<Anchor> {
    if n.local() != "twoCellAnchor" {
        return None;
    }
    let pos = |name: &str| -> Option<(i32, i64, i32, i64)> {
        let p = n.child(name)?;
        let num = |k: &str| p.child(k).map(|c| c.text.trim().to_string()).unwrap_or_default();
        Some((num("col").parse().ok()?, num("colOff").parse().unwrap_or(0), num("row").parse().ok()?, num("rowOff").parse().unwrap_or(0)))
    };
    let (fc, fdx, fr, fdy) = pos("from")?;
    let (tc, tdx, tr, tdy) = pos("to")?;
    Some(Anchor { from_col: fc, from_row: fr, from_dx: fdx, from_dy: fdy, to_col: tc, to_row: tr, to_dx: tdx, to_dy: tdy })
}

fn series_of(ser: &Node) -> Series {
    let f = |n: Option<&Node>| n.and_then(|x| x.find("f")).map(|x| x.text.trim().to_string()).filter(|s| !s.is_empty());
    let tx = ser.child("tx");
    let name = tx.and_then(|t| t.find("v").map(|v| v.text.clone()).or_else(|| {
        let mut s = String::new();
        t.all_text("t", &mut s);
        (!s.is_empty()).then_some(s)
    }));
    let color = ser
        .child("spPr")
        .and_then(|sp| sp.find("srgbClr"))
        .and_then(|c| c.attr("val"))
        .map(|v| v.to_uppercase());
    Series {
        name,
        name_ref: f(tx),
        cat_ref: f(ser.child("cat").or(ser.child("xVal"))),
        val_ref: f(ser.child("val").or(ser.child("yVal"))),
        color,
    }
}

fn parse_chart(xml_src: &str) -> (String, Option<String>, Option<String>, bool, Vec<Series>) {
    let root = parse_tree(xml_src);
    let chart = root.find("chart").filter(|c| c.child("plotArea").is_some());
    let Some(chart) = chart else { return ("column".into(), None, None, false, vec![]) };
    let plot = chart.child("plotArea").unwrap();
    let kinds = [
        ("barChart", "bar"), ("bar3DChart", "bar"), ("lineChart", "line"), ("line3DChart", "line"), ("areaChart", "area"),
        ("area3DChart", "area"), ("pieChart", "pie"), ("pie3DChart", "pie"), ("ofPieChart", "pie"), ("doughnutChart", "doughnut"),
        ("scatterChart", "scatter"), ("bubbleChart", "scatter"), ("radarChart", "radar"), ("stockChart", "line"),
    ];
    let mut kind = "column".to_string();
    let mut grouping = None;
    let mut series = Vec::new();
    for c in &plot.children {
        if let Some((_, k)) = kinds.iter().find(|(n, _)| *n == c.local()) {
            if series.is_empty() {
                kind = if *k == "bar" && c.val("barDir") != Some("bar") { "column".into() } else { k.to_string() };
                grouping = c.val("grouping").map(str::to_string);
            }
            series.extend(c.children.iter().filter(|s| s.local() == "ser").map(series_of));
        }
    }
    let deleted = chart.val("autoTitleDeleted") == Some("1");
    let title = chart.child("title").and_then(|t| {
        let mut s = String::new();
        t.all_text("t", &mut s);
        (!s.trim().is_empty()).then_some(s)
    });
    let title = if deleted { None } else { title.or_else(|| if series.len() == 1 && chart.child("title").is_some() { series[0].name.clone() } else { None }) };
    (kind, grouping, title, chart.child("legend").is_some(), series)
}

/// Every chart in the workbook, per worksheet, in drawing order.
pub fn read_charts(bytes: &[u8]) -> Result<Vec<ChartInfo>, String> {
    let mut pkg = Package::open(bytes)?;
    let (_, sheets) = worksheet_parts(&mut pkg)?;
    let mut out = Vec::new();
    for (sheet, part) in sheets {
        for r in pkg.rels(&part)?.into_iter().filter(|r| r.kind.ends_with("/drawing") && !r.external) {
            if !pkg.has(&r.target) {
                continue;
            }
            let drawing = pkg.read(&r.target)?;
            let drels: HashMap<String, String> = pkg.rels(&r.target)?.into_iter().map(|x| (x.id, x.target)).collect();
            for (index, (s, e, _)) in anchors(&drawing).into_iter().enumerate() {
                let node = parse_tree(&drawing[s..e]);
                let anchor_node = node.children.first();
                let Some(chart_ref) = anchor_node.and_then(|a| a.find("graphicData")).and_then(|g| g.find("chart")) else { continue };
                let Some(target) = chart_ref.attr("id").and_then(|id| drels.get(id)) else { continue };
                if !pkg.has(target) {
                    continue;
                }
                let (kind, grouping, title, legend, series) = parse_chart(&pkg.read(target)?);
                out.push(ChartInfo {
                    sheet: sheet.clone(),
                    drawing: r.target.clone(),
                    index,
                    chart_part: target.clone(),
                    kind,
                    grouping,
                    title,
                    legend,
                    series,
                    anchor: anchor_node.and_then(anchor_of),
                });
            }
        }
    }
    Ok(out)
}

// --------------------------------------------------------------------------- writing

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewSeries {
    pub name: String,
    pub name_ref: Option<String>,
    pub cat_ref: Option<String>,
    pub val_ref: String,
    pub cats: Vec<String>,
    pub values: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewChart {
    pub sheet: String,
    pub kind: String,
    pub title: Option<String>,
    pub series: Vec<NewSeries>,
    pub anchor: Anchor,
}

#[derive(Debug, Default)]
pub struct ChartEdits {
    pub moves: Vec<(String, usize, Anchor)>,
    pub deletes: Vec<(String, usize)>,
    pub adds: Vec<NewChart>,
}

impl ChartEdits {
    pub fn is_empty(&self) -> bool {
        self.moves.is_empty() && self.deletes.is_empty() && self.adds.is_empty()
    }
}

fn pos_xml(p: &str, tag: &str, col: i32, dx: i64, row: i32, dy: i64) -> String {
    format!("<{p}{tag}><{p}col>{col}</{p}col><{p}colOff>{dx}</{p}colOff><{p}row>{row}</{p}row><{p}rowOff>{dy}</{p}rowOff></{p}{tag}>")
}

fn anchor_xml(p: &str, a: &Anchor, id: usize, rid: &str) -> String {
    format!(
        "<{p}twoCellAnchor editAs=\"oneCell\">{}{}<{p}graphicFrame macro=\"\" xmlns:a=\"{NS_A}\"><{p}nvGraphicFramePr><{p}cNvPr id=\"{id}\" name=\"Chart {id}\"/><{p}cNvGraphicFramePr/></{p}nvGraphicFramePr><{p}xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/></{p}xfrm><a:graphic><a:graphicData uri=\"{NS_C}\"><c:chart xmlns:c=\"{NS_C}\" xmlns:r=\"{NS_R}\" r:id=\"{rid}\"/></a:graphicData></a:graphic></{p}graphicFrame><{p}clientData/></{p}twoCellAnchor>",
        pos_xml(p, "from", a.from_col, a.from_dx, a.from_row, a.from_dy),
        pos_xml(p, "to", a.to_col, a.to_dx, a.to_row, a.to_dy),
    )
}

fn esc(s: &str) -> String {
    xml::escape(s)
}

fn series_xml(kind: &str, i: usize, s: &NewSeries) -> String {
    let color = PALETTE[i % PALETTE.len()];
    let tx = match &s.name_ref {
        Some(r) => format!("<c:tx><c:strRef><c:f>{}</c:f><c:strCache><c:ptCount val=\"1\"/><c:pt idx=\"0\"><c:v>{}</c:v></c:pt></c:strCache></c:strRef></c:tx>", esc(r), esc(&s.name)),
        None => format!("<c:tx><c:v>{}</c:v></c:tx>", esc(&s.name)),
    };
    let sp = match kind {
        "line" | "scatter" => format!("<c:spPr><a:ln w=\"28575\" cap=\"rnd\"><a:solidFill><a:srgbClr val=\"{color}\"/></a:solidFill><a:round/></a:ln></c:spPr>"),
        "pie" | "doughnut" => String::new(),
        _ => format!("<c:spPr><a:solidFill><a:srgbClr val=\"{color}\"/></a:solidFill></c:spPr>"),
    };
    let marker = match kind {
        "line" => format!("<c:marker><c:symbol val=\"circle\"/><c:size val=\"5\"/><c:spPr><a:solidFill><a:srgbClr val=\"{color}\"/></a:solidFill></c:spPr></c:marker>"),
        "scatter" => format!("<c:marker><c:symbol val=\"circle\"/><c:size val=\"6\"/><c:spPr><a:solidFill><a:srgbClr val=\"{color}\"/></a:solidFill></c:spPr></c:marker>"),
        _ => String::new(),
    };
    let pts_str: String = s.cats.iter().enumerate().map(|(k, v)| format!("<c:pt idx=\"{k}\"><c:v>{}</c:v></c:pt>", esc(v))).collect();
    let pts_num: String = s.values.iter().enumerate().map(|(k, v)| format!("<c:pt idx=\"{k}\"><c:v>{v}</c:v></c:pt>")).collect();
    let num_ref = |r: &str, pts: &str, n: usize| format!("<c:numRef><c:f>{}</c:f><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val=\"{n}\"/>{pts}</c:numCache></c:numRef>", esc(r));
    let (cat_tag, val_tag) = if kind == "scatter" { ("xVal", "yVal") } else { ("cat", "val") };
    let cat = match &s.cat_ref {
        Some(r) if kind == "scatter" => format!("<c:{cat_tag}>{}</c:{cat_tag}>", num_ref(r, &pts_str, s.cats.len())),
        Some(r) => format!("<c:{cat_tag}><c:strRef><c:f>{}</c:f><c:strCache><c:ptCount val=\"{}\"/>{pts_str}</c:strCache></c:strRef></c:{cat_tag}>", esc(r), s.cats.len()),
        None => String::new(),
    };
    let val = format!("<c:{val_tag}>{}</c:{val_tag}>", num_ref(&s.val_ref, &pts_num, s.values.len()));
    let extra = match kind {
        "column" | "bar" => "<c:invertIfNegative val=\"0\"/>",
        _ => "",
    };
    let tail = match kind {
        "line" | "scatter" => "<c:smooth val=\"0\"/>",
        _ => "",
    };
    format!("<c:ser><c:idx val=\"{i}\"/><c:order val=\"{i}\"/>{tx}{sp}{extra}{marker}{cat}{val}{tail}</c:ser>")
}

fn axes_xml(kind: &str) -> String {
    let cat = |pos: &str| format!("<c:catAx><c:axId val=\"111\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"{pos}\"/><c:numFmt formatCode=\"General\" sourceLinked=\"1\"/><c:majorTickMark val=\"none\"/><c:minorTickMark val=\"none\"/><c:tickLblPos val=\"nextTo\"/><c:crossAx val=\"222\"/><c:crosses val=\"autoZero\"/><c:auto val=\"1\"/><c:lblAlgn val=\"ctr\"/><c:lblOffset val=\"100\"/><c:noMultiLvlLbl val=\"0\"/></c:catAx>");
    let val = |id: u32, cross: u32, pos: &str, between: &str, grid: bool| format!("<c:valAx><c:axId val=\"{id}\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"{pos}\"/>{}<c:numFmt formatCode=\"General\" sourceLinked=\"1\"/><c:majorTickMark val=\"none\"/><c:minorTickMark val=\"none\"/><c:tickLblPos val=\"nextTo\"/><c:crossAx val=\"{cross}\"/><c:crosses val=\"autoZero\"/><c:crossBetween val=\"{between}\"/></c:valAx>", if grid { "<c:majorGridlines/>" } else { "" });
    match kind {
        "pie" | "doughnut" => String::new(),
        "bar" => format!("{}{}", cat("l"), val(222, 111, "b", "between", true)),
        "scatter" => format!("{}{}", val(111, 222, "b", "midCat", false), val(222, 111, "l", "midCat", true)),
        _ => format!("{}{}", cat("b"), val(222, 111, "l", "between", true)),
    }
}

pub fn chart_xml(c: &NewChart) -> String {
    let kind = c.kind.as_str();
    let sers: String = c.series.iter().enumerate().map(|(i, s)| series_xml(kind, i, s)).collect();
    let ax = "<c:axId val=\"111\"/><c:axId val=\"222\"/>";
    let plot = match kind {
        "bar" | "column" => format!("<c:barChart><c:barDir val=\"{}\"/><c:grouping val=\"clustered\"/><c:varyColors val=\"0\"/>{sers}<c:gapWidth val=\"80\"/>{ax}</c:barChart>", if kind == "bar" { "bar" } else { "col" }),
        "line" => format!("<c:lineChart><c:grouping val=\"standard\"/><c:varyColors val=\"0\"/>{sers}<c:marker val=\"1\"/>{ax}</c:lineChart>"),
        "area" => format!("<c:areaChart><c:grouping val=\"standard\"/><c:varyColors val=\"0\"/>{sers}{ax}</c:areaChart>"),
        "pie" => format!("<c:pieChart><c:varyColors val=\"1\"/>{sers}<c:firstSliceAng val=\"0\"/></c:pieChart>"),
        "doughnut" => format!("<c:doughnutChart><c:varyColors val=\"1\"/>{sers}<c:firstSliceAng val=\"0\"/><c:holeSize val=\"55\"/></c:doughnutChart>"),
        _ => format!("<c:scatterChart><c:scatterStyle val=\"lineMarker\"/><c:varyColors val=\"0\"/>{sers}{ax}</c:scatterChart>"),
    };
    let title = match &c.title {
        Some(t) if !t.is_empty() => format!("<c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz=\"1400\" b=\"0\"/></a:pPr><a:r><a:rPr lang=\"en-US\" sz=\"1400\" b=\"0\"/><a:t>{}</a:t></a:r></a:p></c:rich></c:tx><c:overlay val=\"0\"/></c:title><c:autoTitleDeleted val=\"0\"/>", esc(t)),
        _ => "<c:autoTitleDeleted val=\"1\"/>".into(),
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<c:chartSpace xmlns:c=\"{NS_C}\" xmlns:a=\"{NS_A}\" xmlns:r=\"{NS_R}\"><c:roundedCorners val=\"0\"/><c:chart>{title}<c:plotArea><c:layout/>{plot}{}</c:plotArea><c:legend><c:legendPos val=\"b\"/><c:overlay val=\"0\"/></c:legend><c:plotVisOnly val=\"1\"/><c:dispBlanksAs val=\"gap\"/></c:chart></c:chartSpace>",
        axes_xml(kind)
    )
}

fn next_part(names: &[String], taken: &HashSet<String>, dir: &str, stem: &str) -> String {
    (1..)
        .map(|n| format!("{dir}/{stem}{n}.xml"))
        .find(|p| !names.contains(p) && !taken.contains(p))
        .unwrap()
}

/// Apply chart moves, deletes and additions to a saved package.
pub fn apply_chart_edits(bytes: &[u8], edits: &ChartEdits) -> Result<Vec<u8>, String> {
    if edits.is_empty() {
        return Ok(bytes.to_vec());
    }
    let mut pkg = Package::open(bytes)?;
    let names = pkg.names.clone();
    let (_, sheets) = worksheet_parts(&mut pkg)?;
    let mut replace: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut add: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut taken: HashSet<String> = HashSet::new();
    let mut overrides: Vec<(String, &str)> = Vec::new();

    // drawings touched by moves/deletes/adds: current XML, kept in memory while editing
    let mut drawings: BTreeMap<String, String> = BTreeMap::new();
    let mut drawing_rels: BTreeMap<String, String> = BTreeMap::new();
    let read_drawing = |pkg: &mut Package, d: &str, drawings: &mut BTreeMap<String, String>| -> Result<(), String> {
        if !drawings.contains_key(d) {
            drawings.insert(d.to_string(), pkg.read(d)?);
        }
        Ok(())
    };

    // moves and deletes, back to front per drawing so byte ranges stay valid
    let mut per_drawing: BTreeMap<String, Vec<(usize, Option<Anchor>)>> = BTreeMap::new();
    for (d, i, a) in &edits.moves {
        per_drawing.entry(d.clone()).or_default().push((*i, Some(*a)));
    }
    for (d, i) in &edits.deletes {
        per_drawing.entry(d.clone()).or_default().push((*i, None));
    }
    for (d, list) in &per_drawing {
        read_drawing(&mut pkg, d, &mut drawings)?;
        let src = drawings[d].clone();
        let found = anchors(&src);
        let mut ops: Vec<(usize, Option<Anchor>)> = list.clone();
        ops.sort_by(|a, b| b.0.cmp(&a.0));
        let mut out = src.clone();
        for (i, a) in ops {
            let Some((s, e, name)) = found.get(i) else { continue };
            let new = match a {
                None => String::new(),
                Some(a) if xml::local(name) == "twoCellAnchor" => {
                    let p = xml::prefix(name).map(|p| format!("{p}:")).unwrap_or_default();
                    let el = &src[*s..*e];
                    let el = replace_child(el, "from", Some(&pos_xml(&p, "from", a.from_col, a.from_dx, a.from_row, a.from_dy)), &[]);
                    replace_child(&el, "to", Some(&pos_xml(&p, "to", a.to_col, a.to_dx, a.to_row, a.to_dy)), &[])
                }
                Some(_) => continue,
            };
            out = format!("{}{new}{}", &out[..*s], &out[*e..]);
        }
        drawings.insert(d.clone(), out);
    }

    // additions, grouped per sheet
    let mut adds_by_sheet: BTreeMap<String, Vec<&NewChart>> = BTreeMap::new();
    for c in &edits.adds {
        adds_by_sheet.entry(c.sheet.clone()).or_default().push(c);
    }
    for (sheet, charts) in adds_by_sheet {
        let Some((_, part)) = sheets.iter().find(|(n, _)| *n == sheet) else { continue };
        let srels_path = rels_path(part);
        let mut srels = if pkg.has(&srels_path) {
            pkg.read(&srels_path)?
        } else {
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"></Relationships>")
        };
        let existing = pkg.rels(part)?.into_iter().find(|r| r.kind.ends_with("/drawing") && !r.external && names.contains(&r.target));
        let drawing = match existing {
            Some(r) => {
                read_drawing(&mut pkg, &r.target, &mut drawings)?;
                r.target
            }
            None => {
                let d = next_part(&names, &taken, "xl/drawings", "drawing");
                taken.insert(d.clone());
                drawings.insert(d.clone(), format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<xdr:wsDr xmlns:xdr=\"{NS_XDR}\" xmlns:a=\"{NS_A}\"></xdr:wsDr>"));
                overrides.push((d.clone(), CT_DRAWING));
                // wire the sheet to its new drawing
                let rid = format!("rIdSheafDrawing{}", taken.len());
                let target = relative_target(part, &d);
                srels = append_to_root(&srels, &format!("<Relationship Id=\"{rid}\" Type=\"{REL_DRAWING}\" Target=\"{target}\"/>"));
                let sxml = match replace.get(part) {
                    Some(b) => String::from_utf8_lossy(b).into_owned(),
                    None => pkg.read(part)?,
                };
                let sxml = ensure_r_namespace(&sxml);
                let sxml = replace_child(&sxml, "drawing", Some(&format!("<drawing r:id=\"{rid}\"/>")), AFTER_DRAWING);
                replace.insert(part.clone(), sxml.into_bytes());
                if pkg.has(&srels_path) {
                    replace.insert(srels_path.clone(), srels.clone().into_bytes());
                } else {
                    add.insert(srels_path.clone(), srels.clone().into_bytes());
                }
                d
            }
        };
        let drels_path = rels_path(&drawing);
        let mut drels = match drawing_rels.get(&drels_path) {
            Some(x) => x.clone(),
            None if pkg.has(&drels_path) => pkg.read(&drels_path)?,
            None => "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"></Relationships>".into(),
        };
        let mut dxml = drawings[&drawing].clone();
        let root_prefix = tokens(&dxml)
            .find(|t| t.local() == "wsDr" && matches!(t.kind, Kind::Start | Kind::Empty))
            .and_then(|t| xml::prefix(t.name).map(|p| format!("{p}:")))
            .unwrap_or_default();
        for c in charts {
            let chart_part = next_part(&names, &taken, "xl/charts", "chart");
            taken.insert(chart_part.clone());
            add.insert(chart_part.clone(), chart_xml(c).into_bytes());
            overrides.push((chart_part.clone(), CT_CHART));
            let rid = format!("rIdSheafChart{}", taken.len());
            drels = append_to_root(&drels, &format!("<Relationship Id=\"{rid}\" Type=\"{REL_CHART}\" Target=\"{}\"/>", relative_target(&drawing, &chart_part)));
            let id = 1000 + taken.len();
            dxml = append_to_root(&dxml, &anchor_xml(&root_prefix, &c.anchor, id, &rid));
        }
        drawings.insert(drawing.clone(), dxml);
        drawing_rels.insert(drels_path, drels);
    }

    for (d, x) in drawings {
        if names.contains(&d) { replace.insert(d, x.into_bytes()); } else { add.insert(d, x.into_bytes()); }
    }
    for (p, x) in drawing_rels {
        if names.contains(&p) { replace.insert(p, x.into_bytes()); } else { add.insert(p, x.into_bytes()); }
    }
    if !overrides.is_empty() {
        let mut ct = pkg.read("[Content_Types].xml")?;
        for (part, ctype) in &overrides {
            let name = format!("/{part}");
            ct = remove_elements(&ct, |tag| attr(tag, "PartName") == Some(name.as_str()));
            ct = append_to_root(&ct, &format!("<Override PartName=\"{name}\" ContentType=\"{ctype}\"/>"));
        }
        replace.insert("[Content_Types].xml".into(), ct.into_bytes());
    }
    pkg.write(&replace, &HashSet::new(), &add)
}

/// Relationship target from `from` part to `to` part (same tree, e.g. ../drawings/drawing1.xml).
fn relative_target(from: &str, to: &str) -> String {
    let from_dir: Vec<&str> = from.rsplit_once('/').map(|(d, _)| d.split('/').collect()).unwrap_or_default();
    let to_parts: Vec<&str> = to.split('/').collect();
    let common = from_dir.iter().zip(&to_parts).take_while(|(a, b)| a == b).count();
    let mut out: Vec<String> = vec!["..".into(); from_dir.len() - common];
    out.extend(to_parts[common..].iter().map(|s| s.to_string()));
    let rel = out.join("/");
    debug_assert_eq!(resolve(from, &rel), to);
    rel
}

fn ensure_r_namespace(sheet: &str) -> String {
    match tokens(sheet).find(|t| t.local() == "worksheet" && t.kind == Kind::Start) {
        Some(t) if !sheet[t.start..t.end].contains(NS_R) => {
            let tag = set_attr(&sheet[t.start..t.end], "xmlns:r", Some(NS_R));
            format!("{}{tag}{}", &sheet[..t.start], &sheet[t.end..])
        }
        _ => sheet.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_targets() {
        assert_eq!(relative_target("xl/worksheets/sheet1.xml", "xl/drawings/drawing1.xml"), "../drawings/drawing1.xml");
        assert_eq!(relative_target("xl/drawings/drawing1.xml", "xl/charts/chart3.xml"), "../charts/chart3.xml");
    }

    #[test]
    fn parses_bar_chart() {
        let x = r#"<c:chartSpace xmlns:c="c" xmlns:a="a"><c:chart><c:title><c:tx><c:rich><a:p><a:r><a:t>Sales</a:t></a:r></a:p></c:rich></c:tx></c:title><c:plotArea><c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:ser><c:tx><c:strRef><c:f>Data!$B$1</c:f></c:strRef></c:tx><c:spPr><a:solidFill><a:srgbClr val="ff0000"/></a:solidFill></c:spPr><c:cat><c:strRef><c:f>Data!$A$2:$A$7</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>Data!$B$2:$B$7</c:f></c:numRef></c:val></c:ser></c:barChart></c:plotArea><c:legend/></c:chart></c:chartSpace>"#;
        let (kind, grouping, title, legend, series) = parse_chart(x);
        assert_eq!((kind.as_str(), grouping.as_deref(), title.as_deref(), legend), ("column", Some("clustered"), Some("Sales"), true));
        assert_eq!(series[0].val_ref.as_deref(), Some("Data!$B$2:$B$7"));
        assert_eq!(series[0].color.as_deref(), Some("FF0000"));
    }
}
