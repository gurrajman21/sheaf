//! OPC package helpers: read parts, resolve relationships, edit rels and
//! content types textually, and write a new zip that raw-copies every part we
//! didn't change (so untouched parts stay byte-identical, compression included).

use std::collections::{BTreeMap, HashSet};
use std::io::{Cursor, Read, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::xml::{self, attr, tokens, Kind};

pub struct Package<'a> {
    zip: ZipArchive<Cursor<&'a [u8]>>,
    pub names: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Rel {
    pub id: String,
    pub kind: String,
    pub target: String, // resolved part name, no leading slash
    pub external: bool,
}

impl<'a> Package<'a> {
    pub fn open(bytes: &'a [u8]) -> Result<Self, String> {
        let zip = ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("zip: {e}"))?;
        let names = (0..zip.len()).filter_map(|i| zip.name_for_index(i).and_then(|n| n.ok()).map(|n| n.to_string())).collect();
        Ok(Package { zip, names })
    }

    pub fn has(&self, name: &str) -> bool {
        self.names.iter().any(|n| n == name)
    }

    pub fn read(&mut self, name: &str) -> Result<String, String> {
        let mut f = self.zip.by_name(name).map_err(|e| format!("{name}: {e}"))?;
        let mut s = String::new();
        f.read_to_string(&mut s).map_err(|e| format!("{name}: {e}"))?;
        Ok(s)
    }

    pub fn rels(&mut self, part: &str) -> Result<Vec<Rel>, String> {
        let path = rels_path(part);
        if !self.has(&path) {
            return Ok(Vec::new());
        }
        let xml = self.read(&path)?;
        Ok(parse_rels(&xml, part))
    }

    /// Write a new package: `replace` swaps part contents, `drop` removes parts,
    /// `add` appends new parts. Everything else is raw-copied.
    pub fn write(
        &mut self,
        replace: &BTreeMap<String, Vec<u8>>,
        drop: &HashSet<String>,
        add: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Vec<u8>, String> {
        let mut out = ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for i in 0..self.zip.len() {
            let name = self.zip.name_for_index(i).and_then(|n| n.ok()).map(|n| n.to_string()).unwrap_or_default();
            if drop.contains(&name) {
                continue;
            }
            if let Some(data) = replace.get(&name) {
                out.start_file(name.as_str(), opts).map_err(|e| e.to_string())?;
                out.write_all(data).map_err(|e| e.to_string())?;
            } else {
                let f = self.zip.by_index_raw(i).map_err(|e| e.to_string())?;
                out.raw_copy_file(f).map_err(|e| e.to_string())?;
            }
        }
        for (name, data) in add {
            out.start_file(name.as_str(), opts).map_err(|e| e.to_string())?;
            out.write_all(data).map_err(|e| e.to_string())?;
        }
        Ok(out.finish().map_err(|e| e.to_string())?.into_inner())
    }
}

pub fn rels_path(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None if part.is_empty() => "_rels/.rels".to_string(),
        None => format!("_rels/{part}.rels"),
    }
}

pub fn resolve(base_part: &str, target: &str) -> String {
    if let Some(t) = target.strip_prefix('/') {
        return t.to_string();
    }
    let dir = base_part.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let mut parts: Vec<&str> = if dir.is_empty() { vec![] } else { dir.split('/').collect() };
    for seg in target.split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

pub fn parse_rels(xml_src: &str, part: &str) -> Vec<Rel> {
    tokens(xml_src)
        .filter(|t| t.local() == "Relationship" && (t.kind == Kind::Empty || t.kind == Kind::Start))
        .map(|t| {
            let tag = &xml_src[t.start..t.end];
            let external = attr(tag, "TargetMode") == Some("External");
            let raw_target = xml::unescape(attr(tag, "Target").unwrap_or(""));
            Rel {
                id: attr(tag, "Id").unwrap_or("").to_string(),
                kind: attr(tag, "Type").unwrap_or("").to_string(),
                target: if external { raw_target } else { resolve(part, &raw_target) },
                external,
            }
        })
        .collect()
}

/// Remove every element whose tag matches `pred` (used for rels and content-type overrides).
pub fn remove_elements(src: &str, pred: impl Fn(&str) -> bool) -> String {
    let mut out = String::with_capacity(src.len());
    let mut cursor = 0;
    for t in tokens(src) {
        if t.kind == Kind::Empty && pred(&src[t.start..t.end]) {
            out.push_str(&src[cursor..t.start]);
            cursor = t.end;
        }
    }
    out.push_str(&src[cursor..]);
    out
}

/// Insert `fragment` just before the closing tag of the root element,
/// expanding a self-closing root (`<sst .../>`) if needed.
pub fn append_to_root(src: &str, fragment: &str) -> String {
    let root = tokens(src).find(|t| matches!(t.kind, Kind::Start | Kind::Empty));
    match root {
        Some(t) if t.kind == Kind::Empty => {
            let open = src[t.start..t.end].strip_suffix("/>").unwrap_or(&src[t.start..t.end]).trim_end();
            format!("{}{open}>{fragment}</{}>{}", &src[..t.start], t.name, &src[t.end..])
        }
        _ => match src.rfind("</") {
            Some(i) => format!("{}{fragment}{}", &src[..i], &src[i..]),
            None => src.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_into_self_closing_root() {
        let src = r#"<?xml version="1.0"?>
<sst xmlns="x"/>"#;
        assert_eq!(append_to_root(src, "<si/>"), "<?xml version=\"1.0\"?>\n<sst xmlns=\"x\"><si/></sst>");
        assert_eq!(append_to_root("<a><b/></a>", "<c/>"), "<a><b/><c/></a>");
    }

    #[test]
    fn resolves_targets() {
        assert_eq!(resolve("xl/workbook.xml", "worksheets/sheet1.xml"), "xl/worksheets/sheet1.xml");
        assert_eq!(resolve("xl/worksheets/sheet1.xml", "../drawings/d1.xml"), "xl/drawings/d1.xml");
        assert_eq!(resolve("", "xl/workbook.xml"), "xl/workbook.xml");
        assert_eq!(resolve("xl/workbook.xml", "/xl/styles.xml"), "xl/styles.xml");
        assert_eq!(rels_path("xl/workbook.xml"), "xl/_rels/workbook.xml.rels");
        assert_eq!(rels_path(""), "_rels/.rels");
    }
}
