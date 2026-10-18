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
//! Row/column inserts and deletes come in as an explicit operation log
//! ([`StructuralOp`]): the same ops are replayed on a copy of the baseline
//! model (so the cell diff above compares like with like) and on the original
//! XML, where every reference to the edited sheet is shifted (see `structural`).
//!
//! Anything the edits changed that this writer can't express yet (merges,
//! column widths, sheet add/rename, ...) is refused with an error rather than
//! silently lost.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Cursor;

use ironcalc_base::types::Cell;
use ironcalc_base::Model;
use serde::Serialize;

use crate::package::{append_to_root, remove_elements, resolve, Package};
use crate::sheet::{self, CellEdit};
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
    Remove,
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
    let mut sheet_parts = Vec::new();
    for t in tokens(&wb_xml).filter(|t| t.local() == "sheet" && matches!(t.kind, Kind::Empty | Kind::Start)) {
        let tag = &wb_xml[t.start..t.end];
        let rid = xml::attrs(tag).into_iter().find(|(k, _)| k.ends_with(":id")).map(|(_, v)| v);
        if let Some(rel) = rid.and_then(|id| by_id.get(id)) {
            if rel.kind.ends_with("/worksheet") {
                sheet_parts.push((xml::unescape(attr(tag, "name").unwrap_or("")), rel.target.clone()));
            }
        }
    }
    let worksheets = &work.workbook.worksheets;
    if sheet_parts.len() != worksheets.len()
        || sheet_parts.iter().zip(worksheets).any(|((n, _), w)| *n != w.name)
    {
        return Err(format!(
            "sheet mapping mismatch: file {:?} vs model {:?}",
            sheet_parts.iter().map(|(n, _)| n).collect::<Vec<_>>(),
            worksheets.iter().map(|w| &w.name).collect::<Vec<_>>()
        ));
    }
    // ---- structural ops: shift the original package's references first
    let mut overlay: BTreeMap<String, String> = BTreeMap::new();
    let mut remat: BTreeMap<String, HashSet<(i32, i32)>> = BTreeMap::new();
    for op in ops {
        let i = op.sheet() as usize;
        let name = &base.workbook.worksheets.get(i).ok_or("structural op on a missing sheet")?.name;
        crate::structural::shift_package(&mut pkg, &mut overlay, &wb_path, &sheet_parts, &op.to_shift(name), &mut remat)?;
    }
    report.structural_ops = ops.len();
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
    if diffs.iter().all(BTreeMap::is_empty) && ops.is_empty() {
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
        let (_, part) = &sheet_parts[i];
        let forced = remat.remove(part).unwrap_or_default();
        if d.is_empty() && forced.is_empty() {
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
                (Change::Value, t) => match (o_cells.get(&pos), t) {
                    (Some(o), Some(t)) => {
                        report.values_updated += 1;
                        CellEdit::Replace(patch_value(o, t, &mut strings, w_strings)?)
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
        replace.insert(part.clone(), sheet::splice(&orig, &edits, dim.as_deref()).into_bytes());
        report.sheets_rewritten += 1;
    }

    // ---- package-level fixes
    let mut drop = HashSet::new();
    let mut add = BTreeMap::new();
    let mut wb_rels_xml = pkg.read(&crate::package::rels_path(&wb_path))?;
    let mut ct_xml = pkg.read("[Content_Types].xml")?;
    let mut rels_changed = false;
    let mut ct_changed = false;
    // parts only the structural shift touched (other sheets, drawings, charts, ...)
    for (part, x) in overlay {
        replace.insert(part, x.into_bytes());
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
        if bs.merged_cells != ws.merged_cells {
            why.push(format!("{n}: merged cells"));
        }
        if bs.cols != ws.cols {
            why.push(format!("{n}: column widths/styles"));
        }
        if bs.rows != ws.rows {
            let r = bs.rows.iter().zip(&ws.rows).find(|(a, b)| a != b).map(|(a, _)| a.r).or(bs.rows.last().map(|x| x.r));
            why.push(format!("{n}: row heights/styles (row {})", r.unwrap_or(0)));
        }
        if bs.frozen_rows != ws.frozen_rows || bs.frozen_columns != ws.frozen_columns {
            why.push(format!("{n}: frozen panes"));
        }
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
    fn adopt_remaps_strings() {
        let mut st = StringTable::parse(Some("<sst><si><t>a</t></si></sst>".into()));
        let w = vec!["zzz".to_string(), "a".to_string()];
        let out = adopt_cell(r#"<c r="A1" t="s"><v>1</v></c>"#, None, &mut st, &w).unwrap();
        assert_eq!(out, r#"<c r="A1" t="s"><v>0</v></c>"#);
        let out = adopt_cell(r#"<c r="A2" t="s"><v>0</v></c>"#, None, &mut st, &w).unwrap();
        assert_eq!(out, r#"<c r="A2" t="s"><v>1</v></c>"#);
    }
}
