//! Sheet add, rename and delete for the patch writer.
//!
//! Like row/column edits (see `structural`), these come in as ops in the log and
//! are replayed on the original package in order:
//!
//! - rename: the `<sheet name>` in the workbook, every formula that names the
//!   sheet (cells, validation, conditional formats, hyperlinks, defined names,
//!   tables, chart series), pivot cache sources and the app.xml sheet titles.
//! - add: a new empty worksheet part wired into the workbook, its rels and the
//!   content types; its cells and layout then come from the normal cell diff.
//! - delete: the `<sheet>` entry, its part and every part only it used
//!   (drawings, charts, comments, tables, ...); references to it become
//!   `#REF!` as in Excel, sheet-scoped names go and `localSheetId`s shift.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::package::{append_to_root, parse_rels, rels_path, remove_elements, resolve, Package};
use crate::refs;
use crate::structural::{is_open, text_span, Splice};
use crate::xml::{self, attr, set_attr, tokens, Kind, Tok};

/// A worksheet as the package lists it: model name, part, and its `r:id` in the workbook.
#[derive(Debug, Clone)]
pub struct SheetPart {
    pub name: String,
    pub part: String,
    pub rid: String,
}

/// Package edits accumulated while replaying ops, on top of the original zip.
pub struct PkgEdits {
    /// Rewritten parts (also new ones; see `added`).
    pub overlay: BTreeMap<String, String>,
    /// Parts that don't exist in the original package.
    pub added: BTreeSet<String>,
    pub drop: HashSet<String>,
    pub wb_path: String,
    pub wb_rels: String,
    pub ct: String,
    pub rels_changed: bool,
    pub ct_changed: bool,
}

impl PkgEdits {
    pub fn get(&self, pkg: &mut Package, part: &str) -> Result<String, String> {
        match self.overlay.get(part) {
            Some(x) => Ok(x.clone()),
            None => pkg.read(part),
        }
    }

    pub fn exists(&self, pkg: &Package, part: &str) -> bool {
        !self.drop.contains(part) && (self.overlay.contains_key(part) || pkg.has(part))
    }

    fn put(&mut self, part: &str, new: Option<String>) {
        if let Some(x) = new {
            self.overlay.insert(part.to_string(), x);
        }
    }
}

const WORKSHEET_CT: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";
const MAIN_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// Rewrite formula text in elements named `elems` and formula attributes `(element, attr)`.
fn rewrite_part(
    src: &str,
    elems: &[&str],
    attrs: &[(&str, &str)],
    rw: &dyn Fn(&str) -> Result<Option<String>, String>,
) -> Result<Option<String>, String> {
    let toks: Vec<Tok> = tokens(src).collect();
    let mut sp = Splice::new(src);
    for (i, t) in toks.iter().enumerate() {
        if !is_open(t) {
            continue;
        }
        if t.kind == Kind::Start && elems.contains(&t.local()) {
            if let Some((a, b)) = text_span(&toks, i) {
                if let Some(n) = rw(&xml::unescape(&src[a..b]))? {
                    // text content only needs & < > escaped; keep quotes literal as Excel writes them
                    sp.rep(a, b, n.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;"));
                }
            }
        }
        for (el, key) in attrs {
            if t.local() == *el {
                let tag = &src[t.start..t.end];
                if let Some(v) = attr(tag, key) {
                    if let Some(n) = rw(&xml::unescape(v))? {
                        sp.rep(t.start, t.end, set_attr(tag, key, Some(&xml::escape(&n))));
                    }
                }
            }
        }
    }
    Ok(sp.finish())
}

/// Chart and pivot cache parts anywhere in the package.
fn charts_and_caches(pkg: &mut Package) -> Result<(BTreeSet<String>, BTreeSet<String>), String> {
    let mut charts = BTreeSet::new();
    let mut caches = BTreeSet::new();
    let rels_parts: Vec<String> = pkg.names.iter().filter(|n| n.ends_with(".rels")).cloned().collect();
    for rp in rels_parts {
        let owner = rp.replace("_rels/", "").trim_end_matches(".rels").to_string();
        for rel in parse_rels(&pkg.read(&rp)?, &owner) {
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
    Ok((charts, caches))
}

/// Apply `rw` to every formula in the package that could name a sheet.
fn rewrite_all_formulas(
    pkg: &mut Package,
    ed: &mut PkgEdits,
    sheets: &[SheetPart],
    rw: &dyn Fn(&str) -> Result<Option<String>, String>,
) -> Result<(), String> {
    for s in sheets {
        if !ed.exists(pkg, &s.part) {
            continue;
        }
        let src = ed.get(pkg, &s.part)?;
        let new = rewrite_part(&src, &["f", "formula", "formula1", "formula2"], &[("hyperlink", "location")], rw)?;
        ed.put(&s.part, new);
        for rel in pkg.rels(&s.part)? {
            if !rel.external && rel.kind.ends_with("/table") && ed.exists(pkg, &rel.target) {
                let src = ed.get(pkg, &rel.target)?;
                let new = rewrite_part(&src, &["calculatedColumnFormula", "totalsRowFormula"], &[], rw)?;
                ed.put(&rel.target, new);
            }
        }
    }
    let wb_path = ed.wb_path.clone();
    let wb = ed.get(pkg, &wb_path)?;
    let new = rewrite_part(&wb, &["definedName"], &[], rw)?;
    ed.put(&wb_path, new);
    let (charts, _) = charts_and_caches(pkg)?;
    for c in charts {
        if ed.exists(pkg, &c) {
            let src = ed.get(pkg, &c)?;
            let new = rewrite_part(&src, &["f"], &[], rw)?;
            ed.put(&c, new);
        }
    }
    Ok(())
}

/// Byte range of the `<sheet>` element with this relationship id in workbook.xml,
/// and its position among all `<sheet>` elements (what `localSheetId` counts).
fn find_sheet_tag(wb: &str, rid: &str) -> Option<(usize, usize, usize)> {
    let toks: Vec<Tok> = tokens(wb).collect();
    let mut pos = 0;
    for (i, t) in toks.iter().enumerate() {
        if is_open(t) && t.local() == "sheet" {
            let tag = &wb[t.start..t.end];
            if xml::attrs(tag).iter().any(|(k, v)| k.ends_with(":id") && *v == rid) {
                return Some((t.start, xml::element_end(&toks, i), pos));
            }
            pos += 1;
        }
    }
    None
}

// ----------------------------------------------------------------------------- rename

pub fn rename_sheet(pkg: &mut Package, ed: &mut PkgEdits, sheets: &mut [SheetPart], idx: usize, new_name: &str) -> Result<(), String> {
    let s = sheets.get(idx).ok_or("rename: no such sheet")?.clone();
    let old = s.name.clone();
    let wb_path = ed.wb_path.clone();
    let wb = ed.get(pkg, &wb_path)?;
    let (a, _, _) = find_sheet_tag(&wb, &s.rid).ok_or_else(|| format!("rename: sheet {old} not in workbook.xml"))?;
    let end = a + wb[a..].find('>').ok_or("bad <sheet> tag")? + 1;
    let tag = set_attr(&wb[a..end], "name", Some(&xml::escape(new_name)));
    ed.overlay.insert(wb_path.clone(), format!("{}{tag}{}", &wb[..a], &wb[end..]));

    sheets[idx].name = new_name.to_string();
    let (o, n) = (old.clone(), new_name.to_string());
    rewrite_all_formulas(pkg, ed, sheets, &move |f| Ok(refs::rename_sheet_in_formula(f, &o, &n)))?;

    // pivot cache sources name the sheet in an attribute, not a formula
    let (_, caches) = charts_and_caches(pkg)?;
    for c in caches {
        if !ed.exists(pkg, &c) {
            continue;
        }
        let src = ed.get(pkg, &c)?;
        let mut sp = Splice::new(&src);
        for t in tokens(&src) {
            if is_open(&t) && t.local() == "worksheetSource" {
                let tag = &src[t.start..t.end];
                if attr(tag, "sheet").is_some_and(|v| xml::unescape(v).to_lowercase() == old.to_lowercase()) {
                    sp.rep(t.start, t.end, set_attr(tag, "sheet", Some(&xml::escape(new_name))));
                }
            }
        }
        let new = sp.finish();
        ed.put(&c, new);
    }
    // links on shapes and images go to a sheet through a relationship: Target="#'Sheet'!A1"
    let rels_parts: Vec<String> = pkg.names.iter().filter(|n| n.ends_with(".rels") && !ed.drop.contains(*n)).cloned().collect();
    for rp in rels_parts {
        let src = ed.get(pkg, &rp)?;
        let mut sp = Splice::new(&src);
        for t in tokens(&src) {
            let tag = &src[t.start..t.end];
            if !(is_open(&t) && t.local() == "Relationship" && attr(tag, "Type").is_some_and(|k| k.ends_with("/hyperlink"))) {
                continue;
            }
            let target = attr(tag, "Target").map(xml::unescape).unwrap_or_default();
            if let Some(n) = target.strip_prefix('#').and_then(|f| refs::rename_sheet_in_formula(f, &old, new_name)) {
                sp.rep(t.start, t.end, set_attr(tag, "Target", Some(&xml::escape(&format!("#{n}")))));
            }
        }
        let new = sp.finish();
        ed.put(&rp, new);
    }
    app_titles(pkg, ed, |titles| match titles.iter_mut().find(|t| **t == old) {
        Some(t) => {
            *t = new_name.to_string();
            true
        }
        None => false,
    })
}

// ----------------------------------------------------------------------------- add

pub fn add_sheet(pkg: &mut Package, ed: &mut PkgEdits, sheets: &mut Vec<SheetPart>, name: &str) -> Result<(), String> {
    let wb_path = ed.wb_path.clone();
    let wb = ed.get(pkg, &wb_path)?;
    let wb_dir = wb_path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
    let rels = parse_rels(&ed.wb_rels, &wb_path);
    let ws_kind = rels
        .iter()
        .find(|r| r.kind.ends_with("/worksheet"))
        .map(|r| r.kind.clone())
        .unwrap_or_else(|| format!("{REL_NS}/worksheet"));

    // a part name and relationship id nobody uses
    let mut k = 1;
    let part = loop {
        let p = resolve(&wb_path, &format!("worksheets/sheet{k}.xml"));
        if !pkg.has(&p) && !ed.added.contains(&p) {
            break p;
        }
        k += 1;
    };
    let mut n = k;
    let rid = loop {
        let id = format!("rIdSheaf{n}");
        if !rels.iter().any(|r| r.id == id) {
            break id;
        }
        n += 1;
    };
    let target = match part.strip_prefix(&format!("{wb_dir}/")) {
        Some(rel) if !wb_dir.is_empty() => rel.to_string(),
        _ => format!("/{part}"),
    };
    ed.wb_rels = append_to_root(&ed.wb_rels, &format!("<Relationship Id=\"{rid}\" Type=\"{ws_kind}\" Target=\"{target}\"/>"));
    ed.ct = append_to_root(&ed.ct, &format!("<Override PartName=\"/{part}\" ContentType=\"{WORKSHEET_CT}\"/>"));
    ed.rels_changed = true;
    ed.ct_changed = true;

    // <sheet> entry: next sheetId, same relationships prefix as the others
    let toks: Vec<Tok> = tokens(&wb).collect();
    let mut max_id = 0u32;
    let mut r_prefix = None;
    let mut sheets_end = None;
    for (i, t) in toks.iter().enumerate() {
        if is_open(t) && t.local() == "sheet" {
            let tag = &wb[t.start..t.end];
            max_id = max_id.max(attr(tag, "sheetId").and_then(|v| v.parse().ok()).unwrap_or(0));
            if r_prefix.is_none() {
                r_prefix = xml::attrs(tag).iter().find(|(k, _)| k.ends_with(":id")).map(|(k, _)| k.split(':').next().unwrap_or("r").to_string());
            }
        }
        if t.kind == Kind::Start && t.local() == "sheets" {
            let e = xml::element_end(&toks, i);
            sheets_end = Some(wb[..e].rfind("</").ok_or("bad <sheets>")?);
        }
    }
    let at = sheets_end.ok_or("workbook.xml has no <sheets>")?;
    let r = r_prefix.unwrap_or_else(|| "r".into());
    let p = toks.iter().find(|t| t.local() == "sheets").and_then(|t| xml::prefix(t.name)).map(|p| format!("{p}:")).unwrap_or_default();
    let entry = format!("<{p}sheet name=\"{}\" sheetId=\"{}\" {r}:id=\"{rid}\"/>", xml::escape(name), max_id + 1);
    let new_wb = format!("{}{entry}{}", &wb[..at], &wb[at..]);
    ed.overlay.insert(wb_path.clone(), new_wb);

    // an empty worksheet in the workbook's namespace; cells and layout come from the diff
    let root = toks.iter().find(|t| is_open(t)).map(|t| &wb[t.start..t.end]).unwrap_or("");
    let ns = attr(root, "xmlns").unwrap_or(MAIN_NS);
    let rns = xml::attrs(root).into_iter().find(|(k, _)| *k == format!("xmlns:{r}")).map(|(_, v)| v).unwrap_or(REL_NS);
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<worksheet xmlns=\"{ns}\" xmlns:{r}=\"{rns}\"><dimension ref=\"A1\"/><sheetViews><sheetView workbookViewId=\"0\"/></sheetViews><sheetFormatPr defaultRowHeight=\"15\"/><sheetData/><pageMargins left=\"0.7\" right=\"0.7\" top=\"0.75\" bottom=\"0.75\" header=\"0.3\" footer=\"0.3\"/></worksheet>"
    );
    ed.overlay.insert(part.clone(), body);
    ed.added.insert(part.clone());
    sheets.push(SheetPart { name: name.to_string(), part, rid });

    let count_before = sheets.len() - 1;
    app_titles(pkg, ed, |titles| {
        // only when the titles list matches the sheets we know, so the new one goes last
        if titles.len() != count_before {
            return false;
        }
        titles.push(name.to_string());
        true
    })
}

// ----------------------------------------------------------------------------- delete

pub fn delete_sheet(pkg: &mut Package, ed: &mut PkgEdits, sheets: &mut Vec<SheetPart>, idx: usize) -> Result<(), String> {
    let s = sheets.get(idx).ok_or("delete: no such sheet")?.clone();
    let wb_path = ed.wb_path.clone();

    // parts only this sheet uses: reachable from it, and not from anything else once it's gone
    let mut mine = BTreeSet::new();
    let mut todo = vec![s.part.clone()];
    while let Some(p) = todo.pop() {
        if !mine.insert(p.clone()) {
            continue;
        }
        for rel in pkg.rels(&p)? {
            if !rel.external && pkg.has(&rel.target) {
                todo.push(rel.target);
            }
        }
    }
    let mut others = BTreeSet::new();
    let mut todo: Vec<String> = parse_rels(&ed.wb_rels, &wb_path)
        .into_iter()
        .filter(|r| !r.external && r.id != s.rid)
        .map(|r| r.target)
        .collect();
    todo.extend(pkg.rels("")?.into_iter().filter(|r| !r.external && r.target != wb_path).map(|r| r.target));
    while let Some(p) = todo.pop() {
        if p == s.part || !others.insert(p.clone()) {
            continue;
        }
        for rel in pkg.rels(&p)? {
            if !rel.external && pkg.has(&rel.target) {
                todo.push(rel.target);
            }
        }
    }
    let gone: Vec<String> = mine.into_iter().filter(|p| !others.contains(p)).collect();

    // what we can't carry over: tables referenced by name elsewhere, slicers on its pivots/tables
    let has_slicers = pkg.names.iter().any(|n| n.contains("slicerCache") || n.contains("timelineCache"));
    let other_sheets: Vec<SheetPart> = sheets.iter().filter(|x| x.part != s.part).cloned().collect();
    for p in &gone {
        if !pkg.has(p) {
            continue;
        }
        let is_table = p.contains("/tables/");
        if has_slicers && (is_table || p.contains("/pivotTables/")) {
            return Err(format!("sheet {} has a table or pivot table with slicers", s.name));
        }
        if is_table {
            let t = pkg.read(p)?;
            let tag = tokens(&t).find(|x| is_open(x) && x.local() == "table").map(|x| t[x.start..x.end].to_string()).unwrap_or_default();
            let tname = attr(&tag, "displayName").or(attr(&tag, "name")).unwrap_or("").to_lowercase();
            if tname.is_empty() {
                continue;
            }
            let mut texts = vec![ed.get(pkg, &wb_path)?];
            for o in &other_sheets {
                if ed.exists(pkg, &o.part) {
                    texts.push(ed.get(pkg, &o.part)?);
                }
            }
            if texts.iter().any(|x| x.to_lowercase().contains(&format!("{tname}["))) {
                return Err(format!("sheet {} has table {tname}, used by formulas elsewhere", s.name));
            }
        }
    }

    // references to it become #REF! (its own formulas go with it)
    let name = s.name.clone();
    rewrite_all_formulas(pkg, ed, &other_sheets, &move |f| refs::delete_sheet_in_formula(f, &name))?;

    // workbook.xml: the <sheet> entry, names scoped to it, tab indices after it
    let wb = ed.get(pkg, &wb_path)?;
    let (a, b, pos) = find_sheet_tag(&wb, &s.rid).ok_or_else(|| format!("delete: sheet {} not in workbook.xml", s.name))?;
    let total = tokens(&wb).filter(|t| is_open(t) && t.local() == "sheet").count();
    let toks: Vec<Tok> = tokens(&wb).collect();
    let mut sp = Splice::new(&wb);
    sp.rep(a, b, String::new());
    for (i, t) in toks.iter().enumerate() {
        if !is_open(t) {
            continue;
        }
        let tag = &wb[t.start..t.end];
        match t.local() {
            "definedName" => {
                if let Some(id) = attr(tag, "localSheetId").and_then(|v| v.parse::<usize>().ok()) {
                    if id == pos {
                        sp.rep(t.start, xml::element_end(&toks, i), String::new());
                    } else if id > pos {
                        sp.rep(t.start, t.end, set_attr(tag, "localSheetId", Some(&(id - 1).to_string())));
                    }
                }
            }
            "workbookView" => {
                let mut new = tag.to_string();
                for key in ["activeTab", "firstSheet"] {
                    if let Some(v) = attr(tag, key).and_then(|v| v.parse::<usize>().ok()) {
                        let nv = if v > pos { v - 1 } else { v.min(total.saturating_sub(2)) };
                        if nv != v {
                            new = set_attr(&new, key, Some(&nv.to_string()));
                        }
                    }
                }
                if new != tag {
                    sp.rep(t.start, t.end, new);
                }
            }
            _ => {}
        }
    }
    let mut new_wb = sp.finish().unwrap_or(wb.clone());
    // a <definedNames> left empty is invalid
    if let (Some((da, db)), _) = crate::patch::find_child(&new_wb, "definedNames", &[]) {
        if !new_wb[da..db].contains("<definedName ") && !new_wb[da..db].contains(":definedName ") {
            new_wb = format!("{}{}", &new_wb[..da], &new_wb[db..]);
        }
    }
    ed.overlay.insert(wb_path.clone(), new_wb);

    let rid = s.rid.clone();
    ed.wb_rels = remove_elements(&ed.wb_rels, |tag| attr(tag, "Id") == Some(rid.as_str()));
    ed.rels_changed = true;
    for p in &gone {
        let pn = format!("/{p}");
        ed.ct = remove_elements(&ed.ct, |tag| attr(tag, "PartName") == Some(pn.as_str()));
        ed.overlay.remove(p);
        if !ed.added.remove(p) {
            ed.drop.insert(p.clone());
        }
        let rp = rels_path(p);
        if pkg.has(&rp) {
            // some writers (LibreOffice) list rels parts in the content types too
            let rn = format!("/{rp}");
            ed.ct = remove_elements(&ed.ct, |tag| attr(tag, "PartName") == Some(rn.as_str()));
            ed.drop.insert(rp);
        }
    }
    ed.ct_changed = true;
    sheets.remove(idx);

    let old = s.name.clone();
    app_titles(pkg, ed, |titles| match titles.iter().position(|t| *t == old) {
        Some(i) => {
            titles.remove(i);
            true
        }
        None => false,
    })
}

// ----------------------------------------------------------------------------- docProps/app.xml

/// Edit the worksheet titles in docProps/app.xml (`TitlesOfParts`, counted by the
/// "Worksheets" heading pair). `edit` returns false to leave the part alone; so does
/// any layout we don't recognise (the titles are informational, Excel rebuilds them).
fn app_titles(pkg: &mut Package, ed: &mut PkgEdits, edit: impl FnOnce(&mut Vec<String>) -> bool) -> Result<(), String> {
    let Some(app) = pkg.rels("")?.into_iter().find(|r| r.kind.ends_with("/extended-properties")).map(|r| r.target) else {
        return Ok(());
    };
    if !ed.exists(pkg, &app) {
        return Ok(());
    }
    let src = ed.get(pkg, &app)?;
    if let Some(new) = edit_app_titles(&src, edit) {
        ed.overlay.insert(app, new);
    }
    Ok(())
}

fn edit_app_titles(src: &str, edit: impl FnOnce(&mut Vec<String>) -> bool) -> Option<String> {
    let toks: Vec<Tok> = tokens(src).collect();
    let find = |name: &str| toks.iter().position(|t| t.kind == Kind::Start && t.local() == name);
    let (hp, tp) = (find("HeadingPairs")?, find("TitlesOfParts")?);
    // heading pairs: (lpstr name, i4 count) ...; the worksheet titles start after the earlier groups
    let hp_end = xml::element_end(&toks, hp);
    let mut offset = 0usize;
    let mut ws: Option<(usize, (usize, usize))> = None; // (count, span of its i4 text)
    let mut last_name: Option<String> = None;
    let mut i = hp;
    while i < toks.len() && toks[i].start < hp_end {
        if let Some(span) = text_span(&toks, i) {
            match toks[i].local() {
                "lpstr" => last_name = Some(xml::unescape(&src[span.0..span.1])),
                "i4" => {
                    let n: usize = src[span.0..span.1].trim().parse().ok()?;
                    if last_name.as_deref() == Some("Worksheets") && ws.is_none() {
                        ws = Some((n, span));
                    } else if ws.is_none() {
                        offset += n;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    let (count, count_span) = ws?;
    // titles vector
    let tp_end = xml::element_end(&toks, tp);
    let vec_i = (tp..toks.len()).find(|&k| toks[k].start < tp_end && toks[k].kind == Kind::Start && toks[k].local() == "vector")?;
    let items: Vec<(usize, usize)> = (vec_i..toks.len())
        .take_while(|&k| toks[k].start < tp_end)
        .filter(|&k| is_open(&toks[k]) && toks[k].local() == "lpstr")
        .map(|k| (toks[k].start, xml::element_end(&toks, k)))
        .collect();
    if offset + count > items.len() {
        return None;
    }
    let el = |&(a, b): &(usize, usize)| &src[a..b];
    let text_of = |s: &str| {
        let a = s.find('>').map(|x| x + 1).unwrap_or(0);
        let b = s.rfind("</").unwrap_or(s.len());
        xml::unescape(&s[a..b])
    };
    let mut titles: Vec<String> = items[offset..offset + count].iter().map(|x| text_of(el(x))).collect();
    let before = titles.clone();
    if !edit(&mut titles) || titles == before {
        return None;
    }
    let p = xml::prefix(toks[vec_i].name).map(|p| format!("{p}:")).unwrap_or_default();
    let rendered: String = titles.iter().map(|t| format!("<{p}lpstr>{}</{p}lpstr>", xml::escape(t))).collect();
    // replace the worksheet run of titles (or insert where it would be)
    let (ra, rb) = match (items.get(offset), offset + count) {
        (Some(&(a, _)), end) if count > 0 => (a, items[end - 1].1),
        _ => {
            let at = if offset == 0 { toks[vec_i].end } else { items[offset - 1].1 };
            (at, at)
        }
    };
    let vec_tag = &src[toks[vec_i].start..toks[vec_i].end];
    let size = attr(vec_tag, "size").and_then(|v| v.parse::<usize>().ok())?;
    let new_size = size + titles.len() - count;
    let mut sp = Splice::new(src);
    sp.rep(count_span.0, count_span.1, titles.len().to_string());
    sp.rep(toks[vec_i].start, toks[vec_i].end, set_attr(vec_tag, "size", Some(&new_size.to_string())));
    sp.rep(ra, rb, rendered);
    sp.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = r#"<Properties><HeadingPairs><vt:vector size="4" baseType="variant"><vt:variant><vt:lpstr>Worksheets</vt:lpstr></vt:variant><vt:variant><vt:i4>2</vt:i4></vt:variant><vt:variant><vt:lpstr>Named Ranges</vt:lpstr></vt:variant><vt:variant><vt:i4>1</vt:i4></vt:variant></vt:vector></HeadingPairs><TitlesOfParts><vt:vector size="3" baseType="lpstr"><vt:lpstr>Data</vt:lpstr><vt:lpstr>Sum &amp; Co</vt:lpstr><vt:lpstr>Data!Rate</vt:lpstr></vt:vector></TitlesOfParts></Properties>"#;

    #[test]
    fn app_titles_edits() {
        let out = edit_app_titles(APP, |t| {
            t[0] = "Q1".into();
            true
        })
        .unwrap();
        assert!(out.contains("<vt:lpstr>Q1</vt:lpstr><vt:lpstr>Sum &amp; Co</vt:lpstr><vt:lpstr>Data!Rate</vt:lpstr>"), "{out}");
        let out = edit_app_titles(APP, |t| {
            t.push("New".into());
            true
        })
        .unwrap();
        assert!(out.contains(r#"<vt:i4>3</vt:i4>"#) && out.contains(r#"<vt:vector size="4" baseType="lpstr">"#), "{out}");
        assert!(out.contains("<vt:lpstr>Sum &amp; Co</vt:lpstr><vt:lpstr>New</vt:lpstr><vt:lpstr>Data!Rate</vt:lpstr>"), "{out}");
        let out = edit_app_titles(APP, |t| {
            t.remove(0);
            true
        })
        .unwrap();
        assert!(out.contains(r#"<vt:i4>1</vt:i4>"#) && out.contains(r#"size="2" baseType="lpstr"><vt:lpstr>Sum &amp; Co</vt:lpstr><vt:lpstr>Data!Rate"#), "{out}");
        assert!(edit_app_titles("<Properties/>", |_| true).is_none());
    }

    #[test]
    fn rewrites_formula_parts() {
        let ws = r#"<worksheet><sheetData><row r="1"><c r="A1"><f>Data!A1&amp;"Data!A1"</f></c></row></sheetData><dataValidations count="1"><dataValidation sqref="B1"><formula1>Data!$A$1:$A$3</formula1></dataValidation></dataValidations><hyperlinks><hyperlink ref="A1" location="'Data'!A1"/></hyperlinks></worksheet>"#;
        let out = rewrite_part(ws, &["f", "formula1"], &[("hyperlink", "location")], &|f| Ok(refs::rename_sheet_in_formula(f, "Data", "My Data"))).unwrap().unwrap();
        assert!(out.contains(r#"<f>'My Data'!A1&amp;"Data!A1"</f>"#), "{out}");
        assert!(out.contains(r#"<formula1>'My Data'!$A$1:$A$3</formula1>"#));
        assert!(out.contains(r#"location="&apos;My Data&apos;!A1""#) || out.contains(r#"location="'My Data'!A1""#), "{out}");
    }
}
