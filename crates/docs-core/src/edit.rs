//! Editing operations on one container of blocks (see [`crate::blocks`]). Positions are
//! (paragraph index, visible offset) inside that container.

use ooxml::docx::props::{ParaFormat, Props, RunFormat};
use ooxml::docx::{Para, Rev};

use crate::blocks::Blocks;

pub type At = (usize, usize);

fn para(b: &dyn Blocks, i: usize) -> Result<Para, String> {
    b.para(i).ok_or_else(|| "that block is not a paragraph".to_string())
}

/// Insert `text` at `at` (newlines start new paragraphs). Returns where the caret ends.
pub fn insert(b: &mut dyn Blocks, at: At, text: &str, fmt: Option<&RunFormat>, rev: Option<&Rev>, next_style: &dyn Fn(&str) -> Option<String>) -> Result<At, String> {
    let mut cur = at;
    let lines: Vec<&str> = text.split('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        let line = line.trim_end_matches('\r');
        if !line.is_empty() {
            let mut p = para(b, cur.0)?;
            p.insert_text(cur.1, line, fmt, rev);
            b.set_para(cur.0, p);
            cur.1 += line.chars().count();
        }
        if i + 1 < lines.len() {
            cur = split(b, cur, rev, next_style)?;
        }
    }
    Ok(cur)
}

/// Enter: split the paragraph at `at`. At the end of a heading, the new paragraph takes
/// the style's "next" style (usually Normal).
pub fn split(b: &mut dyn Blocks, at: At, rev: Option<&Rev>, next_style: &dyn Fn(&str) -> Option<String>) -> Result<At, String> {
    let mut p = para(b, at.0)?;
    let at_end = at.1 >= p.len();
    let mut tail = p.split(at.1, rev);
    if at_end {
        let style = tail.ppr.as_ref().and_then(|x| x.get("pStyle")).and_then(|x| ooxml::xml::attr(x, "w:val").map(str::to_string));
        if let Some(next) = style.and_then(|id| next_style(&id)) {
            tail.format_paragraph(&ParaFormat { style: Some(if next == "Normal" { None } else { Some(next) }), ..Default::default() }, None);
        }
    }
    b.set_para(at.0, p);
    b.insert_para(at.0 + 1, tail);
    Ok((at.0 + 1, 0))
}

/// Delete between two positions in the same container, joining paragraphs (blocks
/// strictly between them go too). Tracked deletions keep everything and mark it.
pub fn delete(b: &mut dyn Blocks, a: At, z: At, rev: Option<&Rev>) -> Result<At, String> {
    let (from, to) = if a <= z { (a, z) } else { (z, a) };
    if from == to {
        return Ok(from);
    }
    if from.0 == to.0 {
        let mut p = para(b, from.0)?;
        p.delete(from.1, to.1, rev);
        b.set_para(from.0, p);
        return Ok(from);
    }
    if b.kind(from.0) != "p" || b.kind(to.0) != "p" {
        return Err("a selection must start and end in paragraphs".into());
    }
    let mut first = para(b, from.0)?;
    let l = first.len();
    first.delete(from.1, l, rev);
    let mut last = para(b, to.0)?;
    last.delete(0, to.1, rev);
    match rev {
        Some(r) => {
            first.mark_deleted(r);
            b.set_para(from.0, first);
            b.set_para(to.0, last);
            for i in from.0 + 1..to.0 {
                if let Some(mut p) = b.para(i) {
                    let l = p.len();
                    p.delete(0, l, Some(r));
                    p.mark_deleted(r);
                    b.set_para(i, p);
                }
            }
        }
        None => {
            first.merge(last);
            b.set_para(from.0, first);
            for _ in from.0 + 1..=to.0 {
                b.remove(from.0 + 1);
            }
        }
    }
    Ok(from)
}

pub fn format(b: &mut dyn Blocks, a: At, z: At, fmt: &RunFormat, rev: Option<&Rev>) -> Result<(), String> {
    let (from, to) = if a <= z { (a, z) } else { (z, a) };
    for i in from.0..=to.0 {
        let Some(mut p) = b.para(i) else { continue };
        let s = if i == from.0 { from.1 } else { 0 };
        let e = if i == to.0 { to.1 } else { p.len() };
        if s < e {
            p.format(s, e, fmt, rev);
        } else if from == to {
            // Collapsed: the paragraph mark, so typing in an empty paragraph picks it up.
            let mut ppr = p.ppr.take().unwrap_or_else(|| Props::new("w:pPr"));
            let mut mark = ppr.get("rPr").map(Props::parse).unwrap_or_else(|| Props::new("w:rPr"));
            fmt.apply(&mut mark);
            ppr.set("rPr", if mark.is_empty() { None } else { Some(mark.to_xml()) });
            p.ppr = Some(ppr);
        }
        b.set_para(i, p);
    }
    Ok(())
}

/// Paragraph formatting for paragraphs `from..=to`; `indent` moves the left indent by
/// that many half-inch steps.
pub fn para_format(b: &mut dyn Blocks, from: usize, to: usize, fmt: &ParaFormat, indent: Option<i64>, rev: Option<&Rev>) -> Result<(), String> {
    for i in from.min(to)..=to.max(from) {
        let Some(mut p) = b.para(i) else { continue };
        if let Some(d) = indent {
            let cur = p.ppr.as_ref().and_then(|x| x.get("ind")).and_then(|x| ooxml::xml::attr(x, "w:left").and_then(|v| v.parse::<i64>().ok())).unwrap_or(0);
            let left = (cur + d * 720).max(0);
            p.format_paragraph(&ParaFormat { indent: Some((left > 0).then(|| format!(r#"<w:ind w:left="{left}"/>"#))), ..Default::default() }, rev);
        }
        p.format_paragraph(fmt, rev);
        b.set_para(i, p);
    }
    Ok(())
}

pub fn insert_object(b: &mut dyn Blocks, at: At, content: &str, rpr: Option<Props>, rev: Option<&Rev>) -> Result<At, String> {
    let mut p = para(b, at.0)?;
    p.insert_object(at.1, content, rpr, rev);
    b.set_para(at.0, p);
    Ok((at.0, at.1 + 1))
}

/// Insert a block (a table) at the caret: between paragraphs, splitting one if needed.
/// Returns the position just after it.
pub fn insert_block(b: &mut dyn Blocks, at: At, xml: String, next_style: &dyn Fn(&str) -> Option<String>) -> Result<At, String> {
    let len = b.para_len(at.0);
    let idx = if at.1 == 0 {
        at.0
    } else if at.1 >= len {
        at.0 + 1
    } else {
        split(b, at, None, next_style)?.0
    };
    b.insert_xml(idx, xml);
    if idx + 1 >= b.len() || b.kind(idx + 1) != "p" {
        b.insert_para(idx + 1, Para::new("", None, None));
    }
    Ok((idx + 1, 0))
}

/// Plain text between two positions in the same container (paragraphs joined by newlines).
pub fn text(b: &dyn Blocks, a: At, z: At) -> String {
    let (from, to) = if a <= z { (a, z) } else { (z, a) };
    let mut out = Vec::new();
    for i in from.0..=to.0 {
        let Some(p) = b.para(i) else { continue };
        let chars: Vec<char> = p.text().chars().collect();
        let s = if i == from.0 { from.1.min(chars.len()) } else { 0 };
        let e = if i == to.0 { to.1.min(chars.len()) } else { chars.len() };
        out.push(chars[s..e.max(s)].iter().collect::<String>());
    }
    out.join("\n")
}
