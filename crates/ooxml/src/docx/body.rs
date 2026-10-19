//! `w:body` as a list of top-level elements, by byte range, for splicing.

use crate::xml::{element_end, tokens, Kind, Tok};

/// One element of a rebuilt body.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// The original element at this index, copied byte for byte.
    Orig(usize),
    /// Freshly written XML (an edited or inserted paragraph, table, ...).
    Xml(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Element<'a> {
    /// Local name: `p`, `tbl`, `sdt`, `bookmarkStart`, `customXml`, ...
    pub name: &'a str,
    pub start: usize,
    pub end: usize,
}

/// Parsed `document.xml`: everything before the first body element, the
/// elements, the final `w:sectPr`, and everything after.
#[derive(Debug, Clone)]
pub struct Body<'a> {
    pub src: &'a str,
    pub elements: Vec<Element<'a>>,
    /// Final section properties (direct child of `w:body`), if present.
    pub sect_pr: Option<(usize, usize)>,
    /// Offset just past `<w:body>` and of `</w:body>`.
    pub open_end: usize,
    pub close_start: usize,
}

impl<'a> Body<'a> {
    pub fn parse(src: &'a str) -> Result<Self, String> {
        Self::parse_in(src, "body")
    }

    /// Parse the block children of the first `container` element (`body`, `hdr`, `ftr`,
    /// `footnote`, `comment`, `txbxContent`, ...).
    pub fn parse_in(src: &'a str, container: &str) -> Result<Self, String> {
        let toks: Vec<Tok> = tokens(src).collect();
        let bi = toks.iter().position(|t| t.local() == container && matches!(t.kind, Kind::Start | Kind::Empty))
            .ok_or_else(|| format!("no w:{container} element"))?;
        if toks[bi].kind == Kind::Empty {
            let at = toks[bi].end;
            return Ok(Body { src, elements: vec![], sect_pr: None, open_end: at, close_start: at });
        }
        let close = element_end(&toks, bi);
        let close_start = src[..close].rfind("</").ok_or("unterminated w:body")?;
        let mut elements = Vec::new();
        let mut sect_pr = None;
        let mut i = bi + 1;
        while i < toks.len() && toks[i].start < close_start {
            let t = toks[i];
            match t.kind {
                Kind::Start | Kind::Empty => {
                    let end = element_end(&toks, i);
                    if t.local() == "sectPr" && container == "body" {
                        sect_pr = Some((t.start, end));
                    } else {
                        elements.push(Element { name: t.local(), start: t.start, end });
                    }
                    while i < toks.len() && toks[i].start < end {
                        i += 1;
                    }
                }
                _ => i += 1, // comments / stray end tags
            }
        }
        Ok(Body { src, elements, sect_pr, open_end: toks[bi].end, close_start })
    }

    pub fn xml(&self, i: usize) -> &'a str {
        let e = self.elements[i];
        &self.src[e.start..e.end]
    }

    /// Index of the n-th top-level paragraph.
    pub fn paragraph(&self, n: usize) -> Option<usize> {
        self.elements.iter().enumerate().filter(|(_, e)| e.name == "p").nth(n).map(|(i, _)| i)
    }

    pub fn sect_pr_xml(&self) -> Option<&'a str> {
        self.sect_pr.map(|(s, e)| &self.src[s..e])
    }

    /// The whole part with the body replaced by `blocks` (or kept, if `None`).
    /// Runs of consecutive original elements keep the whitespace between them.
    pub fn rebuild(&self, blocks: Option<&[Block]>, sect_pr: Option<&str>) -> String {
        let src = self.src;
        let mut out = String::with_capacity(src.len() + 1024);
        out.push_str(&src[..self.open_end]);
        match blocks {
            None => {
                let end = self.sect_pr.map(|(s, _)| s).unwrap_or(self.close_start);
                out.push_str(&src[self.open_end..end]);
            }
            Some(blocks) => {
                let mut prev: Option<usize> = None;
                for b in blocks {
                    match b {
                        Block::Orig(i) => {
                            let e = self.elements[*i];
                            if let Some(p) = prev.filter(|p| p + 1 == *i) {
                                out.push_str(&src[self.elements[p].end..e.start]);
                            }
                            out.push_str(&src[e.start..e.end]);
                            prev = Some(*i);
                        }
                        Block::Xml(x) => {
                            out.push_str(x);
                            prev = None;
                        }
                    }
                }
            }
        }
        match (sect_pr, self.sect_pr) {
            (Some(new), _) => out.push_str(new),
            (None, Some((s, e))) => out.push_str(&src[s..e]),
            (None, None) => {}
        }
        // Whatever followed the final sectPr (usually nothing), then the closing tags.
        let tail_from = self.sect_pr.map(|(_, e)| e).unwrap_or(self.close_start);
        out.push_str(&src[tail_from..]);
        out
    }

    /// Blocks that reproduce the body unchanged.
    pub fn identity(&self) -> Vec<Block> {
        (0..self.elements.len()).map(Block::Orig).collect()
    }
}
