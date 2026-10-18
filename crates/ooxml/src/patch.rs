//! Patch-in-place xlsx save.
//!
//! IronCalc's own writer regenerates the whole package from its model and drops
//! everything it doesn't model (charts, pivots, comments, validation, VBA, ...).
//! Instead, we start from the original package and change only what the edits
//! changed:
//!
//! 1. Diff the working model against a baseline model loaded from the same
//!    file and evaluated the same way, cell by cell.
//! 2. Unchanged cells keep their original XML byte-for-byte (including Excel's
//!    cached values). Cells whose *computed value* changed keep their XML but get
//!    a new `<v>`. Edited/new cells take the XML IronCalc's exporter writes,
//!    with style and string indices remapped into the original tables.
//! 3. New strings/styles are appended to the original tables; calcChain is
//!    dropped when any cell changed (Excel rebuilds it).
//! 4. Every other part is raw-copied from the original zip.
//!
//! Row/column inserts and deletes and sheet add/rename/delete come in as an
//! explicit operation log ([`StructuralOp`]): the same ops are replayed on a
//! copy of the baseline model (so the cell diff above compares like with like)
//! and on the original XML, where every reference to the edited sheet is
//! shifted or renamed (see `structural` and `sheetops`). A new sheet starts as
//! an empty part and gets its cells from the diff like any other sheet.
//!
//! Anything the edits changed that this writer can't express yet (sheet
//! visibility, defined-name edits, ...) is refused with an error rather than
//! silently lost.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Cursor;

use ironcalc_base::types::Cell;
use ironcalc_base::Model;
use serde::Serialize;

use crate::package::{append_to_root, remove_elements, resolve, Package};
use crate::sheet::{self, CellEdit};
use crate::sheetops::{self, PkgEdits, SheetPart};
pub use crate::structural::StructuralOp;
use crate::tables::{StringTable, StyleTable};
use crate::xml::{self, attr, set_attr, tokens, Kind, Tok};

#[derive(Debug, Default, Serialize)]
pub struct PatchReport {
    pub sheets_rewritten: usize,
    pub cells_replaced: usize,
    pub cells_added: usize,
    pub cells_removed: usize,
    pub values_updated: usize,
    /// Shared-formula members rewritten with explicit formulas because their master changed.
    pub cells_rematerialized: usize,
    pub strings_added: usize,
    pub styles_added: usize,
    pub calc_chain_removed: bool,
    /// Structural ops replayed on the package.
    pub structural_ops: usize,
    pub parts_rewritten: Vec<String>,
}

#[derive(PartialEq, Debug)]
enum Content {
    Empty,
    Bool(bool),
    Num(u64),
    Error(String),
    Str(i32),
    Formula(String),
    Array(String, String),
    Spill(i32, i32),
}

/// Formulas are compared by text, not by their index in the sheet's formula list:
/// structural ops re-add displaced formulas, so indices aren't stable across models.
fn content(c: &Cell, formulas: &[String]) -> (Content, i32) {
    let text = |f: i32| formulas.get(f as usize).cloned().unwrap_or_default();
    match c {
        Cell::EmptyCell { s } => (Content::Empty, *s),
        Cell::BooleanCell { v, s } => (Content::Bool(*v), *s),
        Cell::NumberCell { v, s } => (Content::Num(v.to_bits()), *s),
        Cell::ErrorCell { ei, s } => (Content::Error(ei.to_string()), *s),
        Cell::SharedString { si, s } => (Content::Str(*si), *s),
        Cell::CellFormula { f, s, .. } => (Content::Formula(text(*f)), *s),
        Cell::ArrayFormula { f, s, kind, .. } => (Content::Array(text(*f), format!("{kind:?}")), *s),
        Cell::SpillCell { s, a, .. } => (Content::Spill(a.0, a.1), *s),
    }
}

enum Change {
    Replace,
    Add,
    Value,
    /// Array formula whose spill range changed: new value, and its `ref` follows IronCalc's.
    Resize,
    Remove,
}

/// Spill range (width, height) of an array formula anchor.
fn array_size(c: &Cell) -> Option<(i32, i32)> {
    match c {
        Cell::ArrayFormula { r, .. } => Some(*r),
        _ => None,
    }
}

pub fn save_patched(original: &[u8], base: &Model, work: &Model) -> Result<(Vec<u8>, PatchReport), String> {
    save_patched_ops(original, base, work, &[])
}

/// Like [`save_patched`], for a working model that also had `ops` applied (in order).
pub fn save_patched_ops(
    original: &[u8],
    base: &Model,
    work: &Model,
    ops: &[StructuralOp],
) -> Result<(Vec<u8>, PatchReport), String> {
    // the baseline with the same structural edits, so only real cell edits show in the diff
    // style indices are relative to the file as loaded, before any op added styles
    let st = &base.workbook.styles;
    let style_counts = (st.cell_xfs.len(), st.fonts.len(), st.fills.len(), st.borders.len());
    let shifted;
    let base = if ops.is_empty() {
        base
    } else {
        let mut m = Model::from_workbook(base.workbook.clone(), "en")?;
        for op in ops {
            op.apply(&mut m)?;
        }
        m.evaluate();
        shifted = m;
        &shifted
    };
    check_unsupported(base, work)?;
    let mut report = PatchReport::default();
    let mut pkg = Package::open(original)?;

    // ---- locate parts
    let wb_path = pkg
        .rels("")?
        .into_iter()
        .find(|r| r.kind.ends_with("/officeDocument"))
        .map(|r| r.target)
        .unwrap_or_else(|| "xl/workbook.xml".into());
    let wb_rels = pkg.rels(&wb_path)?;
    let wb_xml = pkg.read(&wb_path)?;
    let by_id: HashMap<&str, _> = wb_rels.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut sheet_parts: Vec<SheetPart> = Vec::new();
    for t in tokens(&wb_xml).filter(|t| t.local() == "sheet" && matches!(t.kind, Kind::Empty | Kind::Start)) {
        let tag = &wb_xml[t.start..t.end];
        let rid = xml::attrs(tag).into_iter().find(|(k, _)| k.ends_with(":id")).map(|(_, v)| v);
        if let Some(rel) = rid.and_then(|id| by_id.get(id)) {
            if rel.kind.ends_with("/worksheet") {
                let name = xml::unescape(attr(tag, "name").unwrap_or(""));
                sheet_parts.push(SheetPart { name, part: rel.target.clone(), rid: rel.id.clone() });
            }
        }
    }
    // ---- structural ops, in order: shift references, add/rename/delete sheets
    let mut ed = PkgEdits {
        overlay: BTreeMap::new(),
        added: BTreeSet::new(),
        drop: HashSet::new(),
        wb_path: wb_path.clone(),
        wb_rels: pkg.read(&crate::package::rels_path(&wb_path))?,
        ct: pkg.read("[Content_Types].xml")?,
        rels_changed: false,
        ct_changed: false,
    };
    let mut remat: BTreeMap<String, HashSet<(i32, i32)>> = BTreeMap::new();
    for op in ops {
        let i = op.sheet() as usize;
        match op {
            StructuralOp::AddSheet { name } => sheetops::add_sheet(&mut pkg, &mut ed, &mut sheet_parts, name)?,
            StructuralOp::RenameSheet { name, .. } => sheetops::rename_sheet(&mut pkg, &mut ed, &mut sheet_parts, i, name)?,
            StructuralOp::DeleteSheet { .. } => {
                let part = sheet_parts.get(i).ok_or("delete: no such sheet")?.part.clone();
                sheetops::delete_sheet(&mut pkg, &mut ed, &mut sheet_parts, i)?;
                remat.remove(&part);
            }
            _ => {
                let name = sheet_parts.get(i).ok_or("structural op on a missing sheet")?.name.clone();
                let list: Vec<(String, String)> = sheet_parts.iter().map(|s| (s.name.clone(), s.part.clone())).collect();
                crate::structural::shift_package(&mut pkg, &mut ed.overlay, &wb_path, &list, &op.to_shift(&name), &mut remat)?;
            }
        }
    }
    // a later shift may have re-read parts of a deleted sheet (its charts, say)
    ed.overlay.retain(|p, _| !ed.drop.contains(p));
    report.structural_ops = ops.len();
    let worksheets = &work.workbook.worksheets;
    if sheet_parts.len() != worksheets.len()
        || sheet_parts.iter().zip(worksheets).any(|(s, w)| s.name != w.name)
    {
        return Err(format!(
            "sheet mapping mismatch: file {:?} vs model {:?}",
            sheet_parts.iter().map(|s| &s.name).collect::<Vec<_>>(),
            worksheets.iter().map(|w| &w.name).collect::<Vec<_>>()
        ));
    }
    let mut overlay = std::mem::take(&mut ed.overlay);
    let sst_rel = wb_rels.iter().find(|r| r.kind.ends_with("/sharedStrings"));
    let styles_rel = wb_rels.iter().find(|r| r.kind.ends_with("/styles"));
    let calc_rel = wb_rels.iter().find(|r| r.kind.ends_with("/calcChain"));

    // ---- diff every sheet first; most saves touch one or two
    let mut diffs: Vec<BTreeMap<(i32, i32), Change>> = Vec::new();
    for (i, ws) in worksheets.iter().enumerate() {
        let bws = &base.workbook.worksheets[i];
        let mut base_cells: HashMap<(i32, i32), Cell> = bws.sheet_data.cells().map(|(r, c, cell)| ((r, c), cell.into_owned())).collect();
        let mut d = BTreeMap::new();
        for (r, c, w) in ws.sheet_data.cells() {
            match base_cells.remove(&(r, c)) {
                None => {
                    d.insert((r, c), Change::Add);
                }
                Some(b) if content(&b, &bws.shared_formulas) != content(&w, &ws.shared_formulas) => {
                    d.insert((r, c), Change::Replace);
                }
                Some(b) if array_size(&b) != array_size(&w) => {
                    d.insert((r, c), Change::Resize);
                }
                Some(b) if b != *w => {
                    d.insert((r, c), Change::Value);
                }
                _ => {}
            }
        }
        for (k, _) in base_cells {
            d.insert(k, Change::Remove);
        }
        diffs.push(d);
    }
    let layouts: Vec<LayoutChanges> = (0..worksheets.len()).map(|i| layout_changes(base, work, i)).collect();
    if diffs.iter().all(BTreeMap::is_empty) && ops.is_empty() && !layouts.iter().any(LayoutChanges::any) {
        // nothing changed: the original bytes are the best possible output
        return Ok((original.to_vec(), report));
    }

    // ---- IronCalc's own serialization of the working model, as a source of cell XML
    let exported = ironcalc::export::save_xlsx_to_writer(work, Cursor::new(Vec::new()))
        .map_err(|e| format!("export: {e:?}"))?
        .into_inner();
    let mut tpkg = Package::open(&exported)?;
    let mut strings = StringTable::parse(match sst_rel {
        Some(r) if pkg.has(&r.target) => Some(pkg.read(&r.target)?),
        _ => None,
    });
    let mut styles = match styles_rel {
        Some(r) if pkg.has(&r.target) => Some(StyleTable::new(pkg.read(&r.target)?, &tpkg.read("xl/styles.xml")?, style_counts)?),
        _ => None,
    };
    let w_strings = &work.workbook.shared_strings;

    let mut replace: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (i, d) in diffs.iter().enumerate() {
        let part = &sheet_parts[i].part;
        let forced = remat.remove(part).unwrap_or_default();
        let layout = &layouts[i];
        if d.is_empty() && forced.is_empty() && !layout.any() && !overlay.contains_key(part) {
            continue;
        }
        let orig_xml = match overlay.remove(part) {
            Some(x) => x,
            None => pkg.read(part)?,
        };
        let orig = sheet::parse(&orig_xml)?;
        if orig.prefix.is_some() {
            return Err(format!("{part}: prefixed worksheet XML not supported"));
        }
        let t_xml = tpkg.read(&format!("xl/worksheets/sheet{}.xml", i + 1))?;
        let t_sheet = sheet::parse(&t_xml)?;
        let t_cells: HashMap<(i32, i32), &str> = t_sheet
            .rows
            .iter()
            .flat_map(|r| r.cells.iter().map(move |c| ((r.r, c.col), c.xml.as_str())))
            .collect();
        let o_cells: HashMap<(i32, i32), &str> = orig
            .rows
            .iter()
            .flat_map(|r| r.cells.iter().map(move |c| ((r.r, c.col), c.xml.as_str())))
            .collect();

        // Shared formulas: members (`<f t="shared" si="N"/>`) borrow their text from the
        // group's master. If the master is replaced or removed, every member must be
        // rewritten with an explicit formula or the file is corrupt.
        let mut groups: HashMap<String, (Option<(i32, i32)>, Vec<(i32, i32)>)> = HashMap::new();
        for (&pos, cell) in &o_cells {
            if let Some((si, master)) = shared_formula(cell) {
                let g = groups.entry(si).or_default();
                if master {
                    g.0 = Some(pos);
                } else {
                    g.1.push(pos);
                }
            }
        }
        let mut rematerialize: Vec<(i32, i32)> = forced.into_iter().collect();
        for (master, members) in groups.values() {
            if let Some(m) = master {
                if matches!(d.get(m), Some(Change::Replace | Change::Remove | Change::Add)) {
                    rematerialize.extend(members.iter().filter(|p| !matches!(d.get(p), Some(Change::Replace | Change::Remove))));
                }
            }
        }

        let mut edits = BTreeMap::new();
        let mut grew = false;
        rematerialize.sort();
        rematerialize.dedup();
        for pos in rematerialize {
            report.cells_rematerialized += 1;
            let edit = match t_cells.get(&pos) {
                Some(t) => CellEdit::Replace(adopt_cell(t, styles.as_mut(), &mut strings, w_strings)?),
                None => CellEdit::Remove, // never leave a member whose master is gone
            };
            edits.insert(pos, edit);
        }
        for (&pos, change) in d {
            if edits.contains_key(&pos) {
                continue;
            }
            let t_cell = t_cells.get(&pos).copied();
            let edit = match (change, t_cell) {
                (Change::Remove, _) | (Change::Replace | Change::Add, None) => {
                    report.cells_removed += 1;
                    CellEdit::Remove
                }
                (Change::Replace | Change::Add, Some(t)) => {
                    if matches!(change, Change::Add) {
                        report.cells_added += 1;
                        grew = true;
                    } else {
                        report.cells_replaced += 1;
                    }
                    CellEdit::Replace(adopt_cell(t, styles.as_mut(), &mut strings, w_strings)?)
                }
                (Change::Value | Change::Resize, t) => match (o_cells.get(&pos), t) {
                    (Some(o), Some(t)) => {
                        report.values_updated += 1;
                        let mut x = patch_value(o, t, &mut strings, w_strings)?;
                        if matches!(change, Change::Resize) {
                            // the old ref would leave new spill cells outside the array (#SPILL!
                            // on reopen) and stale ones inside it
                            x = set_array_ref(&x, array_ref(t).ok_or("resized array without ref")?);
                        }
                        CellEdit::Replace(x)
                    }
                    // cell exists only implicitly in the original (e.g. inside a spill): take IronCalc's
                    (None, Some(t)) => {
                        report.values_updated += 1;
                        CellEdit::Replace(adopt_cell(t, styles.as_mut(), &mut strings, w_strings)?)
                    }
                    _ => continue,
                },
            };
            edits.insert(pos, edit);
        }
        let dim = if grew {
            t_sheet.dimension.map(|(s, e)| attr(&t_xml[s..e], "ref").unwrap_or("A1").to_string())
        } else {
            None
        };
        // row heights/visibility/styles: copy only those attributes from IronCalc's row tag
        let mut row_tags: BTreeMap<i32, String> = BTreeMap::new();
        for &r in &layout.rows {
            let orig_tag = orig.rows.iter().find(|x| x.r == r).map(|x| x.start_tag.clone()).unwrap_or_else(|| format!("<row r=\"{r}\">"));
            let t_tag = t_sheet.rows.iter().find(|x| x.r == r).map(|x| x.start_tag.as_str());
            let mut tag = orig_tag;
            for key in ["ht", "customHeight", "hidden", "customFormat", "s"] {
                let mut v = t_tag.and_then(|t| attr(t, key)).map(str::to_string);
                if key == "s" {
                    if let Some(sv) = v.as_ref().and_then(|x| x.parse::<i32>().ok()) {
                        v = match styles.as_mut() {
                            Some(st) => Some(st.map_xf(sv)?.to_string()),
                            None => None,
                        };
                    }
                }
                tag = set_attr(&tag, key, v.as_deref());
            }
            row_tags.insert(r, tag);
        }
        let mut out = sheet::splice_with_rows(&orig, &edits, dim.as_deref(), &row_tags);
        if layout.merges {
            out = replace_child(&out, "mergeCells", child_xml(&t_xml, "mergeCells").as_deref(), AFTER_MERGE_CELLS);
        }
        if layout.cols {
            let cols = match child_xml(&t_xml, "cols") {
                Some(c) => Some(remap_style_attrs(&c, "style", styles.as_mut())?),
                None => None,
            };
            out = replace_child(&out, "cols", cols.as_deref(), AFTER_COLS);
        }
        if layout.view {
            out = replace_child(&out, "sheetViews", child_xml(&t_xml, "sheetViews").as_deref(), AFTER_SHEET_VIEWS);
        }
        replace.insert(part.clone(), out.into_bytes());
        report.sheets_rewritten += 1;
    }

    // ---- package-level fixes
    let mut drop = std::mem::take(&mut ed.drop);
    let mut add = BTreeMap::new();
    let mut wb_rels_xml = std::mem::take(&mut ed.wb_rels);
    let mut ct_xml = std::mem::take(&mut ed.ct);
    let mut rels_changed = ed.rels_changed;
    let mut ct_changed = ed.ct_changed;
    // parts only the structural ops touched (other sheets, drawings, charts, ...)
    for (part, x) in overlay {
        replace.insert(part, x.into_bytes());
    }
    // new sheets aren't in the original zip
    for part in &ed.added {
        if let Some(x) = replace.remove(part) {
            add.insert(part.clone(), x);
        }
    }
    if let Some(calc) = calc_rel {
        drop.insert(calc.target.clone());
        let id = calc.id.clone();
        wb_rels_xml = remove_elements(&wb_rels_xml, |tag| attr(tag, "Id") == Some(id.as_str()));
        let part_name = format!("/{}", calc.target);
        ct_xml = remove_elements(&ct_xml, |tag| attr(tag, "PartName") == Some(part_name.as_str()));
        rels_changed = true;
        ct_changed = true;
        report.calc_chain_removed = true;
    }
    if strings.changed() {
        let body = strings.render().into_bytes();
        match sst_rel {
            Some(r) if strings.exists() => {
                replace.insert(r.target.clone(), body);
            }
            _ => {
                // no shared-string part yet: create one and wire it up
                let target = resolve(&wb_path, "sharedStrings.xml");
                add.insert(target.clone(), body);
                wb_rels_xml = append_to_root(
                    &wb_rels_xml,
                    "<Relationship Id=\"rIdOoxmlPatchSst\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings\" Target=\"sharedStrings.xml\"/>",
                );
                ct_xml = append_to_root(
                    &ct_xml,
                    &format!("<Override PartName=\"/{target}\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml\"/>"),
                );
                rels_changed = true;
                ct_changed = true;
            }
        }
        report.strings_added = strings.added();
    }
    if let (Some(st), Some(r)) = (&styles, styles_rel) {
        if st.changed() {
            replace.insert(r.target.clone(), st.render().into_bytes());
            report.styles_added = st.added();
        }
    }
    if rels_changed {
        replace.insert(crate::package::rels_path(&wb_path), wb_rels_xml.into_bytes());
    }
    if ct_changed {
        replace.insert("[Content_Types].xml".into(), ct_xml.into_bytes());
    }
    report.parts_rewritten = replace.keys().chain(add.keys()).cloned().collect();
    let bytes = pkg.write(&replace, &drop, &add)?;
    Ok((bytes, report))
}

/// Sheet layout that differs between baseline and working model.
#[derive(Default)]
struct LayoutChanges {
    merges: bool,
    cols: bool,
    view: bool,
    rows: Vec<i32>,
}

impl LayoutChanges {
    fn any(&self) -> bool {
        self.merges || self.cols || self.view || !self.rows.is_empty()
    }
}

fn layout_changes(base: &Model, work: &Model, sheet: usize) -> LayoutChanges {
    let (b, w) = (&base.workbook.worksheets[sheet], &work.workbook.worksheets[sheet]);
    let rows_of = |ws: &ironcalc_base::types::Worksheet| -> HashMap<i32, String> {
        ws.rows.iter().map(|r| (r.r, format!("{r:?}"))).collect()
    };
    let (br, wr) = (rows_of(b), rows_of(w));
    let mut rows: Vec<i32> = br.keys().chain(wr.keys()).copied().filter(|r| br.get(r) != wr.get(r)).collect();
    rows.sort();
    rows.dedup();
    LayoutChanges {
        merges: b.merged_cells != w.merged_cells,
        cols: b.cols != w.cols,
        view: b.frozen_rows != w.frozen_rows || b.frozen_columns != w.frozen_columns || b.show_grid_lines != w.show_grid_lines,
        rows,
    }
}

/// Worksheet children that come after each element we may insert (schema order).
const AFTER_SHEET_VIEWS: &[&str] = &["sheetFormatPr", "cols", "sheetData"];
const AFTER_COLS: &[&str] = &["sheetData"];
const AFTER_MERGE_CELLS: &[&str] = &[
    "phoneticPr", "conditionalFormatting", "dataValidations", "hyperlinks", "printOptions", "pageMargins",
    "pageSetup", "headerFooter", "rowBreaks", "colBreaks", "customProperties", "cellWatches", "ignoredErrors",
    "smartTags", "drawing", "legacyDrawing", "legacyDrawingHF", "drawingHF", "picture", "oleObjects", "controls",
    "webPublishItems", "tableParts", "extLst",
];

/// Byte range of the root's direct child `name`, and where it would be inserted if absent.
pub(crate) fn find_child(src: &str, name: &str, following: &[&str]) -> (Option<(usize, usize)>, usize) {
    let toks: Vec<Tok> = tokens(src).collect();
    let (mut depth, mut found, mut insert_at, mut root_close) = (0usize, None, None, src.len());
    for (i, t) in toks.iter().enumerate() {
        let at_child = depth == 1 && matches!(t.kind, Kind::Start | Kind::Empty);
        if at_child {
            if t.local() == name && found.is_none() {
                found = Some((t.start, xml::element_end(&toks, i)));
            } else if insert_at.is_none() && following.contains(&t.local()) {
                insert_at = Some(t.start);
            }
        }
        match t.kind {
            Kind::Start => depth += 1,
            Kind::End => {
                depth -= 1;
                if depth == 0 {
                    root_close = t.start;
                }
            }
            _ => {}
        }
    }
    (found, insert_at.unwrap_or(root_close))
}

pub(crate) fn child_xml(src: &str, name: &str) -> Option<String> {
    find_child(src, name, &[]).0.map(|(a, b)| src[a..b].to_string())
}

/// Replace, insert or remove the root's direct child `name`.
pub(crate) fn replace_child(src: &str, name: &str, new: Option<&str>, following: &[&str]) -> String {
    match (find_child(src, name, following), new) {
        ((Some((a, b)), _), Some(n)) => format!("{}{n}{}", &src[..a], &src[b..]),
        ((Some((a, b)), _), None) => format!("{}{}", &src[..a], &src[b..]),
        ((None, at), Some(n)) => format!("{}{n}{}", &src[..at], &src[at..]),
        ((None, _), None) => src.to_string(),
    }
}

/// Remap `s`/`style` attributes in IronCalc-exported XML into the patched style table.
fn remap_style_attrs(xml_src: &str, key: &str, styles: Option<&mut StyleTable>) -> Result<String, String> {
    let mut out = String::with_capacity(xml_src.len());
    let mut cursor = 0;
    let mut styles = styles;
    for t in tokens(xml_src) {
        if !matches!(t.kind, Kind::Start | Kind::Empty) {
            continue;
        }
        let tag = &xml_src[t.start..t.end];
        if let Some(sv) = attr(tag, key).and_then(|v| v.parse::<i32>().ok()) {
            let mapped = match styles.as_deref_mut() {
                Some(st) => Some(st.map_xf(sv)?.to_string()),
                None if sv == 0 => Some("0".into()),
                None => None,
            };
            out.push_str(&xml_src[cursor..t.start]);
            out.push_str(&set_attr(tag, key, mapped.as_deref()));
            cursor = t.end;
        }
    }
    out.push_str(&xml_src[cursor..]);
    Ok(out)
}

/// Refuse edits this writer can't carry over yet, so nothing is lost silently.
fn check_unsupported(base: &Model, work: &Model) -> Result<(), String> {
    let (b, w) = (&base.workbook, &work.workbook);
    let mut why = Vec::new();
    if b.worksheets.len() != w.worksheets.len() {
        why.push("sheets added/removed".to_string());
    }
    for (bs, ws) in b.worksheets.iter().zip(&w.worksheets) {
        let n = &ws.name;
        if bs.name != ws.name {
            why.push(format!("sheet renamed ({} -> {})", bs.name, ws.name));
        }
        // merges, column widths, row heights/visibility and frozen panes are carried
        // over by `layout_changes`; sheet-level changes still aren't
        if bs.state != ws.state {
            why.push(format!("{n}: visibility"));
        }
    }
    if b.defined_names != w.defined_names {
        why.push("defined names".into());
    }
    if why.is_empty() {
        Ok(())
    } else {
        Err(format!("unsupported change for patch save: {}", why.join(", ")))
    }
}

/// `Some((si, is_master))` if the cell holds a shared formula.
fn shared_formula(cell: &str) -> Option<(String, bool)> {
    let toks: Vec<Tok> = tokens(cell).collect();
    let i = toks.iter().position(|t| t.local() == "f" && matches!(t.kind, Kind::Start | Kind::Empty))?;
    let tag = &cell[toks[i].start..toks[i].end];
    if attr(tag, "t") != Some("shared") {
        return None;
    }
    let si = attr(tag, "si")?.to_string();
    let has_text = toks[i].kind == Kind::Start && !xml::inner(cell, &toks, i).trim().is_empty();
    Some((si, attr(tag, "ref").is_some() && has_text))
}

fn cell_parts(cell: &str) -> (Vec<Tok<'_>>, String) {
    let toks: Vec<Tok> = tokens(cell).collect();
    let tag = cell[toks[0].start..toks[0].end].to_string();
    (toks, tag)
}

/// Raw `<v>` text of a cell, with shared-string indices translated into the patched table.
fn value_of(cell: &str, strings: &mut StringTable, w_strings: &[String]) -> Result<(Option<String>, Option<String>), String> {
    let (toks, tag) = cell_parts(cell);
    let t = attr(&tag, "t").map(str::to_string);
    let mut v = toks.iter().position(|tk| tk.local() == "v" && tk.kind == Kind::Start).map(|i| xml::inner(cell, &toks, i).to_string());
    if t.as_deref() == Some("s") {
        if let Some(raw) = &v {
            let si: usize = raw.trim().parse().map_err(|_| format!("bad string index {raw}"))?;
            let text = w_strings.get(si).ok_or(format!("string index {si} out of range"))?;
            v = Some(strings.get_or_add(text).to_string());
        }
    }
    Ok((t, v))
}

/// Take a cell exactly as IronCalc exported it, remapping style and string indices.
fn adopt_cell(t_cell: &str, styles: Option<&mut StyleTable>, strings: &mut StringTable, w_strings: &[String]) -> Result<String, String> {
    let (toks, tag) = cell_parts(t_cell);
    let mut new_tag = tag.clone();
    if let Some(s) = attr(&tag, "s").and_then(|v| v.parse::<i32>().ok()) {
        let mapped = match styles {
            Some(st) => st.map_xf(s)?,
            None if s == 0 => 0,
            None => return Err("file has no styles part but the edit needs one".into()),
        };
        let mapped = mapped.to_string();
        new_tag = set_attr(&new_tag, "s", if mapped == "0" { None } else { Some(&mapped) });
    }
    if toks[0].kind == Kind::Empty {
        return Ok(new_tag);
    }
    let (_, v) = value_of(t_cell, strings, w_strings)?;
    let mut body = String::new();
    for (i, tk) in toks.iter().enumerate().skip(1) {
        if tk.kind != Kind::Start && tk.kind != Kind::Empty {
            continue;
        }
        // direct children only
        if toks[1..i].iter().filter(|x| x.kind == Kind::Start).count() != toks[1..i].iter().filter(|x| x.kind == Kind::End).count() {
            continue;
        }
        match tk.local() {
            "v" => {
                if let Some(v) = &v {
                    body.push_str(&format!("<v>{v}</v>"));
                }
            }
            _ => body.push_str(&t_cell[tk.start..xml::element_end(&toks, i)]),
        }
    }
    Ok(format!("{new_tag}{body}</c>"))
}

/// `ref` of a cell's `<f t="array">`.
fn array_ref(cell: &str) -> Option<&str> {
    let tk = tokens(cell).find(|t| t.local() == "f" && matches!(t.kind, Kind::Start | Kind::Empty))?;
    let tag = &cell[tk.start..tk.end];
    (attr(tag, "t") == Some("array")).then(|| attr(tag, "ref")).flatten()
}

/// Set the `ref` of a cell's `<f>`.
fn set_array_ref(cell: &str, new_ref: &str) -> String {
    match tokens(cell).find(|t| t.local() == "f" && matches!(t.kind, Kind::Start | Kind::Empty)) {
        Some(tk) => format!("{}{}{}", &cell[..tk.start], set_attr(&cell[tk.start..tk.end], "ref", Some(new_ref)), &cell[tk.end..]),
        None => cell.to_string(),
    }
}

/// Keep the original cell (formula, attributes, extensions) and replace only its cached value.
fn patch_value(orig: &str, t_cell: &str, strings: &mut StringTable, w_strings: &[String]) -> Result<String, String> {
    let (t, v) = value_of(t_cell, strings, w_strings)?;
    let (toks, tag) = cell_parts(orig);
    let new_tag = set_attr(&tag, "t", t.as_deref()).replacen("/>", ">", 1);
    let mut f = String::new();
    let mut ext = String::new();
    let mut depth = 0;
    for (i, tk) in toks.iter().enumerate().skip(1) {
        match tk.kind {
            Kind::Start | Kind::Empty if depth == 0 => {
                let raw = &orig[tk.start..xml::element_end(&toks, i)];
                match tk.local() {
                    "f" => f.push_str(raw),
                    "extLst" => ext.push_str(raw),
                    _ => {} // old <v>/<is> dropped
                }
                if tk.kind == Kind::Start {
                    depth += 1;
                }
            }
            Kind::Start => depth += 1,
            Kind::End if tk.local() != "c" || depth > 0 => depth -= 1,
            _ => {}
        }
    }
    let v = v.map(|v| format!("<v>{v}</v>")).unwrap_or_default();
    Ok(format!("{new_tag}{f}{v}{ext}</c>"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_value_keeps_formula_and_attrs() {
        let mut st = StringTable::parse(None);
        let none: Vec<String> = Vec::new();
        let out = patch_value(r#"<c r="B2" s="4" cm="1"><f t="shared" si="0"/><v>1</v></c>"#, r#"<c r="B2" s="9" t="str"><f>X</f><v>hi</v></c>"#, &mut st, &none).unwrap();
        assert_eq!(out, r#"<c r="B2" s="4" cm="1" t="str"><f t="shared" si="0"/><v>hi</v></c>"#);
    }

    #[test]
    fn resized_array_takes_new_ref() {
        let mut st = StringTable::parse(None);
        let none: Vec<String> = Vec::new();
        let orig = r#"<c r="F1" cm="1"><f t="array" ref="F1:F2">_xlfn._xlws.FILTER(A1:A7,A1:A7&gt;5)</f><v>8</v></c>"#;
        let t = r#"<c r="F1" cm="1"><f t="array" ref="F1:F3">_xlfn._xlws.FILTER(A1:A7,A1:A7&gt;5)</f><v>9</v></c>"#;
        assert_eq!(array_ref(t), Some("F1:F3"));
        let out = set_array_ref(&patch_value(orig, t, &mut st, &none).unwrap(), array_ref(t).unwrap());
        assert_eq!(out, r#"<c r="F1" cm="1"><f t="array" ref="F1:F3">_xlfn._xlws.FILTER(A1:A7,A1:A7&gt;5)</f><v>9</v></c>"#);
    }

    /// End to end: an edit that makes FILTER return more rows grows the anchor's ref, writes the
    /// new spill cell as a plain value inside it, and the file reopens without #SPILL!.
    #[test]
    fn spill_range_follows_edit() {
        let mut m = Model::new_empty("t", "en", "UTC", "en").unwrap();
        for (r, v) in [(1, "5"), (2, "3"), (3, "8"), (4, "1")] {
            m.set_user_input(0, r, 1, v.into()).unwrap();
        }
        m.set_user_input(0, 1, 6, "=FILTER(A1:A4,A1:A4>4)".into()).unwrap();
        m.evaluate();
        let orig = ironcalc::export::save_xlsx_to_writer(&m, Cursor::new(Vec::new())).unwrap().into_inner();
        let load = |b: &[u8]| {
            let wb = ironcalc::import::load_from_xlsx_bytes(b, "t", "en", "UTC").unwrap();
            let mut m = Model::from_workbook(wb, "en").unwrap();
            m.evaluate();
            m
        };
        let base = load(&orig);
        let mut work = load(&orig);
        work.set_user_input(0, 2, 1, "7".into()).unwrap(); // A2: 3 -> 7, FILTER now 3 rows
        work.evaluate();
        let (out, _) = save_patched(&orig, &base, &work).unwrap();
        let back = load(&out);
        for r in 1..=3 {
            assert_eq!(back.get_formatted_cell_value(0, r, 6).unwrap(), work.get_formatted_cell_value(0, r, 6).unwrap(), "F{r}");
        }
        assert_eq!(back.get_formatted_cell_value(0, 3, 6).unwrap(), "8");

        // and shrinking back drops the cell that left the spill
        let mut work2 = load(&out);
        work2.set_user_input(0, 2, 1, "3".into()).unwrap();
        work2.set_user_input(0, 3, 1, "2".into()).unwrap(); // FILTER now 1 row
        work2.evaluate();
        let base2 = load(&out);
        let (out2, _) = save_patched(&out, &base2, &work2).unwrap();
        let back2 = load(&out2);
        assert_eq!(back2.get_formatted_cell_value(0, 1, 6).unwrap(), "5");
        assert_eq!(back2.get_formatted_cell_value(0, 2, 6).unwrap(), "");
        assert_eq!(back2.get_formatted_cell_value(0, 3, 6).unwrap(), "");
    }

    #[test]
    fn adopt_remaps_strings() {
        let mut st = StringTable::parse(Some("<sst><si><t>a</t></si></sst>".into()));
        let w = vec!["zzz".to_string(), "a".to_string()];
        let out = adopt_cell(r#"<c r="A1" t="s"><v>1</v></c>"#, None, &mut st, &w).unwrap();
        assert_eq!(out, r#"<c r="A1" t="s"><v>0</v></c>"#);
        let out = adopt_cell(r#"<c r="A2" t="s"><v>0</v></c>"#, None, &mut st, &w).unwrap();
        assert_eq!(out, r#"<c r="A2" t="s"><v>1</v></c>"#);
    }
}
