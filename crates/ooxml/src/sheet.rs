//! Worksheet XML: parse `<sheetData>` into rows/cells with byte ranges, and
//! rebuild it with only the changed cells swapped out. Everything outside
//! `<sheetData>` (and every untouched row) is copied byte-for-byte.

use std::collections::BTreeMap;

use crate::xml::{self, attr, set_attr, tokens, Kind, Tok};

#[derive(Debug, Clone)]
pub struct CellXml {
    pub col: i32,
    pub xml: String,
}

#[derive(Debug, Clone)]
pub struct RowXml {
    pub r: i32,
    /// Raw start tag (`<row ...>`), or the whole tag if the row is empty (`<row .../>`).
    pub start_tag: String,
    pub cells: Vec<CellXml>,
    /// Byte range of the whole row in the source.
    pub span: (usize, usize),
}

#[derive(Debug)]
pub struct SheetXml<'a> {
    pub src: &'a str,
    /// (outer start, inner start, inner end, outer end) of `<sheetData>`.
    pub sheet_data: (usize, usize, usize, usize),
    pub sheet_data_empty: bool,
    pub prefix: Option<String>,
    pub rows: Vec<RowXml>,
    pub dimension: Option<(usize, usize)>,
}

pub fn parse_ref(r: &str) -> Option<(i32, i32)> {
    let r = r.trim().trim_start_matches('$');
    let split = r.find(|c: char| c.is_ascii_digit() || c == '$')?;
    let (letters, digits) = r.split_at(split);
    let digits = digits.trim_start_matches('$');
    let mut col = 0i32;
    for ch in letters.chars() {
        if !ch.is_ascii_alphabetic() {
            return None;
        }
        col = col * 26 + (ch.to_ascii_uppercase() as i32 - 64);
    }
    Some((digits.parse().ok()?, col))
}

pub fn col_name(mut n: i32) -> String {
    let mut s = Vec::new();
    while n > 0 {
        let r = (n - 1) % 26;
        s.push((b'A' + r as u8) as char);
        n = (n - 1) / 26;
    }
    s.iter().rev().collect()
}

pub fn cell_ref(row: i32, col: i32) -> String {
    format!("{}{row}", col_name(col))
}

pub fn parse(src: &str) -> Result<SheetXml<'_>, String> {
    let toks: Vec<Tok> = tokens(src).collect();
    let mut out = SheetXml {
        src,
        sheet_data: (0, 0, 0, 0),
        sheet_data_empty: false,
        prefix: None,
        rows: Vec::new(),
        dimension: None,
    };
    let mut i = 0;
    let mut found = false;
    while i < toks.len() {
        let t = toks[i];
        match (t.kind, t.local()) {
            (Kind::Empty, "dimension") => out.dimension = Some((t.start, t.end)),
            (Kind::Empty, "sheetData") => {
                out.sheet_data = (t.start, t.end, t.end, t.end);
                out.sheet_data_empty = true;
                out.prefix = xml::prefix(t.name).map(str::to_string);
                found = true;
            }
            (Kind::Start, "sheetData") => {
                out.prefix = xml::prefix(t.name).map(str::to_string);
                found = true;
                let end = xml::element_end(&toks, i);
                let close = src[..end].rfind('<').unwrap_or(end);
                out.sheet_data = (t.start, t.end, close, end);
                i = parse_rows(src, &toks, i + 1, end, &mut out.rows)?;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    if !found {
        return Err("worksheet has no <sheetData>".into());
    }
    Ok(out)
}

fn parse_rows(src: &str, toks: &[Tok], mut i: usize, stop: usize, rows: &mut Vec<RowXml>) -> Result<usize, String> {
    let mut last_row = 0;
    while i < toks.len() && toks[i].start < stop {
        let t = toks[i];
        if t.local() != "row" || (t.kind != Kind::Start && t.kind != Kind::Empty) {
            i += 1;
            continue;
        }
        let tag = &src[t.start..t.end];
        let r = match attr(tag, "r") {
            Some(v) => v.parse().map_err(|_| format!("bad row number {v}"))?,
            None => last_row + 1,
        };
        last_row = r;
        let mut row = RowXml { r, start_tag: tag.to_string(), cells: Vec::new(), span: (t.start, t.end) };
        if t.kind == Kind::Empty {
            rows.push(row);
            i += 1;
            continue;
        }
        let row_end = xml::element_end(toks, i);
        row.span.1 = row_end;
        i += 1;
        let mut last_col = 0;
        while i < toks.len() && toks[i].start < row_end {
            let c = toks[i];
            if c.local() == "c" && (c.kind == Kind::Start || c.kind == Kind::Empty) {
                let ctag = &src[c.start..c.end];
                let col = match attr(ctag, "r").and_then(parse_ref) {
                    Some((_, col)) => col,
                    None => last_col + 1,
                };
                last_col = col;
                let end = xml::element_end(toks, i);
                let mut cell_xml = src[c.start..end].to_string();
                if attr(ctag, "r").is_none() {
                    // make the position explicit so the cell survives reordering
                    let fixed = set_attr(ctag, "r", Some(&cell_ref(r, col)));
                    cell_xml = format!("{fixed}{}", &src[c.end..end]);
                }
                row.cells.push(CellXml { col, xml: cell_xml });
                while i < toks.len() && toks[i].start < end {
                    i += 1;
                }
                continue;
            }
            i += 1;
        }
        rows.push(row);
    }
    Ok(i)
}

/// What to do with one cell when rebuilding.
pub enum CellEdit {
    Replace(String),
    Remove,
}

/// Rebuild the sheet with `edits` applied. Rows without edits are copied verbatim.
pub fn splice(sheet: &SheetXml, edits: &BTreeMap<(i32, i32), CellEdit>, new_dimension: Option<&str>) -> String {
    let src = sheet.src;
    let p = sheet.prefix.as_deref().map(|p| format!("{p}:")).unwrap_or_default();
    let mut by_row: BTreeMap<i32, Vec<(i32, &CellEdit)>> = BTreeMap::new();
    for ((r, c), e) in edits {
        by_row.entry(*r).or_default().push((*c, e));
    }

    let mut body = String::with_capacity(sheet.sheet_data.2 - sheet.sheet_data.1 + 1024);
    let mut orig_rows = sheet.rows.iter().peekable();
    let mut pending = by_row.into_iter().peekable();
    loop {
        let next_orig = orig_rows.peek().map(|r| r.r);
        let next_edit = pending.peek().map(|(r, _)| *r);
        match (next_orig, next_edit) {
            (None, None) => break,
            (Some(o), e) if e.is_none() || o < e.unwrap() => {
                let row = orig_rows.next().unwrap();
                body.push_str(&src[row.span.0..row.span.1]);
            }
            (o, Some(e)) => {
                let row = if o == Some(e) { orig_rows.next() } else { None };
                let (_, cell_edits) = pending.next().unwrap();
                body.push_str(&rebuild_row(row, e, &cell_edits, &p));
            }
            _ => unreachable!(),
        }
    }

    let (outer_start, inner_start, inner_end, outer_end) = sheet.sheet_data;
    let mut out = String::with_capacity(src.len() + body.len());
    let mut cursor = 0;
    if let (Some((ds, de)), Some(dim)) = (sheet.dimension, new_dimension) {
        if de <= outer_start {
            out.push_str(&src[..ds]);
            out.push_str(&set_attr(&src[ds..de], "ref", Some(dim)));
            cursor = de;
        }
    }
    if sheet.sheet_data_empty {
        out.push_str(&src[cursor..outer_start]);
        out.push_str(&format!("<{p}sheetData>{body}</{p}sheetData>"));
    } else {
        out.push_str(&src[cursor..inner_start]);
        out.push_str(&body);
        out.push_str(&src[inner_end..outer_end]);
    }
    out.push_str(&src[outer_end..]);
    out
}

fn rebuild_row(row: Option<&RowXml>, r: i32, edits: &[(i32, &CellEdit)], p: &str) -> String {
    let mut cells: BTreeMap<i32, String> = BTreeMap::new();
    if let Some(row) = row {
        for c in &row.cells {
            cells.insert(c.col, c.xml.clone());
        }
    }
    for (col, e) in edits {
        match e {
            CellEdit::Replace(x) => {
                cells.insert(*col, x.clone());
            }
            CellEdit::Remove => {
                cells.remove(col);
            }
        }
    }
    // `spans` is an optional hint that may now be wrong; drop it.
    let start = match row {
        Some(row) => {
            let t = set_attr(&row.start_tag, "spans", None);
            let t = set_attr(&t, "r", Some(&r.to_string()));
            t.replacen("/>", ">", 1)
        }
        None => format!("<{p}row r=\"{r}\">"),
    };
    if cells.is_empty() && row.is_none() {
        return String::new();
    }
    let mut out = start;
    for x in cells.values() {
        out.push_str(x);
    }
    out.push_str(&format!("</{p}row>"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHEET: &str = r#"<worksheet><dimension ref="A1:B2"/><sheetData><row r="1" spans="1:2"><c r="A1" s="1"><v>1</v></c><c r="B1" t="s"><v>0</v></c></row><row r="2"><c r="A2"><f>A1*2</f><v>2</v></c></row></sheetData><mergeCells count="1"><mergeCell ref="A5:B5"/></mergeCells></worksheet>"#;

    #[test]
    fn parses_rows_and_cells() {
        let s = parse(SHEET).unwrap();
        assert_eq!(s.rows.len(), 2);
        assert_eq!(s.rows[0].cells[1].col, 2);
        assert!(s.rows[1].cells[0].xml.contains("<f>A1*2</f>"));
    }

    #[test]
    fn untouched_is_identical() {
        let s = parse(SHEET).unwrap();
        assert_eq!(splice(&s, &BTreeMap::new(), None), SHEET);
    }

    #[test]
    fn edits_only_touch_their_rows() {
        let s = parse(SHEET).unwrap();
        let mut e = BTreeMap::new();
        e.insert((2, 1), CellEdit::Replace(r#"<c r="A2"><v>9</v></c>"#.into()));
        e.insert((4, 3), CellEdit::Replace(r#"<c r="C4"><v>5</v></c>"#.into()));
        let out = splice(&s, &e, Some("A1:C4"));
        assert!(out.contains(r#"<row r="1" spans="1:2"><c r="A1" s="1">"#), "row 1 untouched");
        assert!(out.contains(r#"<row r="2"><c r="A2"><v>9</v></c></row><row r="4"><c r="C4"><v>5</v></c></row>"#));
        assert!(out.contains(r#"<dimension ref="A1:C4"/>"#));
        assert!(out.ends_with(r#"<mergeCells count="1"><mergeCell ref="A5:B5"/></mergeCells></worksheet>"#));
    }
}
