//! Patch-in-place writer for .docx (WordprocessingML).
//!
//! Same principle as the spreadsheet writer: the original package is the base.
//! `word/document.xml` is split into its top-level body elements (paragraphs,
//! tables, content controls, ...) as byte ranges; the caller says which of them
//! are kept, replaced or inserted, and only replaced/inserted elements are
//! written fresh. Every other byte of the part, and every other part of the
//! package, is copied unchanged (zip entries raw-copied, compression included).
//!
//! Editing inside an element goes through [`para::Para`] (text, formatting,
//! tracked changes, comments) and [`table`] (cell contents), which keep every
//! child element they don't understand verbatim.

pub mod append;
pub mod comments;
pub mod body;
pub mod revisions;
pub mod story;
pub mod para;
pub mod props;
pub mod table;

use std::collections::{BTreeMap, HashSet};

use crate::package::{self, Package};
use crate::xml::{self, attr, tokens, Kind};

pub use body::{Block, Body};
pub use para::{Para, Rev};

pub const DOC: &str = "word/document.xml";
pub const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
pub const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub const REL_COMMENTS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments";
pub const CT_COMMENTS: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml";

/// Everything a save changes, relative to the original package.
#[derive(Default, Debug, Clone)]
pub struct DocxEdits {
    /// New body: original top-level elements by index, or fresh XML. `None` keeps the body.
    pub body: Option<Vec<Block>>,
    /// Final section properties (`w:body/w:sectPr`) replacement, if any.
    pub sect_pr: Option<String>,
    /// Whole-part replacements (existing parts) and additions (new parts), by part name.
    pub parts: BTreeMap<String, Vec<u8>>,
    /// Content types for added parts (part name -> content type).
    pub content_types: BTreeMap<String, String>,
    /// Default content types by file extension (e.g. `png` -> `image/png`), added if missing.
    pub content_type_defaults: BTreeMap<String, String>,
    /// Parts to remove.
    pub drop: HashSet<String>,
    /// New relationships: (owner part, id, type, target, external).
    pub rels: Vec<NewRel>,
}

#[derive(Debug, Clone)]
pub struct NewRel {
    pub owner: String,
    pub id: String,
    pub kind: String,
    pub target: String,
    pub external: bool,
}

/// An opened .docx: the original bytes plus the parsed body of `document.xml`.
pub struct Docx {
    pub bytes: Vec<u8>,
    pub document: String,
    pub names: Vec<String>,
}

impl Docx {
    pub fn open(bytes: &[u8]) -> Result<Self, String> {
        let mut pkg = Package::open(bytes)?;
        let document = pkg.read(DOC)?;
        Ok(Docx { bytes: bytes.to_vec(), document, names: pkg.names.clone() })
    }

    pub fn body(&self) -> Result<Body<'_>, String> {
        Body::parse(&self.document)
    }

    pub fn read_part(&self, name: &str) -> Result<String, String> {
        Package::open(&self.bytes)?.read(name)
    }

    pub fn has_part(&self, name: &str) -> bool {
        self.names.iter().any(|n| n == name)
    }

    /// Highest `w:id` used by revisions, comments and bookmarks across the main stories,
    /// so new annotations get unique ids.
    pub fn max_annotation_id(&self) -> u32 {
        let mut max = 0u32;
        let mut scan = |s: &str| {
            for t in tokens(s) {
                if matches!(t.kind, Kind::Start | Kind::Empty) {
                    if let Some(v) = attr(&s[t.start..t.end], "w:id").and_then(|v| v.parse::<i64>().ok()) {
                        max = max.max(v.clamp(0, u32::MAX as i64) as u32);
                    }
                }
            }
        };
        scan(&self.document);
        for n in &self.names {
            if (n.starts_with("word/") && n.ends_with(".xml")) && n.as_str() != DOC
                && ["comments", "footnotes", "endnotes", "header", "footer"].iter().any(|k| n[5..].starts_with(k))
            {
                if let Ok(s) = self.read_part(n) {
                    scan(&s);
                }
            }
        }
        max
    }

    /// Relationship ids already used by `owner` (plus ones queued in `edits`).
    pub fn next_rel_id(&self, owner: &str, edits: &DocxEdits) -> String {
        let mut used: HashSet<String> = HashSet::new();
        if let Ok(mut pkg) = Package::open(&self.bytes) {
            if let Ok(rels) = pkg.rels(owner) {
                used.extend(rels.into_iter().map(|r| r.id));
            }
        }
        used.extend(edits.rels.iter().filter(|r| r.owner == owner).map(|r| r.id.clone()));
        (1..).map(|n| format!("rIdS{n}")).find(|id| !used.contains(id)).unwrap()
    }

    /// Write the edited package.
    pub fn save(&self, edits: &DocxEdits) -> Result<Vec<u8>, String> {
        let mut pkg = Package::open(&self.bytes)?;
        let mut replace: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut add: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for (name, data) in &edits.parts {
            if pkg.has(name) {
                replace.insert(name.clone(), data.clone());
            } else {
                add.insert(name.clone(), data.clone());
            }
        }
        if edits.body.is_some() || edits.sect_pr.is_some() {
            let body = self.body()?;
            let doc = body.rebuild(edits.body.as_deref(), edits.sect_pr.as_deref());
            replace.insert(DOC.to_string(), ensure_namespaces(&doc).into_bytes());
        }
        // Relationships, grouped by owner part.
        let mut by_owner: BTreeMap<&str, Vec<&NewRel>> = BTreeMap::new();
        for r in &edits.rels {
            by_owner.entry(r.owner.as_str()).or_default().push(r);
        }
        for (owner, rels) in by_owner {
            let path = package::rels_path(owner);
            let frag: String = rels.iter().map(|r| {
                format!(r#"<Relationship Id="{}" Type="{}" Target="{}"{}/>"#, xml::escape(&r.id), xml::escape(&r.kind),
                        xml::escape(&r.target), if r.external { r#" TargetMode="External""# } else { "" })
            }).collect();
            let base = match replace.get(&path).or_else(|| add.get(&path)) {
                Some(b) => String::from_utf8_lossy(b).into_owned(),
                None if pkg.has(&path) => pkg.read(&path)?,
                None => format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>{}<Relationships xmlns="{}"></Relationships>"#, "\r\n", "http://schemas.openxmlformats.org/package/2006/relationships"),
            };
            let new = package::append_to_root(&base, &frag).into_bytes();
            if pkg.has(&path) { replace.insert(path, new); } else { add.insert(path, new); }
        }
        // Content types for added parts.
        let overrides: Vec<(&String, &String)> = edits.content_types.iter()
            .filter(|(p, _)| add.contains_key(p.as_str()) || !pkg.has(p)).collect();
        if !overrides.is_empty() || !edits.content_type_defaults.is_empty() {
            let ct = pkg.read("[Content_Types].xml")?;
            let mut frag: String = edits.content_type_defaults.iter()
                .filter(|(e, _)| !ct.to_ascii_lowercase().contains(&format!("extension=\"{}\"", e.to_ascii_lowercase())))
                .map(|(e, c)| format!(r#"<Default Extension="{}" ContentType="{}"/>"#, xml::escape(e), xml::escape(c)))
                .collect();
            frag.extend(overrides.iter()
                .filter(|(p, _)| !ct.contains(&format!("PartName=\"/{p}\"")))
                .map(|(p, c)| format!(r#"<Override PartName="/{}" ContentType="{}"/>"#, xml::escape(p), xml::escape(c))));
            if !frag.is_empty() {
                // Defaults must precede overrides; putting everything first keeps that true.
                let new = match ct.find("<Override") {
                    Some(i) if frag.starts_with("<Default") => format!("{}{}{}", &ct[..i], frag, &ct[i..]),
                    _ => package::append_to_root(&ct, &frag),
                };
                replace.insert("[Content_Types].xml".into(), new.into_bytes());
            }
        }
        let drop: HashSet<String> = edits.drop.clone();
        pkg.write(&replace, &drop, &add)
    }

    /// A part as this save will write it: the queued replacement if any, else the original.
    pub fn current(&self, edits: &DocxEdits, name: &str) -> Option<String> {
        match edits.parts.get(name) {
            Some(b) => Some(String::from_utf8_lossy(b).into_owned()),
            None if self.has_part(name) && !edits.drop.contains(name) => self.read_part(name).ok(),
            None => None,
        }
    }

    /// The current content of `name`, creating it (with a relationship from the main
    /// document and a content type override) from `initial` if it doesn't exist yet.
    pub fn ensure_part(&self, edits: &mut DocxEdits, name: &str, content_type: &str, rel_type: &str, initial: &str) -> String {
        if let Some(s) = self.current(edits, name) {
            return s;
        }
        let id = self.next_rel_id(DOC, edits);
        let target = name.strip_prefix("word/").unwrap_or(name).to_string();
        edits.rels.push(NewRel { owner: DOC.into(), id, kind: rel_type.into(), target, external: false });
        edits.content_types.insert(name.into(), content_type.into());
        edits.parts.insert(name.into(), initial.as_bytes().to_vec());
        initial.to_string()
    }

    /// Relationship id from the main document to `part`, if any (existing or queued).
    pub fn rel_id_for(&self, edits: &DocxEdits, part: &str) -> Option<String> {
        let target = part.strip_prefix("word/").unwrap_or(part);
        if let Some(r) = edits.rels.iter().find(|r| r.owner == DOC && r.target == target) {
            return Some(r.id.clone());
        }
        let mut pkg = Package::open(&self.bytes).ok()?;
        pkg.rels(DOC).ok()?.into_iter().find(|r| r.target == part).map(|r| r.id)
    }

    /// Queue a comment: adds it to `word/comments.xml` (creating the part if needed).
    /// The caller anchors it in the text with [`Para::add_comment`]. Returns nothing;
    /// `id` must come from the same counter as revisions (see [`Docx::max_annotation_id`]).
    pub fn queue_comment(&self, edits: &mut DocxEdits, id: u32, author: &str, initials: &str, date: &str, text: &str) -> Result<(), String> {
        const PART: &str = "word/comments.xml";
        let comment = format!(
            r#"<w:comment w:id="{id}" w:author="{}" w:date="{}" w:initials="{}"><w:p><w:pPr><w:pStyle w:val="CommentText"/></w:pPr><w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:annotationRef/></w:r>{}</w:p></w:comment>"#,
            xml::escape(author), xml::escape(date), xml::escape(initials), para::text_run(text, None));
        let initial = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>{}<w:comments xmlns:w="{W_NS}" xmlns:r="{R_NS}"></w:comments>"#, "\r\n");
        let base = self.ensure_part(edits, PART, CT_COMMENTS, REL_COMMENTS, &initial);
        edits.parts.insert(PART.into(), package::append_to_root(&base, &comment).into_bytes());
        for id in ["CommentText", "CommentReference"] {
            append::ensure_builtin_style(self, edits, id);
        }
        Ok(())
    }
}

/// New elements are written with the `w:` and `r:` prefixes; make sure the root declares both.
pub(crate) fn ensure_namespaces(doc: &str) -> String {
    let Some(root) = tokens(doc).find(|t| matches!(t.kind, Kind::Start | Kind::Empty)) else { return doc.to_string() };
    let tag = &doc[root.start..root.end];
    let mut new_tag = tag.to_string();
    if attr(tag, "xmlns:w").is_none() {
        new_tag = xml::set_attr(&new_tag, "xmlns:w", Some(W_NS));
    }
    if attr(tag, "xmlns:r").is_none() {
        new_tag = xml::set_attr(&new_tag, "xmlns:r", Some(REL_NS));
    }
    if new_tag == tag {
        return doc.to_string();
    }
    format!("{}{}{}", &doc[..root.start], new_tag, &doc[root.end..])
}

#[cfg(test)]
mod tests;
