//! Page layout for the page view: Oxi lays the document out, and this turns the
//! result into a JSON display list the canvas draws, with every body text item
//! anchored to a (block, visible offset) position in the editing model.
//!
//! Oxi's paragraph indices refer to its own IR (per section, with body-level
//! content controls flattened and some paragraphs split into image/math blocks),
//! so IR paragraphs are aligned to editor blocks by their text. Oxi's character
//! offsets count tracked-deleted text, which the editor doesn't, so each IR
//! paragraph gets an offset table that skips deleted runs.

use std::collections::HashMap;
use std::sync::OnceLock;

use base64::Engine;
use oxidocs_core::font::FontMetricsRegistry;
use oxidocs_core::ir::Block as IrBlock;
use oxidocs_core::layout::{LayoutContent, LayoutEngine};
use serde_json::{json, Value};

fn metrics() -> &'static FontMetricsRegistry {
    static M: OnceLock<FontMetricsRegistry> = OnceLock::new();
    M.get_or_init(FontMetricsRegistry::load)
}

/// Placeholder text the session puts in empty paragraphs so they get a line box.
pub const ZWSP: &str = "\u{200B}";

fn norm(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace() && !c.is_control() && !matches!(c, '\u{2011}' | '-' | '\u{200B}')).collect()
}

/// Where an IR paragraph lands in the editor, plus its Oxi-offset -> visible-offset table.
struct ParaMap {
    block: usize,
    /// For each run: (Oxi chars before it, visible chars before it, deleted?).
    runs: Vec<(usize, usize, bool)>,
}

impl ParaMap {
    /// Visible offset where an element starts, and how many visible characters it covers
    /// (an element can span runs, e.g. an inserted word touching a deleted one).
    fn visible(&self, run: usize, off: usize, len: usize) -> (usize, usize) {
        let Some(&(oxi0, vis0, del0)) = self.runs.get(run) else {
            return (self.runs.last().map(|r| r.1).unwrap_or(0), 0);
        };
        let start = if del0 { vis0 } else { vis0 + off };
        let (a, b) = (oxi0 + off, oxi0 + off + len);
        let mut n = 0;
        for (i, &(o, _, del)) in self.runs.iter().enumerate().skip(run) {
            let end = self.runs.get(i + 1).map(|r| r.0).unwrap_or(usize::MAX);
            if o >= b {
                break;
            }
            if !del {
                n += end.min(b).saturating_sub(o.max(a));
            }
        }
        (start, n)
    }
}

/// Map (section, IR block index) of every IR paragraph to an editor block.
fn align(doc: &oxidocs_core::Document, kinds: &[&str], texts: &[Option<String>]) -> (HashMap<(usize, usize), ParaMap>, HashMap<(usize, usize), usize>) {
    struct Ir<'a> { key: (usize, usize), para: Option<&'a oxidocs_core::ir::Paragraph>, table: bool }
    let mut ir: Vec<Ir> = Vec::new();
    for (s, page) in doc.pages.iter().enumerate() {
        for (i, b) in page.blocks.iter().enumerate() {
            match b {
                IrBlock::Paragraph(p) => ir.push(Ir { key: (s, i), para: Some(p), table: false }),
                IrBlock::Table(_) => ir.push(Ir { key: (s, i), para: None, table: true }),
                _ => {}
            }
        }
    }
    let ir_text = |p: &oxidocs_core::ir::Paragraph| -> String {
        p.runs.iter().filter(|r| !r.tracked_change.as_ref().is_some_and(|t| t.change_type == "delete" || t.change_type == "moveFrom"))
            .map(|r| r.text.as_str()).collect()
    };
    let mut out = HashMap::new();
    let mut tables = HashMap::new();
    let mut j = 0;
    for (b, kind) in kinds.iter().enumerate() {
        let want = match (*kind, texts.get(b)) {
            ("p", Some(Some(t))) => Some(norm(t)),
            ("tbl", _) => None,
            _ => continue,
        };
        // Look a little ahead: content controls and image-only paragraphs add or drop IR blocks.
        let found = (j..(j + 24).min(ir.len())).find(|&k| match &want {
            Some(w) => ir[k].para.is_some_and(|p| {
                let t = norm(&ir_text(p));
                t == *w || (w.chars().count() >= 12 && t.chars().count() >= 12 && t.chars().take(24).eq(w.chars().take(24)))
            }),
            None => ir[k].table,
        });
        let Some(k) = found else { continue };
        if ir[k].table {
            tables.insert(ir[k].key, b);
        }
        if let Some(p) = ir[k].para {
            let mut runs = Vec::with_capacity(p.runs.len());
            let (mut oxi, mut vis) = (0, 0);
            for r in &p.runs {
                let del = r.tracked_change.as_ref().is_some_and(|t| t.change_type == "delete" || t.change_type == "moveFrom");
                let n = if r.text == ZWSP { 0 } else { r.text.chars().count() };
                runs.push((oxi, vis, del));
                oxi += n;
                if !del {
                    vis += n;
                }
            }
            out.insert(ir[k].key, ParaMap { block: b, runs });
        }
        j = k + 1;
    }
    (out, tables)
}

/// Text the session knows outside the body's top-level paragraphs, for anchoring layout
/// items that Oxi doesn't map to a source position.
#[derive(Default)]
pub struct Anchors {
    /// Paragraphs of each table cell, keyed by the cell's absolute path (`[table, row, cell]`).
    pub cells: HashMap<Vec<i64>, Vec<(usize, String)>>,
    /// Header, footer, footnote and text box paragraphs: (story key, path, text).
    pub stories: Vec<(String, Vec<i64>, String)>,
}

/// Where `item` (painted text) sits in `text`, searching from `cursor` (a few characters of
/// slack for spaces and field results), then from the start. Returns (offset, length).
fn find_in(text: &[char], cursor: usize, item: &str) -> Option<(usize, usize)> {
    let needle: Vec<char> = item.chars().collect();
    if needle.is_empty() || needle.len() > text.len() {
        return None;
    }
    let trimmed: Vec<char> = item.trim().chars().collect();
    if trimmed.is_empty() {
        // A space: it belongs right at the cursor if the text has a space there.
        return (cursor < text.len() && text[cursor].is_whitespace()).then_some((cursor, needle.len().min(text.len() - cursor)));
    }
    let at = |from: usize, to: usize| (from..=to.min(text.len().saturating_sub(needle.len()))).find(|&i| text[i..i + needle.len()] == needle[..]);
    at(cursor, cursor + 3).or_else(|| at(0, text.len())).map(|i| (i, needle.len()))
}

fn color(c: &Option<String>) -> Value {
    match c.as_deref() {
        Some(s) if !s.is_empty() && s != "auto" => json!(if s.starts_with('#') { s.to_string() } else { format!("#{s}") }),
        _ => Value::Null,
    }
}

/// Word's highlight names (Oxi passes them through unless an env var is set).
fn highlight(h: &Option<String>) -> Value {
    let Some(h) = h.as_deref() else { return Value::Null };
    let hex = match h {
        "yellow" => "FFFF00", "green" => "00FF00", "cyan" => "00FFFF", "magenta" => "FF00FF", "blue" => "0000FF",
        "red" => "FF0000", "darkBlue" => "000080", "darkCyan" => "008080", "darkGreen" => "008000",
        "darkMagenta" => "800080", "darkRed" => "800000", "darkYellow" => "808000", "darkGray" => "808080",
        "lightGray" => "C0C0C0", "black" => "000000", "white" => "FFFFFF", "none" => return Value::Null,
        other if other.len() == 6 && other.chars().all(|c| c.is_ascii_hexdigit()) => other,
        _ => return Value::Null,
    };
    json!(format!("#{hex}"))
}

fn r1(v: f32) -> f64 {
    (v as f64 * 100.0).round() / 100.0
}

/// Lay out `bytes` and build the display list. `kinds`/`texts` describe the editor's blocks.
/// `first_block` is added to every block anchor (windowed layout lays out a slice of the body).
pub fn render(bytes: &[u8], kinds: &[&str], texts: &[Option<String>], first_block: usize, anchors: &Anchors) -> Result<Value, String> {
    let doc = oxidocs_core::parse_docx(bytes).map_err(|e| format!("layout: {e:?}"))?;
    let (map, tables) = align(&doc, kinds, texts);
    let story_chars: Vec<Vec<char>> = anchors.stories.iter().map(|(_, _, t)| t.chars().collect()).collect();
    let mut cell_cursor: HashMap<Vec<i64>, usize> = HashMap::new();
    // Comments are shown in the side panel, not as balloons that narrow the page.
    let result = LayoutEngine::for_document(&doc).with_show_comments(false).layout(&doc);
    let mut images: HashMap<String, String> = HashMap::new();
    let mut pages = Vec::with_capacity(result.pages.len());
    for page in &result.pages {
        let mut items = Vec::with_capacity(page.elements.len());
        // Lines that hold body text: an unanchored item on such a line, left of the text,
        // is a list number or bullet, not header/footnote text.
        let body_lines: Vec<(f32, f32)> = page.elements.iter().filter(|e| e.paragraph_index.is_some() && matches!(e.content, LayoutContent::Text { .. })).map(|e| (e.y, e.x)).collect();
        let mut story_cursor: Vec<usize> = vec![0; anchors.stories.len()];
        let mut current_story: Option<usize> = None;
        for e in &page.elements {
            let (x, y, w, h) = (r1(e.x), r1(e.y), r1(e.width), r1(e.height));
            match &e.content {
                LayoutContent::Text { text, font_size, font_family, bold, italic, underline, strikethrough, double_strikethrough, color: c, highlight: hl, is_vertical, .. } => {
                    if text.is_empty() {
                        continue;
                    }
                    let placeholder = text == ZWSP;
                    let text = &if placeholder { String::new() } else { text.clone() };
                    let family = font_family.clone().unwrap_or_else(|| "Calibri".into());
                    let base = e.baseline_offset.unwrap_or_else(|| e.text_y_off - 1.0 + metrics().get(&family).baseline_ascent() * *font_size);
                    let mut item = json!({
                        "k": "t", "x": x, "y": y, "w": w, "h": h, "text": text, "font": family, "size": r1(*font_size),
                        "base": r1(base),
                    });
                    let o = item.as_object_mut().unwrap();
                    if *bold { o.insert("b".into(), json!(true)); }
                    if *italic { o.insert("i".into(), json!(true)); }
                    if *underline { o.insert("u".into(), json!(true)); }
                    if *strikethrough || *double_strikethrough { o.insert("s".into(), json!(true)); }
                    if *is_vertical { o.insert("vert".into(), json!(true)); }
                    if let Some(c) = color(c).as_str() { o.insert("color".into(), json!(c)); }
                    if let Some(c) = highlight(hl).as_str() { o.insert("hl".into(), json!(c)); }
                    let matched_text = e.source_text.clone().unwrap_or_else(|| text.clone());
                    // Table cells: Oxi gives the cell, not the offset; find the text in the cell's paragraph.
                    if let (Some(pi), Some(r), Some(c), Some(cpi)) = (e.paragraph_index, e.cell_row_index, e.cell_col_index, e.cell_paragraph_index) {
                        let sec = e.source_container_index.unwrap_or(0);
                        if let Some(&tb) = tables.get(&(sec, pi)) {
                            let mut cell: Vec<i64> = vec![(tb + first_block) as i64];
                            for &(ar, ac, apos) in &e.cell_ancestor_path {
                                cell.extend([ar as i64, ac as i64, apos as i64]);
                            }
                            cell.extend([r as i64, c as i64]);
                            if let Some((idx, ptext)) = anchors.cells.get(&cell).and_then(|v| v.get(cpi)) {
                                let mut path = cell.clone();
                                path.push(*idx as i64);
                                let chars: Vec<char> = ptext.chars().collect();
                                let cur = cell_cursor.entry(path.clone()).or_insert(0);
                                let found = if placeholder { Some((0, 0)) } else { find_in(&chars, *cur, &matched_text) };
                                if let Some((off, n)) = found {
                                    *cur = off + n;
                                    o.insert("p".into(), json!(path));
                                    o.insert("off".into(), json!(off));
                                    o.insert("n".into(), json!(n));
                                }
                            }
                        }
                    } else if e.paragraph_index.is_none() && !anchors.stories.is_empty() {
                        // Headers, footers, footnotes, text boxes: match against their paragraphs.
                        let marker = body_lines.iter().any(|&(y, x)| (y - e.y).abs() < 1.0 && x > e.x);
                        if !marker {
                            let try_at = |k: usize, cursor: &[usize]| find_in(&story_chars[k], cursor[k], &matched_text).filter(|&(off, _)| off <= cursor[k] + 3);
                            // An empty paragraph's placeholder: the next empty candidate.
                            let empty = placeholder.then(|| {
                                let from = current_story.map(|k| k + 1).unwrap_or(0);
                                (from..story_chars.len()).chain(0..from).find(|&k| story_chars[k].is_empty() && story_cursor[k] == 0).map(|k| (k, (0, 0)))
                            }).flatten();
                            let pick = empty.or_else(|| if placeholder { None } else { current_story.and_then(|k| try_at(k, &story_cursor).map(|f| (k, f)))
                                .or_else(|| (0..story_chars.len()).find_map(|k| try_at(k, &story_cursor).map(|f| (k, f))))
                                .or_else(|| (0..story_chars.len()).find_map(|k| find_in(&story_chars[k], 0, &matched_text).map(|f| (k, f)))) });
                            if let Some((k, (off, n))) = pick {
                                story_cursor[k] = if placeholder { 1 } else { off + n };
                                current_story = Some(k);
                                let (story, path, _) = &anchors.stories[k];
                                if !story.is_empty() {
                                    o.insert("st".into(), json!(story));
                                }
                                o.insert("p".into(), json!(path));
                                o.insert("off".into(), json!(off));
                                o.insert("n".into(), json!(if placeholder { 0 } else { n }));
                            }
                        }
                    }
                    // Anchor body text to the editing model.
                    if let (Some(pi), Some(sec)) = (e.paragraph_index, e.source_container_index.or(Some(0))) {
                        if e.cell_row_index.is_none() {
                            if let Some(pm) = map.get(&(sec, pi)) {
                                let len = if placeholder { 0 } else { e.source_char_len.unwrap_or_else(|| text.chars().count()) };
                                let (vis, n) = pm.visible(e.run_index.unwrap_or(0), e.char_offset.unwrap_or(0), len);
                                o.insert("blk".into(), json!(pm.block + first_block));
                                o.insert("p".into(), json!([pm.block + first_block]));
                                o.insert("off".into(), json!(vis));
                                if n == 0 && len > 0 {
                                    o.insert("del".into(), json!(true));
                                } else {
                                    o.insert("n".into(), json!(n));
                                }
                            }
                        }
                    }
                    items.push(item);
                }
                LayoutContent::Image { data, content_type, crop } => {
                    let id = format!("{:016x}", fnv(data));
                    images.entry(id.clone()).or_insert_with(|| {
                        let ct = content_type.clone().unwrap_or_else(|| sniff(data).into());
                        format!("data:{ct};base64,{}", base64::engine::general_purpose::STANDARD.encode(data))
                    });
                    items.push(json!({"k": "img", "x": x, "y": y, "w": w, "h": h, "id": id, "crop": crop.map(|(t, r, b, l)| vec![t, r, b, l])}));
                }
                LayoutContent::TableBorder { x1, y1, x2, y2, color: c, width, style } => {
                    items.push(json!({"k": "line", "x1": r1(*x1), "y1": r1(*y1), "x2": r1(*x2), "y2": r1(*y2), "w": r1(*width), "color": color(c), "dash": style}));
                }
                LayoutContent::CellShading { color: c } => {
                    items.push(json!({"k": "rect", "x": x, "y": y, "w": w, "h": h, "fill": color(&Some(c.clone()))}));
                }
                LayoutContent::BoxRect { fill, stroke_color, stroke_width, corner_radius } => {
                    items.push(json!({"k": "rect", "x": x, "y": y, "w": w, "h": h, "fill": color(fill), "stroke": color(stroke_color), "sw": r1(*stroke_width), "r": r1(*corner_radius)}));
                }
                LayoutContent::WatermarkText { text, color: c, rotation, .. } => {
                    items.push(json!({"k": "wm", "x": x, "y": y, "w": w, "h": h, "text": text, "color": color(c), "rot": r1(*rotation)}));
                }
                LayoutContent::ClipStart => items.push(json!({"k": "clip", "x": x, "y": y, "w": w, "h": h})),
                LayoutContent::ClipEnd => items.push(json!({"k": "unclip"})),
                _ => {}
            }
        }
        pages.push(json!({"w": r1(page.width), "h": r1(page.height), "items": items}));
    }
    Ok(json!({"pages": pages, "images": images}))
}

fn fnv(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data.iter().step_by((data.len() / 4096).max(1)) {
        h = (h ^ *b as u64).wrapping_mul(0x100000001b3);
    }
    h ^ data.len() as u64
}

fn sniff(d: &[u8]) -> &'static str {
    match d {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xFF, 0xD8, ..] => "image/jpeg",
        [b'G', b'I', b'F', ..] => "image/gif",
        [b'B', b'M', ..] => "image/bmp",
        _ => "application/octet-stream",
    }
}

/// Per page, the block that starts cleanly at its top (its first line is the block's first);
/// per block, the first page it appears on.
pub fn page_index(pages: &[Value], n_blocks: usize) -> (Vec<Option<usize>>, Vec<Option<usize>>) {
    let mut first_page = vec![None; n_blocks];
    let mut clean = Vec::with_capacity(pages.len());
    for (pi, page) in pages.iter().enumerate() {
        let items = page["items"].as_array().cloned().unwrap_or_default();
        let mut top: Option<(f64, usize, u64)> = None;
        for it in &items {
            let (Some(b), Some(y)) = (it["blk"].as_u64(), it["y"].as_f64()) else { continue };
            let b = b as usize;
            if b < n_blocks && first_page[b].is_none() {
                first_page[b] = Some(pi);
            }
            let off = it["off"].as_u64().unwrap_or(0);
            if top.is_none_or(|(ty, tb, _)| y < ty - 0.5 || (y < ty + 0.5 && b < tb)) {
                top = Some((y, b, off));
            }
        }
        clean.push(top.filter(|&(_, b, off)| off == 0 && first_page.get(b).copied().flatten() == Some(pi)).map(|(_, b, _)| b));
    }
    (clean, first_page)
}

/// A cached page with block anchors after the edited one shifted by `delta`.
pub fn shift_blocks(page: &Value, after: usize, delta: i64) -> Value {
    if delta == 0 {
        return page.clone();
    }
    let mut p = page.clone();
    if let Some(items) = p["items"].as_array_mut() {
        for it in items {
            if let Some(b) = it["blk"].as_u64() {
                if b as usize > after {
                    it["blk"] = json!((b as i64 + delta).max(0));
                }
            }
            if it.get("st").is_none() {
                if let Some(b) = it["p"].get(0).and_then(|v| v.as_i64()) {
                    if b as usize > after {
                        it["p"][0] = json!((b + delta).max(0));
                    }
                }
            }
        }
    }
    p
}
