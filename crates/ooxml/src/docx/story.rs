//! Stories other than the main body: headers, footers, footnotes and endnotes.
//!
//! Headers and footers are parts whose root (`w:hdr` / `w:ftr`) holds blocks, so they
//! are edited exactly like the body, with [`Body::parse_in`]. Footnotes and endnotes
//! live together in one part; each `w:footnote`/`w:endnote` holds blocks and is
//! replaced, added or removed as a unit while the rest of the part is copied.

use std::collections::BTreeMap;

use super::body::{Block, Body};
use super::para::{Run, Seg, SegKind, RC};
use super::props::Props;
use super::{append, ensure_namespaces, Docx, DocxEdits, Para, Rev, DOC, R_NS, W_NS};
use crate::package::{self, Package};
use crate::xml::{self, attr, element_end, tokens, Kind, Tok};

const REL_HEADER: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header";
const REL_FOOTER: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer";
const REL_FOOTNOTES: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes";
const REL_ENDNOTES: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/endnotes";
const CT_HEADER: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml";
const CT_FOOTER: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml";
const CT_FOOTNOTES: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml";
const CT_ENDNOTES: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HfKind {
    Header,
    Footer,
}

impl HfKind {
    fn root(self) -> &'static str {
        match self { HfKind::Header => "hdr", HfKind::Footer => "ftr" }
    }
    fn word(self) -> &'static str {
        match self { HfKind::Header => "header", HfKind::Footer => "footer" }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteKind {
    Footnote,
    Endnote,
}

impl NoteKind {
    pub fn element(self) -> &'static str {
        match self { NoteKind::Footnote => "footnote", NoteKind::Endnote => "endnote" }
    }
    pub fn part(self) -> &'static str {
        match self { NoteKind::Footnote => "word/footnotes.xml", NoteKind::Endnote => "word/endnotes.xml" }
    }
    fn style(self) -> (&'static str, &'static str) {
        match self { NoteKind::Footnote => ("FootnoteText", "FootnoteReference"), NoteKind::Endnote => ("EndnoteText", "EndnoteReference") }
    }
}

/// A header or footer part referenced from the main document.
#[derive(Debug, Clone, PartialEq)]
pub struct HfPart {
    pub kind: HfKind,
    pub part: String,
    pub rel_id: String,
}

impl Docx {
    /// Every header and footer part the main document references.
    pub fn headers_footers(&self) -> Vec<HfPart> {
        let Ok(mut pkg) = Package::open(&self.bytes) else { return vec![] };
        pkg.rels(DOC).unwrap_or_default().into_iter().filter_map(|r| {
            let kind = if r.kind == REL_HEADER { HfKind::Header } else if r.kind == REL_FOOTER { HfKind::Footer } else { return None };
            Some(HfPart { kind, part: r.target, rel_id: r.id })
        }).collect()
    }

    /// Replace the blocks of a header/footer part (as [`Block`]s of its current body).
    pub fn edit_header_footer(&self, edits: &mut DocxEdits, part: &str, blocks: &[Block]) -> Result<(), String> {
        let src = self.current(edits, part).ok_or_else(|| format!("{part}: no such part"))?;
        let root = tokens(&src).find(|t| matches!(t.kind, Kind::Start | Kind::Empty)).map(|t| t.local().to_string()).unwrap_or_default();
        let body = Body::parse_in(&src, &root)?;
        let new = ensure_namespaces(&body.rebuild(Some(blocks), None));
        edits.parts.insert(part.to_string(), new.into_bytes());
        Ok(())
    }

    /// Create a new header/footer part holding `blocks_xml`; returns (part name, relationship id).
    /// Attach it to a section with [`set_hf_reference`].
    pub fn add_header_footer(&self, edits: &mut DocxEdits, kind: HfKind, blocks_xml: &str) -> (String, String) {
        let part = (1..).map(|n| format!("word/{}{}.xml", kind.word(), n))
            .find(|p| !self.has_part(p) && !edits.parts.contains_key(p)).unwrap();
        let xml = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>{}<w:{root} xmlns:w="{W_NS}" xmlns:r="{R_NS}">{body}</w:{root}>"#,
                          "\r\n", root = kind.root(), body = if blocks_xml.is_empty() { "<w:p/>" } else { blocks_xml });
        let id = self.next_rel_id(DOC, edits);
        edits.rels.push(super::NewRel { owner: DOC.into(), id: id.clone(), kind: (if kind == HfKind::Header { REL_HEADER } else { REL_FOOTER }).into(), target: part["word/".len()..].to_string(), external: false });
        edits.content_types.insert(part.clone(), (if kind == HfKind::Header { CT_HEADER } else { CT_FOOTER }).into());
        edits.parts.insert(part.clone(), xml.into_bytes());
        append::ensure_builtin_style(self, edits, if kind == HfKind::Header { "Header" } else { "Footer" });
        (part, id)
    }

    /// All notes of a kind (in the current version of the part).
    pub fn notes(&self, edits: &DocxEdits, kind: NoteKind) -> Vec<NoteInfo> {
        let Some(src) = self.current(edits, kind.part()) else { return vec![] };
        let Ok(n) = Notes::parse(&src, kind) else { return vec![] };
        n.notes.iter().map(|x| NoteInfo { id: x.id, special: x.special.clone(), xml: src[x.start..x.end].to_string() }).collect()
    }

    /// Add a note with one paragraph of `text`; returns its id. Put the reference in the
    /// text with [`Para::insert_note_reference`].
    pub fn queue_note(&self, edits: &mut DocxEdits, kind: NoteKind, text: &str) -> Result<i64, String> {
        let el = kind.element();
        let initial = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>{}<w:{el}s xmlns:w="{W_NS}" xmlns:r="{R_NS}"><w:{el} w:type="separator" w:id="-1"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:separator/></w:r></w:p></w:{el}><w:{el} w:type="continuationSeparator" w:id="0"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:continuationSeparator/></w:r></w:p></w:{el}></w:{el}s>"#,
            "\r\n");
        let src = self.ensure_part(edits, kind.part(), if kind == NoteKind::Footnote { CT_FOOTNOTES } else { CT_ENDNOTES },
                                   if kind == NoteKind::Footnote { REL_FOOTNOTES } else { REL_ENDNOTES }, &initial);
        let notes = Notes::parse(&src, kind)?;
        let id = notes.notes.iter().map(|n| n.id).max().unwrap_or(0).max(0) + 1;
        let (text_style, ref_style) = kind.style();
        let mark = match kind { NoteKind::Footnote => "footnoteRef", NoteKind::Endnote => "endnoteRef" };
        let note = format!(
            r#"<w:{el} w:id="{id}"><w:p><w:pPr><w:pStyle w:val="{text_style}"/></w:pPr><w:r><w:rPr><w:rStyle w:val="{ref_style}"/></w:rPr><w:{mark}/></w:r>{}</w:p></w:{el}>"#,
            super::para::text_run(&format!(" {text}"), None));
        edits.parts.insert(kind.part().into(), package::append_to_root(&src, &note).into_bytes());
        append::ensure_builtin_style(self, edits, text_style);
        append::ensure_builtin_style(self, edits, ref_style);
        Ok(id)
    }

    /// Replace the blocks of note `id` (keeping its `w:footnote`/`w:endnote` tag).
    pub fn set_note_blocks(&self, edits: &mut DocxEdits, kind: NoteKind, id: i64, blocks_xml: &str) -> Result<(), String> {
        let src = self.current(edits, kind.part()).ok_or("no notes part")?;
        let notes = Notes::parse(&src, kind)?;
        let n = notes.notes.iter().find(|n| n.id == id).ok_or_else(|| format!("no {} {id}", kind.element()))?;
        let new = format!("{}{}{}{}", &src[..n.start], n.open, blocks_xml, &src[n.close_start..]);
        edits.parts.insert(kind.part().into(), new.into_bytes());
        Ok(())
    }

    /// Remove note `id` from the notes part (remove its reference from the text separately).
    pub fn remove_note(&self, edits: &mut DocxEdits, kind: NoteKind, id: i64) -> Result<(), String> {
        let src = self.current(edits, kind.part()).ok_or("no notes part")?;
        let notes = Notes::parse(&src, kind)?;
        let n = notes.notes.iter().find(|n| n.id == id).ok_or_else(|| format!("no {} {id}", kind.element()))?;
        let new = format!("{}{}", &src[..n.start], &src[n.end..]);
        edits.parts.insert(kind.part().into(), new.into_bytes());
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NoteInfo {
    pub id: i64,
    /// `separator`, `continuationSeparator`, ... for Word's built-in notes.
    pub special: Option<String>,
    pub xml: String,
}

#[derive(Debug, Clone)]
pub struct Note {
    pub id: i64,
    pub special: Option<String>,
    pub open: String,
    pub start: usize,
    pub end: usize,
    /// Offset of the closing tag.
    pub close_start: usize,
}

/// A footnotes/endnotes part split into its notes.
pub struct Notes {
    pub notes: Vec<Note>,
}

impl Notes {
    pub fn parse(src: &str, kind: NoteKind) -> Result<Notes, String> {
        let toks: Vec<Tok> = tokens(src).collect();
        let mut notes = Vec::new();
        let mut i = 0;
        while i < toks.len() {
            let t = toks[i];
            if t.local() == kind.element() && matches!(t.kind, Kind::Start | Kind::Empty) {
                let tag = &src[t.start..t.end];
                let end = element_end(&toks, i);
                let id = attr(tag, "w:id").and_then(|v| v.parse().ok()).unwrap_or(i64::MIN);
                let (open, close_start) = if t.kind == Kind::Empty {
                    (tag.trim_end_matches("/>").trim_end().to_string() + ">", end)
                } else {
                    (tag.to_string(), src[..end].rfind("</").unwrap_or(end))
                };
                notes.push(Note { id, special: attr(tag, "w:type").map(str::to_string), open, start: t.start, end, close_start });
                while i < toks.len() && toks[i].start < end {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        Ok(Notes { notes })
    }
}

/// Section properties with the `kind` reference of `hf_type` (`default`, `first`, `even`)
/// pointing at `rel_id`. `first` also turns on `w:titlePg` (a distinct first page).
pub fn set_hf_reference(sect_pr: &str, kind: HfKind, hf_type: &str, rel_id: &str) -> String {
    let element = match kind { HfKind::Header => "headerReference", HfKind::Footer => "footerReference" };
    let mut p = Props::parse(sect_pr);
    if p.tag.is_empty() {
        p = Props::new("w:sectPr");
    }
    p.children.retain(|(n, x)| !(n == element && attr(x, "w:type").unwrap_or("default") == hf_type));
    let new = format!(r#"<w:{element} w:type="{}" r:id="{}"/>"#, xml::escape(hf_type), xml::escape(rel_id));
    // References come first in sectPr, headers before footers.
    let at = p.children.iter().position(|(n, _)| {
        !(n == "headerReference" || (n == "footerReference" && kind == HfKind::Footer))
    }).unwrap_or(p.children.len());
    p.children.insert(at, (element.to_string(), new));
    if hf_type == "first" && p.get("titlePg").is_none() {
        // titlePg sits after formProt/vAlign/noEndnote and before textDirection/bidi/rtlGutter/docGrid.
        let after = ["textDirection", "bidi", "rtlGutter", "docGrid", "printerSettings", "sectPrChange"];
        let at = p.children.iter().position(|(n, _)| after.contains(&n.as_str())).unwrap_or(p.children.len());
        p.children.insert(at, ("titlePg".into(), "<w:titlePg/>".into()));
    }
    p.to_xml()
}

impl Para {
    /// Insert a footnote/endnote reference mark at `off`.
    pub fn insert_note_reference(&mut self, off: usize, kind: NoteKind, id: i64, rev: Option<&Rev>) {
        let el = match kind { NoteKind::Footnote => "footnoteReference", NoteKind::Endnote => "endnoteReference" };
        let rpr = Props::parse(&format!(r#"<w:rPr><w:rStyle w:val="{}"/></w:rPr>"#, kind.style().1));
        self.insert_object(off, &format!(r#"<w:{el} w:id="{id}"/>"#), Some(rpr), rev);
    }

    /// Insert a run holding `content` (a drawing, field char, note reference, ...) at `off`.
    pub fn insert_object(&mut self, off: usize, content: &str, rpr: Option<Props>, rev: Option<&Rev>) {
        let off = off.min(self.len());
        let at = self.split_at(off);
        let mut wrap = Vec::new();
        if let Some(r) = rev {
            wrap.push(self.new_group("ins", r));
        }
        let run = Run { open: "<w:r>".into(), rpr, content: vec![RC::Raw(content.to_string())] };
        self.segs.insert(at, Seg { kind: SegKind::Run(run), wrap });
    }
}

/// Group blocks into `Block::Orig` where the XML is unchanged from `body`.
pub fn diff_blocks(body: &Body, new_xml: &[String]) -> Vec<Block> {
    let orig: BTreeMap<&str, usize> = (0..body.elements.len()).map(|i| (body.xml(i), i)).collect();
    new_xml.iter().map(|x| match orig.get(x.as_str()) {
        Some(i) => Block::Orig(*i),
        None => Block::Xml(x.clone()),
    }).collect()
}
