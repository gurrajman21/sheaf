//! Locations and block containers.
//!
//! A location is a *story* (the body, a header or footer part, a footnote, an endnote,
//! a comment) plus a *path* of block indices and a visible character offset. Paths
//! descend through tables (`[table, row, cell, block]`) and text boxes
//! (`[paragraph, -(k+1), block]` for the k-th text box anchored in that paragraph), to
//! any depth. The last element is the paragraph's index inside its container.
//!
//! Every editing operation is written once against [`Blocks`], a list of block-level
//! elements; [`edit_in_element`] finds a nested container inside an element's XML,
//! lets the operation run on it and splices the result back.

use ooxml::docx::table::{Item, Table};
use ooxml::docx::{Block, Body, Para};
use ooxml::xml::{element_end, tokens, Kind};
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Story {
    Body,
    /// A header or footer part, e.g. `word/header1.xml`.
    Part(String),
    Footnote(i64),
    Endnote(i64),
    Comment(i64),
}

impl Story {
    pub fn parse(s: &str) -> Story {
        if let Some(p) = s.strip_prefix("h:") {
            Story::Part(p.to_string())
        } else if let Some(n) = s.strip_prefix("fn:").and_then(|n| n.parse().ok()) {
            Story::Footnote(n)
        } else if let Some(n) = s.strip_prefix("en:").and_then(|n| n.parse().ok()) {
            Story::Endnote(n)
        } else if let Some(n) = s.strip_prefix("c:").and_then(|n| n.parse().ok()) {
            Story::Comment(n)
        } else {
            Story::Body
        }
    }

    pub fn key(&self) -> String {
        match self {
            Story::Body => String::new(),
            Story::Part(p) => format!("h:{p}"),
            Story::Footnote(n) => format!("fn:{n}"),
            Story::Endnote(n) => format!("en:{n}"),
            Story::Comment(n) => format!("c:{n}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loc {
    pub story: Story,
    pub path: Vec<i64>,
    pub o: usize,
}

impl Loc {
    pub fn body(b: usize, o: usize) -> Loc {
        Loc { story: Story::Body, path: vec![b as i64], o }
    }

    /// The container's address (story + every path element but the last).
    pub fn container(&self) -> (&Story, &[i64]) {
        (&self.story, &self.path[..self.path.len().saturating_sub(1)])
    }

    /// Index of the paragraph inside its container.
    pub fn idx(&self) -> usize {
        self.path.last().copied().unwrap_or(0).max(0) as usize
    }

    pub fn with(&self, idx: usize, o: usize) -> Loc {
        let mut path = self.path.clone();
        if let Some(l) = path.last_mut() {
            *l = idx as i64;
        }
        Loc { story: self.story.clone(), path, o }
    }

    pub fn same_container(&self, other: &Loc) -> bool {
        self.container() == other.container()
    }

    pub fn parse(v: &Value) -> Result<Loc, String> {
        let o = v["o"].as_u64().unwrap_or(0) as usize;
        let story = Story::parse(v["s"].as_str().unwrap_or(""));
        let path: Vec<i64> = match v["p"].as_array() {
            Some(a) => a.iter().filter_map(|x| x.as_i64()).collect(),
            None => vec![v["b"].as_i64().ok_or("position needs a path")?],
        };
        if path.is_empty() {
            return Err("empty path".into());
        }
        Ok(Loc { story, path, o })
    }

    pub fn json(&self) -> Value {
        json!({"s": self.story.key(), "p": self.path, "o": self.o})
    }
}

/// A list of block-level elements an edit runs on.
pub trait Blocks {
    fn len(&self) -> usize;
    /// `p`, `tbl`, `sdt` or `other`.
    fn kind(&self, i: usize) -> &'static str;
    fn para(&self, i: usize) -> Option<Para>;
    fn set_para(&mut self, i: usize, p: Para);
    fn insert_para(&mut self, i: usize, p: Para);
    fn insert_xml(&mut self, i: usize, xml: String);
    fn remove(&mut self, i: usize);
    fn para_len(&self, i: usize) -> usize {
        self.para(i).map(|p| p.len()).unwrap_or(0)
    }
    /// XML of block `i`.
    fn xml(&self, i: usize) -> Option<String>;
}

pub fn kind_of(xml: &str) -> &'static str {
    match tokens(xml).find(|t| matches!(t.kind, Kind::Start | Kind::Empty)).map(|t| t.local()) {
        Some("p") => "p",
        Some("tbl") => "tbl",
        Some("sdt") => "sdt",
        _ => "other",
    }
}

/// Blocks held as XML strings (nested containers and other stories).
pub struct StrBlocks<'a>(pub &'a mut Vec<String>);

impl Blocks for StrBlocks<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }
    fn kind(&self, i: usize) -> &'static str {
        self.0.get(i).map(|x| kind_of(x)).unwrap_or("other")
    }
    fn para(&self, i: usize) -> Option<Para> {
        self.0.get(i).filter(|x| kind_of(x) == "p").and_then(|x| Para::parse(x).ok())
    }
    fn set_para(&mut self, i: usize, p: Para) {
        self.0[i] = p.to_xml();
    }
    fn insert_para(&mut self, i: usize, p: Para) {
        self.0.insert(i, p.to_xml());
    }
    fn insert_xml(&mut self, i: usize, xml: String) {
        self.0.insert(i, xml);
    }
    fn remove(&mut self, i: usize) {
        self.0.remove(i);
    }
    fn xml(&self, i: usize) -> Option<String> {
        self.0.get(i).cloned()
    }
}

/// The k-th text box content (`w:txbxContent`) in an element, skipping VML fallbacks
/// (their copy is kept in sync when the box is edited). Returns its byte range.
fn textbox_range(xml: &str, k: usize) -> Option<(usize, usize)> {
    let toks: Vec<_> = tokens(xml).collect();
    let mut seen = 0;
    let mut fallback_depth = 0usize;
    for (i, t) in toks.iter().enumerate() {
        match (t.kind, t.local()) {
            (Kind::Start, "Fallback") => fallback_depth += 1,
            (Kind::End, "Fallback") => fallback_depth = fallback_depth.saturating_sub(1),
            (Kind::Start, "txbxContent") if fallback_depth == 0 => {
                if seen == k {
                    return Some((t.start, element_end(&toks, i)));
                }
                seen += 1;
            }
            _ => {}
        }
    }
    None
}

/// Number of (non-fallback) text boxes in an element.
pub fn textbox_count(xml: &str) -> usize {
    (0..).take_while(|&k| textbox_range(xml, k).is_some()).count()
}

/// Blocks inside a text box content element.
fn textbox_blocks(el: &str) -> Vec<String> {
    Body::parse_in(el, "txbxContent").map(|b| (0..b.elements.len()).map(|i| b.xml(i).to_string()).collect()).unwrap_or_default()
}

/// Run `f` on the container `path` points to inside `xml` (a table: `[row, cell, ...]`;
/// a paragraph with text boxes: `[-(k+1), ...]`), descending further through `path`'s
/// remaining `[block, ...]` pairs. Returns the element's new XML.
pub fn edit_in_element<R>(xml: &str, path: &[i64], f: &mut dyn FnMut(&mut dyn Blocks) -> Result<R, String>) -> Result<(String, R), String> {
    let descend = |blocks: &mut Vec<String>, rest: &[i64], f: &mut dyn FnMut(&mut dyn Blocks) -> Result<R, String>| -> Result<R, String> {
        if rest.is_empty() {
            return f(&mut StrBlocks(blocks));
        }
        let i = rest[0].max(0) as usize;
        let el = blocks.get(i).ok_or("path points past the container")?.clone();
        let (new, r) = edit_in_element(&el, &rest[1..], f)?;
        blocks[i] = new;
        Ok(r)
    };
    match kind_of(xml) {
        "tbl" => {
            let (r, c) = (*path.first().ok_or("table path needs a row")? as usize, *path.get(1).ok_or("table path needs a cell")? as usize);
            let mut t = Table::parse(xml)?;
            let cell = t.cell_mut(r, c).ok_or("no such table cell")?;
            let out = descend(&mut cell.blocks, &path[2..], f)?;
            Ok((t.to_xml(), out))
        }
        "p" => {
            let k = path.first().filter(|v| **v < 0).map(|v| (-v - 1) as usize).ok_or("paragraph path needs a text box")?;
            let (s, e) = textbox_range(xml, k).ok_or("no such text box")?;
            let el = &xml[s..e];
            let mut blocks = textbox_blocks(el);
            let out = descend(&mut blocks, &path[1..], f)?;
            let body = Body::parse_in(el, "txbxContent")?;
            let new_el = body.rebuild(Some(&blocks.iter().cloned().map(Block::Xml).collect::<Vec<_>>()), None);
            let mut new = format!("{}{}{}", &xml[..s], new_el, &xml[e..]);
            // Keep a VML fallback copy of the same box in step.
            if let Some(fb) = fallback_twin(&new, s + new_el.len()) {
                new = format!("{}{}{}", &new[..fb.0], new_el, &new[fb.1..]);
            }
            Ok((new, out))
        }
        _ => Err("this element has no editable content".into()),
    }
}

/// A `txbxContent` inside the `mc:Fallback` that follows the edited box's `mc:Choice`.
fn fallback_twin(xml: &str, after: usize) -> Option<(usize, usize)> {
    let rest = &xml[after..];
    let fb = rest.find("<mc:Fallback")?;
    let close = rest[fb..].find("</mc:Fallback>")? + fb;
    let inner = &rest[fb..close];
    let s = inner.find("<w:txbxContent")?;
    let toks: Vec<_> = tokens(&inner[s..]).collect();
    let e = element_end(&toks, 0);
    // Only the fallback of the same AlternateContent (no other box in between).
    if rest[..fb].contains("<w:txbxContent") || rest[..fb].contains("<mc:AlternateContent") {
        return None;
    }
    Some((after + fb + s, after + fb + s + e))
}

/// Every paragraph in `blocks` (recursively through tables and text boxes), with its
/// path relative to the container and its visible text.
pub fn collect_paragraphs(blocks: &[String], prefix: &[i64], out: &mut Vec<(Vec<i64>, String)>) {
    for (i, b) in blocks.iter().enumerate() {
        let mut here = prefix.to_vec();
        here.push(i as i64);
        match kind_of(b) {
            "p" => {
                if let Ok(p) = Para::parse(b) {
                    out.push((here.clone(), p.text()));
                }
                for k in 0..textbox_count(b) {
                    if let Some((s, e)) = textbox_range(b, k) {
                        let mut tp = here.clone();
                        tp.push(-(k as i64) - 1);
                        collect_paragraphs(&textbox_blocks(&b[s..e]), &tp, out);
                    }
                }
            }
            "tbl" => {
                if let Ok(t) = Table::parse(b) {
                    for (r, row) in t.rows().enumerate() {
                        for (c, cell) in row.items.iter().filter_map(|x| match x { Item::Node(c) => Some(c), _ => None }).enumerate() {
                            let mut cp = here.clone();
                            cp.extend([r as i64, c as i64]);
                            collect_paragraphs(&cell.blocks, &cp, out);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Paragraphs of every table cell in a top-level table, keyed by the cell's path
/// (`[table, row, cell]`, nested: `[table, row, cell, block, row, cell]`), in order.
pub fn cell_paragraphs(table_xml: &str, prefix: &[i64], out: &mut std::collections::HashMap<Vec<i64>, Vec<(usize, String)>>) {
    let Ok(t) = Table::parse(table_xml) else { return };
    for (r, row) in t.rows().enumerate() {
        for (c, cell) in row.items.iter().filter_map(|x| match x { Item::Node(c) => Some(c), _ => None }).enumerate() {
            let mut cp = prefix.to_vec();
            cp.extend([r as i64, c as i64]);
            let mut paras = Vec::new();
            for (i, b) in cell.blocks.iter().enumerate() {
                match kind_of(b) {
                    "p" => paras.push((i, Para::parse(b).map(|p| p.text()).unwrap_or_default())),
                    "tbl" => {
                        let mut np = cp.clone();
                        np.push(i as i64);
                        cell_paragraphs(b, &np, out);
                    }
                    _ => {}
                }
            }
            out.insert(cp, paras);
        }
    }
}

/// Give every empty paragraph (recursively through tables) a zero-width space, so the
/// layout gives it a line the caret can sit on. Layout only; never saved.
pub fn fill_empty_paragraphs(blocks: &mut [String], zwsp: &str) {
    for b in blocks.iter_mut() {
        match kind_of(b) {
            "p" => {
                if let Ok(mut p) = Para::parse(b) {
                    if p.is_empty() {
                        p.insert_text(0, zwsp, None, None);
                        *b = p.to_xml();
                    }
                }
            }
            "tbl" => {
                if let Ok(mut t) = Table::parse(b) {
                    let rows = t.rows().count();
                    for r in 0..rows {
                        let cells = t.row_mut(r).map(|row| row.cells().count()).unwrap_or(0);
                        for c in 0..cells {
                            if let Some(cell) = t.cell_mut(r, c) {
                                fill_empty_paragraphs(&mut cell.blocks, zwsp);
                            }
                        }
                    }
                    *b = t.to_xml();
                }
            }
            _ => {}
        }
    }
}

/// Every paragraph's XML in `blocks` (recursively), with its path.
pub fn collect_paragraph_xml(blocks: &[String], prefix: &[i64], out: &mut Vec<(Vec<i64>, String)>) {
    for (i, b) in blocks.iter().enumerate() {
        let mut here = prefix.to_vec();
        here.push(i as i64);
        match kind_of(b) {
            "p" => {
                out.push((here.clone(), b.clone()));
                for k in 0..textbox_count(b) {
                    if let Some((s, e)) = textbox_range(b, k) {
                        let mut tp = here.clone();
                        tp.push(-(k as i64) - 1);
                        collect_paragraph_xml(&textbox_blocks(&b[s..e]), &tp, out);
                    }
                }
            }
            "tbl" => {
                if let Ok(t) = Table::parse(b) {
                    for (r, row) in t.rows().enumerate() {
                        for (c, cell) in row.items.iter().filter_map(|x| match x { Item::Node(c) => Some(c), _ => None }).enumerate() {
                            let mut cp = here.clone();
                            cp.extend([r as i64, c as i64]);
                            collect_paragraph_xml(&cell.blocks, &cp, out);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
