//! Word processor backend (Quire): one open document, editing commands, undo,
//! layout for the page view, and patch-in-place saving.
//!
//! The document is the original package plus a list of top-level body blocks:
//! untouched ones point at the original element (saved byte for byte), edited
//! paragraphs are `ooxml::docx::Para`s. Every command goes through
//! [`Session::dispatch`], used by both the Tauri shell and the dev HTTP bridge.

pub mod autosave;
pub mod blank;
pub mod blocks;
pub mod edit;
#[cfg(unix)]
pub mod envcache;
pub mod layout;
pub mod spell;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ooxml::docx::append::{self, ListKind};
use ooxml::docx::props::{ParaFormat, Props, RunFormat};
use ooxml::docx::revisions::Filter;
use ooxml::docx::story::NoteKind;
use ooxml::docx::{Block, Body, Docx, DocxEdits, Para, Rev};

use blocks::{Blocks, Loc, Story};
use serde_json::{json, Value};

/// One top-level body element.
#[derive(Debug, Clone)]
pub enum B {
    /// The original element at this index of the baseline body.
    Orig(usize),
    /// A paragraph that has been edited (or created).
    Para(Para),
    /// Any other new or edited element (tables, ...), as XML.
    Xml(String),
}

#[derive(Clone)]
struct State {
    /// Package the blocks refer to (the opened file, or the last save).
    base: Arc<Docx>,
    blocks: Vec<B>,
    /// Edits other than the body (numbering, media, settings, ...).
    extra: DocxEdits,
}

pub struct Session {
    state: State,
    path: Option<PathBuf>,
    name: String,
    undo: Vec<State>,
    redo: Vec<State>,
    dirty: bool,
    track: bool,
    author: String,
    next_rev: u32,
    /// The last full layout and the state it was made from (for windowed relayout).
    cache: Option<LayoutCache>,
    /// Words the user added to the dictionary.
    user_words: std::collections::HashSet<String>,
    /// Identifies an unsaved document's recovery file and versions.
    session_id: String,
    /// When the last periodic version snapshot was taken (ms).
    last_version: u128,
}

struct LayoutCache {
    pages: Vec<Value>,
    images: serde_json::Map<String, Value>,
    blocks: Vec<B>,
    extra_key: String,
    /// Per page: the block that starts cleanly at the top of the page, if any.
    clean: Vec<Option<usize>>,
    /// Per block: the first page it appears on.
    first_page: Vec<Option<usize>>,
}

fn same_block(a: &B, b: &B) -> bool {
    match (a, b) {
        (B::Orig(x), B::Orig(y)) => x == y,
        (B::Para(x), B::Para(y)) => x == y,
        (B::Xml(x), B::Xml(y)) => x == y,
        _ => false,
    }
}

/// XML of block `i` in a container.
fn blocks_xml(b: &dyn Blocks, i: usize) -> Result<String, String> {
    b.xml(i).ok_or_else(|| "no such block".to_string())
}

/// The body's top-level blocks as a [`Blocks`] container.
struct BodyBlocks<'a> {
    blocks: &'a mut Vec<B>,
    base: &'a Docx,
}

impl Blocks for BodyBlocks<'_> {
    fn len(&self) -> usize {
        self.blocks.len()
    }
    fn kind(&self, i: usize) -> &'static str {
        match self.blocks.get(i) {
            Some(B::Orig(j)) => self.base.body().map(|b| match b.elements[*j].name { "p" => "p", "tbl" => "tbl", "sdt" => "sdt", _ => "other" }).unwrap_or("other"),
            Some(B::Para(_)) => "p",
            Some(B::Xml(x)) => blocks::kind_of(x),
            None => "other",
        }
    }
    fn para(&self, i: usize) -> Option<Para> {
        match self.blocks.get(i)? {
            B::Para(p) => Some(p.clone()),
            B::Orig(j) => {
                let body = self.base.body().ok()?;
                (body.elements[*j].name == "p").then(|| Para::parse(body.xml(*j)).ok()).flatten()
            }
            B::Xml(x) => (blocks::kind_of(x) == "p").then(|| Para::parse(x).ok()).flatten(),
        }
    }
    fn set_para(&mut self, i: usize, p: Para) {
        self.blocks[i] = B::Para(p);
    }
    fn insert_para(&mut self, i: usize, p: Para) {
        self.blocks.insert(i, B::Para(p));
    }
    fn insert_xml(&mut self, i: usize, xml: String) {
        self.blocks.insert(i, B::Xml(xml));
    }
    fn remove(&mut self, i: usize) {
        self.blocks.remove(i);
    }
    fn xml(&self, i: usize) -> Option<String> {
        Some(match self.blocks.get(i)? {
            B::Orig(j) => self.base.body().ok()?.xml(*j).to_string(),
            B::Para(p) => p.to_xml(),
            B::Xml(x) => x.clone(),
        })
    }
}

fn loc(v: &Value) -> Result<Loc, String> {
    Loc::parse(v)
}

/// UTC timestamp for tracked changes, e.g. `2026-10-18T10:00:00Z`.
fn now_iso() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn run_format(v: &Value) -> RunFormat {
    let opt_str = |k: &str| v.get(k).map(|x| x.as_str().filter(|s| !s.is_empty()).map(str::to_string));
    RunFormat {
        bold: v["bold"].as_bool(),
        italic: v["italic"].as_bool(),
        underline: v["underline"].as_bool(),
        strike: v["strike"].as_bool(),
        size_half_pt: v.get("size").map(|x| x.as_f64().map(|pt| (pt * 2.0).round() as u32)),
        font: opt_str("font"),
        color: opt_str("color").map(|c| c.map(|s| s.trim_start_matches('#').to_uppercase())),
        highlight: opt_str("highlight"),
        vert_align: opt_str("vert_align"),
        style: None,
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        let bytes = blank::document(&[]);
        let docx = Docx::open(&bytes).expect("blank document");
        let mut s = Session {
            state: State { blocks: vec![], base: Arc::new(docx), extra: DocxEdits::default() },
            path: None,
            name: "Untitled".into(),
            undo: vec![],
            redo: vec![],
            dirty: false,
            track: false,
            author: std::env::var("QUIRE_AUTHOR").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "Author".into()),
            next_rev: 1,
            cache: None,
            user_words: Default::default(),
            session_id: format!("{:x}", autosave::now_ms()),
            last_version: 0,
        };
        s.load(bytes).expect("blank document");
        s
    }

    fn load(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        let docx = Docx::open(&bytes)?;
        let n = docx.body()?.elements.len();
        self.next_rev = self.next_rev.max(docx.max_annotation_id() + 1);
        self.track = docx.read_part("word/settings.xml").map(|s| s.contains("<w:trackRevisions")).unwrap_or(false);
        self.state = State { blocks: (0..n).map(B::Orig).collect(), base: Arc::new(docx), extra: DocxEdits::default() };
        self.cache = None;
        Ok(())
    }

    pub fn open(&mut self, path: &Path) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        self.load(bytes)?;
        self.path = Some(path.to_path_buf());
        self.name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.undo.clear();
        self.redo.clear();
        self.dirty = false;
        Ok(())
    }

    // ------------------------------------------------------------------ model helpers

    fn block_name(&self, b: usize) -> &str {
        match &self.state.blocks[b] {
            B::Orig(i) => self.state.base.body().map(|body| body.elements[*i].name).map(|n| match n {
                "p" => "p", "tbl" => "tbl", "sdt" => "sdt", _ => "other",
            }).unwrap_or("other"),
            B::Para(_) => "p",
            B::Xml(x) => if x.trim_start().starts_with("<w:tbl") { "tbl" } else { "other" },
        }
    }

    /// The paragraph at `b` (parsed), if that block is a paragraph.
    pub fn para(&self, b: usize) -> Option<Para> {
        match self.state.blocks.get(b)? {
            B::Para(p) => Some(p.clone()),
            B::Orig(i) => {
                let body = self.state.base.body().ok()?;
                (body.elements[*i].name == "p").then(|| Para::parse(body.xml(*i)).ok()).flatten()
            }
            B::Xml(x) => Para::parse(x).ok(),
        }
    }

    fn edits(&self) -> DocxEdits {
        let mut e = self.state.extra.clone();
        e.body = Some(self.state.blocks.iter().map(|b| match b {
            B::Orig(i) => Block::Orig(*i),
            B::Para(p) => Block::Xml(p.to_xml()),
            B::Xml(x) => Block::Xml(x.clone()),
        }).collect());
        e
    }

    /// The document as it would be saved now.
    pub fn bytes(&self) -> Result<Vec<u8>, String> {
        self.state.base.save(&self.edits())
    }

    fn checkpoint(&mut self) {
        self.undo.push(self.state.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
    }

    fn rev(&self) -> Option<Rev> {
        self.track.then(|| Rev::new(&self.author, &now_iso(), self.next_rev))
    }

    fn done_rev(&mut self, rev: Option<Rev>) {
        if let Some(r) = rev {
            self.next_rev = r.next_id();
        }
    }

    /// XML of a top-level body block.
    fn block_xml(&self, b: usize) -> String {
        match &self.state.blocks[b] {
            B::Orig(i) => self.state.base.body().map(|body| body.xml(*i).to_string()).unwrap_or_default(),
            B::Para(p) => p.to_xml(),
            B::Xml(x) => x.clone(),
        }
    }

    /// Blocks of a story's top level, as XML.
    fn story_blocks(&self, story: &Story) -> Result<Vec<String>, String> {
        let base = &self.state.base;
        let extra = &self.state.extra;
        let (src, container) = match story {
            Story::Body => return Ok((0..self.state.blocks.len()).map(|b| self.block_xml(b)).collect()),
            Story::Part(p) => {
                let src = base.current(extra, p).ok_or_else(|| format!("{p}: no such part"))?;
                let root = ooxml::xml::tokens(&src).find(|t| matches!(t.kind, ooxml::xml::Kind::Start | ooxml::xml::Kind::Empty)).map(|t| t.local().to_string()).unwrap_or_default();
                (src, root)
            }
            Story::Footnote(id) | Story::Endnote(id) => {
                let kind = if matches!(story, Story::Footnote(_)) { NoteKind::Footnote } else { NoteKind::Endnote };
                let note = base.notes(extra, kind).into_iter().find(|n| n.id == *id).ok_or("no such note")?;
                (note.xml, kind.element().to_string())
            }
            Story::Comment(id) => (self.comment_xml(*id)?, "comment".to_string()),
        };
        let body = Body::parse_in(&src, &container)?;
        Ok((0..body.elements.len()).map(|i| body.xml(i).to_string()).collect())
    }

    fn comment_xml(&self, id: i64) -> Result<String, String> {
        let src = self.state.base.current(&self.state.extra, "word/comments.xml").ok_or("no comments")?;
        let start = src.find(&format!(r#"<w:comment w:id="{id}""#)).ok_or("no such comment")?;
        let end = src[start..].find("</w:comment>").map(|e| start + e + "</w:comment>".len()).ok_or("bad comment")?;
        Ok(src[start..end].to_string())
    }

    fn write_story_blocks(&mut self, story: &Story, blocks: Vec<String>) -> Result<(), String> {
        let base = self.state.base.clone();
        let extra = &mut self.state.extra;
        match story {
            Story::Body => unreachable!("body blocks are edited in place"),
            Story::Part(p) => base.edit_header_footer(extra, p, &blocks.into_iter().map(Block::Xml).collect::<Vec<_>>()),
            Story::Footnote(id) => base.set_note_blocks(extra, NoteKind::Footnote, *id, &blocks.concat()),
            Story::Endnote(id) => base.set_note_blocks(extra, NoteKind::Endnote, *id, &blocks.concat()),
            Story::Comment(id) => {
                let src = base.current(extra, "word/comments.xml").ok_or("no comments")?;
                let start = src.find(&format!(r#"<w:comment w:id="{id}""#)).ok_or("no such comment")?;
                let open_end = src[start..].find('>').map(|e| start + e + 1).ok_or("bad comment")?;
                let end = src[start..].find("</w:comment>").map(|e| start + e).ok_or("bad comment")?;
                let new = format!("{}{}{}", &src[..open_end], blocks.concat(), &src[end..]);
                extra.parts.insert("word/comments.xml".into(), new.into_bytes());
                Ok(())
            }
        }
    }

    /// Run `f` on the container holding `at` (any story, any depth).
    fn with_container<R>(&mut self, at: &Loc, f: &mut dyn FnMut(&mut dyn Blocks) -> Result<R, String>) -> Result<R, String> {
        let (story, prefix) = at.container();
        let (story, prefix) = (story.clone(), prefix.to_vec());
        if story == Story::Body {
            if prefix.is_empty() {
                let mut bb = BodyBlocks { blocks: &mut self.state.blocks, base: &self.state.base };
                return f(&mut bb);
            }
            let b0 = prefix[0].max(0) as usize;
            let xml = self.block_xml(b0);
            let (new, r) = blocks::edit_in_element(&xml, &prefix[1..], f)?;
            self.state.blocks[b0] = match Para::parse(&new) {
                Ok(p) if blocks::kind_of(&new) == "p" => B::Para(p),
                _ => B::Xml(new),
            };
            return Ok(r);
        }
        let mut list = self.story_blocks(&story)?;
        let r = if prefix.is_empty() {
            f(&mut blocks::StrBlocks(&mut list))?
        } else {
            let i = prefix[0].max(0) as usize;
            let el = list.get(i).ok_or("path points past the story")?.clone();
            let (new, r) = blocks::edit_in_element(&el, &prefix[1..], f)?;
            list[i] = new;
            r
        };
        self.write_story_blocks(&story, list)?;
        Ok(r)
    }

    /// Read-only access to a container (cloned, then dropped).
    fn read_container<R>(&self, at: &Loc, f: &mut dyn FnMut(&dyn Blocks) -> R) -> Result<R, String> {
        let (story, prefix) = at.container();
        let mut list = if *story == Story::Body && prefix.is_empty() {
            (0..self.state.blocks.len()).map(|b| self.block_xml(b)).collect()
        } else {
            self.story_blocks(story)?
        };
        if prefix.is_empty() {
            return Ok(f(&blocks::StrBlocks(&mut list)));
        }
        let i = prefix[0].max(0) as usize;
        let el = list.get(i).ok_or("path points past the story")?.clone();
        let mut out = None;
        blocks::edit_in_element(&el, &prefix[1..], &mut |b: &mut dyn Blocks| { out = Some(f(&*b)); Ok(()) })?;
        out.ok_or_else(|| "empty container".to_string())
    }

    pub fn para_at(&self, at: &Loc) -> Option<Para> {
        if at.story == Story::Body && at.path.len() == 1 {
            return self.para(at.idx());
        }
        self.read_container(at, &mut |b| b.para(at.idx())).ok().flatten()
    }

    // ------------------------------------------------------------------ editing

    /// Next-style lookups need the styles part; take a snapshot so edits can borrow mutably.
    fn style_snapshot(&self) -> impl Fn(&str) -> Option<String> {
        let styles = self.state.base.current(&self.state.extra, "word/styles.xml").unwrap_or_default();
        move |id: &str| {
            let start = styles.find(&format!(r#"w:styleId="{id}""#))?;
            let end = styles[start..].find("</w:style>").map(|e| start + e)?;
            let block = &styles[start..end];
            let i = block.find("<w:next ")?;
            let next = ooxml::xml::attr(&block[i..block[i..].find('>').map(|e| i + e + 1)?], "w:val")?.to_string();
            (next != id).then_some(next)
        }
    }

    /// One editing command: checkpoint, run on the container, track revision ids.
    fn edit_at<R>(&mut self, at: &Loc, f: &mut dyn FnMut(&mut dyn Blocks, Option<&Rev>) -> Result<R, String>) -> Result<R, String> {
        self.checkpoint();
        let rev = self.rev();
        let r = self.with_container(at, &mut |b: &mut dyn Blocks| f(b, rev.as_ref()));
        if r.is_err() {
            // Nothing changed: drop the checkpoint we just took.
            if let Some(prev) = self.undo.pop() {
                self.state = prev;
            }
        }
        self.done_rev(rev);
        r
    }

    pub fn insert_text(&mut self, at: Loc, text: &str) -> Result<Loc, String> {
        self.insert_formatted(at, text, None)
    }

    /// Insert `text` (newlines start new paragraphs), optionally with extra run formatting
    /// (Bold toggled at a collapsed caret applies to what's typed next).
    pub fn insert_formatted(&mut self, at: Loc, text: &str, fmt: Option<&RunFormat>) -> Result<Loc, String> {
        let next = self.style_snapshot();
        let (i, o) = self.edit_at(&at, &mut |b, rev| edit::insert(b, (at.idx(), at.o), text, fmt, rev, &next))?;
        Ok(at.with(i, o))
    }

    pub fn split(&mut self, at: Loc) -> Result<Loc, String> {
        let next = self.style_snapshot();
        let (i, o) = self.edit_at(&at, &mut |b, rev| edit::split(b, (at.idx(), at.o), rev, &next))?;
        Ok(at.with(i, o))
    }

    /// Delete between two positions; they must be in the same container.
    pub fn delete(&mut self, a: Loc, z: Loc) -> Result<Loc, String> {
        if !a.same_container(&z) {
            return Err("select within one table cell, header or footnote to delete".into());
        }
        let (i, o) = self.edit_at(&a, &mut |b, rev| edit::delete(b, (a.idx(), a.o), (z.idx(), z.o), rev))?;
        Ok(a.with(i, o))
    }

    pub fn format(&mut self, a: Loc, z: Loc, fmt: &RunFormat) -> Result<(), String> {
        if !a.same_container(&z) {
            return Err("select within one container to format".into());
        }
        self.edit_at(&a, &mut |b, rev| edit::format(b, (a.idx(), a.o), (z.idx(), z.o), fmt, rev))
    }

    pub fn para_format(&mut self, a: Loc, z: Loc, v: &Value) -> Result<(), String> {
        let mut fmt = ParaFormat::default();
        let base = self.state.base.clone();
        if let Some(s) = v.get("style") {
            let id = s.as_str().filter(|s| !s.is_empty() && *s != "Normal").map(str::to_string);
            if let Some(id) = &id {
                append::ensure_builtin_style(&base, &mut self.state.extra, id);
            }
            fmt.style = Some(id);
        }
        if let Some(a) = v.get("align") {
            fmt.align = Some(a.as_str().filter(|s| !s.is_empty() && *s != "left").map(str::to_string));
        }
        if let Some(l) = v.get("list").and_then(|l| l.as_str()) {
            fmt.numbering = Some(match l {
                "bullet" | "number" => {
                    let id = append::add_list(&base, &mut self.state.extra, if l == "bullet" { ListKind::Bullet } else { ListKind::Decimal });
                    Some(append::num_pr(id, 0))
                }
                _ => None,
            });
            if l == "bullet" || l == "number" {
                fmt.style = Some(Some("ListParagraph".into()));
            } else if fmt.style.is_none() {
                fmt.style = Some(None);
            }
        }
        if let Some(sp) = v.get("spacing").filter(|x| x.is_object()) {
            // Line spacing as a multiple (1, 1.15, 1.5, 2) and space before/after in points.
            let mut tag = "<w:spacing/>".to_string();
            if let Some(m) = sp["line"].as_f64() {
                tag = ooxml::xml::set_attr(&tag, "w:line", Some(&((m * 240.0).round() as i64).to_string()));
                tag = ooxml::xml::set_attr(&tag, "w:lineRule", Some("auto"));
            }
            for (k, attr) in [("before", "w:before"), ("after", "w:after")] {
                if let Some(pt) = sp[k].as_f64() {
                    tag = ooxml::xml::set_attr(&tag, attr, Some(&((pt * 20.0).round() as i64).to_string()));
                }
            }
            fmt.spacing = Some(Some(tag));
        }
        let indent = v.get("indent").and_then(|d| d.as_i64());
        self.edit_at(&a, &mut |b, rev| edit::para_format(b, a.idx(), z.idx(), &fmt, indent, rev))
    }

    fn insert_object(&mut self, at: Loc, content: &str) -> Result<Loc, String> {
        let (i, o) = self.edit_at(&at, &mut |b, rev| edit::insert_object(b, (at.idx(), at.o), content, None, rev))?;
        Ok(at.with(i, o))
    }

    fn insert_table(&mut self, at: Loc, rows: usize, cols: usize) -> Result<Loc, String> {
        let base = self.state.base.clone();
        append::ensure_builtin_style(&base, &mut self.state.extra, "TableGrid");
        let w = 9360 / cols.max(1);
        let grid: String = (0..cols).map(|_| format!(r#"<w:gridCol w:w="{w}"/>"#)).collect();
        let row: String = format!("<w:tr>{}</w:tr>", (0..cols).map(|_| format!(r#"<w:tc><w:tcPr><w:tcW w:w="{w}" w:type="dxa"/></w:tcPr><w:p/></w:tc>"#)).collect::<String>());
        let tbl = format!(r#"<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="0" w:type="auto"/><w:tblLook w:val="04A0" w:firstRow="1" w:lastRow="0" w:firstColumn="1" w:lastColumn="0" w:noHBand="0" w:noVBand="1"/></w:tblPr><w:tblGrid>{grid}</w:tblGrid>{}</w:tbl>"#, row.repeat(rows.max(1)));
        let next = self.style_snapshot();
        let (i, _) = self.edit_at(&at, &mut |b, _| edit::insert_block(b, (at.idx(), at.o), tbl.clone(), &next))?;
        // Caret into the table's first cell.
        let mut path = at.path.clone();
        *path.last_mut().unwrap() = i as i64 - 1;
        path.extend([0, 0, 0]);
        Ok(Loc { story: at.story.clone(), path, o: 0 })
    }

    /// Table tools at a caret inside a table cell (`[..., table, row, cell, block]`).
    pub fn table_op(&mut self, at: Loc, op: &str, color: Option<&str>) -> Result<Loc, String> {
        if at.path.len() < 4 {
            return Err("put the caret in a table first".into());
        }
        let n = at.path.len();
        let (r, c) = (at.path[n - 3] as usize, at.path[n - 2] as usize);
        let table_loc = Loc { story: at.story.clone(), path: at.path[..n - 3].to_vec(), o: 0 };
        let ti = table_loc.idx();
        let op = op.to_string();
        let color = color.map(str::to_string);
        let mut caret = at.clone();
        let result = self.edit_at(&table_loc, &mut |b, _rev| {
            if b.kind(ti) != "tbl" {
                return Err("not a table".into());
            }
            let x = blocks_xml(b, ti)?;
            let mut t = ooxml::docx::table::Table::parse(&x)?;
            let ok = match op.as_str() {
                "row_above" => t.insert_row_before(r),
                "row_below" => t.insert_row_after(r),
                "col_left" => t.insert_col(r, c, false),
                "col_right" => t.insert_col(r, c, true),
                "delete_row" => t.delete_row(r),
                "delete_col" => t.delete_col(r, c),
                "merge" => t.merge_right(r, c),
                "shade" => t.shade(r, c, color.as_deref().filter(|c| !c.is_empty())),
                "delete_table" => {
                    b.remove(ti);
                    if b.len() == 0 || b.kind(ti.min(b.len() - 1)) != "p" {
                        b.insert_para(ti.min(b.len()), Para::new("", None, None));
                    }
                    return Ok(true);
                }
                _ => return Err(format!("unknown table action {op}")),
            };
            if !ok {
                return Err("that can't be done to this table".into());
            }
            b.remove(ti);
            b.insert_xml(ti, t.to_xml());
            Ok(false)
        })?;
        if result {
            caret = Loc { story: at.story.clone(), path: table_loc.path.clone(), o: 0 };
        } else if op == "row_below" {
            caret.path[n - 3] += 1;
            caret.path[n - 1] = 0;
            caret.o = 0;
        } else if op == "col_right" {
            caret.path[n - 2] += 1;
            caret.path[n - 1] = 0;
            caret.o = 0;
        } else if op == "delete_row" && r > 0 {
            caret.path[n - 3] -= 1;
            caret.path[n - 1] = 0;
            caret.o = 0;
        } else if matches!(op.as_str(), "delete_col") && c > 0 {
            caret.path[n - 2] -= 1;
            caret.path[n - 1] = 0;
            caret.o = 0;
        }
        Ok(caret)
    }

    /// Page size, orientation, margins and columns for every section.
    pub fn page_setup(&mut self, v: &Value) -> Result<(), String> {
        self.checkpoint();
        let apply = |sect: &str| -> String {
            let mut p = Props::parse(if sect.is_empty() { "<w:sectPr/>" } else { sect });
            if p.tag.is_empty() { p = Props::new("w:sectPr"); }
            let cur = p.get("pgSz").map(str::to_string).unwrap_or_else(|| r#"<w:pgSz w:w="12240" w:h="15840"/>"#.into());
            let (mut w, mut h): (i64, i64) = (ooxml::xml::attr(&cur, "w:w").and_then(|x| x.parse().ok()).unwrap_or(12240), ooxml::xml::attr(&cur, "w:h").and_then(|x| x.parse().ok()).unwrap_or(15840));
            let mut landscape = ooxml::xml::attr(&cur, "w:orient") == Some("landscape") || w > h;
            if let Some(size) = v["size"].as_str() {
                let (a, b) = match size { "a4" => (11906, 16838), "legal" => (12240, 20160), "a5" => (8391, 11906), "tabloid" => (15840, 24480), _ => (12240, 15840) };
                (w, h) = if landscape { (b, a) } else { (a, b) };
            }
            if let Some(o) = v["orientation"].as_str() {
                let want = o == "landscape";
                if want != landscape { std::mem::swap(&mut w, &mut h); }
                landscape = want;
            }
            p.set("pgSz", Some(if landscape { format!(r#"<w:pgSz w:w="{w}" w:h="{h}" w:orient="landscape"/>"#) } else { format!(r#"<w:pgSz w:w="{w}" w:h="{h}"/>"#) }));
            if let Some(m) = v["margins"].as_str() {
                let (tb, lr) = match m { "narrow" => (720, 720), "moderate" => (1440, 1080), "wide" => (1440, 2880), _ => (1440, 1440) };
                let old = p.get("pgMar").map(str::to_string).unwrap_or_default();
                let keep = |k: &str, d: &str| ooxml::xml::attr(&old, k).unwrap_or(d).to_string();
                p.set("pgMar", Some(format!(r#"<w:pgMar w:top="{tb}" w:right="{lr}" w:bottom="{tb}" w:left="{lr}" w:header="{}" w:footer="{}" w:gutter="{}"/>"#, keep("w:header", "720"), keep("w:footer", "720"), keep("w:gutter", "0"))));
            }
            if let Some(n) = v["cols"].as_u64() {
                p.set("cols", Some(if n > 1 { format!(r#"<w:cols w:num="{n}" w:space="720"/>"#) } else { r#"<w:cols w:space="720"/>"#.to_string() }));
            }
            p.to_xml()
        };
        let final_sect = self.state.extra.sect_pr.clone().or_else(|| self.state.base.body().ok().and_then(|b| b.sect_pr_xml().map(str::to_string))).unwrap_or_default();
        self.state.extra.sect_pr = Some(apply(&final_sect));
        for b in 0..self.state.blocks.len() {
            if let Some(mut p) = self.para(b) {
                if let Some(sp) = p.ppr.as_ref().and_then(|x| x.get("sectPr")).map(str::to_string) {
                    p.ppr.as_mut().unwrap().set("sectPr", Some(apply(&sp)));
                    self.state.blocks[b] = B::Para(p);
                }
            }
        }
        Ok(())
    }

    /// Current page setup of the last section (for the UI).
    fn page_info(&self) -> Value {
        let sect = self.state.extra.sect_pr.clone().or_else(|| self.state.base.body().ok().and_then(|b| b.sect_pr_xml().map(str::to_string))).unwrap_or_default();
        let p = Props::parse(if sect.is_empty() { "<w:sectPr/>" } else { &sect });
        let sz = p.get("pgSz").unwrap_or("");
        let (w, h): (i64, i64) = (ooxml::xml::attr(sz, "w:w").and_then(|x| x.parse().ok()).unwrap_or(12240), ooxml::xml::attr(sz, "w:h").and_then(|x| x.parse().ok()).unwrap_or(15840));
        let (a, b) = (w.min(h), w.max(h));
        let size = match (a, b) { (11906, 16838) => "a4", (12240, 20160) => "legal", (8391, 11906) => "a5", (15840, 24480) => "tabloid", _ => "letter" };
        let mar = p.get("pgMar").unwrap_or("");
        let l: i64 = ooxml::xml::attr(mar, "w:left").and_then(|x| x.parse().ok()).unwrap_or(1440);
        let t: i64 = ooxml::xml::attr(mar, "w:top").and_then(|x| x.parse().ok()).unwrap_or(1440);
        let margins = match (t, l) { (720, 720) => "narrow", (1440, 1080) => "moderate", (1440, 2880) => "wide", _ => "normal" };
        let cols = p.get("cols").and_then(|x| ooxml::xml::attr(x, "w:num")).and_then(|v| v.parse::<u64>().ok()).unwrap_or(1);
        json!({"size": size, "orientation": if w > h { "landscape" } else { "portrait" }, "margins": margins, "cols": cols})
    }

    pub fn find(&self, q: &str, case: bool) -> Vec<Value> {
        if q.is_empty() {
            return vec![];
        }
        let fold = |s: &str| if case { s.to_string() } else { s.to_lowercase() };
        let needle: Vec<char> = fold(q).chars().collect();
        let mut out = Vec::new();
        for (story, path, text) in self.all_paragraphs() {
            let hay: Vec<char> = fold(&text).chars().collect();
            let mut i = 0;
            while i + needle.len() <= hay.len() {
                if hay[i..i + needle.len()] == needle[..] {
                    out.push(json!({"s": story.key(), "p": path, "o": i, "len": needle.len()}));
                    i += needle.len();
                } else {
                    i += 1;
                }
            }
        }
        out
    }

    /// Every paragraph in the body (tables and text boxes included), headers, footers
    /// and notes: (story, path, visible text), in document order per story.
    pub fn all_paragraphs(&self) -> Vec<(Story, Vec<i64>, String)> {
        let mut out = Vec::new();
        let body: Vec<String> = (0..self.state.blocks.len()).map(|b| self.block_xml(b)).collect();
        let mut paras = Vec::new();
        blocks::collect_paragraphs(&body, &[], &mut paras);
        out.extend(paras.into_iter().map(|(p, t)| (Story::Body, p, t)));
        for story in self.other_stories() {
            if let Ok(list) = self.story_blocks(&story) {
                let mut paras = Vec::new();
                blocks::collect_paragraphs(&list, &[], &mut paras);
                out.extend(paras.into_iter().map(|(p, t)| (story.clone(), p, t)));
            }
        }
        out
    }

    /// Every paragraph's XML (body incl. tables and text boxes, headers, footers, notes).
    fn all_paragraph_xml(&self) -> Vec<(Story, Vec<i64>, String)> {
        let mut out = Vec::new();
        let body: Vec<String> = (0..self.state.blocks.len()).map(|b| self.block_xml(b)).collect();
        let mut paras = Vec::new();
        blocks::collect_paragraph_xml(&body, &[], &mut paras);
        out.extend(paras.into_iter().map(|(p, x)| (Story::Body, p, x)));
        for story in self.other_stories() {
            if let Ok(list) = self.story_blocks(&story) {
                let mut paras = Vec::new();
                blocks::collect_paragraph_xml(&list, &[], &mut paras);
                out.extend(paras.into_iter().map(|(p, x)| (story.clone(), p, x)));
            }
        }
        out
    }

    /// Where each comment is anchored: id -> (start, end).
    fn comment_anchors(&self) -> std::collections::HashMap<String, (Loc, Loc)> {
        let mut starts = std::collections::HashMap::new();
        let mut ends = std::collections::HashMap::new();
        for (story, path, xml) in self.all_paragraph_xml() {
            if !xml.contains("comment") { continue; }
            let Ok(p) = Para::parse(&xml) else { continue };
            for (name, id, off) in p.markers() {
                let l = Loc { story: story.clone(), path: path.clone(), o: off };
                match name.as_str() {
                    "commentRangeStart" => { starts.entry(id).or_insert(l); }
                    "commentRangeEnd" => { ends.insert(id, l); }
                    "commentReference" => { ends.entry(id.clone()).or_insert(l.clone()); starts.entry(id).or_insert(l); }
                    _ => {}
                }
            }
        }
        starts.into_iter().map(|(id, s)| { let e = ends.get(&id).cloned().unwrap_or_else(|| s.clone()); (id, (s, e)) }).collect()
    }

    fn comments_json(&self) -> Value {
        let anchors = self.comment_anchors();
        json!(self.state.base.comments(&self.state.extra).into_iter().map(|c| {
            let a = anchors.get(&c.id.to_string());
            json!({"id": c.id, "author": c.author, "initials": c.initials, "date": c.date, "text": c.text,
                   "parent": c.parent, "done": c.done, "from": a.map(|x| x.0.json()), "to": a.map(|x| x.1.json())})
        }).collect::<Vec<_>>())
    }

    /// Add a comment on `from..to` (or a reply to `parent`, anchored like its parent).
    pub fn add_comment(&mut self, from: Loc, to: Loc, text: &str, parent: Option<i64>) -> Result<i64, String> {
        let (from, to) = match parent {
            Some(p) => self.comment_anchors().get(&p.to_string()).cloned().ok_or("the comment being replied to has no anchor")?,
            None => (from, to),
        };
        if !from.same_container(&to) {
            return Err("comment on text within one container".into());
        }
        let id = self.next_rev as i64;
        self.next_rev += 1;
        let initials: String = self.author.split_whitespace().filter_map(|w| w.chars().next()).collect::<String>().to_uppercase();
        let base = self.state.base.clone();
        self.checkpoint();
        base.add_comment(&mut self.state.extra, id, &self.author, &initials, &now_iso(), text, parent)?;
        let (a, z) = if (from.idx(), from.o) <= (to.idx(), to.o) { (from, to) } else { (to, from) };
        let idu = id as u32;
        self.with_container(&a, &mut |b: &mut dyn Blocks| {
            // End first, so the start offset stays valid when both are in one paragraph.
            let mut last = b.para(z.idx()).ok_or("not a paragraph")?;
            last.insert_comment_reference(z.o, idu);
            last.insert_marker(z.o, &format!(r#"<w:commentRangeEnd w:id="{idu}"/>"#));
            b.set_para(z.idx(), last);
            let mut first = b.para(a.idx()).ok_or("not a paragraph")?;
            first.insert_marker(a.o, &format!(r#"<w:commentRangeStart w:id="{idu}"/>"#));
            b.set_para(a.idx(), first);
            Ok(())
        })?;
        Ok(id)
    }

    /// Delete a comment and its replies, with their anchors.
    pub fn delete_comment(&mut self, id: i64) -> Result<(), String> {
        let all = self.state.base.comments(&self.state.extra);
        let mut ids = vec![id];
        let mut grew = true;
        while grew {
            grew = false;
            for c in &all {
                if c.parent.is_some_and(|p| ids.contains(&p)) && !ids.contains(&c.id) {
                    ids.push(c.id);
                    grew = true;
                }
            }
        }
        self.checkpoint();
        let base = self.state.base.clone();
        base.remove_comments(&mut self.state.extra, &ids)?;
        let keys: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
        for (story, path, xml) in self.all_paragraph_xml() {
            if !keys.iter().any(|k| xml.contains(&format!(r#"w:id="{k}""#))) { continue; }
            let l = Loc { story, path, o: 0 };
            let keys = keys.clone();
            self.with_container(&l, &mut |b: &mut dyn Blocks| {
                if let Some(mut p) = b.para(l.idx()) {
                    let mut changed = false;
                    for k in &keys { changed |= p.remove_comment(k); }
                    if changed { b.set_para(l.idx(), p); }
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    /// The paragraph holding revision `id` (for jumping to a change).
    fn revision_loc(&self, id: &str) -> Option<Loc> {
        let needle = format!(r#"w:id="{id}""#);
        self.all_paragraph_xml().into_iter().find(|(_, _, x)| x.contains(&needle)).map(|(story, path, _)| Loc { story, path, o: 0 })
    }

    /// Headings in the body (top-level paragraphs with an outline level): (block, level, text).
    pub fn outline(&self) -> Vec<(usize, u8, String)> {
        let styles = self.state.base.current(&self.state.extra, "word/styles.xml").unwrap_or_default();
        let levels = append::outline_levels(&styles);
        (0..self.state.blocks.len()).filter_map(|b| {
            let p = self.para(b)?;
            let ppr = p.ppr.as_ref();
            let direct = ppr.and_then(|x| x.get("outlineLvl")).and_then(|x| ooxml::xml::attr(x, "w:val")).and_then(|v| v.parse::<u8>().ok());
            let style = ppr.and_then(|x| x.get("pStyle")).and_then(|x| ooxml::xml::attr(x, "w:val").map(str::to_string));
            let lvl = direct.or_else(|| style.as_ref().and_then(|s| levels.get(s).copied())).filter(|l| *l < 9)?;
            let text = p.text().trim().to_string();
            (!text.is_empty()).then_some((b, lvl, text))
        }).collect()
    }

    /// Table of contents (as an XML block) for headings down to `depth` levels, with page
    /// numbers from the last full layout.
    fn toc_xml(&mut self, title: &str, depth: u8) -> Result<String, String> {
        if self.cache.is_none() {
            self.full_layout()?;
        }
        let first_page = self.cache.as_ref().map(|c| c.first_page.clone()).unwrap_or_default();
        let sect = self.state.extra.sect_pr.clone().or_else(|| self.state.base.body().ok().and_then(|b| b.sect_pr_xml().map(str::to_string))).unwrap_or_default();
        let sp = Props::parse(if sect.is_empty() { "<w:sectPr/>" } else { &sect });
        let num = |el: &str, a: &str, d: i64| sp.get(el).and_then(|x| ooxml::xml::attr(x, a)).and_then(|v| v.parse::<i64>().ok()).unwrap_or(d);
        let tab = num("pgSz", "w:w", 12240) - num("pgMar", "w:left", 1440) - num("pgMar", "w:right", 1440);
        let heads: Vec<_> = self.outline().into_iter().filter(|(_, l, _)| *l < depth).collect();
        let esc = ooxml::xml::escape;
        let mut out = format!(r#"<w:sdt><w:sdtPr><w:docPartObj><w:docPartGallery w:val="Table of Contents"/><w:docPartUnique/></w:docPartObj></w:sdtPr><w:sdtContent><w:p><w:pPr><w:pStyle w:val="TOCHeading"/></w:pPr><w:r><w:t>{}</w:t></w:r></w:p>"#, esc(title));
        let begin = format!(r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> TOC \o "1-{depth}" \h \z \u </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>"#);
        if heads.is_empty() {
            out.push_str(&format!(r#"<w:p>{begin}<w:r><w:t>No headings found. Use the Heading styles, then update the table.</w:t></w:r></w:p>"#));
        }
        for (i, (b, lvl, text)) in heads.iter().enumerate() {
            let page = first_page.get(*b).copied().flatten().map(|p| p + 1).unwrap_or(1);
            out.push_str(&format!(
                r#"<w:p><w:pPr><w:pStyle w:val="TOC{}"/><w:tabs><w:tab w:val="right" w:leader="dot" w:pos="{tab}"/></w:tabs></w:pPr>{}<w:r><w:t xml:space="preserve">{}</w:t></w:r><w:r><w:tab/></w:r><w:r><w:t>{page}</w:t></w:r></w:p>"#,
                lvl + 1, if i == 0 { begin.as_str() } else { "" }, esc(text)));
        }
        out.push_str(r#"<w:p><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p></w:sdtContent></w:sdt>"#);
        let base = self.state.base.clone();
        for st in ["TOCHeading", "TOC1", "TOC2", "TOC3"] {
            append::ensure_builtin_style(&base, &mut self.state.extra, st);
        }
        Ok(out)
    }

    fn toc_block(&self) -> Option<usize> {
        (0..self.state.blocks.len()).find(|&b| {
            let x = self.block_xml(b);
            blocks::kind_of(&x) == "sdt" && x.contains(r#"w:docPartGallery w:val="Table of Contents""#)
        })
    }

    /// Insert a table of contents at the caret (top-level body), then refresh its page numbers.
    pub fn toc_insert(&mut self, at: Loc) -> Result<Loc, String> {
        if at.story != Story::Body || at.path.len() != 1 {
            return Err("insert a table of contents in the body text".into());
        }
        let xml = self.toc_xml("Contents", 3)?;
        let next = self.style_snapshot();
        self.edit_at(&at, &mut |b, _| edit::insert_block(b, (at.idx(), at.o), xml.clone(), &next))?;
        // The table itself moves headings down; lay out again and renumber.
        self.full_layout()?;
        self.toc_update_inner()?;
        let b = self.toc_block().unwrap_or(0);
        Ok(Loc::body(b + 1, 0))
    }

    fn toc_update_inner(&mut self) -> Result<(), String> {
        let b = self.toc_block().ok_or("this document has no table of contents")?;
        let old = self.block_xml(b);
        let first_p = old.find("<w:p").and_then(|s| old[s..].find("</w:p>").map(|e| &old[s..s + e + 6]));
        let title = first_p.and_then(|x| Para::parse(x).ok()).map(|p| p.text()).filter(|t| !t.trim().is_empty() && !t.contains('\t')).unwrap_or_else(|| "Contents".into());
        let depth = old.find(r#"\o "1-"#).and_then(|i| old[i + 6..].chars().next()).and_then(|c| c.to_digit(10)).unwrap_or(3) as u8;
        let xml = self.toc_xml(&title, depth)?;
        self.state.blocks[b] = B::Xml(xml);
        Ok(())
    }

    pub fn toc_update(&mut self) -> Result<(), String> {
        self.checkpoint();
        self.full_layout()?;
        self.toc_update_inner()
    }

    /// Insert a footnote reference at the caret; returns a position inside the new note.
    pub fn footnote_insert(&mut self, at: Loc, text: &str) -> Result<Loc, String> {
        if at.story != Story::Body {
            return Err("footnotes go in the body text".into());
        }
        let base = self.state.base.clone();
        self.checkpoint();
        let id = base.queue_note(&mut self.state.extra, NoteKind::Footnote, text)?;
        let rev = self.rev();
        self.with_container(&at, &mut |b: &mut dyn Blocks| {
            let mut p = b.para(at.idx()).ok_or("not a paragraph")?;
            p.insert_note_reference(at.o, NoteKind::Footnote, id, rev.as_ref());
            b.set_para(at.idx(), p);
            Ok(())
        })?;
        self.done_rev(rev);
        Ok(Loc { story: Story::Footnote(id), path: vec![0], o: text.chars().count() + 1 })
    }

    /// Page numbers in the default header or footer: "Page X of Y" (or just X), aligned.
    pub fn page_numbers(&mut self, footer: bool, align: &str, with_total: bool) -> Result<(), String> {
        use ooxml::docx::story::{set_hf_reference, HfKind};
        let field = |instr: &str| format!(r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> {instr} </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#);
        let style = if footer { "Footer" } else { "Header" };
        let body = if with_total {
            format!(r#"<w:r><w:t xml:space="preserve">Page </w:t></w:r>{}<w:r><w:t xml:space="preserve"> of </w:t></w:r>{}"#, field("PAGE"), field("NUMPAGES"))
        } else {
            field("PAGE")
        };
        let jc = match align { "left" => "", "right" => r#"<w:jc w:val="right"/>"#, _ => r#"<w:jc w:val="center"/>"# };
        let para = format!(r#"<w:p><w:pPr><w:pStyle w:val="{style}"/>{jc}</w:pPr>{body}</w:p>"#);
        self.checkpoint();
        let base = self.state.base.clone();
        let kind = if footer { HfKind::Footer } else { HfKind::Header };
        let sect = self.state.extra.sect_pr.clone().or_else(|| base.body().ok().and_then(|b| b.sect_pr_xml().map(str::to_string))).unwrap_or_else(|| "<w:sectPr/>".into());
        let element = if footer { "footerReference" } else { "headerReference" };
        let existing = Props::parse(&sect).children.iter().find(|(n, x)| n == element && ooxml::xml::attr(x, "w:type").unwrap_or("default") == "default")
            .and_then(|(_, x)| ooxml::xml::attr(x, "r:id").map(str::to_string));
        let part = existing.and_then(|rid| base.headers_footers().into_iter().find(|h| h.rel_id == rid).map(|h| h.part));
        match part {
            Some(part) => {
                // Replace an empty header/footer, otherwise add the line at its end.
                let story = Story::Part(part);
                let mut list = self.story_blocks(&story)?;
                let empty = list.iter().all(|b| Para::parse(b).map(|p| p.is_empty()).unwrap_or(false));
                if empty { list.clear(); }
                list.push(para);
                self.write_story_blocks(&story, list)?;
            }
            None => {
                let (_, rid) = base.add_header_footer(&mut self.state.extra, kind, &para);
                self.state.extra.sect_pr = Some(set_hf_reference(&sect, kind, "default", &rid));
            }
        }
        Ok(())
    }

    /// Relationship owner part of a story (where its hyperlinks' targets are recorded).
    fn rel_owner(story: &Story) -> String {
        match story {
            Story::Body => ooxml::docx::DOC.to_string(),
            Story::Part(p) => p.clone(),
            Story::Footnote(_) => "word/footnotes.xml".into(),
            Story::Endnote(_) => "word/endnotes.xml".into(),
            Story::Comment(_) => "word/comments.xml".into(),
        }
    }

    /// Make `from..to` (one paragraph) a link to `url`.
    pub fn link_insert(&mut self, from: Loc, to: Loc, url: &str) -> Result<(), String> {
        if !from.same_container(&to) || from.idx() != to.idx() {
            return Err("select text within one paragraph to link it".into());
        }
        let url = if url.contains("://") || url.starts_with("mailto:") { url.to_string() } else { format!("https://{url}") };
        let owner = Self::rel_owner(&from.story);
        let base = self.state.base.clone();
        let rid = base.next_rel_id(&owner, &self.state.extra);
        self.checkpoint();
        self.state.extra.rels.push(ooxml::docx::NewRel { owner, id: rid.clone(), kind: "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink".into(), target: url, external: true });
        append::ensure_builtin_style(&base, &mut self.state.extra, "Hyperlink");
        let (a, z) = if from.o <= to.o { (from.o, to.o) } else { (to.o, from.o) };
        self.with_container(&from, &mut |b: &mut dyn Blocks| {
            let mut p = b.para(from.idx()).ok_or("not a paragraph")?;
            p.format(a, z, &RunFormat { style: Some(Some("Hyperlink".into())), ..Default::default() }, None);
            p.wrap_range(a, z, &format!(r#"<w:hyperlink r:id="{rid}" w:history="1">"#), "</w:hyperlink>", "hyperlink");
            b.set_para(from.idx(), p);
            Ok(())
        })
    }

    /// The link at a position: its address and text range.
    pub fn link_at(&self, at: &Loc) -> Option<Value> {
        let p = self.para_at(at)?;
        let (open, s, e) = p.link_at(at.o)?;
        let url = if let Some(rid) = ooxml::xml::attr(&open, "r:id") {
            let owner = Self::rel_owner(&at.story);
            self.state.extra.rels.iter().find(|r| r.owner == owner && r.id == rid).map(|r| r.target.clone())
                .or_else(|| {
                    let mut pkg = ooxml::package::Package::open(&self.state.base.bytes).ok()?;
                    pkg.rels(&owner).ok()?.into_iter().find(|r| r.id == rid).map(|r| r.target)
                })
        } else {
            ooxml::xml::attr(&open, "w:anchor").map(|a| format!("#{a}"))
        };
        Some(json!({"url": url, "from": at.with(at.idx(), s).json(), "to": at.with(at.idx(), e).json()}))
    }

    /// Misspelled words everywhere in the document.
    pub fn misspellings(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for (story, path, text) in self.all_paragraphs() {
            for (o, len, word) in spell::words(&text) {
                if !spell::check(&word, &self.user_words) {
                    out.push(json!({"s": story.key(), "p": path, "o": o, "len": len, "word": word}));
                }
            }
        }
        out
    }

    /// Replace matches of `q` with `with`: the one at `at`, or every match.
    pub fn replace(&mut self, q: &str, with: &str, case: bool, only: Option<Loc>) -> Result<usize, String> {
        let matches: Vec<Loc> = self.find(q, case).into_iter().filter_map(|m| Loc::parse(&m).ok()).collect();
        let targets: Vec<Loc> = match &only {
            Some(at) => matches.into_iter().filter(|m| m == at).collect(),
            None => matches,
        };
        if targets.is_empty() {
            return Ok(0);
        }
        let n = q.chars().count();
        self.checkpoint();
        let rev = self.rev();
        // Last match first, so earlier offsets stay valid.
        for m in targets.iter().rev() {
            let with = with.to_string();
            self.with_container(m, &mut |b: &mut dyn Blocks| {
                let mut p = b.para(m.idx()).ok_or("not a paragraph")?;
                p.insert_text(m.o + n, &with, None, rev.as_ref());
                p.delete(m.o, m.o + n, rev.as_ref());
                b.set_para(m.idx(), p);
                Ok(())
            })?;
        }
        self.done_rev(rev);
        Ok(targets.len())
    }

    /// The selection as simple HTML (headings, lists, bold/italic/underline/strike, color,
    /// size, links) for other apps' clipboards.
    pub fn html(&self, a: &Loc, z: &Loc) -> Result<String, String> {
        let (from, to) = if (a.idx(), a.o) <= (z.idx(), z.o) { (a.clone(), z.clone()) } else { (z.clone(), a.clone()) };
        if !from.same_container(&to) && !(from.story == Story::Body && from.path.len() == 1 && to.path.len() == 1) {
            return Err("select within one container".into());
        }
        let esc = |t: &str| t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let mut out = String::new();
        let mut in_list = false;
        for i in from.idx()..=to.idx() {
            let Some(p) = self.para_at(&from.with(i, 0)) else { continue };
            let style = p.ppr.as_ref().and_then(|x| x.get("pStyle")).and_then(|x| ooxml::xml::attr(x, "w:val").map(str::to_string)).unwrap_or_default();
            let list = p.ppr.as_ref().is_some_and(|x| x.get("numPr").is_some());
            let tag = match style.as_str() { "Title" | "Heading1" => "h1", "Heading2" => "h2", "Heading3" => "h3", "Quote" => "blockquote", _ if list => "li", _ => "p" };
            if list && !in_list { out.push_str("<ul>"); in_list = true; }
            if !list && in_list { out.push_str("</ul>"); in_list = false; }
            let (s0, e0) = (if i == from.idx() { from.o } else { 0 }, if i == to.idx() { to.o } else { usize::MAX });
            let mut body = String::new();
            let mut pos = 0;
            for seg in &p.segs {
                let ooxml::docx::para::SegKind::Run(r) = &seg.kind else { continue };
                if p.hidden(seg) { continue; }
                let text = r.text();
                let n = text.chars().count();
                let (a1, b1) = (s0.max(pos), e0.min(pos + n));
                if a1 < b1 {
                    let piece: String = text.chars().skip(a1 - pos).take(b1 - a1).collect();
                    let mut h = esc(&piece).replace('\t', "&emsp;").replace('\n', "<br>");
                    if let Some(rp) = &r.rpr {
                        let on = |n: &str| rp.toggle(n) == Some(true);
                        let mut css = Vec::new();
                        if let Some(c) = rp.get("color").and_then(|x| ooxml::xml::attr(x, "w:val")).filter(|c| *c != "auto") { css.push(format!("color:#{c}")); }
                        if let Some(sz) = rp.get("sz").and_then(|x| ooxml::xml::attr(x, "w:val")).and_then(|v| v.parse::<f64>().ok()) { css.push(format!("font-size:{}pt", sz / 2.0)); }
                        if let Some(f) = rp.get("rFonts").and_then(|x| ooxml::xml::attr(x, "w:ascii")) { css.push(format!("font-family:'{f}'")); }
                        if !css.is_empty() { h = format!(r#"<span style="{}">{h}</span>"#, css.join(";")); }
                        if on("b") { h = format!("<b>{h}</b>"); }
                        if on("i") { h = format!("<i>{h}</i>"); }
                        if rp.get("u").is_some_and(|x| ooxml::xml::attr(x, "w:val") != Some("none")) { h = format!("<u>{h}</u>"); }
                        if on("strike") { h = format!("<s>{h}</s>"); }
                    }
                    body.push_str(&h);
                }
                pos += n;
            }
            out.push_str(&format!("<{tag}>{body}</{tag}>"));
        }
        if in_list { out.push_str("</ul>"); }
        Ok(out)
    }

    /// Paste structured paragraphs at `at`: the first joins the paragraph at the caret,
    /// the rest become new paragraphs with their style / list kind; runs keep their
    /// formatting and links. Returns where the caret ends.
    pub fn paste_rich(&mut self, at: Loc, paras: &[Value]) -> Result<Loc, String> {
        if paras.is_empty() {
            return Ok(at);
        }
        let base = self.state.base.clone();
        self.checkpoint();
        // One bullet and one numbered list for the whole paste.
        let mut lists: std::collections::HashMap<&str, u32> = Default::default();
        for p in paras {
            if let Some(k) = p["list"].as_str().filter(|k| *k == "bullet" || *k == "number") {
                if !lists.contains_key(k) {
                    let id = append::add_list(&base, &mut self.state.extra, if k == "bullet" { ListKind::Bullet } else { ListKind::Decimal });
                    lists.insert(if k == "bullet" { "bullet" } else { "number" }, id);
                }
            }
            if let Some(st) = p["style"].as_str().filter(|s| !s.is_empty() && *s != "Normal") {
                append::ensure_builtin_style(&base, &mut self.state.extra, st);
            }
        }
        let has_links = paras.iter().any(|p| p["runs"].as_array().is_some_and(|r| r.iter().any(|x| x["link"].is_string())));
        if has_links { append::ensure_builtin_style(&base, &mut self.state.extra, "Hyperlink"); }
        let owner = Self::rel_owner(&at.story);
        let rev = self.rev();
        let next = self.style_snapshot();
        let mut cur = at.clone();
        let mut new_rels: Vec<(String, String)> = Vec::new();
        let mut rel_n = 0usize;
        let rel_base = base.next_rel_id(&owner, &self.state.extra);
        for (pi, p) in paras.iter().enumerate() {
            if pi > 0 {
                let (i, o) = self.with_container(&cur, &mut |b: &mut dyn Blocks| edit::split(b, (cur.idx(), cur.o), rev.as_ref(), &next))?;
                cur = cur.with(i, o);
                // A pasted paragraph takes its own style (and list), not the one it split from.
                let style = p["style"].as_str().filter(|s| !s.is_empty() && *s != "Normal").map(str::to_string);
                let list = p["list"].as_str().and_then(|k| lists.get(k).copied());
                let fmt = ParaFormat {
                    style: Some(if list.is_some() { Some("ListParagraph".into()) } else { style }),
                    numbering: Some(list.map(|id| append::num_pr(id, 0))),
                    ..Default::default()
                };
                let idx = cur.idx();
                self.with_container(&cur, &mut |b: &mut dyn Blocks| {
                    let mut q = b.para(idx).ok_or("not a paragraph")?;
                    q.format_paragraph(&fmt, None);
                    b.set_para(idx, q);
                    Ok(())
                })?;
            }
            for r in p["runs"].as_array().cloned().unwrap_or_default() {
                let text = r["text"].as_str().unwrap_or("").replace(['\r', '\n'], " ");
                if text.is_empty() { continue; }
                let fmt = run_format(&r);
                let n = text.chars().count();
                let start = cur.o;
                let link = r["link"].as_str().map(str::to_string);
                let rid = link.as_ref().map(|_| { rel_n += 1; format!("{rel_base}p{rel_n}") });
                if let (Some(l), Some(id)) = (&link, &rid) { new_rels.push((id.clone(), l.clone())); }
                let idx = cur.idx();
                self.with_container(&cur, &mut |b: &mut dyn Blocks| {
                    let mut q = b.para(idx).ok_or("not a paragraph")?;
                    q.insert_text(start, &text, Some(&fmt), rev.as_ref());
                    if let Some(id) = &rid {
                        q.format(start, start + n, &RunFormat { style: Some(Some("Hyperlink".into())), ..Default::default() }, None);
                        q.wrap_range(start, start + n, &format!(r#"<w:hyperlink r:id="{id}" w:history="1">"#), "</w:hyperlink>", "hyperlink");
                    }
                    b.set_para(idx, q);
                    Ok(())
                })?;
                cur.o += n;
            }
        }
        for (id, url) in new_rels {
            self.state.extra.rels.push(ooxml::docx::NewRel { owner: owner.clone(), id, kind: "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink".into(), target: url, external: true });
        }
        self.done_rev(rev);
        Ok(cur)
    }

    /// Headers, footers, footnotes and endnotes (not comments: those live in the sidebar).
    fn other_stories(&self) -> Vec<Story> {
        let base = &self.state.base;
        let mut out: Vec<Story> = base.headers_footers().into_iter().map(|h| Story::Part(h.part)).collect();
        for r in &self.state.extra.rels {
            if (r.kind.ends_with("/header") || r.kind.ends_with("/footer")) && r.owner == ooxml::docx::DOC {
                let part = format!("word/{}", r.target);
                if !out.contains(&Story::Part(part.clone())) {
                    out.push(Story::Part(part));
                }
            }
        }
        for n in base.notes(&self.state.extra, NoteKind::Footnote) {
            if n.special.is_none() && n.id > 0 { out.push(Story::Footnote(n.id)); }
        }
        for n in base.notes(&self.state.extra, NoteKind::Endnote) {
            if n.special.is_none() && n.id > 0 { out.push(Story::Endnote(n.id)); }
        }
        out
    }

    fn text_range(&self, a: &Loc, z: &Loc) -> Result<String, String> {
        if a.story == Story::Body && a.path.len() == 1 && z.path.len() == 1 {
            // Top-level body ranges can span tables: take each paragraph in between.
            let (from, to) = if (a.idx(), a.o) <= (z.idx(), z.o) { (a, z) } else { (z, a) };
            let mut out = Vec::new();
            for b in from.idx()..=to.idx() {
                let Some(p) = self.para(b) else { continue };
                let chars: Vec<char> = p.text().chars().collect();
                let s = if b == from.idx() { from.o.min(chars.len()) } else { 0 };
                let e = if b == to.idx() { to.o.min(chars.len()) } else { chars.len() };
                out.push(chars[s..e.max(s)].iter().collect::<String>());
            }
            return Ok(out.join("\n"));
        }
        if !a.same_container(z) {
            return Err("select within one container".into());
        }
        self.read_container(a, &mut |b| edit::text(b, (a.idx(), a.o), (z.idx(), z.o)))
    }

    fn resolve(&mut self, accept: bool, author: Option<String>, ids: Option<Vec<String>>) -> Result<usize, String> {
        let bytes = self.bytes()?;
        let docx = Docx::open(&bytes)?;
        let mut e = DocxEdits::default();
        let n = docx.resolve_revisions(&mut e, accept, &Filter { author, ids })?;
        if n > 0 {
            self.checkpoint();
            let out = docx.save(&e)?;
            let track = self.track;
            self.load(out)?;
            self.track = track;
        }
        Ok(n)
    }

    fn set_track(&mut self, on: bool) -> Result<(), String> {
        self.checkpoint();
        self.track = on;
        let base = self.state.base.clone();
        base.set_track_revisions(&mut self.state.extra, on)
    }

    fn doc_key(&self) -> String {
        autosave::key_for(self.path.as_deref(), &self.session_id)
    }

    pub fn save(&mut self, path: Option<&Path>) -> Result<(), String> {
        let target = path.map(Path::to_path_buf).or_else(|| self.path.clone()).ok_or("choose where to save")?;
        let bytes = self.bytes()?;
        std::fs::write(&target, &bytes).map_err(|e| format!("{}: {e}", target.display()))?;
        // The recovery copy (under the old key, if this was Save As) is no longer needed.
        autosave::clear_recovery(&self.doc_key());
        let new_key = autosave::key_for(Some(&target), &self.session_id);
        autosave::clear_recovery(&new_key);
        let _ = autosave::save_version(&new_key, &bytes, "Saved");
        // The saved file becomes the new baseline (undo history is kept as full states).
        let track = self.track;
        self.load(bytes)?;
        self.track = track;
        self.path = Some(target.clone());
        self.name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.dirty = false;
        Ok(())
    }

    pub fn info(&self) -> Value {
        json!({
            "name": self.name,
            "path": self.path.as_ref().map(|p| p.display().to_string()),
            "dirty": self.dirty,
            "can_undo": !self.undo.is_empty(),
            "can_redo": !self.redo.is_empty(),
            "track": self.track,
            "author": self.author,
            "blocks": self.state.blocks.len(),
            "words": self.word_count(),
            "page": self.page_info(),
            "toc": self.toc_block().is_some(),
        })
    }

    /// Words in the body, table cells included.
    fn word_count(&self) -> usize {
        fn count(xml: &str) -> usize {
            // Visible text of every paragraph in this block (tables: every cell paragraph).
            let mut n = 0;
            for (i, _) in xml.match_indices("<w:p") {
                let rest = &xml[i..];
                let ok = rest[4..].starts_with(['>', ' ', '/']);
                if !ok { continue; }
                let end = rest.find("</w:p>").map(|e| e + 6).unwrap_or(rest.len());
                if let Ok(p) = Para::parse(&rest[..end]) {
                    n += p.text().split_whitespace().count();
                }
            }
            n
        }
        (0..self.state.blocks.len()).map(|b| match self.para(b) {
            Some(p) => p.text().split_whitespace().count(),
            None => count(&match &self.state.blocks[b] {
                B::Orig(i) => self.state.base.body().map(|body| body.xml(*i).to_string()).unwrap_or_default(),
                B::Xml(x) => x.clone(),
                B::Para(p) => p.to_xml(),
            }),
        }).sum()
    }

    /// Block kinds, for mapping layout back to blocks.
    fn kinds(&self) -> Vec<&'static str> {
        (0..self.state.blocks.len()).map(|b| match self.block_name(b) { "p" => "p", "tbl" => "tbl", "sdt" => "sdt", _ => "other" }).collect()
    }

    /// Plain text of every paragraph block (None for non-paragraphs), for layout alignment.
    fn texts(&self) -> Vec<Option<String>> {
        (0..self.state.blocks.len()).map(|b| self.para(b).map(|p| p.text())).collect()
    }

    // ------------------------------------------------------------------ layout

    /// Paragraphs the layout can't anchor by itself: table cells, headers, footers,
    /// footnotes, endnotes and text boxes.
    fn anchors(&self) -> layout::Anchors {
        let mut a = layout::Anchors::default();
        let body: Vec<String> = (0..self.state.blocks.len()).map(|b| self.block_xml(b)).collect();
        for (b, x) in body.iter().enumerate() {
            if blocks::kind_of(x) == "tbl" {
                blocks::cell_paragraphs(x, &[b as i64], &mut a.cells);
            }
        }
        // Text boxes in the body: paragraphs whose path goes through a negative (text box) step.
        let mut paras = Vec::new();
        blocks::collect_paragraphs(&body, &[], &mut paras);
        a.stories.extend(paras.into_iter().filter(|(p, _)| p.iter().any(|v| *v < 0)).map(|(p, t)| (String::new(), p, t)));
        for story in self.other_stories() {
            if let Ok(list) = self.story_blocks(&story) {
                let mut paras = Vec::new();
                blocks::collect_paragraphs(&list, &[], &mut paras);
                a.stories.extend(paras.into_iter().map(|(p, t)| (story.key(), p, t)));
            }
        }
        a
    }

    /// Body blocks for layout: empty paragraphs (also in tables) get a zero-width space so
    /// they have a line.
    fn layout_blocks(&self, range: std::ops::Range<usize>) -> Vec<Block> {
        let e = self.edits();
        let body = e.body.unwrap_or_default();
        let kinds = self.kinds();
        range.map(|b| match self.para(b).filter(|p| p.is_empty()) {
            Some(mut p) => {
                p.insert_text(0, layout::ZWSP, None, None);
                Block::Xml(p.to_xml())
            }
            None if kinds[b] == "tbl" => {
                let mut one = vec![self.block_xml(b)];
                let before = one[0].clone();
                blocks::fill_empty_paragraphs(&mut one, layout::ZWSP);
                if one[0] == before { body[b].clone() } else { Block::Xml(one.pop().unwrap()) }
            }
            None => body[b].clone(),
        }).collect()
    }

    /// Edits for layout: placeholders in empty header, footer and note paragraphs too.
    fn layout_edits(&self) -> DocxEdits {
        let mut e = self.edits();
        let base = &self.state.base;
        for story in self.other_stories() {
            let Ok(mut list) = self.story_blocks(&story) else { continue };
            let before = list.clone();
            blocks::fill_empty_paragraphs(&mut list, layout::ZWSP);
            if list == before {
                continue;
            }
            let _ = match &story {
                Story::Part(p) => base.edit_header_footer(&mut e, p, &list.into_iter().map(Block::Xml).collect::<Vec<_>>()),
                Story::Footnote(id) => base.set_note_blocks(&mut e, NoteKind::Footnote, *id, &list.concat()),
                Story::Endnote(id) => base.set_note_blocks(&mut e, NoteKind::Endnote, *id, &list.concat()),
                _ => Ok(()),
            };
        }
        e
    }

    fn extra_key(&self) -> String {
        let e = &self.state.extra;
        format!("{:?}|{:?}|{:?}|{}", e.parts.iter().map(|(k, v)| (k, v.len(), v.iter().map(|b| *b as u64).sum::<u64>())).collect::<Vec<_>>(), e.sect_pr, e.rels.len(), e.drop.len())
    }

    fn full_layout(&mut self) -> Result<Value, String> {
        let mut e = self.layout_edits();
        e.body = Some(self.layout_blocks(0..self.state.blocks.len()));
        let bytes = self.state.base.save(&e)?;
        let out = layout::render(&bytes, &self.kinds(), &self.texts(), 0, &self.anchors())?;
        let pages: Vec<Value> = out["pages"].as_array().cloned().unwrap_or_default();
        let images = out["images"].as_object().cloned().unwrap_or_default();
        let (clean, first_page) = layout::page_index(&pages, self.state.blocks.len());
        self.cache = Some(LayoutCache { pages, images, blocks: self.state.blocks.clone(), extra_key: self.extra_key(), clean, first_page });
        Ok(json!({"pages": out["pages"], "images": out["images"], "stale": false}))
    }

    /// Lay out only the pages an edit can change (see module docs of `layout`); falls back
    /// to a full layout whenever the shortcut isn't safe.
    fn layout(&mut self, quick: bool) -> Result<Value, String> {
        const WINDOW: usize = 40;
        let Some(cache) = self.cache.as_ref().filter(|_| quick) else { return self.full_layout() };
        if cache.extra_key != self.extra_key() {
            return self.full_layout();
        }
        let (old, new) = (&cache.blocks, &self.state.blocks);
        let Some(dirty) = (0..old.len().max(new.len())).find(|&i| match (old.get(i), new.get(i)) {
            (Some(a), Some(b)) => !same_block(a, b),
            _ => true,
        }) else {
            return Ok(json!({"pages": cache.pages, "images": cache.images, "stale": false}));
        };
        let delta = new.len() as i64 - old.len() as i64;
        // Latest page that starts cleanly with a block at or before the edit.
        let Some((page, start)) = cache.clean.iter().enumerate().rev()
            .find_map(|(p, c)| c.filter(|&b| b <= dirty && b < old.len()).map(|b| (p, b))) else { return self.full_layout() };
        // Stay inside one section: no section break between `start` and the edit.
        let sect_end = (start..new.len()).find(|&b| self.para(b).and_then(|p| p.ppr).is_some_and(|p| p.get("sectPr").is_some()));
        if sect_end.is_some_and(|e| e < dirty) {
            return self.full_layout();
        }
        let end = (dirty + WINDOW).min(new.len()).min(sect_end.map(|e| e + 1).unwrap_or(usize::MAX));
        let reaches_end = end == new.len();
        // The section's properties, numbering pages from where the window starts.
        let mut blocks = self.layout_blocks(start..end);
        let mut sect = match sect_end {
            Some(e) => {
                let mut p = self.para(e).unwrap();
                let s = p.ppr.as_ref().and_then(|x| x.get("sectPr")).unwrap_or_default().to_string();
                if e < end {
                    if let Some(x) = p.ppr.as_mut() { x.set("sectPr", None); }
                    if p.is_empty() { p.insert_text(0, layout::ZWSP, None, None); }
                    blocks[e - start] = Block::Xml(p.to_xml());
                }
                Props::parse(&s)
            }
            None => Props::parse(self.state.extra.sect_pr.as_deref().or(self.state.base.body()?.sect_pr_xml()).unwrap_or("<w:sectPr/>")),
        };
        let section_start = (0..start).rev().find(|&b| self.para(b).and_then(|p| p.ppr).is_some_and(|p| p.get("sectPr").is_some())).map(|b| b + 1).unwrap_or(0);
        if start > section_start {
            sect.set("titlePg", None);
            let first_of_section = cache.first_page.get(section_start).copied().flatten().unwrap_or(0);
            let base_no = sect.get("pgNumType").and_then(|x| ooxml::xml::attr(x, "w:start").and_then(|v| v.parse::<usize>().ok()))
                .map(|s0| s0 + page - first_of_section).unwrap_or(page + 1);
            let tag = sect.get("pgNumType").map(|x| ooxml::xml::set_attr(x.trim_end_matches("</w:pgNumType>"), "w:start", Some(&base_no.to_string())))
                .unwrap_or_else(|| format!(r#"<w:pgNumType w:start="{base_no}"/>"#));
            sect.set("pgNumType", Some(if tag.ends_with("/>") { tag } else { format!("{tag}</w:pgNumType>") }));
        }
        // A paragraph at the top of a page after a natural break loses its space before;
        // at the start of the window document it wouldn't, so drop it explicitly.
        let hard_break = self.para(start.saturating_sub(1)).is_some_and(|p| p.to_xml().contains(r#"w:type="page""#))
            || self.para(start).and_then(|p| p.ppr).is_some_and(|p| p.get("pageBreakBefore").is_some());
        if start > 0 && !hard_break {
            if let Some(mut p) = self.para(start) {
                if p.is_empty() { p.insert_text(0, layout::ZWSP, None, None); }
                let mut ppr = p.ppr.take().unwrap_or_else(|| Props::new("w:pPr"));
                let sp = ppr.get("spacing").map(str::to_string).unwrap_or_else(|| "<w:spacing/>".into());
                let mut tag = ooxml::xml::set_attr(&sp, "w:before", Some("0"));
                for a in ["w:beforeLines", "w:beforeAutospacing"] { tag = ooxml::xml::set_attr(&tag, a, None); }
                ppr.set("spacing", Some(tag));
                p.ppr = Some(ppr);
                blocks[0] = Block::Xml(p.to_xml());
            }
        }
        let mut e = self.layout_edits();
        e.body = Some(blocks);
        e.sect_pr = Some(if sect.tag.is_empty() { "<w:sectPr/>".into() } else { sect.to_xml() });
        let bytes = self.state.base.save(&e)?;
        let kinds = self.kinds();
        let texts = self.texts();
        let win = layout::render(&bytes, &kinds[start..end], &texts[start..end], start, &self.anchors())?;
        let mut win_pages: Vec<Value> = win["pages"].as_array().cloned().unwrap_or_default();
        let mut images = cache.images.clone();
        if let Some(m) = win["images"].as_object() { images.extend(m.clone()); }
        let mut pages: Vec<Value> = cache.pages[..page].to_vec();
        let stale = !reaches_end;
        if reaches_end {
            pages.append(&mut win_pages);
        } else {
            // Drop the window's last (possibly cut short) page; continue with the old pages
            // from where the block after it used to start, shifted by the blocks added/removed.
            win_pages.pop();
            pages.append(&mut win_pages);
            let old_next = (end as i64 - 1 - delta).max(0) as usize;
            let from = cache.first_page.get(old_next).copied().flatten().map(|p| p + 1).unwrap_or(cache.pages.len());
            for p in cache.pages.iter().skip(from) {
                pages.push(layout::shift_blocks(p, dirty, delta));
            }
        }
        Ok(json!({"pages": pages, "images": images, "stale": stale}))
    }

    // ------------------------------------------------------------------ dispatch

    pub fn dispatch(&mut self, cmd: &str, a: &Value) -> Result<Value, String> {
        let s = |k: &str| a[k].as_str().unwrap_or("").to_string();
        match cmd {
            "info" => Ok(self.info()),
            "new" => {
                autosave::clear_recovery(&self.doc_key());
                *self = Session { author: self.author.clone(), ..Session::new() };
                Ok(self.info())
            }
            "open" => {
                if !self.dirty { autosave::clear_recovery(&self.doc_key()); }
                self.open(Path::new(&s("path")))?;
                Ok(self.info())
            }
            "save" => {
                let p = s("path");
                self.save(if p.is_empty() { None } else { Some(Path::new(&p)) })?;
                Ok(self.info())
            }
            "layout" => self.layout(a["quick"].as_bool().unwrap_or(false)),
            "insert" => {
                let fmt = a.get("fmt").filter(|f| f.is_object()).map(run_format);
                Ok(self.insert_formatted(loc(&a["at"])?, &s("text"), fmt.as_ref())?.json())
            }
            "split" => Ok(self.split(loc(&a["at"])?)?.json()),
            "delete" => Ok(self.delete(loc(&a["from"])?, loc(&a["to"])?)?.json()),
            "format" => {
                self.format(loc(&a["from"])?, loc(&a["to"])?, &run_format(&a["fmt"]))?;
                Ok(Value::Null)
            }
            "para_format" => {
                self.para_format(loc(&a["from"])?, loc(&a["to"])?, &a["fmt"])?;
                Ok(Value::Null)
            }
            "page_break" => Ok(self.insert_object(loc(&a["at"])?, r#"<w:br w:type="page"/>"#)?.json()),
            "table" => Ok(self.insert_table(loc(&a["at"])?, a["rows"].as_u64().unwrap_or(3) as usize, a["cols"].as_u64().unwrap_or(3) as usize)?.json()),
            "picture" => {
                let path = s("path");
                let data = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
                let ext = Path::new(&path).extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
                // Natural size at 96 dpi, scaled down to fit the text width (6.5in = 624px).
                let (nw, nh) = image_size(&data).unwrap_or((320, 240));
                let scale = (624.0 / nw as f64).min(1.0);
                let (w, h) = (a["w"].as_u64().map(|v| v as u32).unwrap_or((nw as f64 * scale) as u32),
                              a["h"].as_u64().map(|v| v as u32).unwrap_or((nh as f64 * scale) as u32));
                let at = loc(&a["at"])?;
                let base = self.state.base.clone();
                let rid = append::add_image(&base, &mut self.state.extra, &data, &ext)?;
                let id = append::max_drawing_id(&self.bytes().ok().and_then(|b| Docx::open(&b).ok()).map(|d| d.document).unwrap_or_default()) + 1;
                Ok(self.insert_object(at, &append::inline_picture(&rid, w, h, id, "Picture"))?.json())
            }
            "text" => Ok(json!(self.text_range(&loc(&a["from"])?, &loc(&a["to"])?)?)),
            "table_op" => Ok(self.table_op(loc(&a["at"])?, &s("op"), a["color"].as_str())?.json()),
            "page_setup" => {
                self.page_setup(a)?;
                Ok(self.info())
            }
            "find" => Ok(json!(self.find(&s("q"), a["case"].as_bool().unwrap_or(false)))),
            "para_info" => {
                let p = self.para_at(&loc(&a["at"])?);
                let ppr = p.as_ref().and_then(|p| p.ppr.clone());
                let style = ppr.as_ref().and_then(|x| x.get("pStyle")).and_then(|x| ooxml::xml::attr(x, "w:val").map(str::to_string));
                let align = ppr.as_ref().and_then(|x| x.get("jc")).and_then(|x| ooxml::xml::attr(x, "w:val").map(str::to_string));
                let list = ppr.as_ref().and_then(|x| x.get("numPr")).is_some();
                Ok(json!({"style": style.unwrap_or_else(|| "Normal".into()), "align": align.unwrap_or_else(|| "left".into()), "list": list, "len": p.map(|p| p.len())}))
            }
            "track" => {
                self.set_track(a["on"].as_bool().unwrap_or(!self.track))?;
                Ok(self.info())
            }
            "revisions" => {
                let docx = Docx::open(&self.bytes()?)?;
                Ok(json!(docx.revisions().into_iter().map(|r| {
                    let at = self.revision_loc(&r.id).map(|l| l.json());
                    json!({"id": r.id, "kind": r.kind, "author": r.author, "date": r.date, "part": r.part, "text": r.text, "at": at})
                }).collect::<Vec<_>>()))
            }
            "accept_all" | "reject_all" => {
                let author = a["author"].as_str().map(str::to_string);
                Ok(json!({"resolved": self.resolve(cmd == "accept_all", author, None)?}))
            }
            "accept" | "reject" => {
                let ids: Vec<String> = a["ids"].as_array().map(|v| v.iter().filter_map(|x| x.as_str().map(str::to_string).or_else(|| x.as_i64().map(|n| n.to_string()))).collect()).unwrap_or_default();
                Ok(json!({"resolved": self.resolve(cmd == "accept", None, Some(ids))?}))
            }
            "comments" => Ok(self.comments_json()),
            "autosave" => {
                // Recovery copy of unsaved changes; a version snapshot every few minutes.
                if !self.dirty {
                    return Ok(json!({"saved": false}));
                }
                let bytes = self.bytes()?;
                let key = self.doc_key();
                autosave::save_recovery(&key, &bytes, &self.name, self.path.as_deref())?;
                let now = autosave::now_ms();
                if now - self.last_version > 5 * 60 * 1000 {
                    self.last_version = now;
                    let _ = autosave::save_version(&key, &bytes, "Autosaved");
                }
                Ok(json!({"saved": true, "at": now as u64}))
            }
            "recovery_list" => Ok(json!(autosave::list_recovery())),
            "discard" => { autosave::clear_recovery(&self.doc_key()); Ok(Value::Null) }
            "recovery_discard" => { autosave::clear_recovery(&s("key")); Ok(Value::Null) }
            "recovery_restore" => {
                let key = s("key");
                let meta = autosave::list_recovery().into_iter().find(|v| v["key"] == key.as_str()).ok_or("nothing to recover")?;
                let bytes = autosave::recovery_bytes(&key)?;
                self.load(bytes)?;
                self.path = meta["path"].as_str().map(PathBuf::from);
                self.name = meta["name"].as_str().unwrap_or("Recovered").to_string();
                self.undo.clear();
                self.redo.clear();
                self.dirty = true;
                if self.path.is_none() {
                    // Keep autosaving into the same recovery file.
                    self.session_id = key.trim_start_matches("untitled-").to_string();
                }
                Ok(self.info())
            }
            "versions" => Ok(json!(autosave::list_versions(&self.doc_key()))),
            "version_restore" => {
                let bytes = autosave::version_bytes(&self.doc_key(), &s("id"))?;
                let docx = Docx::open(&bytes)?;
                let n = docx.body()?.elements.len();
                // Restoring is an ordinary (undoable) change to the open document.
                self.checkpoint();
                self.state = State { blocks: (0..n).map(B::Orig).collect(), base: Arc::new(docx), extra: DocxEdits::default() };
                self.cache = None;
                Ok(self.info())
            }
            "spell" => Ok(json!(self.misspellings())),
            "html" => Ok(json!(self.html(&loc(&a["from"])?, &loc(&a["to"])?)?)),
            "paste_rich" => {
                let paras = a["paras"].as_array().cloned().unwrap_or_default();
                Ok(self.paste_rich(loc(&a["at"])?, &paras)?.json())
            }
            "suggest" => Ok(json!(spell::suggest(&s("word")))),
            "spell_words" => {
                // The UI sends its saved custom words; returns nothing.
                self.user_words = a["words"].as_array().map(|v| v.iter().filter_map(|w| w.as_str().map(str::to_string)).collect()).unwrap_or_default();
                Ok(Value::Null)
            }
            "replace" => {
                let only = a.get("at").filter(|v| v.is_object()).map(loc).transpose()?;
                Ok(json!({"replaced": self.replace(&s("q"), &s("with"), a["case"].as_bool().unwrap_or(false), only)?}))
            }
            "outline" => Ok(json!(self.outline().into_iter().map(|(b, l, t)| json!({"p": [b], "level": l, "text": t})).collect::<Vec<_>>())),
            "toc_insert" => Ok(self.toc_insert(loc(&a["at"])?)?.json()),
            "toc_update" => { self.toc_update()?; Ok(Value::Null) }
            "footnote_insert" => Ok(self.footnote_insert(loc(&a["at"])?, a["text"].as_str().unwrap_or(""))?.json()),
            "page_numbers" => {
                self.page_numbers(a["where"].as_str() != Some("header"), a["align"].as_str().unwrap_or("center"), a["total"].as_bool().unwrap_or(true))?;
                Ok(Value::Null)
            }
            "link_insert" => { self.link_insert(loc(&a["from"])?, loc(&a["to"])?, &s("url"))?; Ok(Value::Null) }
            "link_at" => Ok(self.link_at(&loc(&a["at"])?).unwrap_or(Value::Null)),
            "link_remove" => {
                let at = loc(&a["at"])?;
                self.edit_at(&at, &mut |b, _| {
                    let mut p = b.para(at.idx()).ok_or("not a paragraph")?;
                    if !p.unlink_at(at.o) { return Err("there's no link here".into()); }
                    b.set_para(at.idx(), p);
                    Ok(())
                })?;
                Ok(Value::Null)
            }
            "comment_add" => {
                let at = loc(&a["from"])?;
                let to = a.get("to").filter(|v| v.is_object()).map(loc).transpose()?.unwrap_or_else(|| at.clone());
                let id = self.add_comment(at, to, &s("text"), a["parent"].as_i64())?;
                Ok(json!({"id": id}))
            }
            "comment_edit" => {
                self.checkpoint();
                let base = self.state.base.clone();
                base.set_comment_text(&mut self.state.extra, a["id"].as_i64().ok_or("id")?, &s("text"))?;
                Ok(Value::Null)
            }
            "comment_resolve" => {
                self.checkpoint();
                let base = self.state.base.clone();
                base.set_comment_done(&mut self.state.extra, a["id"].as_i64().ok_or("id")?, a["done"].as_bool().unwrap_or(true))?;
                Ok(Value::Null)
            }
            "comment_delete" => {
                self.delete_comment(a["id"].as_i64().ok_or("id")?)?;
                Ok(Value::Null)
            }
            "undo" => {
                let prev = self.undo.pop().ok_or("nothing to undo")?;
                self.redo.push(std::mem::replace(&mut self.state, prev));
                self.dirty = true;
                Ok(self.info())
            }
            "redo" => {
                let next = self.redo.pop().ok_or("nothing to redo")?;
                self.undo.push(std::mem::replace(&mut self.state, next));
                self.dirty = true;
                Ok(self.info())
            }
            "template" => {
                let paras: Vec<(String, String)> = a["paragraphs"].as_array().map(|v| v.iter().map(|p| (p[0].as_str().unwrap_or("").to_string(), p[1].as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
                let refs: Vec<(&str, &str)> = paras.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
                *self = Session { author: self.author.clone(), ..Session::new() };
                self.load(blank::document(&refs))?;
                self.name = s("name");
                Ok(self.info())
            }
            _ => Err(format!("unknown command {cmd}")),
        }
    }
}

/// Pixel size of a PNG, JPEG or GIF from its header.
fn image_size(d: &[u8]) -> Option<(u32, u32)> {
    match d {
        [0x89, b'P', b'N', b'G', ..] if d.len() >= 24 => Some((u32::from_be_bytes(d[16..20].try_into().ok()?), u32::from_be_bytes(d[20..24].try_into().ok()?))),
        [b'G', b'I', b'F', ..] if d.len() >= 10 => Some((u16::from_le_bytes([d[6], d[7]]) as u32, u16::from_le_bytes([d[8], d[9]]) as u32)),
        [0xFF, 0xD8, ..] => {
            let mut i = 2;
            while i + 9 < d.len() {
                if d[i] != 0xFF { i += 1; continue; }
                let marker = d[i + 1];
                let len = u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize;
                if (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker) {
                    return Some((u16::from_be_bytes([d[i + 7], d[i + 8]]) as u32, u16::from_be_bytes([d[i + 5], d[i + 6]]) as u32));
                }
                i += 2 + len;
            }
            None
        }
        _ => None,
    }
    .filter(|(w, h)| *w > 0 && *h > 0)
}

#[cfg(test)]
mod tests;

#[cfg(all(test, unix))]
fast_oxi_env!();
