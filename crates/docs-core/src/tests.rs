use serde_json::json;

use super::*;
use crate::blocks::Loc;

/// Tests must never write autosaves or versions into the real app data folder.
fn isolate() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| std::env::set_var("QUIRE_DATA_DIR", std::env::temp_dir().join(format!("quire-test-data-{}", std::process::id()))));
}

fn session(paras: &[(&str, &str)]) -> Session {
    isolate();
    let mut s = Session::new();
    s.load(blank::document(paras)).unwrap();
    s
}

fn texts(s: &Session) -> Vec<String> {
    (0..s.state.blocks.len()).filter_map(|b| s.para(b).map(|p| p.text())).collect()
}

#[test]
fn typing_enter_and_backspace() {
    let mut s = session(&[("Title", "Hello"), ("", "World")]);
    let c = s.insert_text(Loc::body(0, 5), ", there").unwrap();
    assert_eq!(c, Loc::body(0, 12));
    let c = s.split(Loc::body(0, 6)).unwrap();
    assert_eq!(c, Loc::body(1, 0));
    assert_eq!(texts(&s), vec!["Hello,", " there", "World"]);
    // Backspace at the start of a paragraph joins it to the previous one.
    s.delete(Loc::body(0, 6), Loc::body(1, 0)).unwrap();
    assert_eq!(texts(&s), vec!["Hello, there", "World"]);
    let c = s.insert_text(Loc::body(1, 5), "\nNew line\nAnother").unwrap();
    assert_eq!(texts(&s), vec!["Hello, there", "World", "New line", "Another"]);
    assert_eq!(c, Loc::body(3, 7));
}

#[test]
fn delete_across_paragraphs_and_undo_redo() {
    let mut s = session(&[("", "One two"), ("", "Three"), ("", "Four five")]);
    s.delete(Loc::body(0, 4), Loc::body(2, 5)).unwrap();
    assert_eq!(texts(&s), vec!["One five"]);
    s.dispatch("undo", &json!({})).unwrap();
    assert_eq!(texts(&s), vec!["One two", "Three", "Four five"]);
    s.dispatch("redo", &json!({})).unwrap();
    assert_eq!(texts(&s), vec!["One five"]);
}

#[test]
fn formatting_lists_and_save_roundtrip() {
    let dir = std::env::temp_dir().join(format!("quire-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("out.docx");
    let mut s = session(&[("", "Bold me please"), ("", "Item")]);
    s.format(Loc::body(0, 0), Loc::body(0, 4), &RunFormat { bold: Some(true), ..Default::default() }).unwrap();
    s.dispatch("para_format", &json!({"from": {"p": [1]}, "to": {"p": [1]}, "fmt": {"list": "bullet"}})).unwrap();
    s.dispatch("para_format", &json!({"from": {"p": [0]}, "to": {"p": [0]}, "fmt": {"style": "Heading1", "align": "center"}})).unwrap();
    s.save(Some(&path)).unwrap();
    let mut r = Session::new();
    r.open(&path).unwrap();
    let doc = r.state.base.document.clone();
    assert!(doc.contains("<w:b/>") && doc.contains(r#"<w:pStyle w:val="Heading1"/>"#) && doc.contains(r#"<w:jc w:val="center"/>"#), "{doc}");
    assert!(doc.contains("<w:numPr>"));
    assert!(r.state.base.read_part("word/numbering.xml").is_ok());
    assert_eq!(texts(&r), vec!["Bold me please", "Item"]);
}

#[test]
fn tracked_editing_and_accept() {
    let mut s = session(&[("", "Original text")]);
    s.dispatch("track", &json!({"on": true})).unwrap();
    s.insert_text(Loc::body(0, 9), "new ").unwrap();
    s.delete(Loc::body(0, 0), Loc::body(0, 10)).unwrap();
    let revs = s.dispatch("revisions", &json!({})).unwrap();
    assert!(revs.as_array().unwrap().len() >= 2, "{revs}");
    assert_eq!(texts(&s), vec!["ew text"]);
    s.dispatch("accept_all", &json!({})).unwrap();
    assert_eq!(s.dispatch("revisions", &json!({})).unwrap().as_array().unwrap().len(), 0);
    assert_eq!(texts(&s), vec!["ew text"]);
}

#[test]
fn layout_anchors_text_to_blocks() {
    let mut s = session(&[("Title", "Report"), ("", "First paragraph of text.")]);
    s.insert_text(Loc::body(1, 6), "real ").unwrap();
    let l = s.dispatch("layout", &json!({})).unwrap();
    let items = l["pages"][0]["items"].as_array().unwrap();
    let real = items.iter().find(|i| i["text"] == "real").expect("inserted word laid out");
    assert_eq!(real["blk"], 1);
    assert_eq!(real["off"], 6);
    assert!(items.iter().any(|i| i["text"] == "Report" && i["blk"] == 0));
}

/// Windowed relayout must match a full layout on the pages it produces.
#[test]
fn windowed_layout_matches_full() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/docx/synthetic/long_flow.docx");
    if !path.exists() {
        return; // corpus not fetched
    }
    let mut s = Session::new();
    s.open(&path).unwrap();
    s.dispatch("layout", &json!({})).unwrap();
    // Edit a paragraph deep in the document.
    let n = s.state.blocks.len();
    let b = (n * 2 / 3..n).find(|&b| s.para(b).is_some_and(|p| p.len() > 3)).unwrap_or(n - 1);
    s.insert_text(Loc::body(b, 1), "EDIT").unwrap();
    let t = std::time::Instant::now();
    let quick = s.dispatch("layout", &json!({"quick": true})).unwrap();
    let quick_ms = t.elapsed().as_millis();
    let t = std::time::Instant::now();
    let full = s.dispatch("layout", &json!({})).unwrap();
    let full_ms = t.elapsed().as_millis();
    let (qp, fp) = (quick["pages"].as_array().unwrap(), full["pages"].as_array().unwrap());
    let texts = |p: &Value| p["items"].as_array().unwrap().iter().filter(|i| i["k"] == "t").map(|i| format!("{}@{:.0},{:.0}:{}", i["text"], i["x"].as_f64().unwrap(), i["y"].as_f64().unwrap(), i["blk"])).collect::<Vec<_>>();
    // Every page up to the window's end must be identical (the tail may be stale).
    let fresh = if quick["stale"] == false { qp.len() } else { qp.len().min(fp.len()) - 1 };
    let mut same = 0;
    for i in 0..fresh.min(fp.len()) {
        if texts(&qp[i]) == texts(&fp[i]) { same += 1; } else {
            let (a, b) = (texts(&qp[i]), texts(&fp[i]));
            let d = a.iter().zip(&b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
            eprintln!("page {i} differs at item {d}: quick {:?} full {:?} (len {} vs {})", a.get(d), b.get(d), a.len(), b.len());
        }
    }
    eprintln!("quick {quick_ms}ms vs full {full_ms}ms; {same}/{fresh} fresh pages identical; pages quick {} full {}", qp.len(), fp.len());
    assert!(same + 1 >= fresh, "windowed pages differ from full layout");
    assert!(quick_ms < full_ms, "windowed layout should be faster");
}

fn synthetic(name: &str) -> Option<Session> {
    isolate();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/docx/synthetic").join(name);
    if !path.exists() {
        return None;
    }
    let mut s = Session::new();
    s.open(&path).unwrap();
    Some(s)
}

fn loc_of(story: &str, path: &[i64], o: usize) -> Loc {
    Loc::parse(&json!({"s": story, "p": path, "o": o})).unwrap()
}

/// Typing in table cells (nested too), headers, footnotes and text boxes, then save + reopen.
#[test]
fn editing_every_story() {
    let Some(mut s) = synthetic("tables.docx") else { return };
    s.insert_text(loc_of("", &[1, 1, 0, 0], 5), "east").unwrap();
    s.insert_text(loc_of("", &[5, 0, 1, 0, 0, 0, 0], 1), " nested").unwrap();
    let paras = s.all_paragraphs();
    assert!(paras.iter().any(|(_, p, t)| p == &vec![1, 1, 0, 0] && t == "Northeast"), "{paras:?}");
    assert!(paras.iter().any(|(_, p, t)| p.len() == 7 && t == "x nested"));
    // Enter inside a cell adds a paragraph to that cell.
    let c = s.split(loc_of("", &[1, 1, 0, 0], 5)).unwrap();
    assert_eq!(c.path, vec![1, 1, 0, 1]);

    let Some(mut h) = synthetic("headers_footers.docx") else { return };
    let header = h.all_paragraphs().into_iter().find(|(st, _, t)| matches!(st, Story::Part(_)) && t.starts_with("Quarterly report")).unwrap();
    h.insert_text(Loc { story: header.0.clone(), path: header.1.clone(), o: 9 }, " (draft)").unwrap();
    let Some(mut f) = synthetic("footnotes.docx") else { return };
    f.insert_text(loc_of("fn:3", &[0], 29), " Checked.").unwrap();
    let Some(mut t) = synthetic("textbox.docx") else { return };
    t.insert_text(loc_of("", &[1, -1, 0], 8), " big").unwrap();

    let dir = std::env::temp_dir().join(format!("quire-stories-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (sess, name, want) in [(&mut s, "t.docx", "x nested"), (&mut h, "h.docx", "Quarterly (draft) report"), (&mut f, "f.docx", "page 30. Checked."), (&mut t, "x.docx", "Callout: big key")] {
        let p = dir.join(name);
        sess.save(Some(&p)).unwrap();
        let mut r = Session::new();
        r.open(&p).unwrap();
        let all: Vec<String> = r.all_paragraphs().into_iter().map(|(_, _, t)| t).collect();
        assert!(all.iter().any(|t| t.contains(want)), "{name}: {all:?}");
    }
}

#[test]
fn table_tools_and_page_setup() {
    let mut s = session(&[("", "Before"), ("", "After")]);
    let c = s.dispatch("table", &json!({"at": {"p": [0], "o": 6}, "rows": 2, "cols": 2})).unwrap();
    assert_eq!(c["p"], json!([1, 0, 0, 0]));
    let at = loc_of("", &[1, 0, 0, 0], 0);
    s.insert_text(at.clone(), "A").unwrap();
    s.table_op(at.clone(), "col_right", None).unwrap();
    s.table_op(at.clone(), "row_below", None).unwrap();
    s.table_op(at.clone(), "shade", Some("FFF2CC")).unwrap();
    let x = s.block_xml(1);
    let t = ooxml::docx::table::Table::parse(&x).unwrap();
    assert_eq!(t.rows().count(), 3);
    assert_eq!(t.grid().len(), 3);
    assert!(t.rows().all(|r| r.cells().count() == 3), "{x}");
    assert!(x.contains(r#"w:fill="FFF2CC""#));
    s.table_op(at.clone(), "merge", None).unwrap();
    let t = ooxml::docx::table::Table::parse(&s.block_xml(1)).unwrap();
    assert_eq!(t.rows().next().unwrap().cells().count(), 2);
    assert!(s.block_xml(1).contains(r#"<w:gridSpan w:val="2"/>"#));
    s.table_op(loc_of("", &[1, 1, 0, 0], 0), "delete_col", None).unwrap();
    s.table_op(at, "delete_table", None).unwrap();
    assert_eq!(s.kinds(), vec!["p", "p"]);

    s.page_setup(&json!({"size": "a4", "orientation": "landscape", "margins": "narrow", "cols": 2})).unwrap();
    let info = s.info();
    assert_eq!(info["page"], json!({"size": "a4", "orientation": "landscape", "margins": "narrow", "cols": 2}));
}

#[test]
fn comments_threads_and_single_change_review() {
    let mut s = session(&[("", "Revenue grew twelve percent."), ("", "Costs held.")]);
    let id = s.dispatch("comment_add", &json!({"from": {"p": [0], "o": 13}, "to": {"p": [0], "o": 27}, "text": "Source?"})).unwrap()["id"].as_i64().unwrap();
    let reply = s.dispatch("comment_add", &json!({"from": {"p": [0], "o": 0}, "text": "Q3 report, p. 4", "parent": id})).unwrap()["id"].as_i64().unwrap();
    s.dispatch("comment_resolve", &json!({"id": id, "done": true})).unwrap();
    s.dispatch("comment_edit", &json!({"id": reply, "text": "Q3 report, page 4"})).unwrap();
    let list = s.dispatch("comments", &json!({})).unwrap();
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 2);
    let root = list.iter().find(|c| c["id"] == id).unwrap();
    assert_eq!(root["done"], true);
    assert_eq!(root["from"], json!({"s": "", "p": [0], "o": 13}));
    assert_eq!(root["to"], json!({"s": "", "p": [0], "o": 27}));
    let r = list.iter().find(|c| c["id"] == reply).unwrap();
    assert_eq!(r["parent"], id);
    assert_eq!(r["text"], "Q3 report, page 4");
    // Saved and reopened, the thread survives.
    let path = std::env::temp_dir().join(format!("quire-comments-{}.docx", std::process::id()));
    s.save(Some(&path)).unwrap();
    let mut o = Session::new();
    o.open(&path).unwrap();
    assert_eq!(o.dispatch("comments", &json!({})).unwrap().as_array().unwrap().len(), 2);
    assert!(o.state.base.read_part("word/commentsExtended.xml").unwrap().contains("w15:paraIdParent"));
    // Deleting the root removes the reply and every anchor.
    o.dispatch("comment_delete", &json!({"id": id})).unwrap();
    assert_eq!(o.dispatch("comments", &json!({})).unwrap().as_array().unwrap().len(), 0);
    assert!(!o.block_xml(0).contains("comment"));

    // Review one change at a time.
    let mut t = session(&[("", "One two three")]);
    t.dispatch("track", &json!({"on": true})).unwrap();
    t.insert_text(Loc::body(0, 3), " and a half").unwrap();
    t.delete(Loc::body(0, 15), Loc::body(0, 19)).unwrap();
    let revs = t.dispatch("revisions", &json!({})).unwrap();
    let ins = revs.as_array().unwrap().iter().find(|r| r["kind"] == "ins").unwrap();
    assert_eq!(ins["at"]["p"], json!([0]));
    t.dispatch("reject", &json!({"ids": [ins["id"].clone()]})).unwrap();
    let left = t.dispatch("revisions", &json!({})).unwrap();
    assert_eq!(left.as_array().unwrap().len(), 1);
    assert_eq!(left[0]["kind"], "del");
}

#[test]
fn structure_toc_footnotes_page_numbers_links() {
    let mut s = session(&[("Title", "Report"), ("", "Intro text."), ("Heading1", "Results"), ("", "Numbers."), ("Heading2", "Detail"), ("", "More.")]);
    let outline = s.dispatch("outline", &json!({})).unwrap();
    let levels: Vec<(i64, i64)> = outline.as_array().unwrap().iter().map(|h| (h["p"][0].as_i64().unwrap(), h["level"].as_i64().unwrap())).collect();
    assert_eq!(levels, vec![(2, 0), (4, 1)], "{outline}");
    let after = s.dispatch("toc_insert", &json!({"at": {"p": [1], "o": 0}})).unwrap();
    let toc = s.toc_block().unwrap();
    assert_eq!(toc, 1);
    assert_eq!(after["p"], json!([2]));
    let x = s.block_xml(toc);
    assert!(x.contains(r#"TOC \o "1-3""#) && x.contains("Results") && x.contains("Detail") && x.contains(r#"w:val="TOC2""#), "{x}");
    s.dispatch("toc_update", &json!({})).unwrap();
    assert!(s.block_xml(s.toc_block().unwrap()).contains("Results"));

    let fnloc = s.dispatch("footnote_insert", &json!({"at": {"p": [3], "o": 5}, "text": "Source."})).unwrap();
    assert!(fnloc["s"].as_str().unwrap().starts_with("fn:"));
    s.insert_text(Loc::parse(&fnloc).unwrap(), " Checked.").unwrap();
    assert!(s.block_xml(3).contains("footnoteReference"));

    s.dispatch("page_numbers", &json!({"where": "footer", "align": "center"})).unwrap();
    s.dispatch("link_insert", &json!({"from": {"p": [5], "o": 0}, "to": {"p": [5], "o": 7}, "url": "example.com"})).unwrap();
    let link = s.dispatch("link_at", &json!({"at": {"p": [5], "o": 3}})).unwrap();
    assert_eq!(link["url"], "https://example.com");
    let path = std::env::temp_dir().join(format!("quire-structure-{}.docx", std::process::id()));
    s.save(Some(&path)).unwrap();
    let mut o = Session::new();
    o.open(&path).unwrap();
    let all: Vec<String> = o.all_paragraphs().into_iter().map(|(_, _, t)| t).collect();
    assert!(all.iter().any(|t| t.contains("Source. Checked.")), "{all:?}");
    assert!(o.state.base.headers_footers().iter().any(|h| o.state.base.read_part(&h.part).unwrap().contains("NUMPAGES")));
    assert_eq!(o.dispatch("link_at", &json!({"at": {"p": [5], "o": 3}})).unwrap()["url"], "https://example.com");
    o.dispatch("link_remove", &json!({"at": {"p": [5], "o": 3}})).unwrap();
    assert!(o.dispatch("link_at", &json!({"at": {"p": [5], "o": 3}})).unwrap().is_null());
}

#[test]
fn spelling_and_replace() {
    let mut s = session(&[("", "This sentense has a mistake and a typpo."), ("", "Replace cats with dogs: cats, Cats.")]);
    let bad: Vec<String> = s.misspellings().iter().map(|m| m["word"].as_str().unwrap().to_string()).collect();
    assert_eq!(bad, vec!["sentense", "typpo"]);
    assert!(spell::suggest("sentense").iter().any(|w| w == "sentence"));
    s.dispatch("spell_words", &json!({"words": ["typpo"]})).unwrap();
    assert_eq!(s.misspellings().len(), 1);
    let n = s.dispatch("replace", &json!({"q": "cats", "with": "dogs"})).unwrap();
    assert_eq!(n["replaced"], 3);
    assert_eq!(s.para(1).unwrap().text(), "Replace dogs with dogs: dogs, dogs.");
}

#[test]
fn rich_copy_and_paste() {
    let mut s = session(&[("Heading1", "Title here"), ("", "Plain and bold.")]);
    s.format(Loc::body(1, 10), Loc::body(1, 14), &RunFormat { bold: Some(true), ..Default::default() }).unwrap();
    let html = s.html(&Loc::body(0, 0), &Loc::body(1, 15)).unwrap();
    assert_eq!(html, "<h1>Title here</h1><p>Plain and <b>bold</b>.</p>");
    let end = s.paste_rich(Loc::body(1, 15), &[
        json!({"runs": [{"text": " Pasted "}, {"text": "link", "link": "https://example.com", "underline": true}]}),
        json!({"style": "Heading2", "runs": [{"text": "Section"}]}),
        json!({"list": "bullet", "runs": [{"text": "one", "italic": true}]}),
        json!({"list": "bullet", "runs": [{"text": "two"}]}),
    ]).unwrap();
    assert_eq!(end.path, vec![4]);
    assert_eq!(s.para(1).unwrap().text(), "Plain and bold. Pasted link");
    assert!(s.block_xml(1).contains("<w:hyperlink"));
    assert!(s.block_xml(2).contains(r#"w:val="Heading2""#));
    assert!(s.block_xml(3).contains("<w:numPr>") && s.block_xml(3).contains("<w:i/>"));
    assert!(s.block_xml(4).contains("<w:numPr>"));
    let path = std::env::temp_dir().join(format!("quire-paste-{}.docx", std::process::id()));
    s.save(Some(&path)).unwrap();
    let mut o = Session::new();
    o.open(&path).unwrap();
    assert_eq!(o.dispatch("link_at", &json!({"at": {"p": [1], "o": 24}})).unwrap()["url"], "https://example.com");
}

#[test]
fn autosave_recovery_and_versions() {
    isolate();
    let dir = autosave::data_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let doc = dir.join("doc.docx");
    let mut s = session(&[("", "First draft")]);
    s.save(Some(&doc)).unwrap();
    assert!(!s.dispatch("versions", &json!({})).unwrap().as_array().unwrap().is_empty());
    s.insert_text(Loc::body(0, 11), " with edits").unwrap();
    assert_eq!(s.dispatch("autosave", &json!({})).unwrap()["saved"], true);
    let rec = s.dispatch("recovery_list", &json!({})).unwrap();
    assert_eq!(rec.as_array().unwrap().len(), 1);
    // A "crash": a new session restores the unsaved edits.
    let mut r = Session::new();
    let key = rec[0]["key"].as_str().unwrap().to_string();
    r.dispatch("recovery_restore", &json!({"key": key})).unwrap();
    assert_eq!(r.para(0).unwrap().text(), "First draft with edits");
    assert!(r.info()["dirty"].as_bool().unwrap());
    r.save(None).unwrap();
    assert!(r.dispatch("recovery_list", &json!({})).unwrap().as_array().unwrap().is_empty());
    let versions = r.dispatch("versions", &json!({})).unwrap();
    assert!(versions.as_array().unwrap().len() >= 2);
    // Restore the first saved version (undoable).
    let oldest = versions.as_array().unwrap().last().unwrap()["id"].as_str().unwrap().to_string();
    r.dispatch("version_restore", &json!({"id": oldest})).unwrap();
    assert_eq!(r.para(0).unwrap().text(), "First draft");
    r.dispatch("undo", &json!({})).unwrap();
    assert_eq!(r.para(0).unwrap().text(), "First draft with edits");
}
