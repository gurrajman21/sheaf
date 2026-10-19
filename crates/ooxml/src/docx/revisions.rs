//! Tracked changes: list them, accept or reject them (all, or filtered by author/id).
//!
//! Handled: inserted/deleted/moved runs (`w:ins`, `w:del`, `w:moveFrom`, `w:moveTo` and
//! move range markers), formatting changes (`w:rPrChange`, `w:pPrChange`, `w:sectPrChange`,
//! table/row/cell property changes), inserted/deleted paragraph marks (paragraphs merge
//! the way Word merges them) and inserted/deleted table rows. Every story is covered:
//! body, headers, footers, footnotes, endnotes and comments.

use std::cell::Cell;

use super::body::{Block, Body};
use super::para::{Para, SegKind};
use super::props::Props;
use super::story::{diff_blocks, NoteKind, Notes};
use super::table::{Item, Table};
use super::{ensure_namespaces, Docx, DocxEdits, DOC};
use crate::xml::{self, attr, tokens, Kind};

/// What a tracked change is, for listing and filtering.
#[derive(Debug, Clone, PartialEq)]
pub struct Revision {
    pub id: String,
    /// `ins`, `del`, `moveFrom`, `moveTo`, `rPrChange`, `pPrChange`, `paragraphMark`, `rowIns`, ...
    pub kind: String,
    pub author: String,
    pub date: String,
    /// Part the change is in (`word/document.xml`, `word/header1.xml`, ...).
    pub part: String,
    /// Inserted or deleted text (empty for formatting changes).
    pub text: String,
}

const CHANGE_TAGS: &[&str] = &["ins", "del", "moveFrom", "moveTo", "rPrChange", "pPrChange", "sectPrChange",
                               "tblPrChange", "trPrChange", "tcPrChange", "tblGridChange", "numberingChange", "cellIns", "cellDel", "cellMerge"];

/// Which changes to resolve. `None` fields match everything.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub author: Option<String>,
    pub ids: Option<Vec<String>>,
}

impl Filter {
    fn matches(&self, tag: &str) -> bool {
        let a = attr(tag, "w:author").map(xml::unescape);
        let id = attr(tag, "w:id").map(str::to_string);
        self.author.as_ref().map_or(true, |x| a.as_deref() == Some(x.as_str()))
            && self.ids.as_ref().map_or(true, |ids| id.as_ref().is_some_and(|i| ids.contains(i)))
    }
}

struct Ctx<'a> {
    accept: bool,
    filter: &'a Filter,
    count: Cell<usize>,
}

impl Ctx<'_> {
    fn hit(&self, tag: &str) -> bool {
        let m = self.filter.matches(tag);
        if m {
            self.count.set(self.count.get() + 1);
        }
        m
    }
}

/// First start/empty tag of an element's XML.
fn open_tag(x: &str) -> &str {
    tokens(x).find(|t| matches!(t.kind, Kind::Start | Kind::Empty)).map(|t| &x[t.start..t.end]).unwrap_or("")
}

/// Resolve a `*PrChange` child of a property element: accept drops it; reject restores
/// the old properties (keeping the children listed in `keep`, which the change doesn't record).
fn resolve_pr_change(props: &mut Props, change: &str, keep: &[&str], ctx: &Ctx) -> bool {
    let Some(ch) = props.get(change).map(str::to_string) else { return false };
    if !ctx.hit(open_tag(&ch)) {
        return false;
    }
    props.set(change, None);
    if !ctx.accept {
        // The old properties are the change's only child element.
        let inner_start = ch.find('>').map(|i| i + 1).unwrap_or(ch.len());
        let inner_end = ch.rfind("</").unwrap_or(ch.len()).max(inner_start);
        let old = Props::parse(&ch[inner_start..inner_end]);
        let kept: Vec<(String, String)> = props.children.iter().filter(|(n, _)| keep.contains(&n.as_str())).cloned().collect();
        // Revision markers inside the snapshot are separate changes, resolved on their own.
        props.children = old.children.into_iter().filter(|(n, _)| !matches!(n.as_str(), "ins" | "del" | "moveFrom" | "moveTo")).collect();
        for (n, x) in kept {
            props.set(&n, Some(x));
        }
    }
    true
}


/// Replace the inner content of the first `container` element in `x` with `f(inner)`.
fn map_inner(x: &str, container: &str, f: impl FnOnce(&str) -> String) -> String {
    let toks: Vec<_> = tokens(x).collect();
    let Some(i) = toks.iter().position(|t| t.local() == container && t.kind == Kind::Start) else { return x.to_string() };
    let end = xml::element_end(&toks, i);
    let close = x[..end].rfind("</").unwrap_or(end);
    let inner_start = toks[i].end;
    if close < inner_start {
        return x.to_string();
    }
    format!("{}{}{}", &x[..inner_start], f(&x[inner_start..close]), &x[close..])
}

/// Resolve changes nested inside raw XML: text boxes (`w:txbxContent`, possibly several,
/// e.g. DrawingML + VML fallback), inline content controls and list numbering changes.
fn resolve_nested(x: &str, ctx: &Ctx) -> String {
    if !CHANGE_TAGS.iter().any(|t| x.contains(&format!(":{t} "))) {
        return x.to_string();
    }
    let toks: Vec<_> = tokens(x).collect();
    let mut out = String::with_capacity(x.len());
    let mut cursor = 0;
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        let start_like = matches!(t.kind, Kind::Start | Kind::Empty);
        if start_like && t.local() == "txbxContent" && t.kind == Kind::Start {
            let end = xml::element_end(&toks, i);
            let el = &x[t.start..end];
            out.push_str(&x[cursor..t.start]);
            out.push_str(&match Body::parse_in(el, "txbxContent") {
                Ok(body) => {
                    let resolved = resolve_blocks(&container_blocks(&body), ctx);
                    body.rebuild(Some(&resolved.into_iter().map(Block::Xml).collect::<Vec<_>>()), None)
                }
                Err(_) => el.to_string(),
            });
            cursor = end;
            while i < toks.len() && toks[i].start < end {
                i += 1;
            }
            continue;
        }
        if start_like && t.local() == "numberingChange" && ctx.hit(&x[t.start..t.end]) {
            let end = xml::element_end(&toks, i);
            out.push_str(&x[cursor..t.start]);
            cursor = end;
            while i < toks.len() && toks[i].start < end {
                i += 1;
            }
            continue;
        }
        i += 1;
    }
    out.push_str(&x[cursor..]);
    out
}

/// Inline content control (`w:sdt` inside a paragraph): its content is runs.
fn resolve_inline_sdt(x: &str, ctx: &Ctx) -> String {
    map_inner(x, "sdtContent", |inner| {
        let fake = format!("<w:p>{inner}</w:p>");
        match Para::parse(&fake) {
            Ok(mut p) => {
                resolve_para(&mut p, ctx);
                let s = p.to_xml();
                s["<w:p>".len()..s.len() - "</w:p>".len()].to_string()
            }
            Err(_) => inner.to_string(),
        }
    })
}

enum Mark {
    Keep,
    /// The paragraph mark goes away: join with the following paragraph.
    Merge,
}

fn resolve_para(p: &mut Para, ctx: &Ctx) -> Mark {
    let mut mark = Mark::Keep;
    // Paragraph properties and the paragraph mark.
    if let Some(ppr) = p.ppr.as_mut() {
        resolve_pr_change(ppr, "pPrChange", &["rPr", "sectPr", "pPrChange"], ctx);
        if let Some(rpr_raw) = ppr.get("rPr").map(str::to_string) {
            let mut rpr = Props::parse(&rpr_raw);
            for (m, merge_if_accept) in [("ins", false), ("del", true), ("moveTo", false), ("moveFrom", true)] {
                if let Some(x) = rpr.get(m).map(str::to_string) {
                    if ctx.hit(open_tag(&x)) {
                        rpr.set(m, None);
                        if merge_if_accept == ctx.accept {
                            mark = Mark::Merge;
                        }
                    }
                }
            }
            resolve_pr_change(&mut rpr, "rPrChange", &[], ctx);
            ppr.set("rPr", if rpr.is_empty() { None } else { Some(rpr.to_xml()) });
        }
        if let Some(sect) = ppr.get("sectPr").map(str::to_string) {
            let mut sp = Props::parse(&sect);
            if resolve_pr_change(&mut sp, "sectPrChange", &["headerReference", "footerReference"], ctx) {
                ppr.set("sectPr", Some(sp.to_xml()));
            }
        }
        if ppr.is_empty() {
            p.ppr = None;
        }
    }
    // Run-level wrappers.
    let mut drop_group = vec![false; p.groups.len()];
    let mut unwrap_group = vec![false; p.groups.len()];
    for (gi, g) in p.groups.iter().enumerate() {
        let inserted = matches!(g.kind.as_str(), "ins" | "moveTo");
        let deleted = matches!(g.kind.as_str(), "del" | "moveFrom");
        if (inserted || deleted) && ctx.hit(&g.open) {
            if inserted == ctx.accept {
                unwrap_group[gi] = true;
            } else {
                drop_group[gi] = true;
            }
        }
    }
    let mut segs = Vec::with_capacity(p.segs.len());
    for mut s in std::mem::take(&mut p.segs) {
        if s.wrap.iter().any(|g| drop_group[*g]) {
            // Runs inside a rejected insertion / accepted deletion disappear; markers
            // (bookmarks, comment ranges) inside them stay so their pairs stay balanced.
            if matches!(s.kind, SegKind::Run(_)) {
                continue;
            }
        }
        let restoring = s.wrap.iter().any(|g| unwrap_group[*g] && matches!(p.groups[*g].kind.as_str(), "del" | "moveFrom"));
        s.wrap.retain(|g| !unwrap_group[*g] && !drop_group[*g]);
        match &mut s.kind {
            SegKind::Raw(x) => {
                let name = tokens(x).find(|t| matches!(t.kind, Kind::Start | Kind::Empty)).map(|t| t.local().to_string()).unwrap_or_default();
                if matches!(name.as_str(), "moveFromRangeStart" | "moveFromRangeEnd" | "moveToRangeStart" | "moveToRangeEnd") && ctx.filter.matches(open_tag(x)) {
                    continue;
                }
                *x = if name == "sdt" { resolve_inline_sdt(x, ctx) } else { resolve_nested(x, ctx) };
            }
            SegKind::Run(run) => {
                for c in run.content.iter_mut() {
                    if let super::para::RC::Raw(x) = c {
                        if restoring {
                            *x = x.replace("w:delInstrText", "w:instrText");
                        }
                        *x = resolve_nested(x, ctx);
                    }
                }
                if let Some(rpr) = run.rpr.as_mut() {
                    resolve_pr_change(rpr, "rPrChange", &[], ctx);
                    if rpr.is_empty() {
                        run.rpr = None;
                    }
                }
            }
        }
        segs.push(s);
    }
    p.segs = segs;
    mark
}

/// `a` loses its paragraph mark: its content joins `b`, which keeps its own properties.
fn join(a: Para, mut b: Para) -> Para {
    let n = a.groups.len();
    let mut groups = a.groups;
    groups.append(&mut b.groups);
    let mut segs = a.segs;
    for mut s in b.segs {
        for g in s.wrap.iter_mut() {
            *g += n;
        }
        segs.push(s);
    }
    Para { open: a.open, ppr: b.ppr, segs, groups }
}

fn resolve_props_xml(raw: &str, change: &str, keep: &[&str], ctx: &Ctx) -> String {
    let mut p = Props::parse(raw);
    if resolve_pr_change(&mut p, change, keep, ctx) { p.to_xml() } else { raw.to_string() }
}

fn resolve_table(xml_src: &str, ctx: &Ctx) -> Option<String> {
    let Ok(mut t) = Table::parse(xml_src) else { return Some(xml_src.to_string()) };
    let mut items = Vec::new();
    for item in std::mem::take(&mut t.items) {
        match item {
            Item::Raw(x) => {
                let name = tokens(&x).find(|t| matches!(t.kind, Kind::Start | Kind::Empty)).map(|t| t.local().to_string()).unwrap_or_default();
                items.push(Item::Raw(match name.as_str() {
                    "tblPr" => resolve_props_xml(&x, "tblPrChange", &[], ctx),
                    "tblGrid" => resolve_props_xml(&x, "tblGridChange", &[], ctx),
                    // Rows wrapped in a content control / custom XML.
                    "sdt" | "customXml" => map_inner(&x, if name == "sdt" { "sdtContent" } else { "customXml" }, |inner| {
                        let fake = format!("<w:tbl>{inner}</w:tbl>");
                        match resolve_table(&fake, ctx) {
                            Some(t) => t["<w:tbl>".len()..t.len() - "</w:tbl>".len()].to_string(),
                            None => String::new(),
                        }
                    }),
                    _ => x,
                }));
            }
            Item::Node(mut row) => {
                let mut drop_row = false;
                let mut cells = Vec::new();
                for ci in std::mem::take(&mut row.items) {
                    match ci {
                        Item::Raw(x) if open_tag(&x).contains("trPr") => {
                            let mut p = Props::parse(&x);
                            for (m, drop_if_accept) in [("ins", false), ("del", true)] {
                                if let Some(tag) = p.get(m).map(str::to_string) {
                                    if ctx.hit(open_tag(&tag)) {
                                        p.set(m, None);
                                        if drop_if_accept == ctx.accept {
                                            drop_row = true;
                                        }
                                    }
                                }
                            }
                            resolve_pr_change(&mut p, "trPrChange", &[], ctx);
                            if !p.is_empty() {
                                cells.push(Item::Raw(p.to_xml()));
                            }
                        }
                        Item::Node(mut cell) => {
                            if let Some(tc) = &cell.tc_pr {
                                let mut p = Props::parse(tc);
                                resolve_pr_change(&mut p, "tcPrChange", &[], ctx);
                                for m in ["cellIns", "cellDel", "cellMerge"] {
                                    if p.get(m).is_some_and(|x| ctx.hit(open_tag(x))) {
                                        p.set(m, None);
                                    }
                                }
                                cell.tc_pr = if p.is_empty() { None } else { Some(p.to_xml()) };
                            }
                            cell.blocks = resolve_blocks(&cell.blocks, ctx);
                            cells.push(Item::Node(cell));
                        }
                        Item::Raw(x) if matches!(open_tag(&x).trim_start_matches('<').split([' ', '>', '/']).next(), Some("w:sdt" | "w:customXml")) => {
                            // Cells wrapped in a content control / custom XML.
                            let container = if open_tag(&x).starts_with("<w:sdt") { "sdtContent" } else { "customXml" };
                            cells.push(Item::Raw(map_inner(&x, container, |inner| {
                                let fake = format!("<w:tbl><w:tr>{inner}</w:tr></w:tbl>");
                                match resolve_table(&fake, ctx) {
                                    Some(t) => t["<w:tbl><w:tr>".len()..t.len() - "</w:tr></w:tbl>".len()].to_string(),
                                    None => String::new(),
                                }
                            })));
                        }
                        other => cells.push(other),
                    }
                }
                row.items = cells;
                if !drop_row {
                    items.push(Item::Node(row));
                }
            }
        }
    }
    t.items = items;
    if t.rows().count() == 0 { None } else { Some(t.to_xml()) }
}

/// Resolve a list of block-level elements (body, cell, header, note, comment content).
fn resolve_blocks(blocks: &[String], ctx: &Ctx) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(blocks.len());
    let mut carry: Option<Para> = None;
    let flush = |carry: &mut Option<Para>, out: &mut Vec<String>| {
        if let Some(c) = carry.take() {
            out.push(c.to_xml());
        }
    };
    for b in blocks {
        let name = tokens(b).find(|t| matches!(t.kind, Kind::Start | Kind::Empty)).map(|t| t.local().to_string()).unwrap_or_default();
        match name.as_str() {
            "p" => {
                let Ok(mut p) = Para::parse(b) else {
                    flush(&mut carry, &mut out);
                    out.push(b.clone());
                    continue;
                };
                let mark = resolve_para(&mut p, ctx);
                if let Some(prev) = carry.take() {
                    p = join(prev, p);
                }
                match mark {
                    Mark::Merge => carry = Some(p),
                    Mark::Keep => out.push(p.to_xml()),
                }
            }
            "tbl" => {
                flush(&mut carry, &mut out);
                if let Some(x) = resolve_table(b, ctx) {
                    out.push(x);
                }
            }
            "sdt" | "customXml" => {
                flush(&mut carry, &mut out);
                let container = if name == "sdt" { "sdtContent" } else { "customXml" };
                match Body::parse_in(b, container) {
                    Ok(body) => {
                        let inner: Vec<String> = (0..body.elements.len()).map(|i| body.xml(i).to_string()).collect();
                        let resolved = resolve_blocks(&inner, ctx);
                        let blocks: Vec<Block> = resolved.into_iter().map(Block::Xml).collect();
                        out.push(body.rebuild(Some(&blocks), None));
                    }
                    Err(_) => out.push(b.clone()),
                }
            }
            "moveFromRangeStart" | "moveFromRangeEnd" | "moveToRangeStart" | "moveToRangeEnd" if ctx.filter.matches(open_tag(b)) => {}
            _ => {
                flush(&mut carry, &mut out);
                out.push(b.clone());
            }
        }
    }
    flush(&mut carry, &mut out);
    out
}

fn container_blocks(body: &Body) -> Vec<String> {
    (0..body.elements.len()).map(|i| body.xml(i).to_string()).collect()
}

impl Docx {
    /// Every tracked change in the document's stories, in document order.
    pub fn revisions(&self) -> Vec<Revision> {
        let mut parts = vec![DOC.to_string()];
        parts.extend(self.headers_footers().into_iter().map(|h| h.part));
        for p in ["word/footnotes.xml", "word/endnotes.xml", "word/comments.xml"] {
            if self.has_part(p) {
                parts.push(p.to_string());
            }
        }
        let mut out = Vec::new();
        for part in parts {
            let Ok(src) = (if part == DOC { Ok(self.document.clone()) } else { self.read_part(&part) }) else { continue };
            let toks: Vec<_> = tokens(&src).collect();
            for (i, t) in toks.iter().enumerate() {
                if !matches!(t.kind, Kind::Start | Kind::Empty) || !CHANGE_TAGS.contains(&t.local()) {
                    continue;
                }
                let tag = &src[t.start..t.end];
                // Only real revisions carry an author (w:ins also appears as a run property marker).
                let Some(author) = attr(tag, "w:author") else { continue };
                let parent = toks[..i].iter().rev().find(|p| p.kind == Kind::Start).map(|p| p.local()).unwrap_or("");
                let kind = match (t.local(), parent) {
                    ("ins" | "del", "rPr") if t.kind == Kind::Empty => "paragraphMark".to_string(),
                    ("ins", "trPr") => "rowIns".into(),
                    ("del", "trPr") => "rowDel".into(),
                    (k, _) => k.to_string(),
                };
                let text = if t.kind == Kind::Start && matches!(t.local(), "ins" | "del" | "moveFrom" | "moveTo") {
                    let end = xml::element_end(&toks, i);
                    let mut s = String::new();
                    for (j, x) in toks.iter().enumerate().skip(i + 1) {
                        if x.start >= end {
                            break;
                        }
                        if x.kind == Kind::Start && matches!(x.local(), "t" | "delText") {
                            s.push_str(&xml::unescape(xml::inner(&src, &toks, j)));
                        }
                    }
                    s
                } else {
                    String::new()
                };
                out.push(Revision {
                    id: attr(tag, "w:id").unwrap_or("").to_string(),
                    kind,
                    author: xml::unescape(author),
                    date: attr(tag, "w:date").unwrap_or("").to_string(),
                    part: part.clone(),
                    text,
                });
            }
        }
        out
    }

    /// Accept (`accept = true`) or reject the tracked changes matching `filter` in every
    /// story. Queues the changed parts in `edits` and returns how many changes were resolved.
    pub fn resolve_revisions(&self, edits: &mut DocxEdits, accept: bool, filter: &Filter) -> Result<usize, String> {
        let ctx = Ctx { accept, filter, count: Cell::new(0) };
        // Main body (respecting body edits already queued).
        {
            let current = self.current(edits, DOC).unwrap_or_else(|| self.document.clone());
            let current = if edits.body.is_some() || edits.sect_pr.is_some() {
                self.body()?.rebuild(edits.body.as_deref(), edits.sect_pr.as_deref())
            } else {
                current
            };
            let body = Body::parse(&current)?;
            let before = ctx.count.get();
            let resolved = resolve_blocks(&container_blocks(&body), &ctx);
            let sect = body.sect_pr_xml().map(|s| resolve_props_xml(s, "sectPrChange", &["headerReference", "footerReference"], &ctx));
            if ctx.count.get() > before {
                let orig = self.body()?;
                if current == self.document {
                    edits.body = Some(diff_blocks(&orig, &resolved));
                    edits.sect_pr = sect.filter(|s| Some(s.as_str()) != orig.sect_pr_xml());
                } else {
                    edits.body = Some(resolved.into_iter().map(Block::Xml).collect());
                    edits.sect_pr = sect;
                }
            }
        }
        // Headers and footers.
        for hf in self.headers_footers() {
            let Some(src) = self.current(edits, &hf.part) else { continue };
            let root = tokens(&src).find(|t| matches!(t.kind, Kind::Start | Kind::Empty)).map(|t| t.local().to_string()).unwrap_or_default();
            let Ok(body) = Body::parse_in(&src, &root) else { continue };
            let before = ctx.count.get();
            let resolved = resolve_blocks(&container_blocks(&body), &ctx);
            if ctx.count.get() > before {
                let blocks = diff_blocks(&body, &resolved);
                edits.parts.insert(hf.part.clone(), ensure_namespaces(&body.rebuild(Some(&blocks), None)).into_bytes());
            }
        }
        // Notes and comments: each note/comment holds blocks.
        let containers = [(NoteKind::Footnote.part(), "footnote"), (NoteKind::Endnote.part(), "endnote"), ("word/comments.xml", "comment")];
        for (part, element) in containers {
            let Some(src) = self.current(edits, part) else { continue };
            let before = ctx.count.get();
            let mut out = String::with_capacity(src.len());
            let mut cursor = 0;
            let kind = if element == "endnote" { NoteKind::Endnote } else { NoteKind::Footnote };
            let notes = if element == "comment" { comment_spans(&src) } else { Notes::parse(&src, kind)?.notes.iter().map(|n| (n.start, n.end)).collect() };
            for (start, end) in notes {
                let one = &src[start..end];
                let Ok(body) = Body::parse_in(one, element) else { continue };
                let resolved = resolve_blocks(&container_blocks(&body), &ctx);
                let blocks = diff_blocks(&body, &resolved);
                out.push_str(&src[cursor..start]);
                out.push_str(&body.rebuild(Some(&blocks), None));
                cursor = end;
            }
            out.push_str(&src[cursor..]);
            if ctx.count.get() > before {
                edits.parts.insert(part.to_string(), out.into_bytes());
            }
        }
        Ok(ctx.count.get())
    }

    /// Turn "Track changes" on or off for the document (`w:trackRevisions` in settings).
    pub fn set_track_revisions(&self, edits: &mut DocxEdits, on: bool) -> Result<(), String> {
        const PART: &str = "word/settings.xml";
        let Some(src) = self.current(edits, PART) else {
            return if on { Err("document has no settings part".into()) } else { Ok(()) };
        };
        let has = tokens(&src).find(|t| t.local() == "trackRevisions" && matches!(t.kind, Kind::Start | Kind::Empty));
        let new = match (has, on) {
            (Some(_), true) | (None, false) => return Ok(()),
            (Some(t), false) => {
                let toks: Vec<_> = tokens(&src).collect();
                let i = toks.iter().position(|x| x.start == t.start).unwrap();
                format!("{}{}", &src[..t.start], &src[xml::element_end(&toks, i)..])
            }
            (None, true) => {
                // trackRevisions follows these in CT_Settings.
                const BEFORE: &[&str] = &["writeProtection", "view", "zoom", "removePersonalInformation", "removeDateAndTime",
                    "doNotDisplayPageBoundaries", "displayBackgroundShape", "printPostScriptOverText", "printFractionalCharacterWidth",
                    "printFormsData", "embedTrueTypeFonts", "embedSystemFonts", "saveSubsetFonts", "saveFormsData", "mirrorMargins",
                    "alignBordersAndEdges", "bordersDoNotSurroundHeader", "bordersDoNotSurroundFooter", "gutterAtTop",
                    "hideSpellingErrors", "hideGrammaticalErrors", "activeWritingStyle", "proofState", "formsDesign",
                    "attachedTemplate", "linkStyles", "stylePaneFormatFilter", "stylePaneSortMethod", "documentType", "mailMerge", "revisionView"];
                let toks: Vec<_> = tokens(&src).collect();
                let root = toks.iter().position(|t| t.kind == Kind::Start).ok_or("empty settings")?;
                let mut at = toks[root].end;
                let mut i = root + 1;
                while i < toks.len() {
                    let t = toks[i];
                    if matches!(t.kind, Kind::Start | Kind::Empty) {
                        let end = xml::element_end(&toks, i);
                        if BEFORE.contains(&t.local()) {
                            at = end;
                        } else {
                            break;
                        }
                        while i < toks.len() && toks[i].start < end {
                            i += 1;
                        }
                    } else {
                        i += 1;
                    }
                }
                format!("{}<w:trackRevisions/>{}", &src[..at], &src[at..])
            }
        };
        edits.parts.insert(PART.into(), new.into_bytes());
        Ok(())
    }
}

fn comment_spans(src: &str) -> Vec<(usize, usize)> {
    let toks: Vec<_> = tokens(src).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        if t.local() == "comment" && t.kind == Kind::Start {
            let end = xml::element_end(&toks, i);
            out.push((t.start, end));
            while i < toks.len() && toks[i].start < end {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}
