use std::io::{Cursor, Read, Write};

use super::props::{ParaFormat, RunFormat};
use super::table::Table;
use super::*;

const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

fn doc_xml(body: &str) -> String {
    format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document {NS}><w:body>{body}<w:sectPr><w:pgSz w:w="12240" w:h="15840"/></w:sectPr></w:body></w:document>"#)
}

fn package(document: &str) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    let files = [
        ("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#.to_string()),
        ("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_string()),
        ("word/document.xml", document.to_string()),
        ("word/_rels/document.xml.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#.to_string()),
        ("word/styles.xml", format!(r#"<?xml version="1.0"?><w:styles {NS}><w:style w:type="paragraph" w:styleId="Normal"/></w:styles>"#)),
    ];
    for (n, d) in files {
        z.start_file(n, o).unwrap();
        z.write_all(d.as_bytes()).unwrap();
    }
    z.finish().unwrap().into_inner()
}

fn read(bytes: &[u8], name: &str) -> Option<String> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut f = z.by_name(name).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s)
}

fn raw_entry(bytes: &[u8], name: &str) -> Vec<u8> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let idx = z.index_for_name(name).unwrap();
    let f = z.by_index_raw(idx).unwrap();
    let mut v = Vec::new();
    let mut f = f;
    f.read_to_end(&mut v).unwrap();
    v
}

fn rev() -> Rev {
    Rev::new("Ana", "2026-10-18T10:00:00Z", 100)
}

#[test]
fn identity_rebuild_is_byte_exact() {
    let d = doc_xml(r#"<w:p><w:r><w:t>One</w:t></w:r></w:p>
  <w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>
  <w:p w14:paraId="1"><w:r><w:t>Two</w:t></w:r></w:p>"#);
    let b = Body::parse(&d).unwrap();
    assert_eq!(b.elements.len(), 3);
    assert_eq!(b.elements[1].name, "tbl");
    assert_eq!(b.rebuild(Some(&b.identity()), None), d);
    assert_eq!(b.rebuild(None, None), d);
    let swapped = b.rebuild(Some(&[Block::Orig(2), Block::Xml("<w:p/>".into()), Block::Orig(0)]), None);
    assert!(swapped.contains(r#"<w:body><w:p w14:paraId="1"><w:r><w:t>Two</w:t></w:r></w:p><w:p/><w:p><w:r><w:t>One</w:t></w:r></w:p><w:sectPr>"#));
}

#[test]
fn insert_text_inherits_formatting_and_merges() {
    let mut p = Para::parse(r#"<w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:rPr><w:b/></w:rPr><w:t>Hello</w:t></w:r><w:r><w:t xml:space="preserve"> world</w:t></w:r></w:p>"#).unwrap();
    assert_eq!(p.text(), "Hello world");
    p.insert_text(5, ",", None, None);
    assert_eq!(p.text(), "Hello, world");
    let x = p.to_xml();
    assert!(x.contains("<w:rPr><w:b/></w:rPr><w:t>Hello,</w:t>"), "{x}");
    p.insert_text(2, "XY", None, None);
    assert_eq!(p.text(), "HeXYllo, world");
    assert!(p.to_xml().starts_with(r#"<w:p><w:pPr><w:jc w:val="center"/></w:pPr>"#));
}

#[test]
fn delete_untracked_and_tracked() {
    let src = r#"<w:p><w:r><w:t>abc</w:t></w:r><w:bookmarkStart w:id="0" w:name="x"/><w:r><w:rPr><w:i/></w:rPr><w:t>def</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#;
    let mut p = Para::parse(src).unwrap();
    p.delete(1, 5, None);
    assert_eq!(p.text(), "af");
    assert!(p.to_xml().contains("bookmarkStart"), "bookmarks survive deletion");

    let mut p = Para::parse(src).unwrap();
    let r = rev();
    p.delete(1, 5, Some(&r));
    assert_eq!(p.text(), "af", "deleted text is hidden");
    let x = p.to_xml();
    assert!(x.contains(r#"<w:del w:id="100" w:author="Ana""#), "{x}");
    assert!(x.contains("<w:delText>bc</w:delText>") && x.contains("<w:delText>de</w:delText>"), "{x}");
    // Reparse: same visible text.
    assert_eq!(Para::parse(&x).unwrap().text(), "af");
}

#[test]
fn tracked_insert_then_delete_own_insertion() {
    let mut p = Para::parse(r#"<w:p><w:r><w:t>ac</w:t></w:r></w:p>"#).unwrap();
    let r = rev();
    p.insert_text(1, "b", None, Some(&r));
    p.insert_text(2, "B", None, Some(&r));
    let x = p.to_xml();
    assert_eq!(x.matches("<w:ins ").count(), 1, "adjacent insertions share one w:ins: {x}");
    assert_eq!(p.text(), "abBc");
    p.delete(1, 3, Some(&r));
    assert_eq!(p.text(), "ac");
    assert!(!p.to_xml().contains("<w:del "), "deleting your own insertion just removes it");
}

#[test]
fn tracked_formatting_records_old_properties() {
    let mut p = Para::parse(r#"<w:p><w:r><w:rPr><w:i/></w:rPr><w:t>word</w:t></w:r></w:p>"#).unwrap();
    let r = rev();
    p.format(0, 2, &RunFormat { bold: Some(true), ..Default::default() }, Some(&r));
    let x = p.to_xml();
    assert!(x.contains(r#"<w:rPr><w:b/><w:bCs/><w:i/><w:rPrChange w:id="100" w:author="Ana" w:date="2026-10-18T10:00:00Z"><w:rPr><w:i/></w:rPr></w:rPrChange></w:rPr><w:t>wo</w:t>"#), "{x}");
    assert!(x.contains("<w:rPr><w:i/></w:rPr><w:t>rd</w:t>"), "{x}");
}

#[test]
fn hyperlinks_and_unknown_content_are_kept() {
    let src = r#"<w:p><w:hyperlink r:id="rId5"><w:r><w:rPr><w:rStyle w:val="Hyperlink"/></w:rPr><w:t>link</w:t></w:r></w:hyperlink><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>3</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><m:oMath xmlns:m="m"><m:r><m:t>x</m:t></m:r></m:oMath></w:p>"#;
    let p = Para::parse(src).unwrap();
    assert_eq!(p.to_xml(), src, "parse + serialize is lossless for untouched paragraphs");
    assert_eq!(p.text(), "link3");
    let mut p = p;
    p.insert_text(2, "-", None, None);
    let x = p.to_xml();
    assert!(x.starts_with(r#"<w:p><w:hyperlink r:id="rId5"><w:r><w:rPr><w:rStyle w:val="Hyperlink"/></w:rPr><w:t>li</w:t></w:r><w:r><w:rPr><w:rStyle w:val="Hyperlink"/></w:rPr><w:t>-</w:t></w:r>"#) || x.contains("<w:t>li-</w:t>"), "{x}");
    assert!(x.contains("</w:hyperlink><w:r><w:fldChar"), "{x}");
    assert!(x.contains("<m:oMath"));
}

#[test]
fn split_and_merge_keep_section_break_on_last_paragraph() {
    let mut p = Para::parse(r#"<w:p w14:paraId="AB"><w:pPr><w:pStyle w:val="Heading1"/><w:sectPr><w:type w:val="nextPage"/></w:sectPr></w:pPr><w:r><w:t>HeadTail</w:t></w:r></w:p>"#).unwrap();
    let tail = p.split(4, None);
    assert_eq!(p.text(), "Head");
    assert_eq!(tail.text(), "Tail");
    assert!(!p.to_xml().contains("sectPr"));
    assert!(tail.to_xml().contains("<w:sectPr>") && tail.to_xml().contains("Heading1"));
    assert!(!tail.open.contains("paraId"));
    let mut p2 = p.clone();
    p2.merge(tail);
    assert_eq!(p2.text(), "HeadTail");
    assert!(p2.to_xml().contains("sectPr"));
}

#[test]
fn empty_paragraph_uses_mark_formatting() {
    let mut p = Para::parse(r#"<w:p><w:pPr><w:rPr><w:sz w:val="40"/></w:rPr></w:pPr></w:p>"#).unwrap();
    p.insert_text(0, "Big", None, None);
    assert!(p.to_xml().contains(r#"<w:r><w:rPr><w:sz w:val="40"/></w:rPr><w:t>Big</w:t></w:r>"#), "{}", p.to_xml());
}

#[test]
fn paragraph_formatting_tracked() {
    let mut p = Para::parse(r#"<w:p><w:pPr><w:pStyle w:val="Normal"/></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#).unwrap();
    p.format_paragraph(&ParaFormat { align: Some(Some("center".into())), ..Default::default() }, Some(&rev()));
    let x = p.to_xml();
    assert!(x.contains(r#"<w:pStyle w:val="Normal"/><w:jc w:val="center"/><w:pPrChange w:id="100""#), "{x}");
    assert!(x.contains(r#"><w:pPr><w:pStyle w:val="Normal"/></w:pPr></w:pPrChange>"#), "{x}");
}

#[test]
fn table_cell_text_keeps_formatting() {
    let src = r#"<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/></w:tblPr><w:tblGrid><w:gridCol w:w="100"/></w:tblGrid><w:tr><w:trPr><w:tblHeader/></w:trPr><w:tc><w:tcPr><w:shd w:fill="FF0000"/></w:tcPr><w:p><w:pPr><w:jc w:val="right"/></w:pPr><w:r><w:rPr><w:b/></w:rPr><w:t>old</w:t></w:r></w:p><w:p><w:r><w:t>more</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
    let mut t = Table::parse(src).unwrap();
    assert_eq!(t.to_xml(), src);
    t.cell_mut(0, 0).unwrap().set_text("new");
    let x = t.to_xml();
    assert!(x.contains(r#"<w:tc><w:tcPr><w:shd w:fill="FF0000"/></w:tcPr><w:p><w:pPr><w:jc w:val="right"/></w:pPr><w:r><w:rPr><w:b/></w:rPr><w:t>new</w:t></w:r></w:p></w:tc>"#), "{x}");
    assert!(t.insert_row_after(0));
    assert_eq!(t.rows().count(), 2);
    assert!(t.to_xml().contains("<w:tblHeader/>"));
}

#[test]
fn save_splices_body_and_keeps_other_parts() {
    let d = doc_xml(r#"<w:p><w:r><w:t>Keep me</w:t></w:r></w:p><w:p><w:r><w:t>Edit me</w:t></w:r></w:p>"#);
    let bytes = package(&d);
    let docx = Docx::open(&bytes).unwrap();
    let body = docx.body().unwrap();
    let mut p = Para::parse(body.xml(1)).unwrap();
    p.insert_text(7, "!", None, None);
    let new_para = Para::new("Inserted", None, None);
    let edits = DocxEdits {
        body: Some(vec![Block::Orig(0), Block::Xml(p.to_xml()), Block::Xml(new_para.to_xml())]),
        ..Default::default()
    };
    let out = docx.save(&edits).unwrap();
    let doc = read(&out, DOC).unwrap();
    assert!(doc.contains("<w:p><w:r><w:t>Keep me</w:t></w:r></w:p><w:p><w:r><w:t>Edit me!</w:t></w:r></w:p><w:p><w:r><w:t>Inserted</w:t></w:r></w:p><w:sectPr>"), "{doc}");
    for part in ["word/styles.xml", "[Content_Types].xml", "word/_rels/document.xml.rels"] {
        assert_eq!(raw_entry(&out, part), raw_entry(&bytes, part), "{part} raw-copied");
    }
}

#[test]
fn comments_create_part_relationship_and_content_type() {
    let d = doc_xml(r#"<w:p><w:r><w:t>Revenue grew</w:t></w:r></w:p>"#);
    let bytes = package(&d);
    let docx = Docx::open(&bytes).unwrap();
    let r = Rev::new("Ana", "2026-10-18T10:00:00Z", docx.max_annotation_id() + 1);
    let id = r.id();
    let mut edits = DocxEdits::default();
    docx.queue_comment(&mut edits, id, "Ana", "A", &r.date, "Source?").unwrap();
    let mut p = Para::parse(docx.body().unwrap().xml(0)).unwrap();
    p.add_comment(8, 12, id);
    edits.body = Some(vec![Block::Xml(p.to_xml())]);
    let out = docx.save(&edits).unwrap();
    let doc = read(&out, DOC).unwrap();
    assert!(doc.contains(&format!(r#"<w:t xml:space="preserve">Revenue </w:t></w:r><w:commentRangeStart w:id="{id}"/>"#)), "{doc}");
    assert!(doc.contains(&format!(r#"<w:commentRangeEnd w:id="{id}"/><w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:commentReference w:id="{id}"/></w:r>"#)), "{doc}");
    let comments = read(&out, "word/comments.xml").unwrap();
    assert!(comments.contains(r#"w:author="Ana""#) && comments.contains("Source?"));
    assert!(read(&out, "word/_rels/document.xml.rels").unwrap().contains(r#"Target="comments.xml""#));
    assert!(read(&out, "[Content_Types].xml").unwrap().contains(r#"PartName="/word/comments.xml""#));
}

// ------------------------------------------------------------------ appenders, stories, revisions

use super::append::{self, ListKind};
use super::revisions::Filter;
use super::story::{self as st, HfKind, NoteKind};

fn package_with(document: &str, extra: &[(&str, &str)]) -> Vec<u8> {
    let base = package(document);
    let mut zin = zip::ZipArchive::new(Cursor::new(base.as_slice())).unwrap();
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    for i in 0..zin.len() {
        let mut f = zin.by_index(i).unwrap();
        let name = f.name().unwrap().to_string();
        let mut data = Vec::new();
        f.read_to_end(&mut data).unwrap();
        if let Some((_, d)) = extra.iter().find(|(n, _)| *n == name) {
            data = d.as_bytes().to_vec();
        }
        z.start_file(name, o).unwrap();
        z.write_all(&data).unwrap();
    }
    for (n, d) in extra {
        if zin.index_for_name(n).is_none() {
            z.start_file(*n, o).unwrap();
            z.write_all(d.as_bytes()).unwrap();
        }
    }
    z.finish().unwrap().into_inner()
}

#[test]
fn styles_are_appended_once() {
    let bytes = package(&doc_xml("<w:p/>"));
    let docx = Docx::open(&bytes).unwrap();
    let mut e = DocxEdits::default();
    assert!(append::ensure_builtin_style(&docx, &mut e, "Heading1"));
    assert!(!append::ensure_builtin_style(&docx, &mut e, "Heading1"), "second time is a no-op");
    assert!(!append::ensure_builtin_style(&docx, &mut e, "Normal") , "unknown built-ins are ignored");
    let out = docx.save(&e).unwrap();
    let styles = read(&out, "word/styles.xml").unwrap();
    assert!(styles.contains(r#"w:styleId="Normal""#) && styles.contains(r#"w:styleId="Heading1""#));
}

#[test]
fn lists_create_numbering_part_in_schema_order() {
    let bytes = package(&doc_xml("<w:p/>"));
    let docx = Docx::open(&bytes).unwrap();
    let mut e = DocxEdits::default();
    let a = append::add_list(&docx, &mut e, ListKind::Bullet);
    let b = append::add_list(&docx, &mut e, ListKind::Decimal);
    assert_eq!((a, b), (1, 2));
    let out = docx.save(&e).unwrap();
    let num = read(&out, "word/numbering.xml").unwrap();
    let last_abs = num.rfind("<w:abstractNum ").unwrap();
    let first_num = num.find("<w:num ").unwrap();
    assert!(last_abs < first_num, "abstractNums precede nums: {num}");
    assert!(num.contains(r#"<w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>"#));
    assert!(read(&out, "word/_rels/document.xml.rels").unwrap().contains("numbering.xml"));
    assert!(read(&out, "[Content_Types].xml").unwrap().contains("/word/numbering.xml"));
    assert!(read(&out, "word/styles.xml").unwrap().contains("ListParagraph"));
}

#[test]
fn pictures_add_media_rel_and_default_content_type() {
    let bytes = package(&doc_xml("<w:p/>"));
    let docx = Docx::open(&bytes).unwrap();
    let mut e = DocxEdits::default();
    let rid = append::add_image(&docx, &mut e, b"\x89PNG fake", "png").unwrap();
    let mut p = Para::new("", None, None);
    p.insert_object(0, &append::inline_picture(&rid, 200, 100, append::max_drawing_id(&docx.document) + 1, "Picture 1"), None, None);
    e.body = Some(vec![Block::Xml(p.to_xml())]);
    let out = docx.save(&e).unwrap();
    assert!(zip::ZipArchive::new(Cursor::new(out.as_slice())).unwrap().index_for_name("word/media/sheaf_image1.png").is_some());
    let ct = read(&out, "[Content_Types].xml").unwrap();
    assert!(ct.contains(r#"<Default Extension="png" ContentType="image/png"/>"#), "{ct}");
    assert!(read(&out, DOC).unwrap().contains(&format!(r#"r:embed="{rid}""#)));
    assert!(read(&out, DOC).unwrap().contains(r#"cx="1905000""#));
}

#[test]
fn headers_footers_add_edit_and_attach() {
    let bytes = package(&doc_xml("<w:p><w:r><w:t>Body</w:t></w:r></w:p>"));
    let docx = Docx::open(&bytes).unwrap();
    let mut e = DocxEdits::default();
    let (part, rid) = docx.add_header_footer(&mut e, HfKind::Header, &Para::new("Draft", None, None).to_xml());
    let body = docx.body().unwrap();
    e.sect_pr = Some(st::set_hf_reference(body.sect_pr_xml().unwrap(), HfKind::Header, "first", &rid));
    let out = docx.save(&e).unwrap();
    let doc = read(&out, DOC).unwrap();
    assert!(doc.contains(&format!(r#"<w:sectPr><w:headerReference w:type="first" r:id="{rid}"/><w:pgSz w:w="12240" w:h="15840"/><w:titlePg/></w:sectPr>"#)), "{doc}");
    assert!(read(&out, &part).unwrap().contains("<w:t>Draft</w:t>"));

    // Reopen and edit the header in place.
    let docx2 = Docx::open(&out).unwrap();
    let hf = docx2.headers_footers();
    assert_eq!(hf.len(), 1);
    let src = docx2.read_part(&hf[0].part).unwrap();
    let hb = Body::parse_in(&src, "hdr").unwrap();
    let mut p = Para::parse(hb.xml(0)).unwrap();
    p.insert_text(5, " v2", None, None);
    let mut e2 = DocxEdits::default();
    docx2.edit_header_footer(&mut e2, &hf[0].part, &[Block::Xml(p.to_xml())]).unwrap();
    let out2 = docx2.save(&e2).unwrap();
    assert!(read(&out2, &hf[0].part).unwrap().contains("Draft v2"));
}

#[test]
fn footnotes_created_with_separators_and_reference() {
    let bytes = package(&doc_xml("<w:p><w:r><w:t>Claim.</w:t></w:r></w:p>"));
    let docx = Docx::open(&bytes).unwrap();
    let mut e = DocxEdits::default();
    let id = docx.queue_note(&mut e, NoteKind::Footnote, "Source, p. 4.").unwrap();
    assert_eq!(id, 1);
    let mut p = Para::parse(docx.body().unwrap().xml(0)).unwrap();
    p.insert_note_reference(6, NoteKind::Footnote, id, None);
    e.body = Some(vec![Block::Xml(p.to_xml())]);
    let id2 = docx.queue_note(&mut e, NoteKind::Footnote, "Second.").unwrap();
    assert_eq!(id2, 2);
    docx.set_note_blocks(&mut e, NoteKind::Footnote, id2, &Para::new("Rewritten.", None, None).to_xml()).unwrap();
    let out = docx.save(&e).unwrap();
    let notes = read(&out, "word/footnotes.xml").unwrap();
    assert!(notes.contains(r#"w:type="separator" w:id="-1""#) && notes.contains(r#"w:type="continuationSeparator" w:id="0""#));
    assert!(notes.contains("Source, p. 4.") && notes.contains("Rewritten.") && !notes.contains("Second."));
    let doc = read(&out, DOC).unwrap();
    assert!(doc.contains(r#"<w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteReference w:id="1"/></w:r>"#), "{doc}");
    assert!(read(&out, "word/styles.xml").unwrap().contains("FootnoteText"));
    let docx2 = Docx::open(&out).unwrap();
    assert_eq!(docx2.notes(&DocxEdits::default(), NoteKind::Footnote).len(), 4);
}

const TRACKED: &str = concat!(
    r#"<w:p><w:r><w:t xml:space="preserve">Keep </w:t></w:r>"#,
    r#"<w:ins w:id="1" w:author="Ana" w:date="d"><w:r><w:t xml:space="preserve">added </w:t></w:r></w:ins>"#,
    r#"<w:del w:id="2" w:author="Ben" w:date="d"><w:r><w:delText xml:space="preserve">gone </w:delText></w:r></w:del>"#,
    r#"<w:r><w:rPr><w:b/><w:rPrChange w:id="3" w:author="Ana" w:date="d"><w:rPr><w:i/></w:rPr></w:rPrChange></w:rPr><w:t>end</w:t></w:r></w:p>"#,
    r#"<w:p><w:pPr><w:rPr><w:ins w:id="4" w:author="Ana" w:date="d"/></w:rPr></w:pPr><w:r><w:t>First</w:t></w:r></w:p>"#,
    r#"<w:p><w:r><w:t>Second</w:t></w:r></w:p>"#,
    r#"<w:tbl><w:tblGrid><w:gridCol w:w="100"/></w:tblGrid><w:tr><w:trPr><w:del w:id="5" w:author="Ben" w:date="d"/></w:trPr><w:tc><w:p><w:r><w:t>row1</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>row2</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
    r#"<w:p><w:r><w:t>Untouched</w:t></w:r></w:p>"#);

fn body_texts(doc: &str) -> Vec<String> {
    let b = Body::parse(doc).unwrap();
    (0..b.elements.len()).map(|i| {
        if b.elements[i].name == "p" { Para::parse(b.xml(i)).unwrap().text() } else { Table::parse(b.xml(i)).unwrap().to_xml() }
    }).collect()
}

#[test]
fn revisions_are_listed() {
    let bytes = package(&doc_xml(TRACKED));
    let docx = Docx::open(&bytes).unwrap();
    let revs = docx.revisions();
    let kinds: Vec<_> = revs.iter().map(|r| (r.kind.as_str(), r.author.as_str(), r.text.as_str())).collect();
    assert_eq!(kinds, vec![("ins", "Ana", "added "), ("del", "Ben", "gone "), ("rPrChange", "Ana", ""), ("paragraphMark", "Ana", ""), ("rowDel", "Ben", "")]);
}

#[test]
fn accept_all_revisions() {
    let bytes = package(&doc_xml(TRACKED));
    let docx = Docx::open(&bytes).unwrap();
    let mut e = DocxEdits::default();
    let n = docx.resolve_revisions(&mut e, true, &Filter::default()).unwrap();
    assert_eq!(n, 5);
    let out = docx.save(&e).unwrap();
    let doc = read(&out, DOC).unwrap();
    let t = body_texts(&doc);
    assert_eq!(t[0], "Keep added end");
    assert_eq!(t[1], "First", "accepting an inserted paragraph mark keeps the paragraphs apart");
    assert!(!t[3].contains("row1") && t[3].contains("row2"), "accepted row deletion removes the row");
    assert!(doc.contains("<w:rPr><w:b/></w:rPr><w:t>end</w:t>"), "{doc}");
    assert!(!doc.contains("w:author"), "no revision left: {doc}");
    assert!(doc.contains("<w:p><w:r><w:t>Untouched</w:t></w:r></w:p>"));
    assert!(matches!(e.body.as_ref().unwrap().last(), Some(Block::Orig(_))), "unchanged blocks stay byte-copied");
}

#[test]
fn reject_all_revisions() {
    let bytes = package(&doc_xml(TRACKED));
    let docx = Docx::open(&bytes).unwrap();
    let mut e = DocxEdits::default();
    docx.resolve_revisions(&mut e, false, &Filter::default()).unwrap();
    let out = docx.save(&e).unwrap();
    let doc = read(&out, DOC).unwrap();
    let t = body_texts(&doc);
    assert_eq!(t[0], "Keep gone end", "rejected deletion comes back, insertion goes");
    assert!(doc.contains("<w:t xml:space=\"preserve\">gone </w:t>"), "restored as w:t: {doc}");
    assert_eq!(t[1], "FirstSecond", "rejecting an inserted paragraph mark merges the paragraphs");
    assert!(t[2].contains("row1") && t[2].contains("row2"), "rejected row deletion keeps the row");
    assert!(doc.contains("<w:rPr><w:i/></w:rPr><w:t>end</w:t>"), "old formatting restored: {doc}");
}

#[test]
fn resolve_by_author_only() {
    let bytes = package(&doc_xml(TRACKED));
    let docx = Docx::open(&bytes).unwrap();
    let mut e = DocxEdits::default();
    let n = docx.resolve_revisions(&mut e, true, &Filter { author: Some("Ben".into()), ids: None }).unwrap();
    assert_eq!(n, 2);
    let out = docx.save(&e).unwrap();
    let remaining = Docx::open(&out).unwrap().revisions();
    assert!(remaining.iter().all(|r| r.author == "Ana"), "{remaining:?}");
    assert_eq!(remaining.len(), 3);
}

#[test]
fn revisions_in_headers_and_footnotes_are_resolved() {
    let hdr = format!(r#"<?xml version="1.0"?><w:hdr {NS}><w:p><w:ins w:id="7" w:author="Ana" w:date="d"><w:r><w:t>new header</w:t></w:r></w:ins></w:p></w:hdr>"#);
    let notes = format!(r#"<?xml version="1.0"?><w:footnotes {NS}><w:footnote w:id="1"><w:p><w:del w:id="8" w:author="Ana" w:date="d"><w:r><w:delText>old note</w:delText></w:r></w:del></w:p></w:footnote></w:footnotes>"#);
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes" Target="footnotes.xml"/></Relationships>"#;
    let bytes = package_with(&doc_xml("<w:p/>"), &[("word/header1.xml", &hdr), ("word/footnotes.xml", &notes), ("word/_rels/document.xml.rels", rels)]);
    let docx = Docx::open(&bytes).unwrap();
    assert_eq!(docx.revisions().len(), 2);
    let mut e = DocxEdits::default();
    assert_eq!(docx.resolve_revisions(&mut e, false, &Filter::default()).unwrap(), 2);
    let out = docx.save(&e).unwrap();
    assert!(!read(&out, "word/header1.xml").unwrap().contains("new header"));
    assert!(read(&out, "word/footnotes.xml").unwrap().contains("<w:t>old note</w:t>"));
}

#[test]
fn track_revisions_setting_toggles() {
    let settings = format!(r#"<?xml version="1.0"?><w:settings {NS}><w:zoom w:percent="100"/><w:defaultTabStop w:val="720"/></w:settings>"#);
    let bytes = package_with(&doc_xml("<w:p/>"), &[("word/settings.xml", &settings)]);
    let docx = Docx::open(&bytes).unwrap();
    let mut e = DocxEdits::default();
    docx.set_track_revisions(&mut e, true).unwrap();
    let s = String::from_utf8(e.parts["word/settings.xml"].clone()).unwrap();
    assert!(s.contains(r#"<w:zoom w:percent="100"/><w:trackRevisions/><w:defaultTabStop"#), "{s}");
    docx.set_track_revisions(&mut e, false).unwrap();
    assert!(!String::from_utf8(e.parts["word/settings.xml"].clone()).unwrap().contains("trackRevisions"));
}
