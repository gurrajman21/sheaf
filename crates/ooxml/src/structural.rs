//! Row/column insert and delete for the patch writer.
//!
//! The caller passes the operations it applied to the model, in order. Each one
//! is replayed on the original package part by part: the edited sheet's cells,
//! rows, merges, validation, conditional formatting, links, filters, breaks and
//! anchored objects move; formulas anywhere that point at the edited sheet
//! (other sheets, defined names, charts, pivot caches, tables) are rewritten.
//! Only references and positions change; every other byte of a part is kept,
//! and parts with nothing to shift are not touched at all.

use std::collections::{BTreeMap, HashMap, HashSet};

use ironcalc_base::Model;
use serde::Deserialize;

use crate::package::{parse_rels, Package};
use crate::refs::{self, Axis, Body, Shift};
use crate::sheet::{cell_ref, parse_ref};
use crate::xml::{self, attr, set_attr, tokens, Kind, Tok};

/// A structural edit, as applied to the IronCalc model. Sheets are model indices.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum StructuralOp {
    InsertRows { sheet: u32, at: i32, count: i32 },
    DeleteRows { sheet: u32, at: i32, count: i32 },
    InsertCols { sheet: u32, at: i32, count: i32 },
    DeleteCols { sheet: u32, at: i32, count: i32 },
}

impl StructuralOp {
    /// Apply to the model. IronCalc 0.9 shifts cell and conditional-format formulas but
    /// not defined names or table ranges; those are shifted here with the same rewriter
    /// the package uses, so the model and the saved file agree. Once IronCalc has moved
    /// the cells this never fails, so the model and the op log can't drift apart.
    pub fn apply(&self, model: &mut Model) -> Result<(), String> {
        let name = model.workbook.worksheet(self.sheet())?.name.clone();
        // IronCalc can fail halfway (cells moved, merges taken out); put the model back
        let backup = model.workbook.clone();
        let res = match *self {
            StructuralOp::InsertRows { sheet, at, count } => model.insert_rows(sheet, at, count),
            StructuralOp::DeleteRows { sheet, at, count } => model.delete_rows(sheet, at, count),
            StructuralOp::InsertCols { sheet, at, count } => model.insert_columns(sheet, at, count),
            StructuralOp::DeleteCols { sheet, at, count } => model.delete_columns(sheet, at, count),
        };
        if let Err(e) = res {
            *model = Model::from_workbook(backup, "en")?;
            return Err(e);
        }
        let shift = self.to_shift(&name);
        let sheets = &model.workbook.worksheets;
        let updates: Vec<(String, Option<u32>, String)> = model
            .workbook
            .defined_names
            .iter()
            .filter_map(|d| {
                let f = refs::shift_formula(&d.formula, None, &shift)?;
                let scope = d.sheet_id.and_then(|id| sheets.iter().position(|w| w.sheet_id == id)).map(|i| i as u32);
                Some((d.name.clone(), scope, f))
            })
            .collect();
        for (n, scope, f) in updates {
            // IronCalc can't parse some names it imported (e.g. `Sheet!#REF!`); such a name
            // keeps its old formula in the model, the file still gets the shifted one
            let _ = model.update_defined_name(&n, scope, &n, scope, &f);
        }
        for t in model.workbook.tables.values_mut() {
            if shift.is_sheet(&t.sheet_name) {
                if let Some(Some(r)) = refs::shift_sqref(&t.reference, &shift) {
                    t.reference = r;
                }
            }
        }
        Ok(())
    }

    pub fn sheet(&self) -> u32 {
        match *self {
            StructuralOp::InsertRows { sheet, .. }
            | StructuralOp::DeleteRows { sheet, .. }
            | StructuralOp::InsertCols { sheet, .. }
            | StructuralOp::DeleteCols { sheet, .. } => sheet,
        }
    }

    pub fn to_shift(&self, sheet_name: &str) -> Shift {
        let (axis, at, count) = match *self {
            StructuralOp::InsertRows { at, count, .. } => (Axis::Row, at, count),
            StructuralOp::DeleteRows { at, count, .. } => (Axis::Row, at, -count),
            StructuralOp::InsertCols { at, count, .. } => (Axis::Col, at, count),
            StructuralOp::DeleteCols { at, count, .. } => (Axis::Col, at, -count),
        };
        Shift { sheet: sheet_name.to_string(), axis, at, count }
    }
}

// ----------------------------------------------------------------------------- splicing

/// Byte-range replacements over a source string. A replacement that covers
/// earlier ones (removing a whole container) supersedes them.
struct Splice<'a> {
    src: &'a str,
    reps: Vec<(usize, usize, String)>,
}

impl<'a> Splice<'a> {
    fn new(src: &'a str) -> Self {
        Splice { src, reps: Vec::new() }
    }

    fn rep(&mut self, s: usize, e: usize, text: String) {
        self.reps.retain(|&(a, b, _)| !(a >= s && b <= e));
        self.reps.push((s, e, text));
    }

    fn finish(mut self) -> Option<String> {
        if self.reps.is_empty() {
            return None;
        }
        self.reps.sort_by_key(|r| r.0);
        let mut out = String::with_capacity(self.src.len());
        let mut cur = 0;
        for (s, e, t) in &self.reps {
            out.push_str(&self.src[cur..*s]);
            out.push_str(t);
            cur = *e;
        }
        out.push_str(&self.src[cur..]);
        Some(out)
    }
}

fn is_open(t: &Tok) -> bool {
    matches!(t.kind, Kind::Start | Kind::Empty)
}

/// Text span of a simple element (`<f>text</f>`): between its start tag and its end tag.
fn text_span(toks: &[Tok], i: usize) -> Option<(usize, usize)> {
    if toks[i].kind != Kind::Start {
        return None;
    }
    let next = toks.get(i + 1)?;
    (next.kind == Kind::End && next.name == toks[i].name).then_some((toks[i].end, next.start))
}

fn skip_past(toks: &[Tok], i: usize, end: usize) -> usize {
    let mut j = i + 1;
    while j < toks.len() && toks[j].start < end {
        j += 1;
    }
    j
}

/// Rewrite escaped element text in place; `None` if no reference moved.
fn shift_text(raw: &str, current: Option<&str>, s: &Shift) -> Option<String> {
    // text content only needs & < > escaped; keep quotes literal as Excel writes them
    refs::shift_formula(&xml::unescape(raw), current, s).map(|f| f.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;"))
}

fn shift_attr_formula(sp: &mut Splice, t: &Tok, key: &str, current: Option<&str>, s: &Shift) {
    let tag = &sp.src[t.start..t.end];
    if let Some(v) = attr(tag, key) {
        if let Some(n) = refs::shift_formula(&xml::unescape(v), current, s) {
            let new = set_attr(tag, key, Some(&xml::escape(&n)));
            sp.rep(t.start, t.end, new);
        }
    }
}

/// Shift a range-list attribute on the edited sheet. Returns false if every range was deleted.
fn shift_attr_sqref(sp: &mut Splice, t: &Tok, key: &str, s: &Shift) -> bool {
    let tag = sp.src[t.start..t.end].to_string();
    // a replacement for this tag may already be queued (several attributes on one tag)
    let current = sp.reps.iter().find(|r| r.0 == t.start && r.1 == t.end).map(|r| r.2.clone()).unwrap_or(tag);
    let Some(v) = attr(&current, key) else { return true };
    match refs::shift_sqref(v, s) {
        None => true,
        Some(None) => false,
        Some(Some(n)) => {
            let new = set_attr(&current, key, Some(&n));
            sp.rep(t.start, t.end, new);
            true
        }
    }
}

fn set_tag_attr(sp: &mut Splice, t: &Tok, key: &str, value: Option<&str>) {
    let current = sp
        .reps
        .iter()
        .find(|r| r.0 == t.start && r.1 == t.end)
        .map(|r| r.2.clone())
        .unwrap_or_else(|| sp.src[t.start..t.end].to_string());
    let new = set_attr(&current, key, value);
    sp.rep(t.start, t.end, new);
}

// ----------------------------------------------------------------------------- worksheets

/// What a worksheet rewrite produced.
#[derive(Debug, Default)]
pub struct SheetResult {
    pub xml: Option<String>,
    /// Cells (in post-shift coordinates) whose shared-formula group no longer derives
    /// correctly after the shift; the writer replaces them with explicit formulas.
    pub rematerialize: Vec<(i32, i32)>,
}

#[derive(Default)]
struct Group {
    master: Option<((i32, i32), String)>,
    range: Option<(i32, i32, i32, i32)>, // r1, c1, r2, c2
    cells: Vec<(i32, i32)>,
}

/// Containers that must keep at least one child and may carry a `count`.
const CONTAINERS: &[(&str, &str)] = &[
    ("mergeCells", "mergeCell"),
    ("dataValidations", "dataValidation"),
    ("hyperlinks", "hyperlink"),
    ("cols", "col"),
    ("rowBreaks", "brk"),
    ("colBreaks", "brk"),
    ("protectedRanges", "protectedRange"),
    ("ignoredErrors", "ignoredError"),
];

/// Rewrite one worksheet for `s`. `name` is this sheet's name; when it is the
/// edited sheet, positions move too, otherwise only formulas pointing at it change.
pub fn shift_worksheet(src: &str, name: &str, s: &Shift) -> Result<SheetResult, String> {
    let own = s.is_sheet(name);
    let cur = Some(name);
    let toks: Vec<Tok> = tokens(src).collect();
    let mut sp = Splice::new(src);
    let mut row_r = 0;
    let mut col_c = 0;
    // open containers: (token index, child name, children seen, children removed)
    let mut stack: Vec<(usize, &str, usize, usize)> = Vec::new();
    // shared-formula groups, from the whole sheet before anything is removed: a
    // master in a deleted row/column still decides what its surviving members need
    let groups = shared_groups(src, &toks);
    let mut af_first: Option<(i32, i32)> = None; // autoFilter first column (old, new)
    let rows = s.axis == Axis::Row;

    let remove = |sp: &mut Splice, stack: &mut Vec<(usize, &str, usize, usize)>, i: usize, child: &str| -> usize {
        let end = xml::element_end(&toks, i);
        sp.rep(toks[i].start, end, String::new());
        if let Some(top) = stack.last_mut() {
            if top.1 == child {
                top.3 += 1;
            }
        }
        skip_past(&toks, i, end)
    };

    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        if t.kind == Kind::End {
            if let Some(&(si, _, seen, removed)) = stack.last() {
                if toks[si].name == t.name {
                    stack.pop();
                    if removed > 0 && removed >= seen {
                        sp.rep(toks[si].start, t.end, String::new());
                    } else if removed > 0 {
                        let tag = &src[toks[si].start..toks[si].end];
                        for key in ["count", "manualBreakCount"] {
                            if let Some(n) = attr(tag, key).and_then(|v| v.parse::<usize>().ok()) {
                                set_tag_attr(&mut sp, &toks[si], key, Some(&n.saturating_sub(removed).to_string()));
                            }
                        }
                    }
                }
            }
            i += 1;
            continue;
        }
        if !is_open(&t) {
            i += 1;
            continue;
        }
        let tag = &src[t.start..t.end];
        let local = t.local();
        if let Some(top) = stack.last_mut() {
            if top.1 == local {
                top.2 += 1;
            }
        }
        if own {
            if let Some(&(_, child)) = CONTAINERS.iter().find(|(c, _)| *c == local) {
                if t.kind == Kind::Start {
                    stack.push((i, child, 0, 0));
                }
                i += 1;
                continue;
            }
        }
        match local {
            "row" => {
                row_r = attr(tag, "r").and_then(|v| v.parse().ok()).unwrap_or(row_r + 1);
                col_c = 0;
                if own && rows {
                    match s.pos(row_r) {
                        None => {
                            i = remove(&mut sp, &mut stack, i, "row");
                            continue;
                        }
                        Some(n) if n != row_r || attr(tag, "r").is_none() => {
                            set_tag_attr(&mut sp, &t, "r", Some(&n.to_string()));
                        }
                        _ => {}
                    }
                } else if own {
                    if let Some((a, b)) = attr(tag, "spans").and_then(|v| v.split_once(':')) {
                        if let (Ok(a), Ok(b)) = (a.parse::<i32>(), b.parse::<i32>()) {
                            match s.span(a, b) {
                                Some((x, y)) if (x, y) != (a, b) => set_tag_attr(&mut sp, &t, "spans", Some(&format!("{x}:{y}"))),
                                None => set_tag_attr(&mut sp, &t, "spans", None),
                                _ => {}
                            }
                        }
                    }
                }
            }
            "c" => {
                col_c = attr(tag, "r").and_then(parse_ref).map(|(_, c)| c).unwrap_or(col_c + 1);
                let cell = (row_r, col_c);
                if own {
                    let new = if rows { s.pos(row_r).map(|r| (r, col_c)) } else { s.pos(col_c).map(|c| (row_r, c)) };
                    match new {
                        None => {
                            i = remove(&mut sp, &mut stack, i, "c");
                            continue;
                        }
                        Some(p) if p != cell || attr(tag, "r").is_none() => {
                            set_tag_attr(&mut sp, &t, "r", Some(&cell_ref(p.0, p.1)));
                        }
                        _ => {}
                    }
                }
            }
            "f" => {
                let text = text_span(&toks, i).map(|(a, b)| &src[a..b]);
                if let (Some((a, b)), Some(text)) = (text_span(&toks, i), text) {
                    if let Some(n) = shift_text(text, cur, s) {
                        sp.rep(a, b, n);
                    }
                }
                if own {
                    shift_attr_sqref(&mut sp, &t, "ref", s);
                    for k in ["r1", "r2"] {
                        shift_attr_sqref(&mut sp, &t, k, s);
                    }
                }
            }
            "formula" | "formula1" | "formula2" => {
                if let Some((a, b)) = text_span(&toks, i) {
                    if let Some(n) = shift_text(&src[a..b], cur, s) {
                        sp.rep(a, b, n);
                    }
                }
            }
            "hyperlink" => {
                shift_attr_formula(&mut sp, &t, "location", cur, s);
                if own && !shift_attr_sqref(&mut sp, &t, "ref", s) {
                    i = remove(&mut sp, &mut stack, i, "hyperlink");
                    continue;
                }
            }
            // extLst (x14 conditional formatting, validation, sparklines)
            "sqref" if own && xml::prefix(t.name).is_some() => {
                if let Some((a, b)) = text_span(&toks, i) {
                    if let Some(Some(n)) = refs::shift_sqref(&src[a..b], s) {
                        sp.rep(a, b, n);
                    }
                }
            }
            _ if !own => {}
            "dimension" => {
                shift_attr_sqref(&mut sp, &t, "ref", s);
            }
            "mergeCell" => {
                let keep = shift_attr_sqref(&mut sp, &t, "ref", s);
                let single = sp
                    .reps
                    .iter()
                    .find(|r| r.0 == t.start)
                    .and_then(|r| attr(&r.2, "ref").map(|v| v.split_once(':').is_none_or(|(a, b)| a == b)))
                    .unwrap_or(false);
                if !keep || single {
                    i = remove(&mut sp, &mut stack, i, "mergeCell");
                    continue;
                }
            }
            "conditionalFormatting" | "dataValidation" | "protectedRange" | "ignoredError" => {
                if !shift_attr_sqref(&mut sp, &t, "sqref", s) {
                    i = remove(&mut sp, &mut stack, i, local);
                    continue;
                }
            }
            "selection" => {
                shift_attr_sqref(&mut sp, &t, "activeCell", s);
                shift_attr_sqref(&mut sp, &t, "sqref", s);
            }
            "pane" => {
                shift_attr_sqref(&mut sp, &t, "topLeftCell", s);
            }
            "cellWatch" => {
                if !shift_attr_sqref(&mut sp, &t, "r", s) {
                    i = remove(&mut sp, &mut stack, i, local);
                    continue;
                }
            }
            "autoFilter" => {
                if let Some(b) = attr(tag, "ref").and_then(refs::parse_body) {
                    let first = match b {
                        Body::Area(a, _) | Body::Cell(a) => Some(a.col.v),
                        _ => None,
                    };
                    if let Some(f) = first {
                        let nf = refs::shift_body(&b, s).and_then(|nb| match nb {
                            Body::Area(a, _) | Body::Cell(a) => Some(a.col.v),
                            _ => None,
                        });
                        af_first = nf.map(|n| (f, n));
                    }
                }
                shift_attr_sqref(&mut sp, &t, "ref", s);
            }
            "filterColumn" if !rows => {
                if let (Some((old, new)), Some(id)) = (af_first, attr(tag, "colId").and_then(|v| v.parse::<i32>().ok())) {
                    match s.pos(old + id) {
                        None => {
                            i = remove(&mut sp, &mut stack, i, local);
                            continue;
                        }
                        Some(c) if c - new != id => set_tag_attr(&mut sp, &t, "colId", Some(&(c - new).to_string())),
                        _ => {}
                    }
                }
            }
            "sortState" | "sortCondition" => {
                shift_attr_sqref(&mut sp, &t, "ref", s);
            }
            "col" if !rows => {
                let lo = attr(tag, "min").and_then(|v| v.parse().ok());
                let hi = attr(tag, "max").and_then(|v| v.parse().ok());
                if let (Some(lo), Some(hi)) = (lo, hi) {
                    match s.span(lo, hi) {
                        None => {
                            i = remove(&mut sp, &mut stack, i, "col");
                            continue;
                        }
                        Some((a, b)) if (a, b) != (lo, hi) => {
                            set_tag_attr(&mut sp, &t, "min", Some(&a.to_string()));
                            set_tag_attr(&mut sp, &t, "max", Some(&b.to_string()));
                        }
                        _ => {}
                    }
                }
            }
            "brk" => {
                let parent_rows = stack.last().map(|(si, ..)| toks[*si].local() == "rowBreaks").unwrap_or(false);
                if parent_rows == rows {
                    if let Some(id) = attr(tag, "id").and_then(|v| v.parse::<i32>().ok()) {
                        match s.pos(id) {
                            None => {
                                i = remove(&mut sp, &mut stack, i, "brk");
                                continue;
                            }
                            Some(n) if n != id => set_tag_attr(&mut sp, &t, "id", Some(&n.to_string())),
                            _ => {}
                        }
                    }
                }
            }
            // form controls and OLE objects anchored in the sheet itself (x14)
            "anchor" if t.kind == Kind::Start => {
                let end = xml::element_end(&toks, i);
                let mode = if attr(tag, "moveWithCells") == Some("1") && attr(tag, "sizeWithCells") != Some("1") {
                    Mode::Move
                } else if attr(tag, "moveWithCells") == Some("1") {
                    Mode::TwoCell
                } else {
                    Mode::Fixed
                };
                shift_anchor(&mut sp, &toks, i, end, mode, s);
                i = skip_past(&toks, i, end);
                continue;
            }
            _ => {}
        }
        i += 1;
    }

    // shared formulas: members derive their formula from the master by offset, which
    // only stays right if the whole group and everything it reads move together
    let mut remat = Vec::new();
    for g in groups.values() {
        let Some(((mr, mc), text)) = &g.master else { continue };
        let (r1, c1, r2, c2) = g.range.unwrap_or((*mr, *mc, *mr, *mc));
        if group_shifts_uniformly(text, (*mr, *mc), (r1, c1, r2, c2), name, s) {
            continue;
        }
        for &(r, c) in &g.cells {
            let p = if !own {
                Some((r, c))
            } else if rows {
                s.pos(r).map(|n| (n, c))
            } else {
                s.pos(c).map(|n| (r, n))
            };
            remat.extend(p);
        }
    }
    remat.sort();
    Ok(SheetResult { xml: sp.finish(), rematerialize: remat })
}

fn shared_groups(src: &str, toks: &[Tok]) -> HashMap<String, Group> {
    let mut groups: HashMap<String, Group> = HashMap::new();
    let (mut row_r, mut col_c, mut cell) = (0, 0, (0, 0));
    for (i, t) in toks.iter().enumerate() {
        if !is_open(t) {
            continue;
        }
        let tag = &src[t.start..t.end];
        match t.local() {
            "row" => {
                row_r = attr(tag, "r").and_then(|v| v.parse().ok()).unwrap_or(row_r + 1);
                col_c = 0;
            }
            "c" => {
                col_c = attr(tag, "r").and_then(parse_ref).map(|(_, c)| c).unwrap_or(col_c + 1);
                cell = (row_r, col_c);
            }
            "f" if attr(tag, "t") == Some("shared") => {
                let Some(si) = attr(tag, "si") else { continue };
                let g = groups.entry(si.to_string()).or_default();
                g.cells.push(cell);
                let text = text_span(toks, i).map(|(a, b)| &src[a..b]).filter(|x| !x.trim().is_empty());
                if let (Some(r), Some(text)) = (attr(tag, "ref"), text) {
                    g.master = Some((cell, xml::unescape(text)));
                    g.range = refs::parse_body(r).map(|b| match b {
                        Body::Cell(p) => (p.row.v, p.col.v, p.row.v, p.col.v),
                        Body::Area(a, c) => (a.row.v.min(c.row.v), a.col.v.min(c.col.v), a.row.v.max(c.row.v), a.col.v.max(c.col.v)),
                        _ => (cell.0, cell.1, cell.0, cell.1),
                    });
                }
            }
            _ => {}
        }
    }
    groups
}

fn group_shifts_uniformly(formula: &str, master: (i32, i32), range: (i32, i32, i32, i32), sheet: &str, s: &Shift) -> bool {
    let rows = s.axis == Axis::Row;
    let (r1, c1, r2, c2) = range;
    let (lo, hi, m) = if rows { (r1, r2, master.0) } else { (c1, c2, master.1) };
    if s.is_sheet(sheet) && s.uniform_delta(lo, hi).is_none() {
        return false;
    }
    let ok = |p: refs::Part| p.abs || s.uniform_delta(p.v + (lo - m), p.v + (hi - m)).is_some();
    for r in refs::find_refs(formula) {
        if !refs::targets(&r, Some(sheet), s) {
            continue;
        }
        let good = match (r.body, rows) {
            (Body::Cell(p), true) => ok(p.row),
            (Body::Cell(p), false) => ok(p.col),
            (Body::Area(a, b), true) => ok(a.row) && ok(b.row),
            (Body::Area(a, b), false) => ok(a.col) && ok(b.col),
            (Body::Rows(a, b), true) | (Body::Cols(a, b), false) => ok(a) && ok(b),
            _ => true,
        };
        if !good {
            return false;
        }
    }
    true
}

// ----------------------------------------------------------------------------- anchors

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    TwoCell, // each corner follows its cell
    Move,    // moves with its top-left cell, keeps its size
    Fixed,
}

/// `from`/`to` markers (0-based `col`/`row` children) between tokens `i..end`.
fn shift_anchor(sp: &mut Splice, toks: &[Tok], i: usize, end: usize, mode: Mode, s: &Shift) {
    if mode == Mode::Fixed {
        return;
    }
    let src = sp.src;
    let want = if s.axis == Axis::Row { ("row", "rowOff") } else { ("col", "colOff") };
    // (index span, offset span, value)
    let mut marks: Vec<((usize, usize), Option<(usize, usize)>, i32)> = Vec::new();
    let mut j = i + 1;
    while j < toks.len() && toks[j].start < end {
        let t = toks[j];
        if t.kind == Kind::Start && matches!(t.local(), "from" | "to") {
            let mend = xml::element_end(toks, j);
            let mut idx = None;
            let mut off = None;
            let mut k = j + 1;
            while k < toks.len() && toks[k].start < mend {
                if let Some(span) = text_span(toks, k) {
                    if toks[k].local() == want.0 {
                        idx = src[span.0..span.1].trim().parse::<i32>().ok().map(|v| (span, v));
                    } else if toks[k].local() == want.1 {
                        off = Some(span);
                    }
                }
                k += 1;
            }
            if let Some((span, v)) = idx {
                marks.push((span, off, v));
            }
            j = skip_past(toks, j, mend);
            continue;
        }
        j += 1;
    }
    let Some(&(_, _, from)) = marks.first() else { return };
    let moved = |v: i32| -> (i32, bool) {
        match s.pos(v + 1) {
            Some(n) => (n - 1, false),
            None => (s.at - 1, true), // the corner's cell was deleted: snap to the gap
        }
    };
    let delta = moved(from).0 - from;
    for (n, (span, off, v)) in marks.into_iter().enumerate() {
        let (nv, snapped) = if mode == Mode::Move { (v + delta, n == 0 && moved(from).1) } else { moved(v) };
        if nv != v {
            sp.rep(span.0, span.1, nv.to_string());
        }
        if snapped {
            if let Some(o) = off {
                sp.rep(o.0, o.1, "0".into());
            }
        }
    }
}

/// DrawingML (`xl/drawings/drawingN.xml`) anchored on the edited sheet.
pub fn shift_drawing(src: &str, s: &Shift) -> Option<String> {
    let toks: Vec<Tok> = tokens(src).collect();
    let mut sp = Splice::new(src);
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        let mode = match (t.kind, t.local()) {
            (Kind::Start, "twoCellAnchor") => match attr(&src[t.start..t.end], "editAs") {
                Some("oneCell") => Mode::Move,
                Some("absolute") => Mode::Fixed,
                _ => Mode::TwoCell,
            },
            (Kind::Start, "oneCellAnchor") => Mode::Move,
            _ => {
                i += 1;
                continue;
            }
        };
        let end = xml::element_end(&toks, i);
        shift_anchor(&mut sp, &toks, i, end, mode, s);
        i = skip_past(&toks, i, end);
    }
    sp.finish()
}

/// Legacy VML (comment boxes, form controls): `x:Anchor`, `x:Row`, `x:Column` are 0-based.
/// Comment shapes whose cell was deleted are removed.
pub fn shift_vml(src: &str, s: &Shift) -> Option<String> {
    let toks: Vec<Tok> = tokens(src).collect();
    let mut sp = Splice::new(src);
    let rows = s.axis == Axis::Row;
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        if !(t.kind == Kind::Start && t.local() == "shape") {
            i += 1;
            continue;
        }
        let end = xml::element_end(&toks, i);
        let mut anchor = None;
        let mut cell_idx = None;
        for k in i + 1..toks.len() {
            if toks[k].start >= end {
                break;
            }
            if let Some(span) = text_span(&toks, k) {
                match toks[k].local() {
                    "Anchor" => anchor = Some(span),
                    "Row" if rows => cell_idx = Some(span),
                    "Column" if !rows => cell_idx = Some(span),
                    _ => {}
                }
            }
        }
        if let Some(span) = cell_idx {
            if let Ok(v) = src[span.0..span.1].trim().parse::<i32>() {
                match s.pos(v + 1) {
                    None => {
                        sp.rep(t.start, end, String::new());
                        i = skip_past(&toks, i, end);
                        continue;
                    }
                    Some(n) if n - 1 != v => sp.rep(span.0, span.1, (n - 1).to_string()),
                    _ => {}
                }
            }
        }
        if let Some(span) = anchor {
            let mut nums: Vec<i32> = src[span.0..span.1].split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if nums.len() == 8 {
                let (a, b) = if rows { (2, 6) } else { (0, 4) };
                let mut changed = false;
                for idx in [a, b] {
                    let (nv, snapped) = match s.pos(nums[idx] + 1) {
                        Some(n) => (n - 1, false),
                        None => (s.at - 1, true),
                    };
                    if nv != nums[idx] || snapped {
                        nums[idx] = nv;
                        if snapped {
                            nums[idx + 1] = 0;
                        }
                        changed = true;
                    }
                }
                if changed {
                    sp.rep(span.0, span.1, nums.iter().map(i32::to_string).collect::<Vec<_>>().join(", "));
                }
            }
        }
        i = skip_past(&toks, i, end);
    }
    sp.finish()
}

/// Comments and threaded comments: `ref` moves; comments on deleted cells go.
pub fn shift_comments(src: &str, s: &Shift) -> Option<String> {
    let toks: Vec<Tok> = tokens(src).collect();
    let mut sp = Splice::new(src);
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        if is_open(&t) && matches!(t.local(), "comment" | "threadedComment") {
            if !shift_attr_sqref(&mut sp, &t, "ref", s) {
                let end = xml::element_end(&toks, i);
                sp.rep(t.start, end, String::new());
                i = skip_past(&toks, i, end);
                continue;
            }
        }
        i += 1;
    }
    sp.finish()
}

/// Table parts on the edited sheet. Column edits inside a table, or deleting its
/// header or every data row, would need the table restructured: refused.
pub fn shift_table(src: &str, sheet: &str, s: &Shift) -> Result<Option<String>, String> {
    let toks: Vec<Tok> = tokens(src).collect();
    let mut sp = Splice::new(src);
    let mut filter: Option<String> = None;
    for (i, t) in toks.iter().enumerate() {
        if !is_open(t) {
            continue;
        }
        let tag = &src[t.start..t.end];
        match t.local() {
            "table" => {
                let name = attr(tag, "displayName").or(attr(tag, "name")).unwrap_or("?");
                let Some(Body::Area(a, b)) = attr(tag, "ref").and_then(refs::parse_body) else { continue };
                let header = attr(tag, "headerRowCount").map_or(1, |v| v.parse().unwrap_or(1));
                let totals = attr(tag, "totalsRowCount").map_or(0, |v| v.parse().unwrap_or(0));
                let (lo, hi) = if s.axis == Axis::Row { (a.row.v, b.row.v) } else { (a.col.v, b.col.v) };
                let touches = if s.count > 0 { s.at > lo && s.at <= hi } else { s.at <= hi && s.at - s.count > lo };
                if touches && s.axis == Axis::Col {
                    return Err(format!("table {name}: inserting/deleting columns inside a table"));
                }
                if s.axis == Axis::Row && s.count < 0 && touches {
                    let first_data = lo + header;
                    let last_data = hi - totals;
                    let (d0, d1) = (s.at, s.at - s.count - 1);
                    if (header > 0 && d0 <= lo) || (totals > 0 && d1 >= hi) || (d0 <= first_data && d1 >= last_data) {
                        return Err(format!("table {name}: deleting its header, totals or every data row"));
                    }
                }
                if s.count < 0 && s.span(lo, hi).is_none() {
                    return Err(format!("table {name}: deleting the whole table"));
                }
                shift_attr_sqref(&mut sp, t, "ref", s);
                // the filter covers header + data rows; it must follow the table, not move
                // on its own (inserting at the totals row grows the data, not the filter's end)
                if let Some(Body::Area(na, nb)) = sp.reps.last().and_then(|r| attr(&r.2, "ref").and_then(refs::parse_body)) {
                    filter = Some(fmt_area(na.row.v, na.col.v, nb.row.v - totals, nb.col.v));
                }
            }
            "autoFilter" => match &filter {
                Some(f) if attr(tag, "ref") != Some(f.as_str()) => set_tag_attr(&mut sp, t, "ref", Some(f)),
                Some(_) => {}
                None => {
                    shift_attr_sqref(&mut sp, t, "ref", s);
                }
            },
            "sortState" | "sortCondition" => {
                shift_attr_sqref(&mut sp, t, "ref", s);
            }
            "calculatedColumnFormula" | "totalsRowFormula" => {
                if let Some((a, b)) = text_span(&toks, i) {
                    if let Some(n) = shift_text(&src[a..b], Some(sheet), s) {
                        sp.rep(a, b, n);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(sp.finish())
}

fn fmt_area(r1: i32, c1: i32, r2: i32, c2: i32) -> String {
    format!("{}:{}", cell_ref(r1, c1), cell_ref(r2, c2))
}

/// Every element whose local name is in `names`, attribute `key` shifted as a range on the edited sheet.
fn shift_tags(src: &str, names: &[&str], key: &str, s: &Shift) -> Option<String> {
    let mut sp = Splice::new(src);
    for t in tokens(src) {
        if is_open(&t) && names.contains(&t.local()) {
            shift_attr_sqref(&mut sp, &t, key, s);
        }
    }
    sp.finish()
}

/// Pivot cache source on the edited sheet. A source range fully deleted is refused.
pub fn shift_pivot_cache(src: &str, s: &Shift) -> Result<Option<String>, String> {
    let mut sp = Splice::new(src);
    for t in tokens(src) {
        if is_open(&t) && t.local() == "worksheetSource" {
            let tag = &src[t.start..t.end];
            if attr(tag, "sheet").is_some_and(|n| s.is_sheet(&xml::unescape(n))) && !shift_attr_sqref(&mut sp, &t, "ref", s) {
                return Err("pivot cache source range deleted".into());
            }
        }
    }
    Ok(sp.finish())
}

/// Text of every element named `names` treated as a formula (charts: `c:f`; workbook: `definedName`).
fn shift_formula_elements(src: &str, names: &[&str], current: Option<&str>, s: &Shift) -> Option<String> {
    let toks: Vec<Tok> = tokens(src).collect();
    let mut sp = Splice::new(src);
    for (i, t) in toks.iter().enumerate() {
        if t.kind == Kind::Start && names.contains(&t.local()) {
            if let Some((a, b)) = text_span(&toks, i) {
                if let Some(n) = shift_text(&src[a..b], current, s) {
                    sp.rep(a, b, n);
                }
            }
        }
    }
    sp.finish()
}

// ----------------------------------------------------------------------------- package

/// Apply `s` to every affected part. `overlay` holds parts already rewritten by
/// earlier ops (read through it, write back into it). Returns cells to
/// rematerialize per worksheet part, in post-shift coordinates.
pub fn shift_package(
    pkg: &mut Package,
    overlay: &mut BTreeMap<String, String>,
    wb_path: &str,
    sheets: &[(String, String)],
    s: &Shift,
    remat: &mut BTreeMap<String, HashSet<(i32, i32)>>,
) -> Result<(), String> {
    fn get(pkg: &mut Package, overlay: &BTreeMap<String, String>, part: &str) -> Result<String, String> {
        match overlay.get(part) {
            Some(x) => Ok(x.clone()),
            None => pkg.read(part),
        }
    }
    let put = |overlay: &mut BTreeMap<String, String>, part: &str, new: Option<String>| {
        if let Some(x) = new {
            overlay.insert(part.to_string(), x);
        }
    };

    // earlier ops' rematerialized cells move with this one
    if let Some((_, own_part)) = sheets.iter().find(|(n, _)| s.is_sheet(n)) {
        if let Some(cells) = remat.get_mut(own_part) {
            *cells = cells
                .iter()
                .filter_map(|&(r, c)| if s.axis == Axis::Row { s.pos(r).map(|n| (n, c)) } else { s.pos(c).map(|n| (r, n)) })
                .collect();
        }
    }

    for (name, part) in sheets {
        if !pkg.has(part) {
            continue;
        }
        let src = get(pkg, overlay, part)?;
        let res = shift_worksheet(&src, name, s)?;
        put(overlay, part, res.xml);
        if !res.rematerialize.is_empty() {
            remat.entry(part.clone()).or_default().extend(res.rematerialize);
        }
        if !s.is_sheet(name) {
            continue;
        }
        for rel in pkg.rels(part)? {
            if rel.external || !pkg.has(&rel.target) {
                continue;
            }
            let k = rel.kind.as_str();
            let handled = ["/drawing", "/vmlDrawing", "/comments", "/threadedComment", "/table", "/pivotTable"];
            if !handled.iter().any(|h| k.ends_with(h)) {
                continue; // printer settings, embeddings, ... (binary or unrelated)
            }
            let src = get(pkg, overlay, &rel.target)?;
            let new = if k.ends_with("/drawing") {
                shift_drawing(&src, s)
            } else if k.ends_with("/vmlDrawing") {
                shift_vml(&src, s)
            } else if k.ends_with("/comments") || k.ends_with("/threadedComment") {
                shift_comments(&src, s)
            } else if k.ends_with("/table") {
                shift_table(&src, name, s)?
            } else if k.ends_with("/pivotTable") {
                shift_tags(&src, &["location"], "ref", s)
            } else {
                None
            };
            put(overlay, &rel.target, new);
        }
    }

    // defined names (print areas, filter ranges, named ranges)
    let wb = get(pkg, overlay, wb_path)?;
    put(overlay, wb_path, shift_formula_elements(&wb, &["definedName"], None, s));

    // charts and pivot caches anywhere in the package
    let rels_parts: Vec<String> = pkg.names.iter().filter(|n| n.ends_with(".rels")).cloned().collect();
    let mut charts = HashSet::new();
    let mut caches = HashSet::new();
    for rp in rels_parts {
        let owner = rp.replace("_rels/", "").trim_end_matches(".rels").to_string();
        let xml_src = pkg.read(&rp)?;
        for rel in parse_rels(&xml_src, &owner) {
            if rel.external {
                continue;
            }
            if rel.kind.ends_with("/chart") || rel.kind.ends_with("/chartEx") {
                charts.insert(rel.target);
            } else if rel.kind.ends_with("/pivotCacheDefinition") {
                caches.insert(rel.target);
            }
        }
    }
    for c in charts {
        if pkg.has(&c) {
            let src = get(pkg, overlay, &c)?;
            put(overlay, &c, shift_formula_elements(&src, &["f"], None, s));
        }
    }
    for c in caches {
        if pkg.has(&c) {
            let src = get(pkg, overlay, &c)?;
            put(overlay, &c, shift_pivot_cache(&src, s)?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ins_rows(at: i32, n: i32) -> Shift {
        Shift { sheet: "Data".into(), axis: Axis::Row, at, count: n }
    }
    fn ins_cols(at: i32, n: i32) -> Shift {
        Shift { sheet: "Data".into(), axis: Axis::Col, at, count: n }
    }

    const SHEET: &str = concat!(
        r#"<worksheet><dimension ref="A1:C6"/><sheetViews><sheetView><selection activeCell="B5" sqref="B5"/></sheetView></sheetViews>"#,
        r#"<cols><col min="2" max="3" width="20" customWidth="1"/></cols>"#,
        r#"<sheetData><row r="1" spans="1:3"><c r="A1"><v>1</v></c></row>"#,
        r#"<row r="5" ht="30" customHeight="1"><c r="A5"><v>5</v></c><c r="B5"><f>SUM(A1:A5)+Other!A5</f><v>6</v></c></row>"#,
        r#"<row r="6"><c r="C6"><f>Data!A5&amp;"x"</f><v>0</v></c></row></sheetData>"#,
        r#"<mergeCells count="2"><mergeCell ref="A5:B5"/><mergeCell ref="A6:B6"/></mergeCells>"#,
        r#"<conditionalFormatting sqref="A1:A6"><cfRule type="expression" priority="1"><formula>$A5&gt;3</formula></cfRule></conditionalFormatting>"#,
        r#"<dataValidations count="1"><dataValidation type="list" sqref="C5:C6"><formula1>$A$1:$A$5</formula1></dataValidation></dataValidations>"#,
        r#"<hyperlinks><hyperlink ref="A6" location="'Data'!A5"/></hyperlinks>"#,
        r#"<autoFilter ref="A1:C6"/><rowBreaks count="1" manualBreakCount="1"><brk id="5" max="16383" man="1"/></rowBreaks>"#,
        r#"<extLst><ext><x14:sparklineGroups><x14:sparklineGroup><x14:sparklines><x14:sparkline><xm:f>Data!A1:A5</xm:f><xm:sqref>C1</xm:sqref></x14:sparkline></x14:sparklines></x14:sparklineGroup></x14:sparklineGroups></ext></extLst>"#,
        r#"</worksheet>"#
    );

    #[test]
    fn worksheet_insert_rows() {
        let out = shift_worksheet(SHEET, "Data", &ins_rows(3, 2)).unwrap().xml.unwrap();
        for want in [
            r#"<dimension ref="A1:C8"/>"#,
            r#"<selection activeCell="B7" sqref="B7"/>"#,
            r#"<row r="1" spans="1:3"><c r="A1"><v>1</v></c></row>"#,
            r#"<row r="7" ht="30" customHeight="1"><c r="A7"><v>5</v></c><c r="B7"><f>SUM(A1:A7)+Other!A5</f>"#,
            r#"<row r="8"><c r="C8"><f>Data!A7&amp;"x"</f>"#,
            r#"<mergeCell ref="A7:B7"/><mergeCell ref="A8:B8"/>"#,
            r#"<conditionalFormatting sqref="A1:A8"><cfRule type="expression" priority="1"><formula>$A7&gt;3</formula>"#,
            r#"<dataValidation type="list" sqref="C7:C8"><formula1>$A$1:$A$7</formula1>"#,
            r#"<hyperlink ref="A8" location="'Data'!A7"/>"#,
            r#"<autoFilter ref="A1:C8"/>"#,
            r#"<brk id="7" max="16383" man="1"/>"#,
            r#"<xm:f>Data!A1:A7</xm:f><xm:sqref>C1</xm:sqref>"#,
            r#"<cols><col min="2" max="3" width="20" customWidth="1"/></cols>"#,
        ] {
            assert!(out.contains(want), "missing {want}\n{out}");
        }
    }

    #[test]
    fn worksheet_delete_rows() {
        let out = shift_worksheet(SHEET, "Data", &ins_rows(5, -1)).unwrap().xml.unwrap();
        assert!(!out.contains("customHeight"), "row 5 is gone");
        assert!(out.contains(r#"<row r="5"><c r="C5"><f>Data!#REF!&amp;"x"</f>"#), "{out}");
        assert!(out.contains(r#"<mergeCells count="1"><mergeCell ref="A5:B5"/></mergeCells>"#), "{out}");
        assert!(out.contains(r#"<formula1>$A$1:$A$4</formula1>"#));
        assert!(out.contains(r#"<dataValidation type="list" sqref="C5">"#));
        assert!(!out.contains("<rowBreaks"), "break on a deleted row goes, and its empty container: {out}");
        assert!(out.contains(r#"<hyperlink ref="A5" location="'Data'!#REF!"/>"#));
    }

    #[test]
    fn worksheet_columns() {
        let out = shift_worksheet(SHEET, "Data", &ins_cols(2, 1)).unwrap().xml.unwrap();
        assert!(out.contains(r#"<col min="3" max="4" width="20" customWidth="1"/>"#), "{out}");
        assert!(out.contains(r#"<row r="1" spans="1:4">"#));
        assert!(out.contains(r#"<c r="A5"><v>5</v></c><c r="C5"><f>SUM(A1:A5)+Other!A5</f>"#));
        assert!(out.contains(r#"<mergeCell ref="A5:C5"/>"#));
        let del = shift_worksheet(SHEET, "Data", &ins_cols(1, -1)).unwrap().xml.unwrap();
        assert!(del.contains(r#"<row r="5" ht="30" customHeight="1"><c r="A5"><f>SUM(#REF!)+Other!A5</f>"#), "{del}");
        assert!(!del.contains("<mergeCells"), "A5:B5 collapses to one cell: {del}");
    }

    #[test]
    fn other_sheets_only_formulas() {
        let other = r#"<worksheet><sheetData><row r="5"><c r="A5"><f>Data!A5+A5</f></c></row></sheetData><mergeCells count="1"><mergeCell ref="A5:B5"/></mergeCells></worksheet>"#;
        let out = shift_worksheet(other, "Other", &ins_rows(1, 1)).unwrap().xml.unwrap();
        assert_eq!(out, other.replace("Data!A5+A5", "Data!A6+A5"));
        let untouched = r#"<worksheet><sheetData><row r="5"><c r="A5"><f>A5</f></c></row></sheetData></worksheet>"#;
        assert!(shift_worksheet(untouched, "Other", &ins_rows(1, 1)).unwrap().xml.is_none());
    }

    #[test]
    fn shared_formulas_rematerialized_when_split() {
        // B1:B6 share =A1*2; inserting at row 4 splits the group
        let mut rows = String::from(r#"<worksheet><sheetData><row r="1"><c r="B1"><f t="shared" ref="B1:B6" si="0">A1*2</f><v>2</v></c></row>"#);
        for r in 2..=6 {
            rows.push_str(&format!(r#"<row r="{r}"><c r="B{r}"><f t="shared" si="0"/><v>2</v></c></row>"#));
        }
        rows.push_str("</sheetData></worksheet>");
        let res = shift_worksheet(&rows, "Data", &ins_rows(4, 1)).unwrap();
        assert_eq!(res.rematerialize.len(), 6);
        assert!(res.rematerialize.contains(&(7, 2)));
        // inserting below the group and everything it reads: still valid
        let res = shift_worksheet(&rows, "Data", &ins_rows(10, 1)).unwrap();
        assert!(res.rematerialize.is_empty() && res.xml.is_none());
        // a group on another sheet reading across the insertion point
        let other = rows.replace("A1*2", "Data!A1*2");
        let res = shift_worksheet(&other, "Other", &ins_rows(4, 1)).unwrap();
        assert_eq!(res.rematerialize.len(), 6);
        // the master's own row deleted: surviving members must be rematerialized
        let res = shift_worksheet(&rows, "Data", &ins_rows(1, -1)).unwrap();
        assert_eq!(res.rematerialize, vec![(1, 2), (2, 2), (3, 2), (4, 2), (5, 2)]);
        // absolute references are fine
        let abs = rows.replace("A1*2", "$A$1*2");
        let res = shift_worksheet(&abs, "Other", &ins_rows(1, 1)).unwrap();
        assert!(res.rematerialize.is_empty());
    }

    #[test]
    fn drawings() {
        let d = r#"<xdr:wsDr><xdr:twoCellAnchor><xdr:from><xdr:col>1</xdr:col><xdr:colOff>5</xdr:colOff><xdr:row>2</xdr:row><xdr:rowOff>7</xdr:rowOff></xdr:from><xdr:to><xdr:col>4</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>9</xdr:row><xdr:rowOff>3</xdr:rowOff></xdr:to></xdr:twoCellAnchor><xdr:oneCellAnchor><xdr:from><xdr:col>0</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>20</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from></xdr:oneCellAnchor><xdr:twoCellAnchor editAs="absolute"><xdr:from><xdr:col>0</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>20</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from></xdr:twoCellAnchor></xdr:wsDr>"#;
        // insert 2 rows at row 6 (0-based 5): inside the first shape (rows 3..10), above the second
        let out = shift_drawing(d, &ins_rows(6, 2)).unwrap();
        assert!(out.contains("<xdr:row>2</xdr:row><xdr:rowOff>7</xdr:rowOff></xdr:from>"));
        assert!(out.contains("<xdr:row>11</xdr:row><xdr:rowOff>3</xdr:rowOff></xdr:to>"));
        assert!(out.contains("<xdr:row>22</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from></xdr:oneCellAnchor>"));
        assert!(out.ends_with(r#"<xdr:twoCellAnchor editAs="absolute"><xdr:from><xdr:col>0</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>20</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from></xdr:twoCellAnchor></xdr:wsDr>"#));
        // deleting the start row of the first shape snaps it to the gap
        let out = shift_drawing(d, &ins_rows(3, -1)).unwrap();
        assert!(out.contains("<xdr:row>2</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from>"), "{out}");
        assert!(out.contains("<xdr:row>8</xdr:row><xdr:rowOff>3</xdr:rowOff></xdr:to>"));
        let out = shift_drawing(d, &ins_cols(1, 1)).unwrap();
        assert!(out.contains("<xdr:col>2</xdr:col><xdr:colOff>5</xdr:colOff>"));
    }

    #[test]
    fn vml_and_comments() {
        let v = r#"<xml><v:shape id="a"><x:ClientData ObjectType="Note"><x:Anchor>
    2, 15, 3, 10, 4, 15, 7, 4</x:Anchor><x:Row>4</x:Row><x:Column>1</x:Column></x:ClientData></v:shape><v:shape id="b"><br><x:ClientData ObjectType="Note"><x:Anchor>1, 0, 0, 0, 2, 0, 1, 0</x:Anchor><x:Row>0</x:Row><x:Column>0</x:Column></x:ClientData></v:shape></xml>"#;
        let out = shift_vml(v, &ins_rows(2, 3)).unwrap();
        assert!(out.contains("<x:Anchor>2, 15, 6, 10, 4, 15, 10, 4</x:Anchor><x:Row>7</x:Row>"), "{out}");
        assert!(out.contains("<x:Anchor>1, 0, 0, 0, 2, 0, 4, 0</x:Anchor><x:Row>0</x:Row>"), "{out}");
        let out = shift_vml(v, &ins_rows(5, -1)).unwrap();
        assert!(!out.contains(r#"id="a""#), "comment on a deleted row goes: {out}");
        let c = r#"<comments><commentList><comment ref="B5" authorId="0"><text/></comment><comment ref="A1" authorId="0"><text/></comment></commentList></comments>"#;
        assert_eq!(shift_comments(c, &ins_rows(2, 3)).unwrap(), c.replace("B5", "B8"));
        assert_eq!(shift_comments(c, &ins_rows(5, -1)).unwrap(), r#"<comments><commentList><comment ref="A1" authorId="0"><text/></comment></commentList></comments>"#);
    }

    #[test]
    fn tables_pivots_charts_names() {
        let t = r#"<table id="1" name="T" displayName="T" ref="A1:C10"><autoFilter ref="A1:C10"/><tableColumns count="1"><tableColumn id="1" name="x"><calculatedColumnFormula>Data!$A$5*2</calculatedColumnFormula></tableColumn></tableColumns></table>"#;
        let out = shift_table(t, "Data", &ins_rows(4, 2)).unwrap().unwrap();
        assert!(out.contains(r#"ref="A1:C12"><autoFilter ref="A1:C12"/>"#) && out.contains("Data!$A$7*2"), "{out}");
        assert!(shift_table(t, "Data", &ins_cols(2, 1)).is_err());
        assert!(shift_table(t, "Data", &ins_cols(4, 1)).unwrap().is_none());
        assert!(shift_table(t, "Data", &ins_rows(1, -1)).is_err(), "header row");
        assert!(shift_table(t, "Data", &ins_rows(3, -2)).unwrap().unwrap().contains(r#"ref="A1:C8""#));

        // inserting at the totals row grows the data; the filter (header + data) follows
        let tt = r#"<table id="1" name="T" displayName="T" ref="B4:D15" totalsRowCount="1"><autoFilter ref="B4:D14"/></table>"#;
        let out = shift_table(tt, "Data", &ins_rows(15, 2)).unwrap().unwrap();
        assert!(out.contains(r#"ref="B4:D17" totalsRowCount="1"><autoFilter ref="B4:D16"/>"#), "{out}");

        let cache = r#"<pivotCacheDefinition><cacheSource type="worksheet"><worksheetSource ref="A1:D20" sheet="Data"/></cacheSource></pivotCacheDefinition>"#;
        assert!(shift_pivot_cache(cache, &ins_rows(5, 1)).unwrap().unwrap().contains(r#"ref="A1:D21""#));
        assert!(shift_pivot_cache(&cache.replace("Data", "Else"), &ins_rows(5, 1)).unwrap().is_none());

        let chart = r#"<c:chart><c:ser><c:tx><c:strRef><c:f>Data!$B$1</c:f></c:strRef></c:tx><c:val><c:numRef><c:f>(Data!$B$2:$B$9,'Data'!$C$2)</c:f></c:numRef></c:val></c:ser></c:chart>"#;
        assert_eq!(shift_formula_elements(chart, &["f"], None, &ins_rows(3, 1)).unwrap(), chart.replace("$B$9", "$B$10"));

        let wb = r#"<definedNames><definedName name="_xlnm.Print_Area" localSheetId="0">Data!$A$1:$H$40</definedName><definedName name="_xlnm.Print_Titles" localSheetId="0">Data!$1:$2</definedName><definedName name="x">Else!$A$1</definedName></definedNames>"#;
        let out = shift_formula_elements(wb, &["definedName"], None, &ins_rows(1, 1)).unwrap();
        assert!(out.contains("Data!$A$2:$H$41") && out.contains("Data!$2:$3") && out.contains("Else!$A$1"));
    }
}
