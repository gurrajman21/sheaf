//! Editable paragraph (`w:p`) that keeps everything it doesn't understand.
//!
//! A paragraph is parsed into a flat list of *segments*: runs (`w:r`) and raw
//! elements (bookmarks, comment ranges, content controls, math, ...). Each segment
//! remembers which inline wrappers it sits in (hyperlink, tracked insertion or
//! deletion, smart tag, simple field, ...), so text can be split and inserted
//! anywhere while the wrappers are re-emitted around the right segments.
//!
//! Offsets are in Unicode scalar values over the *visible* text: `w:t` text, one
//! character per `w:tab` and `w:br`. Deleted text (`w:del`, `w:moveFrom`) and
//! field codes don't count, so offsets match what the user sees.
//!
//! With a [`Rev`], edits are written as tracked changes: insertions as `w:ins`,
//! deletions as `w:del` with `w:delText`, formatting as `w:rPrChange`.

use std::cell::Cell;

use super::props::{ParaFormat, Props, RunFormat};
use crate::xml::{self, element_end, tokens, Kind, Tok};

/// Author, date and id source for tracked changes and comments.
#[derive(Debug)]
pub struct Rev {
    pub author: String,
    /// ISO 8601, e.g. `2026-10-18T10:00:00Z`.
    pub date: String,
    next_id: Cell<u32>,
}

impl Rev {
    /// `first_id` must be above every `w:id` already in the document
    /// ([`super::Docx::max_annotation_id`] + 1).
    pub fn new(author: &str, date: &str, first_id: u32) -> Self {
        Rev { author: author.into(), date: date.into(), next_id: Cell::new(first_id) }
    }

    /// The id the next change will get.
    pub fn next_id(&self) -> u32 {
        self.next_id.get()
    }

    pub fn id(&self) -> u32 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }

    fn attrs(&self) -> String {
        format!(r#"w:id="{}" w:author="{}" w:date="{}""#, self.id(), xml::escape(&self.author), xml::escape(&self.date))
    }
}

const GROUPS: &[&str] = &["hyperlink", "ins", "del", "moveTo", "moveFrom", "smartTag", "customXml", "fldSimple", "dir", "bdo"];
const HIDDEN_GROUPS: &[&str] = &["del", "moveFrom"];

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub kind: String, // local name
    pub open: String,
    pub close: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RC {
    Text(String),
    Tab(String),
    Break(String),
    /// Anything else in a run (drawings, field chars, instrText, footnote refs, delText, ...).
    Raw(String),
}

impl RC {
    fn len(&self) -> usize {
        match self {
            RC::Text(t) => t.chars().count(),
            RC::Tab(_) | RC::Break(_) => 1,
            RC::Raw(_) => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub open: String,
    pub rpr: Option<Props>,
    pub content: Vec<RC>,
}

impl Run {
    pub fn new(rpr: Option<Props>, text: &str) -> Self {
        Run { open: "<w:r>".into(), rpr, content: text_content(text) }
    }

    fn len(&self) -> usize {
        self.content.iter().map(RC::len).sum()
    }

    pub fn text(&self) -> String {
        self.content.iter().map(|c| match c {
            RC::Text(t) => t.as_str(),
            RC::Tab(_) => "\t",
            RC::Break(_) => "\n",
            RC::Raw(_) => "",
        }).collect()
    }

    /// Split at visible offset `at` (0 < at < len): `self` keeps the head, returns the tail.
    fn split_off(&mut self, at: usize) -> Run {
        let mut pos = 0;
        let mut in_tail = false;
        let (mut head, mut tail) = (Vec::new(), Vec::new());
        for c in std::mem::take(&mut self.content) {
            let l = c.len();
            if in_tail {
                tail.push(c);
            } else if l == 0 || pos + l <= at {
                head.push(c); // zero-width items at the split point stay with the text before
            } else if pos >= at {
                in_tail = true;
                tail.push(c);
            } else {
                in_tail = true;
                if let RC::Text(t) = c {
                    let b = t.char_indices().nth(at - pos).map(|(i, _)| i).unwrap_or(t.len());
                    head.push(RC::Text(t[..b].to_string()));
                    tail.push(RC::Text(t[b..].to_string()));
                }
            }
            pos += l;
        }
        self.content = head;
        Run { open: self.open.clone(), rpr: self.rpr.clone(), content: tail }
    }

    fn to_xml(&self, deleted: bool) -> String {
        let mut s = self.open.clone();
        if let Some(p) = &self.rpr {
            s.push_str(&p.to_xml());
        }
        for c in &self.content {
            match c {
                RC::Text(t) if t.is_empty() => {}
                RC::Text(t) => {
                    let tag = if deleted { "w:delText" } else { "w:t" };
                    let space = if t.starts_with(char::is_whitespace) || t.ends_with(char::is_whitespace) { r#" xml:space="preserve""# } else { "" };
                    s.push_str(&format!("<{tag}{space}>{}</{tag}>", xml::escape(t)));
                }
                RC::Tab(x) | RC::Break(x) | RC::Raw(x) => s.push_str(x),
            }
        }
        s.push_str("</w:r>");
        s
    }
}

fn text_content(text: &str) -> Vec<RC> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for ch in text.chars() {
        match ch {
            '\t' | '\n' => {
                if !buf.is_empty() {
                    out.push(RC::Text(std::mem::take(&mut buf)));
                }
                out.push(if ch == '\t' { RC::Tab("<w:tab/>".into()) } else { RC::Break("<w:br/>".into()) });
            }
            c => buf.push(c),
        }
    }
    if !buf.is_empty() || out.is_empty() {
        out.push(RC::Text(buf));
    }
    out
}

/// A run with plain text, as XML (used for comment bodies and new paragraphs).
pub fn text_run(text: &str, rpr: Option<&Props>) -> String {
    Run::new(rpr.cloned(), text).to_xml(false)
}

#[derive(Debug, Clone, PartialEq)]
pub enum SegKind {
    Run(Run),
    Raw(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Seg {
    pub kind: SegKind,
    /// Indices into `Para::groups`, outermost first.
    pub wrap: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Para {
    pub open: String,
    pub ppr: Option<Props>,
    pub segs: Vec<Seg>,
    pub groups: Vec<Group>,
}

impl Para {
    /// Parse one `<w:p>…</w:p>` element.
    pub fn parse(src: &str) -> Result<Para, String> {
        let toks: Vec<Tok> = tokens(src).collect();
        let first = toks.iter().position(|t| matches!(t.kind, Kind::Start | Kind::Empty)).ok_or("empty paragraph xml")?;
        let root = toks[first];
        if root.local() != "p" {
            return Err(format!("not a paragraph: {}", root.name));
        }
        let open = if root.kind == Kind::Empty {
            src[root.start..root.end].trim_end_matches("/>").trim_end().to_string() + ">"
        } else {
            src[root.start..root.end].to_string()
        };
        let mut p = Para { open, ppr: None, segs: vec![], groups: vec![] };
        if root.kind == Kind::Start {
            let end = element_end(&toks, first);
            p.parse_children(src, &toks, first + 1, end, &mut vec![]);
        }
        Ok(p)
    }

    fn parse_children(&mut self, src: &str, toks: &[Tok], mut i: usize, end: usize, wrap: &mut Vec<usize>) {
        while i < toks.len() && toks[i].start < end {
            let t = toks[i];
            if !matches!(t.kind, Kind::Start | Kind::Empty) {
                i += 1;
                continue;
            }
            let e = element_end(toks, i);
            let raw = &src[t.start..e];
            let name = t.local();
            let mut next = i + 1;
            while next < toks.len() && toks[next].start < e {
                next += 1;
            }
            if name == "pPr" && wrap.is_empty() {
                self.ppr = Some(Props::parse(raw));
            } else if name == "r" {
                self.segs.push(Seg { kind: SegKind::Run(parse_run(src, toks, i, e)), wrap: wrap.clone() });
            } else if GROUPS.contains(&name) && t.kind == Kind::Start {
                self.groups.push(Group {
                    kind: name.to_string(),
                    open: src[t.start..t.end].to_string(),
                    close: format!("</{}>", t.name),
                });
                wrap.push(self.groups.len() - 1);
                let before = self.segs.len();
                self.parse_children(src, toks, i + 1, e, wrap);
                wrap.pop();
                if self.segs.len() == before {
                    // empty wrapper: keep it verbatim
                    self.segs.push(Seg { kind: SegKind::Raw(raw.to_string()), wrap: wrap.clone() });
                }
            } else {
                self.segs.push(Seg { kind: SegKind::Raw(raw.to_string()), wrap: wrap.clone() });
            }
            i = next;
        }
    }

    /// A new paragraph with plain text, optional paragraph and run properties.
    pub fn new(text: &str, ppr: Option<Props>, rpr: Option<Props>) -> Para {
        let mut p = Para { open: "<w:p>".into(), ppr, segs: vec![], groups: vec![] };
        if !text.is_empty() {
            p.segs.push(Seg { kind: SegKind::Run(Run::new(rpr, text)), wrap: vec![] });
        }
        p
    }

    /// Whether a segment is tracked-deleted (or moved away) and so not visible.
    pub fn hidden(&self, s: &Seg) -> bool {
        s.wrap.iter().any(|g| HIDDEN_GROUPS.contains(&self.groups[*g].kind.as_str()))
    }

    fn seg_len(&self, s: &Seg) -> usize {
        match &s.kind {
            SegKind::Run(r) if !self.hidden(s) => r.len(),
            _ => 0,
        }
    }

    pub fn len(&self) -> usize {
        self.segs.iter().map(|s| self.seg_len(s)).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Visible offset ranges of each run that has visible text, in order.
    pub fn run_ranges(&self) -> Vec<(usize, usize)> {
        let mut pos = 0;
        let mut out = Vec::new();
        for s in &self.segs {
            let l = self.seg_len(s);
            if l > 0 {
                out.push((pos, pos + l));
            }
            pos += l;
        }
        out
    }

    /// Visible text (tabs as `\t`, breaks as `\n`).
    pub fn text(&self) -> String {
        self.segs.iter().filter(|s| !self.hidden(s)).filter_map(|s| match &s.kind {
            SegKind::Run(r) => Some(r.text()),
            _ => None,
        }).collect()
    }

    /// Ensure a segment boundary at `off`; returns the index of the first segment at or after it.
    pub(crate) fn split_at(&mut self, off: usize) -> usize {
        let mut pos = 0;
        let mut i = 0;
        while i < self.segs.len() {
            let l = self.seg_len(&self.segs[i]);
            if pos == off {
                return i;
            }
            if off < pos + l {
                let wrap = self.segs[i].wrap.clone();
                let SegKind::Run(r) = &mut self.segs[i].kind else { unreachable!() };
                let tail = r.split_off(off - pos);
                self.segs.insert(i + 1, Seg { kind: SegKind::Run(tail), wrap });
                return i + 1;
            }
            pos += l;
            i += 1;
        }
        self.segs.len()
    }

    /// Run whose formatting new text at `off` should take: the visible run ending at
    /// `off`, else the one starting there (Word's rule).
    fn style_source(&self, off: usize) -> Option<(Option<Props>, Vec<usize>)> {
        let mut pos = 0;
        let mut before = None;
        let mut after = None;
        for s in &self.segs {
            let l = self.seg_len(s);
            if let (SegKind::Run(r), true) = (&s.kind, l > 0) {
                if pos < off && off <= pos + l {
                    before = Some((r.rpr.clone(), s.wrap.clone()));
                } else if pos >= off && after.is_none() {
                    after = Some((r.rpr.clone(), s.wrap.clone()));
                }
            }
            pos += l;
        }
        before.or(after)
    }

    pub(crate) fn new_group(&mut self, kind: &str, rev: &Rev) -> usize {
        self.groups.push(Group { kind: kind.into(), open: format!("<w:{kind} {}>", rev.attrs()), close: format!("</w:{kind}>") });
        self.groups.len() - 1
    }

    fn own_ins(&self, wrap: &[usize], rev: &Rev) -> Option<usize> {
        wrap.iter().copied().find(|g| {
            let g = &self.groups[*g];
            g.kind == "ins" && xml::attr(&g.open, "w:author").map(xml::unescape).as_deref() == Some(rev.author.as_str())
        })
    }

    /// Insert `text` at `off`, formatted like the surrounding text (plus `fmt`).
    pub fn insert_text(&mut self, off: usize, text: &str, fmt: Option<&RunFormat>, rev: Option<&Rev>) {
        if text.is_empty() {
            return;
        }
        let off = off.min(self.len());
        let (mut rpr, mut wrap) = self.style_source(off).unwrap_or_else(|| (self.mark_rpr(), vec![]));
        // New text never goes inside a deletion or someone else's tracked change.
        wrap.retain(|g| !matches!(self.groups[*g].kind.as_str(), "del" | "moveFrom" | "moveTo" | "ins"));
        if let Some(f) = fmt {
            let mut p = rpr.take().unwrap_or_else(|| Props::new("w:rPr"));
            f.apply(&mut p);
            rpr = if p.is_empty() { None } else { Some(p) };
        }
        if let Some(p) = rpr.as_mut() {
            p.set("rPrChange", None);
            p.set("ins", None);
            p.set("del", None);
        }
        let at = self.split_at(off);
        if let Some(rev) = rev {
            // Extend our own adjacent insertion instead of starting a new one.
            let prev_ins = at.checked_sub(1).and_then(|i| self.own_ins(&self.segs[i].wrap, rev));
            match prev_ins {
                Some(g) => wrap.push(g),
                None => {
                    let g = self.new_group("ins", rev);
                    wrap.push(g);
                }
            }
        }
        // Append to the previous run when it is formatted and wrapped identically.
        if let Some(i) = at.checked_sub(1) {
            let same = matches!(&self.segs[i].kind, SegKind::Run(r) if r.rpr == rpr && matches!(r.content.last(), Some(RC::Text(_)))) && self.segs[i].wrap == wrap;
            if same && !text.contains(['\t', '\n']) {
                if let SegKind::Run(r) = &mut self.segs[i].kind {
                    if let Some(RC::Text(t)) = r.content.last_mut() {
                        t.push_str(text);
                        return;
                    }
                }
            }
        }
        self.segs.insert(at, Seg { kind: SegKind::Run(Run::new(rpr, text)), wrap });
    }

    /// Run properties of the paragraph mark (what Word uses for text typed into an empty paragraph).
    fn mark_rpr(&self) -> Option<Props> {
        let raw = self.ppr.as_ref()?.get("rPr")?;
        let mut p = Props::parse(raw);
        p.set("ins", None);
        p.set("del", None);
        p.set("rPrChange", None);
        if p.is_empty() { None } else { Some(p) }
    }

    /// Delete visible text in `start..end`.
    pub fn delete(&mut self, start: usize, end: usize, rev: Option<&Rev>) {
        let end = end.min(self.len());
        if start >= end {
            return;
        }
        let a = self.split_at(start);
        let b = self.split_at(end);
        let mut del_group: Option<usize> = None;
        let mut i = a;
        let mut stop = b;
        while i < stop {
            let len = self.seg_len(&self.segs[i]);
            if len == 0 || !matches!(self.segs[i].kind, SegKind::Run(_)) {
                i += 1;
                continue;
            }
            let own = rev.and_then(|r| self.own_ins(&self.segs[i].wrap, r)).is_some();
            match rev {
                Some(r) if !own => {
                    let g = *del_group.get_or_insert_with(|| self.new_group("del", r));
                    self.segs[i].wrap.push(g);
                    i += 1;
                }
                _ => {
                    // Untracked, or removing our own tracked insertion: drop the text,
                    // keep anything else the run carried (drawings, field chars, ...).
                    let SegKind::Run(run) = &mut self.segs[i].kind else { unreachable!() };
                    run.content.retain(|c| c.len() == 0);
                    if run.content.is_empty() {
                        self.segs.remove(i);
                        stop -= 1;
                    } else {
                        i += 1;
                    }
                }
            }
        }
    }

    /// Apply run formatting to `start..end`.
    pub fn format(&mut self, start: usize, end: usize, fmt: &RunFormat, rev: Option<&Rev>) {
        let end = end.min(self.len());
        if start >= end {
            return;
        }
        let a = self.split_at(start);
        let b = self.split_at(end);
        for i in a..b {
            if self.seg_len(&self.segs[i]) == 0 {
                continue;
            }
            let own = rev.and_then(|r| self.own_ins(&self.segs[i].wrap, r)).is_some();
            let SegKind::Run(run) = &mut self.segs[i].kind else { continue };
            let mut p = run.rpr.take().unwrap_or_else(|| Props::new("w:rPr"));
            if let (Some(r), false) = (rev, own) {
                if p.get("rPrChange").is_none() {
                    let old = p.inner(&["rPrChange", "ins", "del", "moveFrom", "moveTo"]);
                    let change = format!("<w:rPrChange {}><w:rPr>{old}</w:rPr></w:rPrChange>", r.attrs());
                    fmt.apply(&mut p);
                    p.set("rPrChange", Some(change));
                } else {
                    fmt.apply(&mut p);
                }
            } else {
                fmt.apply(&mut p);
            }
            run.rpr = if p.is_empty() { None } else { Some(p) };
        }
    }

    /// Apply paragraph formatting.
    pub fn format_paragraph(&mut self, fmt: &ParaFormat, rev: Option<&Rev>) {
        let mut p = self.ppr.take().unwrap_or_else(|| Props::new("w:pPr"));
        if let Some(r) = rev {
            if p.get("pPrChange").is_none() {
                let old = p.inner(&["pPrChange", "rPr", "sectPr"]);
                fmt.apply(&mut p);
                p.set("pPrChange", Some(format!("<w:pPrChange {}><w:pPr>{old}</w:pPr></w:pPrChange>", r.attrs())));
                self.ppr = Some(p);
                return;
            }
        }
        fmt.apply(&mut p);
        self.ppr = if p.is_empty() { None } else { Some(p) };
    }

    /// Anchor comment `id` on `start..end` (the comment body goes in comments.xml,
    /// see [`super::Docx::queue_comment`]).
    pub fn add_comment(&mut self, start: usize, end: usize, id: u32) {
        let end = end.min(self.len()).max(start);
        let b = self.split_at(end);
        let reference = Run {
            open: "<w:r>".into(),
            rpr: Some(Props::parse(r#"<w:rPr><w:rStyle w:val="CommentReference"/></w:rPr>"#)),
            content: vec![RC::Raw(format!(r#"<w:commentReference w:id="{id}"/>"#))],
        };
        self.segs.insert(b, Seg { kind: SegKind::Run(reference), wrap: vec![] });
        self.segs.insert(b, Seg { kind: SegKind::Raw(format!(r#"<w:commentRangeEnd w:id="{id}"/>"#)), wrap: vec![] });
        let a = self.split_at(start);
        self.segs.insert(a, Seg { kind: SegKind::Raw(format!(r#"<w:commentRangeStart w:id="{id}"/>"#)), wrap: vec![] });
    }

    /// Wrap visible text `start..end` in an inline element (a hyperlink), outermost.
    pub fn wrap_range(&mut self, start: usize, end: usize, open: &str, close: &str, kind: &str) {
        let end = end.min(self.len());
        if start >= end {
            return;
        }
        let a = self.split_at(start);
        let b = self.split_at(end);
        self.groups.push(Group { kind: kind.into(), open: open.into(), close: close.into() });
        let g = self.groups.len() - 1;
        for s in &mut self.segs[a..b] {
            s.wrap.insert(0, g);
        }
    }

    /// The hyperlink around visible offset `off`: (open tag, start, end).
    pub fn link_at(&self, off: usize) -> Option<(String, usize, usize)> {
        let mut pos = 0;
        let mut found: Option<(usize, usize, usize)> = None; // group, start, end
        for s in &self.segs {
            let l = self.seg_len(s);
            for &g in &s.wrap {
                if self.groups[g].kind == "hyperlink" {
                    match &mut found {
                        Some((fg, _, e)) if *fg == g => *e = pos + l,
                        None if pos <= off && off <= pos + l => found = Some((g, pos, pos + l)),
                        _ => {}
                    }
                }
            }
            pos += l;
        }
        // Extend the start back over earlier segments of the same link.
        let (g, mut start, end) = found?;
        let mut p2 = 0;
        for s in &self.segs {
            if s.wrap.contains(&g) {
                start = start.min(p2);
                break;
            }
            p2 += self.seg_len(s);
        }
        Some((self.groups[g].open.clone(), start, end))
    }

    /// Remove the hyperlink around `off` (its text stays). Returns whether there was one.
    pub fn unlink_at(&mut self, off: usize) -> bool {
        let Some((open, _, _)) = self.link_at(off) else { return false };
        let Some(g) = self.groups.iter().position(|x| x.kind == "hyperlink" && x.open == open) else { return false };
        for s in &mut self.segs {
            s.wrap.retain(|w| *w != g);
            if let SegKind::Run(r) = &mut s.kind {
                if let Some(p) = r.rpr.as_mut() {
                    if p.get("rStyle").is_some_and(|x| xml::attr(x, "w:val") == Some("Hyperlink")) {
                        p.set("rStyle", None);
                    }
                }
                if r.rpr.as_ref().is_some_and(|p| p.is_empty()) {
                    r.rpr = None;
                }
            }
        }
        true
    }

    /// Markers inside the paragraph (bookmarks, comment ranges, revisions' range marks):
    /// (local name, `w:id`, visible offset). Comment references are reported as
    /// `commentReference`.
    pub fn markers(&self) -> Vec<(String, String, usize)> {
        let mut out = Vec::new();
        let mut pos = 0;
        for s in &self.segs {
            match &s.kind {
                SegKind::Raw(x) => {
                    if let Some(t) = tokens(x).find(|t| matches!(t.kind, Kind::Start | Kind::Empty)) {
                        let id = xml::attr(&x[t.start..t.end], "w:id").unwrap_or("").to_string();
                        out.push((t.local().to_string(), id, pos));
                    }
                }
                SegKind::Run(r) => {
                    for c in &r.content {
                        if let RC::Raw(x) = c {
                            if let Some(t) = tokens(x).find(|t| t.local() == "commentReference") {
                                out.push(("commentReference".into(), xml::attr(&x[t.start..t.end], "w:id").unwrap_or("").to_string(), pos));
                            }
                        }
                    }
                }
            }
            pos += self.seg_len(s);
        }
        out
    }

    /// Insert a raw paragraph-level element (a marker such as `commentRangeStart`) at `off`.
    pub fn insert_marker(&mut self, off: usize, xml: &str) {
        let at = self.split_at(off.min(self.len()));
        self.segs.insert(at, Seg { kind: SegKind::Raw(xml.to_string()), wrap: vec![] });
    }

    /// Insert a comment reference run at `off`.
    pub fn insert_comment_reference(&mut self, off: usize, id: u32) {
        let at = self.split_at(off.min(self.len()));
        let run = Run {
            open: "<w:r>".into(),
            rpr: Some(Props::parse(r#"<w:rPr><w:rStyle w:val="CommentReference"/></w:rPr>"#)),
            content: vec![RC::Raw(format!(r#"<w:commentReference w:id="{id}"/>"#))],
        };
        self.segs.insert(at, Seg { kind: SegKind::Run(run), wrap: vec![] });
    }

    /// Remove a comment's range markers and reference from this paragraph.
    /// Returns whether anything was removed.
    pub fn remove_comment(&mut self, id: &str) -> bool {
        let before = self.segs.len();
        let has = |x: &str, names: &[&str]| tokens(x).any(|t| names.contains(&t.local()) && xml::attr(&x[t.start..t.end], "w:id") == Some(id));
        self.segs.retain(|s| match &s.kind {
            SegKind::Raw(x) => !has(x, &["commentRangeStart", "commentRangeEnd"]),
            SegKind::Run(r) => !r.content.iter().any(|c| matches!(c, RC::Raw(x) if has(x, &["commentReference"]))),
        });
        self.segs.len() != before
    }

    /// Split at `off` (Enter). `self` keeps the head; the returned paragraph gets the tail
    /// and the same paragraph properties (minus section break and change history).
    /// With `rev`, the paragraph mark is recorded as inserted.
    pub fn split(&mut self, off: usize, rev: Option<&Rev>) -> Para {
        let at = self.split_at(off.min(self.len()));
        let tail: Vec<Seg> = self.segs.split_off(at);
        let mut ppr = self.ppr.clone();
        if let Some(p) = ppr.as_mut() {
            p.set("sectPr", None);
            p.set("pPrChange", None);
            if let Some(r) = p.get("rPr").map(Props::parse) {
                let mut r = r;
                r.set("ins", None);
                r.set("del", None);
                p.set("rPr", if r.is_empty() { None } else { Some(r.to_xml()) });
            }
        }
        // The section break, if any, stays with the *last* paragraph of the section.
        if let Some(sect) = self.ppr.as_ref().and_then(|p| p.get("sectPr")).map(str::to_string) {
            if let Some(p) = self.ppr.as_mut() {
                p.set("sectPr", None);
            }
            ppr.get_or_insert_with(|| Props::new("w:pPr")).set("sectPr", Some(sect));
        }
        if let Some(r) = rev {
            let p = self.ppr.get_or_insert_with(|| Props::new("w:pPr"));
            let mut mark = p.get("rPr").map(Props::parse).unwrap_or_else(|| Props::new("w:rPr"));
            mark.set("ins", Some(format!("<w:ins {}/>", r.attrs())));
            p.set("rPr", Some(mark.to_xml()));
        }
        let open = strip_paragraph_ids(&self.open);
        Para { open, ppr, segs: tail, groups: self.groups.clone() }
    }

    /// Record this paragraph's mark as deleted (tracked join with the next paragraph).
    pub fn mark_deleted(&mut self, rev: &Rev) {
        let p = self.ppr.get_or_insert_with(|| Props::new("w:pPr"));
        let mut mark = p.get("rPr").map(Props::parse).unwrap_or_else(|| Props::new("w:rPr"));
        if mark.get("ins").is_some_and(|x| xml::attr(x, "w:author").map(xml::unescape).as_deref() == Some(rev.author.as_str())) {
            mark.set("ins", None); // undoing our own inserted mark
        } else {
            mark.set("del", Some(format!("<w:del {}/>", rev.attrs())));
        }
        p.set("rPr", if mark.is_empty() { None } else { Some(mark.to_xml()) });
    }

    /// Whether the paragraph mark is recorded as deleted.
    pub fn mark_is_deleted(&self) -> bool {
        self.ppr.as_ref().and_then(|p| p.get("rPr")).is_some_and(|r| Props::parse(r).get("del").is_some())
    }

    /// Run properties in effect at `off` (the run before the caret, else after).
    pub fn rpr_at(&self, off: usize) -> Option<Props> {
        self.style_source(off).and_then(|(r, _)| r).or_else(|| self.mark_rpr())
    }

    /// Join `next` onto the end of this paragraph (Backspace at the start of `next`).
    pub fn merge(&mut self, next: Para) {
        let base = self.groups.len();
        self.groups.extend(next.groups);
        for mut s in next.segs {
            for g in s.wrap.iter_mut() {
                *g += base;
            }
            self.segs.push(s);
        }
        // The merged paragraph ends with next's paragraph mark, so it takes its section break.
        if let Some(sect) = next.ppr.as_ref().and_then(|p| p.get("sectPr")) {
            self.ppr.get_or_insert_with(|| Props::new("w:pPr")).set("sectPr", Some(sect.to_string()));
        }
    }

    /// Serialize. Untouched runs keep their original run tag and properties.
    pub fn to_xml(&self) -> String {
        let mut s = self.open.clone();
        if let Some(p) = &self.ppr {
            s.push_str(&p.to_xml());
        }
        let mut stack: Vec<usize> = Vec::new();
        for seg in &self.segs {
            let common = stack.iter().zip(&seg.wrap).take_while(|(a, b)| a == b).count();
            while stack.len() > common {
                s.push_str(&self.groups[stack.pop().unwrap()].close);
            }
            for g in &seg.wrap[common..] {
                s.push_str(&self.groups[*g].open);
                stack.push(*g);
            }
            match &seg.kind {
                SegKind::Run(r) => s.push_str(&r.to_xml(seg.wrap.iter().any(|g| self.groups[*g].kind == "del"))),
                SegKind::Raw(x) => s.push_str(x),
            }
        }
        while let Some(g) = stack.pop() {
            s.push_str(&self.groups[g].close);
        }
        let close = format!("</{}>", self.open.trim_start_matches('<').split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or("w:p"));
        s.push_str(&close);
        s
    }
}

/// Word's paragraph ids must stay unique; a split-off paragraph drops them.
fn strip_paragraph_ids(open: &str) -> String {
    let mut s = open.to_string();
    for a in ["w14:paraId", "w14:textId"] {
        s = xml::set_attr(&s, a, None);
    }
    s
}

fn parse_run(src: &str, toks: &[Tok], i: usize, end: usize) -> Run {
    let t = toks[i];
    let open = if t.kind == Kind::Empty {
        src[t.start..t.end].trim_end_matches("/>").trim_end().to_string() + ">"
    } else {
        src[t.start..t.end].to_string()
    };
    let mut run = Run { open, rpr: None, content: vec![] };
    if t.kind == Kind::Empty {
        return run;
    }
    let mut j = i + 1;
    while j < toks.len() && toks[j].start < end {
        let c = toks[j];
        if !matches!(c.kind, Kind::Start | Kind::Empty) {
            j += 1;
            continue;
        }
        let e = element_end(toks, j);
        let raw = &src[c.start..e];
        match c.local() {
            "rPr" => run.rpr = Some(Props::parse(raw)),
            "t" | "delText" => run.content.push(RC::Text(if c.kind == Kind::Empty { String::new() } else { xml::unescape(xml::inner(src, toks, j)) })),
            "tab" => run.content.push(RC::Tab(raw.to_string())),
            "br" | "cr" => run.content.push(RC::Break(raw.to_string())),
            _ => run.content.push(RC::Raw(raw.to_string())),
        }
        while j < toks.len() && toks[j].start < e {
            j += 1;
        }
    }
    run
}
