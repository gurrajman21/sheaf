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
