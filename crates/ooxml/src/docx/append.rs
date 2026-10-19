//! Appenders for shared definitions: styles, list numbering, pictures.
//!
//! Existing definitions are never rewritten. New ones are appended (or inserted
//! where the schema requires), with ids above every id already in use.

use super::{Docx, DocxEdits, NewRel, DOC, R_NS, W_NS};
use crate::package;
use crate::xml::{attr, escape, tokens, Kind};

pub const STYLES: &str = "word/styles.xml";
pub const NUMBERING: &str = "word/numbering.xml";
const CT_STYLES: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml";
const CT_NUMBERING: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml";
const REL_STYLES: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";
const REL_NUMBERING: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering";
const REL_IMAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";

// --------------------------------------------------------------------------- styles

/// Definitions of the built-in styles Sheaf may need to reference, matching Word's defaults.
pub fn builtin_style(id: &str) -> Option<&'static str> {
    Some(match id {
        "Heading1" => r#"<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="9"/><w:qFormat/><w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before="240" w:after="0"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:eastAsiaTheme="majorEastAsia" w:hAnsiTheme="majorHAnsi" w:cstheme="majorBidi"/><w:color w:val="2F5496" w:themeColor="accent1" w:themeShade="BF"/><w:sz w:val="32"/><w:szCs w:val="32"/></w:rPr></w:style>"#,
        "Heading2" => r#"<w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="9"/><w:unhideWhenUsed/><w:qFormat/><w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before="40" w:after="0"/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:eastAsiaTheme="majorEastAsia" w:hAnsiTheme="majorHAnsi" w:cstheme="majorBidi"/><w:color w:val="2F5496" w:themeColor="accent1" w:themeShade="BF"/><w:sz w:val="26"/><w:szCs w:val="26"/></w:rPr></w:style>"#,
        "Heading3" => r#"<w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="9"/><w:unhideWhenUsed/><w:qFormat/><w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before="40" w:after="0"/><w:outlineLvl w:val="2"/></w:pPr><w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:eastAsiaTheme="majorEastAsia" w:hAnsiTheme="majorHAnsi" w:cstheme="majorBidi"/><w:color w:val="1F3763" w:themeColor="accent1" w:themeShade="7F"/><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr></w:style>"#,
        "Title" => r#"<w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="10"/><w:qFormat/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/><w:contextualSpacing/></w:pPr><w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:eastAsiaTheme="majorEastAsia" w:hAnsiTheme="majorHAnsi" w:cstheme="majorBidi"/><w:spacing w:val="-10"/><w:kern w:val="28"/><w:sz w:val="56"/><w:szCs w:val="56"/></w:rPr></w:style>"#,
        "ListParagraph" => r#"<w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="34"/><w:qFormat/><w:pPr><w:ind w:left="720"/><w:contextualSpacing/></w:pPr></w:style>"#,
        "Hyperlink" => r#"<w:style w:type="character" w:styleId="Hyperlink"><w:name w:val="Hyperlink"/><w:basedOn w:val="DefaultParagraphFont"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/><w:rPr><w:color w:val="0563C1" w:themeColor="hyperlink"/><w:u w:val="single"/></w:rPr></w:style>"#,
        "FootnoteText" => r#"<w:style w:type="paragraph" w:styleId="FootnoteText"><w:name w:val="footnote text"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style>"#,
        "FootnoteReference" => r#"<w:style w:type="character" w:styleId="FootnoteReference"><w:name w:val="footnote reference"/><w:basedOn w:val="DefaultParagraphFont"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/><w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style>"#,
        "EndnoteText" => r#"<w:style w:type="paragraph" w:styleId="EndnoteText"><w:name w:val="endnote text"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style>"#,
        "EndnoteReference" => r#"<w:style w:type="character" w:styleId="EndnoteReference"><w:name w:val="endnote reference"/><w:basedOn w:val="DefaultParagraphFont"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/><w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style>"#,
        "CommentText" => r#"<w:style w:type="paragraph" w:styleId="CommentText"><w:name w:val="annotation text"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/><w:pPr><w:spacing w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style>"#,
        "CommentReference" => r#"<w:style w:type="character" w:styleId="CommentReference"><w:name w:val="annotation reference"/><w:basedOn w:val="DefaultParagraphFont"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/><w:rPr><w:sz w:val="16"/><w:szCs w:val="16"/></w:rPr></w:style>"#,
        "Header" => r#"<w:style w:type="paragraph" w:styleId="Header"><w:name w:val="header"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/><w:pPr><w:tabs><w:tab w:val="center" w:pos="4680"/><w:tab w:val="right" w:pos="9360"/></w:tabs><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr></w:style>"#,
        "Footer" => r#"<w:style w:type="paragraph" w:styleId="Footer"><w:name w:val="footer"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/><w:pPr><w:tabs><w:tab w:val="center" w:pos="4680"/><w:tab w:val="right" w:pos="9360"/></w:tabs><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr></w:style>"#,
        "TableGrid" => r#"<w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:basedOn w:val="TableNormal"/><w:uiPriority w:val="39"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:left w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:bottom w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:right w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideH w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="auto"/></w:tblBorders></w:tblPr></w:style>"#,
        "TOCHeading" => r#"<w:style w:type="paragraph" w:styleId="TOCHeading"><w:name w:val="TOC Heading"/><w:basedOn w:val="Heading1"/><w:next w:val="Normal"/><w:uiPriority w:val="39"/><w:unhideWhenUsed/><w:qFormat/><w:pPr><w:outlineLvl w:val="9"/></w:pPr></w:style>"#,
        "TOC1" => r#"<w:style w:type="paragraph" w:styleId="TOC1"><w:name w:val="toc 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="39"/><w:unhideWhenUsed/><w:pPr><w:tabs><w:tab w:val="right" w:leader="dot" w:pos="9350"/></w:tabs><w:spacing w:after="100"/></w:pPr></w:style>"#,
        "TOC2" => r#"<w:style w:type="paragraph" w:styleId="TOC2"><w:name w:val="toc 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="39"/><w:unhideWhenUsed/><w:pPr><w:tabs><w:tab w:val="right" w:leader="dot" w:pos="9350"/></w:tabs><w:spacing w:after="100"/><w:ind w:left="220"/></w:pPr></w:style>"#,
        "TOC3" => r#"<w:style w:type="paragraph" w:styleId="TOC3"><w:name w:val="toc 3"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="39"/><w:unhideWhenUsed/><w:pPr><w:tabs><w:tab w:val="right" w:leader="dot" w:pos="9350"/></w:tabs><w:spacing w:after="100"/><w:ind w:left="440"/></w:pPr></w:style>"#,
        _ => return None,
    })
}

/// Outline level (0 = Heading 1) of each paragraph style, following `w:basedOn`.
pub fn outline_levels(styles_xml: &str) -> std::collections::HashMap<String, u8> {
    use std::collections::HashMap;
    let mut direct: HashMap<String, (Option<u8>, Option<String>)> = HashMap::new();
    let toks: Vec<_> = tokens(styles_xml).collect();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        if t.local() == "style" && t.kind == Kind::Start {
            let end = crate::xml::element_end(&toks, i);
            let tag = &styles_xml[t.start..t.end];
            if let Some(id) = attr(tag, "w:styleId") {
                let body = &styles_xml[t.end..end];
                let lvl = tokens(body).find(|x| x.local() == "outlineLvl").and_then(|x| attr(&body[x.start..x.end], "w:val")).and_then(|v| v.parse().ok());
                let based = tokens(body).find(|x| x.local() == "basedOn").and_then(|x| attr(&body[x.start..x.end], "w:val")).map(str::to_string);
                direct.insert(id.to_string(), (lvl, based));
            }
            while i < toks.len() && toks[i].start < end {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    let mut out = HashMap::new();
    for id in direct.keys() {
        let mut cur = Some(id.clone());
        for _ in 0..12 {
            let Some(c) = cur else { break };
            match direct.get(&c) {
                Some((Some(l), _)) => { if *l < 9 { out.insert(id.clone(), *l); } break; }
                Some((None, b)) => cur = b.clone(),
                None => break,
            }
        }
    }
    out
}

/// Style ids defined in the (current) styles part.
pub fn style_ids(styles_xml: &str) -> Vec<String> {
    tokens(styles_xml)
        .filter(|t| t.local() == "style" && matches!(t.kind, Kind::Start | Kind::Empty))
        .filter_map(|t| attr(&styles_xml[t.start..t.end], "w:styleId").map(str::to_string))
        .collect()
}

/// Add `style_xml` (a whole `<w:style>`) unless a style with the same id exists.
/// Returns true if it was added.
pub fn ensure_style(docx: &Docx, edits: &mut DocxEdits, style_xml: &str) -> bool {
    let id = tokens(style_xml).find(|t| t.local() == "style")
        .and_then(|t| attr(&style_xml[t.start..t.end], "w:styleId").map(str::to_string));
    let Some(id) = id else { return false };
    let initial = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>{}<w:styles xmlns:w="{W_NS}" xmlns:r="{R_NS}"></w:styles>"#, "\r\n");
    let styles = docx.ensure_part(edits, STYLES, CT_STYLES, REL_STYLES, &initial);
    if style_ids(&styles).iter().any(|s| *s == id) {
        return false;
    }
    edits.parts.insert(STYLES.into(), package::append_to_root(&styles, style_xml).into_bytes());
    true
}

/// [`ensure_style`] for one of the built-in definitions in [`builtin_style`].
pub fn ensure_builtin_style(docx: &Docx, edits: &mut DocxEdits, id: &str) -> bool {
    builtin_style(id).map(|x| ensure_style(docx, edits, x)).unwrap_or(false)
}

// --------------------------------------------------------------------------- numbering

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ListKind {
    Bullet,
    Decimal,
}

fn max_attr(src: &str, element: &str, key: &str) -> Option<i64> {
    tokens(src)
        .filter(|t| t.local() == element && matches!(t.kind, Kind::Start | Kind::Empty))
        .filter_map(|t| attr(&src[t.start..t.end], key).and_then(|v| v.parse::<i64>().ok()))
        .max()
}

fn abstract_num(id: i64, kind: ListKind) -> String {
    let mut lvls = String::new();
    for l in 0..9 {
        let left = 720 * (l + 1);
        let lvl = match kind {
            ListKind::Bullet => {
                let (text, font) = match l % 3 { 0 => ("\u{F0B7}", "Symbol"), 1 => ("o", "Courier New"), _ => ("\u{F0A7}", "Wingdings") };
                format!(r#"<w:lvl w:ilvl="{l}"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="{text}"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="{left}" w:hanging="360"/></w:pPr><w:rPr><w:rFonts w:ascii="{font}" w:hAnsi="{font}" w:hint="default"/></w:rPr></w:lvl>"#)
            }
            ListKind::Decimal => {
                let fmt = match l % 3 { 0 => "decimal", 1 => "lowerLetter", _ => "lowerRoman" };
                let jc = if l % 3 == 2 { "right" } else { "left" };
                let hanging = if l % 3 == 2 { 180 } else { 360 };
                format!(r#"<w:lvl w:ilvl="{l}"><w:start w:val="1"/><w:numFmt w:val="{fmt}"/><w:lvlText w:val="%{}."/><w:lvlJc w:val="{jc}"/><w:pPr><w:ind w:left="{left}" w:hanging="{hanging}"/></w:pPr></w:lvl>"#, l + 1)
            }
        };
        lvls.push_str(&lvl);
    }
    format!(r#"<w:abstractNum w:abstractNumId="{id}"><w:multiLevelType w:val="hybridMultilevel"/>{lvls}</w:abstractNum>"#)
}

/// Add a new list definition; returns the `w:numId` to put in paragraphs' `w:numPr`.
pub fn add_list(docx: &Docx, edits: &mut DocxEdits, kind: ListKind) -> u32 {
    let initial = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>{}<w:numbering xmlns:w="{W_NS}" xmlns:r="{R_NS}"></w:numbering>"#, "\r\n");
    let num = docx.ensure_part(edits, NUMBERING, CT_NUMBERING, REL_NUMBERING, &initial);
    let abs_id = max_attr(&num, "abstractNum", "w:abstractNumId").map(|m| m + 1).unwrap_or(0);
    let num_id = max_attr(&num, "num", "w:numId").map(|m| m + 1).unwrap_or(1).max(1);
    let abs = abstract_num(abs_id, kind);
    // Schema order: every abstractNum before the first num.
    let with_abs = match tokens(&num).find(|t| t.local() == "num" && matches!(t.kind, Kind::Start | Kind::Empty)) {
        Some(t) => format!("{}{}{}", &num[..t.start], abs, &num[t.start..]),
        None => match tokens(&num).find(|t| t.local() == "numIdMacAtCleanup") {
            Some(t) => format!("{}{}{}", &num[..t.start], abs, &num[t.start..]),
            None => package::append_to_root(&num, &abs),
        },
    };
    let num_xml = format!(r#"<w:num w:numId="{num_id}"><w:abstractNumId w:val="{abs_id}"/></w:num>"#);
    let out = match tokens(&with_abs).find(|t| t.local() == "numIdMacAtCleanup") {
        Some(t) => format!("{}{}{}", &with_abs[..t.start], num_xml, &with_abs[t.start..]),
        None => package::append_to_root(&with_abs, &num_xml),
    };
    edits.parts.insert(NUMBERING.into(), out.into_bytes());
    ensure_builtin_style(docx, edits, "ListParagraph");
    num_id as u32
}

/// `w:numPr` for a paragraph in list `num_id` at level `ilvl`.
pub fn num_pr(num_id: u32, ilvl: u32) -> String {
    format!(r#"<w:numPr><w:ilvl w:val="{ilvl}"/><w:numId w:val="{num_id}"/></w:numPr>"#)
}

// --------------------------------------------------------------------------- pictures

pub fn image_content_type(ext: &str) -> Option<&'static str> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        "emf" => "image/x-emf",
        "wmf" => "image/x-wmf",
        _ => return None,
    })
}

/// Add an image part and a relationship from the main document; returns the relationship id.
pub fn add_image(docx: &Docx, edits: &mut DocxEdits, data: &[u8], ext: &str) -> Result<String, String> {
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    let ct = image_content_type(&ext).ok_or_else(|| format!("unsupported image type .{ext}"))?;
    let name = (1..).map(|n| format!("word/media/sheaf_image{n}.{ext}"))
        .find(|n| !docx.has_part(n) && !edits.parts.contains_key(n)).unwrap();
    edits.parts.insert(name.clone(), data.to_vec());
    edits.content_type_defaults.entry(ext.clone()).or_insert_with(|| ct.to_string());
    let id = docx.next_rel_id(DOC, edits);
    edits.rels.push(NewRel { owner: DOC.into(), id: id.clone(), kind: REL_IMAGE.into(), target: name["word/".len()..].to_string(), external: false });
    Ok(id)
}

/// Highest `wp:docPr` id in the main document (drawing ids must be unique).
pub fn max_drawing_id(document_xml: &str) -> u32 {
    max_attr(document_xml, "docPr", "id").unwrap_or(0).clamp(0, u32::MAX as i64) as u32
}

/// Inline picture run content (`<w:drawing>`), sized in pixels at 96 dpi.
pub fn inline_picture(rel_id: &str, width_px: u32, height_px: u32, docpr_id: u32, name: &str) -> String {
    let (cx, cy) = (width_px as u64 * 9525, height_px as u64 * 9525);
    let name = escape(name);
    format!(concat!(
        r#"<w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing">"#,
        r#"<wp:extent cx="{cx}" cy="{cy}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:docPr id="{id}" name="{name}"/>"#,
        r#"<wp:cNvGraphicFramePr><a:graphicFrameLocks xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" noChangeAspect="1"/></wp:cNvGraphicFramePr>"#,
        r#"<a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">"#,
        r#"<pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:nvPicPr><pic:cNvPr id="{id}" name="{name}"/><pic:cNvPicPr/></pic:nvPicPr>"#,
        r#"<pic:blipFill><a:blip r:embed="{rid}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>"#,
        r#"<pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr>"#,
        r#"</pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing>"#),
        cx = cx, cy = cy, id = docpr_id, name = name, rid = escape(rel_id))
}
