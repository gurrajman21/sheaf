//! Phase D0 runner: one .docx through one engine (rdocx or Oxi), JSON result on stdout.
//!
//! Usage: docx_bench <job.json>
//!
//! Job: {"engine", "src", "layout_src", "roundtrip_dst", "edit_dst"?, "edits"?:
//!       {"target": <paragraph text, whitespace removed>, "pt_index": <index among
//!        top-level w:p/w:tbl>, "p_index": <index among top-level w:p>, "has_table": bool}}
//!
//! Stages: open (original), layout (of `layout_src`, normally a copy with Word's cached
//! page breaks removed), roundtrip save (no edits), edit save (scripted edits).
//! Per page we report the first ~120 characters of body text from the top of the page,
//! so the driver can compare page starts with Word's.

use std::collections::BTreeMap;
use std::time::Instant;

use serde_json::{json, Value};

const APPEND: &str = " [EDIT-APPEND]";
const NEW_PARA: &str = "[EDIT-NEW-PARAGRAPH]";
const CELL: &str = "[EDIT-CELL]";

fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

struct Out {
    stages: BTreeMap<String, Value>,
    fields: serde_json::Map<String, Value>,
}

impl Out {
    fn stage<T>(&mut self, name: &str, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        let t = Instant::now();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
            .unwrap_or_else(|p| Err(format!("panic: {}", panic_msg(&p))));
        let ms = (t.elapsed().as_secs_f64() * 1000.0 * 10.0).round() / 10.0;
        self.stages.insert(name.into(), json!({"ok": r.is_ok(), "ms": ms}));
        r.map_err(|e| format!("{name}: {e}"))
    }
}

fn panic_msg(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<String>().cloned()
        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown".into())
}

/// Body text of a page in reading order, starting from its top-left element.
/// `items` are (x, y, text) in the engine's emission order.
fn page_text(items: &[(f64, f64, String)]) -> String {
    let Some(top) = items.iter().map(|i| i.1).reduce(f64::min) else { return String::new() };
    let start = items.iter().enumerate()
        .filter(|(_, i)| i.1 <= top + 1.5)
        .min_by(|a, b| a.1.0.total_cmp(&b.1.0))
        .map(|(n, _)| n).unwrap_or(0);
    let mut s = String::new();
    for i in &items[start..] {
        s.push_str(&i.2);
        if s.chars().count() > 160 { break; }
    }
    s.chars().take(160).collect()
}

// ------------------------------------------------------------------------------------- rdocx

mod rd {
    use super::*;
    use rdocx::Document;
    use rdocx_layout::{PositionedElement, WordStory};

    fn collect(el: &PositionedElement, wl: &rdocx_layout::WordLayoutResult, out: &mut Vec<(f64, f64, String)>) {
        let body = |src: &Option<rdocx_layout::SourceSpan>| {
            src.as_ref().and_then(|s| wl.source_node(s.node)).map(|p| matches!(p.story, WordStory::Document)).unwrap_or(false)
        };
        if std::env::var("BENCH_DEBUG").is_ok() {
            match el {
                PositionedElement::Text(g) => eprintln!("text {:?} src={:?} path={:?}", g.text, g.source.is_some(), g.source.as_ref().and_then(|s| wl.source_node(s.node))),
                PositionedElement::MultilingualText(g) => eprintln!("mtext {:?}", g.logical_text),
                PositionedElement::Group(g) => eprintln!("group {}", g.children.len()),
                PositionedElement::MarkedContent { children, .. } => eprintln!("marked {}", children.len()),
                _ => eprintln!("other"),
            }
        }
        match el {
            PositionedElement::Text(g) if body(&g.source) => out.push((g.origin.x, g.origin.y, g.text.clone())),
            PositionedElement::MultilingualText(g) if body(&g.source) => out.push((g.origin.x, g.origin.y, g.logical_text.clone())),
            PositionedElement::Group(g) => for c in &g.children { collect(c, wl, out) },
            PositionedElement::MarkedContent { children, .. } => for c in children { collect(c, wl, out) },
            _ => {}
        }
    }

    pub fn run(job: &Value, out: &mut Out) -> Result<(), String> {
        let src = job["src"].as_str().unwrap();
        let mut doc = out.stage("open", || Document::open(src).map_err(|e| e.to_string()))?;
        let layout_src = job["layout_src"].as_str().unwrap_or(src);
        let starts = out.stage("layout", || {
            let ldoc = Document::open(layout_src).map_err(|e| e.to_string())?;
            let wl = ldoc.layout().map_err(|e| e.to_string())?;
            Ok(wl.layout.pages.iter().map(|p| {
                let mut items = Vec::new();
                for e in &p.elements { collect(e, &wl, &mut items) }
                page_text(&items)
            }).collect::<Vec<_>>())
        })?;
        out.fields.insert("pages".into(), json!(starts.len()));
        out.fields.insert("starts".into(), json!(starts));
        out.stage("roundtrip", || {
            let mut d = Document::open(src).map_err(|e| e.to_string())?;
            d.save(job["roundtrip_dst"].as_str().unwrap()).map_err(|e| e.to_string())
        })?;
        if let (Some(dst), Some(edits)) = (job["edit_dst"].as_str(), job.get("edits").filter(|e| !e.is_null())) {
            let mut errors = Vec::<String>::new();
            out.stage("edit", || {
                let target = edits["target"].as_str().unwrap_or("");
                let pidx = doc.paragraphs().iter().position(|p| squash(&p.text()) == target);
                let bidx = doc.body_items().position(|b| match b {
                    rdocx::BodyItemRef::Paragraph(p) => squash(&p.text()) == target,
                    _ => false,
                });
                match pidx {
                    Some(i) => {
                        if let Some(mut p) = doc.paragraph_mut(i) {
                            match p.run_mut(0) {
                                Some(mut r) => { r.set_bold(true); }
                                None => errors.push("bold: no editable first run".into()),
                            }
                            p.add_run(APPEND);
                        }
                    }
                    None => errors.push("target paragraph not found".into()),
                }
                match bidx {
                    Some(b) => { doc.insert_paragraph(b + 1, NEW_PARA); }
                    None => errors.push("insert: target not a body item".into()),
                }
                if edits["has_table"].as_bool().unwrap_or(false) {
                    match doc.table_mut(0).and_then(|mut t| t.cell(0, 0).map(|mut c| c.set_text(CELL))) {
                        Some(_) => {}
                        None => errors.push("cell: no table cell (0,0)".into()),
                    }
                }
                doc.save(dst).map_err(|e| e.to_string())
            })?;
            if !errors.is_empty() { out.fields.insert("edit_errors".into(), json!(errors)); }
        }
        Ok(())
    }
}

// --------------------------------------------------------------------------------------- Oxi

mod ox {
    use super::*;
    use oxidocs_core::editor::RunProps;
    use oxidocs_core::layout::{LayoutContent, LayoutEngine};
    use oxidocs_core::{parse_docx, DocxEditor};

    pub fn layout_starts(bytes: &[u8]) -> Result<Vec<String>, String> {
        let doc = parse_docx(bytes).map_err(|e| format!("{e:?}"))?;
        let res = LayoutEngine::for_document(&doc).layout(&doc);
        Ok(res.pages.iter().map(|p| {
            let items: Vec<(f64, f64, String)> = p.elements.iter()
                .filter(|e| e.paragraph_index.is_some())
                .filter_map(|e| match &e.content {
                    LayoutContent::Text { text, .. } => Some((e.x as f64, e.y as f64, text.clone())),
                    _ => None,
                }).collect();
            page_text(&items)
        }).collect())
    }

    pub fn run(job: &Value, out: &mut Out) -> Result<(), String> {
        let src = job["src"].as_str().unwrap();
        let bytes = std::fs::read(src).map_err(|e| e.to_string())?;
        out.stage("open", || parse_docx(&bytes).map(|_| ()).map_err(|e| format!("{e:?}")))?;
        let layout_src = job["layout_src"].as_str().unwrap_or(src);
        let lbytes = std::fs::read(layout_src).map_err(|e| e.to_string())?;
        let starts = out.stage("layout", || layout_starts(&lbytes))?;
        out.fields.insert("pages".into(), json!(starts.len()));
        out.fields.insert("starts".into(), json!(starts));
        out.stage("roundtrip", || {
            let ed = DocxEditor::new(&bytes).map_err(|e| format!("{e:?}"))?;
            let data = ed.save().map_err(|e| format!("{e:?}"))?;
            std::fs::write(job["roundtrip_dst"].as_str().unwrap(), data).map_err(|e| e.to_string())
        })?;
        if let (Some(dst), Some(edits)) = (job["edit_dst"].as_str(), job.get("edits").filter(|e| !e.is_null())) {
            let mut errors = Vec::<String>::new();
            out.stage("edit", || {
                let mut ed = DocxEditor::new(&bytes).map_err(|e| format!("{e:?}"))?;
                let runs = ed.addressable_runs().map_err(|e| format!("{e:?}"))?;
                let target = edits["target"].as_str().unwrap_or("");
                let p = edits["p_index"].as_u64().map(|v| v as usize);
                let pt = edits["pt_index"].as_u64().map(|v| v as usize);
                let found = p.filter(|&i| runs.get(i).map(|r| squash(&r.concat()) == target).unwrap_or(false))
                    .or_else(|| runs.iter().position(|r| squash(&r.concat()) == target));
                match found {
                    Some(i) => {
                        let n = runs[i].len();
                        if n > 0 {
                            ed.set_run_format(i, 0, RunProps { bold: Some(true), ..Default::default() });
                        } else {
                            errors.push("bold: no runs".into());
                        }
                        ed.insert_run(i, n, APPEND, None);
                    }
                    None => errors.push("target paragraph not found".into()),
                }
                if edits["has_table"].as_bool().unwrap_or(false) {
                    ed.set_cell_text(0, 0, 0, CELL);
                }
                match pt {
                    Some(b) => ed.insert_paragraph(b + 1, NEW_PARA, None, None),
                    None => errors.push("insert: no body index".into()),
                }
                let data = ed.save().map_err(|e| format!("{e:?}"))?;
                std::fs::write(dst, data).map_err(|e| e.to_string())
            })?;
            if !errors.is_empty() { out.fields.insert("edit_errors".into(), json!(errors)); }
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------------------- Sheaf

/// Sheaf's own writer (`ooxml::docx`) for saving; Oxi for layout (the Phase D0 pick).
mod sh {
    use super::*;
    use ooxml::docx::props::RunFormat;
    use ooxml::docx::table::Table;
    use ooxml::docx::{Block, Docx, DocxEdits, Para, Rev};

    pub fn run(job: &Value, out: &mut Out, track: bool) -> Result<(), String> {
        let src = job["src"].as_str().unwrap();
        let bytes = std::fs::read(src).map_err(|e| e.to_string())?;
        let docx = out.stage("open", || Docx::open(&bytes))?;
        let layout_src = job["layout_src"].as_str().unwrap_or(src);
        let lbytes = std::fs::read(layout_src).map_err(|e| e.to_string())?;
        let starts = out.stage("layout", || ox::layout_starts(&lbytes))?;
        out.fields.insert("pages".into(), json!(starts.len()));
        out.fields.insert("starts".into(), json!(starts));
        out.stage("roundtrip", || {
            let data = docx.save(&DocxEdits::default())?;
            std::fs::write(job["roundtrip_dst"].as_str().unwrap(), data).map_err(|e| e.to_string())
        })?;
        if let (Some(dst), Some(edits)) = (job["edit_dst"].as_str(), job.get("edits").filter(|e| !e.is_null())) {
            let mut errors = Vec::<String>::new();
            out.stage("edit", || {
                let body = docx.body()?;
                let rev = track.then(|| Rev::new("Sheaf", "2026-10-18T10:00:00Z", docx.max_annotation_id() + 1));
                let rev = rev.as_ref();
                let target = edits["target"].as_str().unwrap_or("");
                let mut blocks: Vec<Block> = body.identity();
                let found = body.elements.iter().enumerate().filter(|(_, e)| e.name == "p")
                    .find(|(i, _)| Para::parse(body.xml(*i)).map(|p| squash(&p.text()) == target).unwrap_or(false))
                    .map(|(i, _)| i);
                if edits["has_table"].as_bool().unwrap_or(false) {
                    match body.elements.iter().position(|e| e.name == "tbl") {
                        Some(ti) => {
                            let mut t = Table::parse(body.xml(ti))?;
                            match t.cell_mut(0, 0) {
                                Some(c) => {
                                    c.set_text(CELL);
                                    blocks[ti] = Block::Xml(t.to_xml());
                                }
                                None => errors.push("cell: no cell (0,0)".into()),
                            }
                        }
                        None => errors.push("cell: no table".into()),
                    }
                }
                match found {
                    Some(i) => {
                        let mut p = Para::parse(body.xml(i))?;
                        let chars: Vec<char> = p.text().chars().collect();
                        let first_text = p.run_ranges().into_iter()
                            .find(|&(a, b)| chars[a..b].iter().any(|c| !c.is_whitespace()));
                        if let Some((a, b)) = first_text {
                            p.format(a, b, &RunFormat { bold: Some(true), ..Default::default() }, rev);
                        } else {
                            errors.push("bold: no runs".into());
                        }
                        let len = p.len();
                        p.insert_text(len, APPEND, None, rev);
                        let new = if track {
                            let len = p.len();
                            let mut tail = p.split(len, rev);
                            tail.insert_text(0, NEW_PARA, None, rev);
                            tail
                        } else {
                            Para::new(NEW_PARA, None, None)
                        };
                        blocks[i] = Block::Xml(p.to_xml());
                        blocks.insert(i + 1, Block::Xml(new.to_xml()));
                    }
                    None => errors.push("target paragraph not found".into()),
                }
                let data = docx.save(&DocxEdits { body: Some(blocks), ..Default::default() })?;
                // Reload check: both other engines must still open the saved file.
                let oxi_ok = oxidocs_core::parse_docx(&data).is_ok();
                let rdocx_ok = rdocx::Document::from_bytes(&data).is_ok();
                if !oxi_ok { errors.push("reload: oxi can't parse the saved file".into()); }
                if !rdocx_ok { errors.push("reload: rdocx can't open the saved file".into()); }
                std::fs::write(dst, data).map_err(|e| e.to_string())
            })?;
            if !errors.is_empty() { out.fields.insert("edit_errors".into(), json!(errors)); }
        }
        Ok(())
    }
}

// ------------------------------------------------------------- Sheaf: revisions and stories

mod sh2 {
    use super::*;
    use ooxml::docx::append::{self, ListKind};
    use ooxml::docx::props::ParaFormat;
    use ooxml::docx::revisions::Filter;
    use ooxml::docx::story::{HfKind, NoteKind};
    use ooxml::docx::{Block, Body, Docx, DocxEdits, Para};

    fn reload(data: &[u8], errors: &mut Vec<String>) {
        if oxidocs_core::parse_docx(data).is_err() { errors.push("reload: oxi can't parse the saved file".into()); }
        if rdocx::Document::from_bytes(data).is_err() { errors.push("reload: rdocx can't open the saved file".into()); }
    }

    /// Accept or reject every tracked change; `edit_dst` gets the result.
    pub fn revisions(job: &Value, out: &mut Out, accept: bool) -> Result<(), String> {
        let src = job["src"].as_str().unwrap();
        let bytes = std::fs::read(src).map_err(|e| e.to_string())?;
        let docx = out.stage("open", || Docx::open(&bytes))?;
        out.fields.insert("revisions_before".into(), json!(docx.revisions().len()));
        if let Some(dst) = job["edit_dst"].as_str() {
            let mut errors = Vec::new();
            let mut counts = (0, 0);
            out.stage("edit", || {
                let mut e = DocxEdits::default();
                let n = docx.resolve_revisions(&mut e, accept, &Filter::default())?;
                let data = docx.save(&e)?;
                reload(&data, &mut errors);
                counts = (n, Docx::open(&data).map(|d| d.revisions().len()).unwrap_or(usize::MAX));
                std::fs::write(dst, data).map_err(|e| e.to_string())
            })?;
            let (n, after) = counts;
            out.fields.insert("resolved".into(), json!(n));
            out.fields.insert("revisions_after".into(), json!(after));
            if !errors.is_empty() { out.fields.insert("edit_errors".into(), json!(errors)); }
        }
        Ok(())
    }


    /// Header edit, new footnote, list numbering and an inline picture on the target paragraph.
    pub fn stories(job: &Value, out: &mut Out) -> Result<(), String> {
        let src = job["src"].as_str().unwrap();
        let bytes = std::fs::read(src).map_err(|e| e.to_string())?;
        let docx = out.stage("open", || Docx::open(&bytes))?;
        let (Some(dst), Some(edits)) = (job["edit_dst"].as_str(), job.get("edits").filter(|e| !e.is_null())) else { return Ok(()) };
        let mut errors = Vec::<String>::new();
        let mut done = serde_json::Map::new();
        out.stage("edit", || {
            let mut e = DocxEdits::default();
            // 1. Header: append to the first paragraph of the first header.
            if let Some(h) = docx.headers_footers().into_iter().find(|h| h.kind == HfKind::Header) {
                let src = docx.read_part(&h.part)?;
                let body = Body::parse_in(&src, "hdr")?;
                let mut blocks = body.identity();
                if let Some(i) = body.paragraph(0) {
                    let mut p = Para::parse(body.xml(i))?;
                    let n = p.len();
                    p.insert_text(n, " [EDIT-HDR]", None, None);
                    blocks[i] = Block::Xml(p.to_xml());
                    docx.edit_header_footer(&mut e, &h.part, &blocks)?;
                    done.insert("header".into(), json!(h.part));
                }
            }
            // 2-4. Footnote, list, picture on the target paragraph.
            let body = docx.body()?;
            let target = edits["target"].as_str().unwrap_or("");
            let found = body.elements.iter().enumerate().filter(|(_, x)| x.name == "p")
                .find(|(i, _)| Para::parse(body.xml(*i)).map(|p| squash(&p.text()) == target).unwrap_or(false))
                .map(|(i, _)| i);
            let Some(i) = found else {
                errors.push("target paragraph not found".into());
                return Err("no target".into());
            };
            let mut p = Para::parse(body.xml(i))?;
            let id = docx.queue_note(&mut e, NoteKind::Footnote, "[EDIT-FN]")?;
            let n = p.len();
            p.insert_note_reference(n, NoteKind::Footnote, id, None);
            done.insert("footnote".into(), json!(id));
            let num = append::add_list(&docx, &mut e, ListKind::Decimal);
            p.format_paragraph(&ParaFormat { numbering: Some(Some(append::num_pr(num, 0))), ..Default::default() }, None);
            done.insert("num_id".into(), json!(num));
            let png = tiny_png();
            let rid = append::add_image(&docx, &mut e, &png, "png")?;
            let pid = append::max_drawing_id(&docx.document) + 1;
            p.insert_object(0, &append::inline_picture(&rid, 32, 32, pid, "Sheaf picture"), None, None);
            done.insert("image_rel".into(), json!(rid));
            let mut blocks = body.identity();
            blocks[i] = Block::Xml(p.to_xml());
            e.body = Some(blocks);
            let data = docx.save(&e)?;
            reload(&data, &mut errors);
            std::fs::write(dst, data).map_err(|e| e.to_string())
        })?;
        out.fields.insert("stories".into(), Value::Object(done));
        if !errors.is_empty() { out.fields.insert("edit_errors".into(), json!(errors)); }
        Ok(())
    }

    /// A valid 1x1 PNG.
    fn tiny_png() -> Vec<u8> {
        vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0,
             0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x60, 0x60, 0x60, 0xF8, 0x0F, 0, 0x01, 0x04,
             0x01, 0x00, 0x5F, 0xE5, 0xC3, 0x4B, 0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82]
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: docx_bench <job.json>");
    let job: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("read job")).expect("job json");
    let engine = job["engine"].as_str().unwrap_or("").to_string();
    let mut out = Out { stages: BTreeMap::new(), fields: serde_json::Map::new() };
    // Engines print diagnostics; keep stdout for the JSON line only.
    let r = match engine.as_str() {
        "rdocx" => rd::run(&job, &mut out),
        "oxi" => ox::run(&job, &mut out),
        "sheaf" => sh::run(&job, &mut out, false),
        "sheaf-track" => sh::run(&job, &mut out, true),
        "sheaf-accept" => sh2::revisions(&job, &mut out, true),
        "sheaf-reject" => sh2::revisions(&job, &mut out, false),
        "sheaf-stories" => sh2::stories(&job, &mut out),
        e => Err(format!("unknown engine {e}")),
    };
    let mut res = out.fields;
    res.insert("engine".into(), json!(engine));
    res.insert("stages".into(), json!(out.stages));
    res.insert("ok".into(), json!(r.is_ok()));
    if let Err(e) = r { res.insert("error".into(), json!(e.chars().take(800).collect::<String>())); }
    println!("{}", Value::Object(res));
}
