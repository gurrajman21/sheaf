//! Read-only facts about a package that IronCalc doesn't model, so the UI can
//! tell people what's in the file even when it can't display it yet.

use serde::Serialize;

use crate::package::Package;
use crate::xml::{self, attr, tokens, Kind};

#[derive(Debug, Default, Clone, Serialize, PartialEq)]
pub struct SheetObjects {
    pub charts: usize,
    pub images: usize,
    pub comments: usize,
    pub pivot_tables: usize,
    pub tables: usize,
}

/// Objects per worksheet, keyed by sheet name in workbook order.
pub fn sheet_objects(bytes: &[u8]) -> Result<Vec<(String, SheetObjects)>, String> {
    let mut pkg = Package::open(bytes)?;
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
        let Some(rel) = wb_rels.iter().find(|r| r.id == rid) else { continue };
        if !rel.kind.ends_with("/worksheet") {
            continue;
        }
        let name = xml::unescape(attr(tag, "name").unwrap_or(""));
        let mut objects = SheetObjects::default();
        for r in pkg.rels(&rel.target)? {
            let kind = r.kind.rsplit('/').next().unwrap_or("");
            match kind {
                "drawing" => {
                    for d in pkg.rels(&r.target)? {
                        match d.kind.rsplit('/').next().unwrap_or("") {
                            "chart" => objects.charts += 1,
                            "image" => objects.images += 1,
                            _ => {}
                        }
                    }
                }
                "comments" if pkg.has(&r.target) => {
                    let x = pkg.read(&r.target)?;
                    objects.comments += tokens(&x).filter(|t| t.local() == "comment" && t.kind != Kind::End).count();
                }
                "pivotTable" => objects.pivot_tables += 1,
                "table" => objects.tables += 1,
                _ => {}
            }
        }
        out.push((name, objects));
    }
    Ok(out)
}

/// A data-validation rule (only what the UI needs for dropdown lists).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Validation {
    pub kind: String,
    /// space-separated ranges, e.g. "A2:A100 C2"
    pub sqref: String,
    /// raw formula1: a quoted list ("a,b,c") or a reference / name
    pub formula1: String,
}

/// Data validations per worksheet (sheet name, rules), including x14 extension rules.
pub fn validations(bytes: &[u8]) -> Result<Vec<(String, Vec<Validation>)>, String> {
    let mut pkg = Package::open(bytes)?;
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
        let Some(rel) = wb_rels.iter().find(|r| r.id == rid && r.kind.ends_with("/worksheet")) else { continue };
        if !pkg.has(&rel.target) {
            continue;
        }
        let src = pkg.read(&rel.target)?;
        let toks: Vec<xml::Tok> = tokens(&src).collect();
        let mut rules = Vec::new();
        for (i, t) in toks.iter().enumerate() {
            if t.local() != "dataValidation" || t.kind != Kind::Start {
                continue;
            }
            let dtag = &src[t.start..t.end];
            let kind = attr(dtag, "type").unwrap_or("any").to_string();
            let end = xml::element_end(&toks, i);
            let mut sqref = attr(dtag, "sqref").map(str::to_string).unwrap_or_default();
            let mut formula1 = String::new();
            let mut j = i + 1;
            while j < toks.len() && toks[j].start < end {
                let c = toks[j];
                if c.kind == Kind::Start {
                    match c.local() {
                        "formula1" if formula1.is_empty() => {
                            // x14 rules wrap the formula in <xm:f>
                            let inner = xml::inner(&src, &toks, j);
                            let f = tokens(inner).position(|x| x.local() == "f");
                            formula1 = match f {
                                Some(_) => {
                                    let it: Vec<xml::Tok> = tokens(inner).collect();
                                    let k = it.iter().position(|x| x.local() == "f" && x.kind == Kind::Start).unwrap_or(0);
                                    xml::unescape(xml::inner(inner, &it, k))
                                }
                                None => xml::unescape(inner),
                            };
                        }
                        "sqref" => sqref = xml::unescape(xml::inner(&src, &toks, j)),
                        _ => {}
                    }
                }
                j += 1;
            }
            rules.push(Validation { kind, sqref, formula1: formula1.trim().to_string() });
        }
        out.push((xml::unescape(attr(tag, "name").unwrap_or("")), rules));
    }
    Ok(out)
}
