//! A1 reference rewriting for row/column insert and delete.
//!
//! Formulas (cells, defined names, chart series, validation, conditional
//! formatting) and plain range lists (`sqref`, `ref`) are scanned for
//! references and only the references that point at the edited sheet are
//! rewritten; every other byte of the text is kept. Semantics follow Excel:
//!
//! - insert at `at`: positions `>= at` move by `count`; a range grows when the
//!   insertion falls strictly inside it, and moves when it starts at or after `at`.
//! - delete `at..at+count`: positions after the gap move back; a range loses the
//!   deleted part, and a reference left with nothing becomes `#REF!`.
//!
//! Absolute (`$`) and relative parts shift the same way: a structural edit
//! moves cells, it doesn't copy formulas.

pub const MAX_ROW: i32 = 1_048_576;
pub const MAX_COL: i32 = 16_384;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Row,
    Col,
}

/// One structural edit along one axis of one sheet. `count > 0` inserts, `count < 0` deletes.
#[derive(Debug, Clone)]
pub struct Shift {
    pub sheet: String,
    pub axis: Axis,
    pub at: i32,
    pub count: i32,
}

impl Shift {
    fn max(&self) -> i32 {
        match self.axis {
            Axis::Row => MAX_ROW,
            Axis::Col => MAX_COL,
        }
    }

    /// Where a single row/column index goes; `None` if it was deleted or pushed off the sheet.
    pub fn pos(&self, p: i32) -> Option<i32> {
        if self.count > 0 {
            let q = if p >= self.at { p + self.count } else { p };
            (q <= self.max()).then_some(q)
        } else {
            let n = -self.count;
            if p < self.at {
                Some(p)
            } else if p < self.at + n {
                None
            } else {
                Some(p - n)
            }
        }
    }

    /// Where an inclusive span goes; `None` if nothing of it is left.
    pub fn span(&self, lo: i32, hi: i32) -> Option<(i32, i32)> {
        let (lo, hi) = (lo.min(hi), lo.max(hi));
        if self.count > 0 {
            let lo2 = if lo >= self.at { lo + self.count } else { lo };
            let hi2 = if hi >= self.at { hi + self.count } else { hi };
            (lo2 <= self.max()).then_some((lo2, hi2.min(self.max())))
        } else {
            let n = -self.count;
            let lo2 = if lo < self.at { lo } else if lo < self.at + n { self.at } else { lo - n };
            let hi2 = if hi < self.at { hi } else if hi < self.at + n { self.at - 1 } else { hi - n };
            (hi2 >= lo2).then_some((lo2, hi2))
        }
    }

    /// The same shift for a pure translation check: `Some(delta)` if every index in
    /// `lo..=hi` moves by the same amount and none is deleted.
    pub fn uniform_delta(&self, lo: i32, hi: i32) -> Option<i32> {
        let (lo, hi) = (lo.min(hi), lo.max(hi));
        let a = self.pos(lo)? - lo;
        let b = self.pos(hi)? - hi;
        if a != b {
            return None;
        }
        // a delete gap strictly inside the span also breaks uniformity
        if self.count < 0 && lo < self.at && hi >= self.at {
            return None;
        }
        Some(a)
    }

    pub fn is_sheet(&self, name: &str) -> bool {
        self.sheet.to_lowercase() == name.to_lowercase()
    }
}

// ----------------------------------------------------------------------------- parsing

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Part {
    pub v: i32,
    pub abs: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pt {
    pub col: Part,
    pub row: Part,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    Cell(Pt),
    Area(Pt, Pt),
    Cols(Part, Part),
    Rows(Part, Part),
}

/// A reference found in formula text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Byte range of the reference body (after any `Sheet!` prefix).
    pub body_span: (usize, usize),
    pub body: Body,
    /// Unquoted sheet name if the reference was qualified.
    pub sheet: Option<String>,
    /// External workbook (`[1]Sheet!A1`) or 3D (`Sheet1:Sheet3!A1`): never ours to shift.
    pub foreign: bool,
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.' || c == '\\' || c == '$' || c == '?' || !c.is_ascii()
}

fn parse_col(s: &str) -> Option<Part> {
    let (abs, s) = match s.strip_prefix('$') {
        Some(r) => (true, r),
        None => (false, s),
    };
    if s.is_empty() || s.len() > 3 || !s.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    let v = s.bytes().fold(0i32, |a, b| a * 26 + (b.to_ascii_uppercase() - b'A' + 1) as i32);
    (v <= MAX_COL).then_some(Part { v, abs })
}

fn parse_row(s: &str) -> Option<Part> {
    let (abs, s) = match s.strip_prefix('$') {
        Some(r) => (true, r),
        None => (false, s),
    };
    if s.is_empty() || s.len() > 7 || !s.bytes().all(|b| b.is_ascii_digit()) || s.starts_with('0') {
        return None;
    }
    let v: i32 = s.parse().ok()?;
    (v <= MAX_ROW).then_some(Part { v, abs })
}

fn parse_cell(s: &str) -> Option<Pt> {
    let start = usize::from(s.starts_with('$'));
    let split = start + s[start..].find(|c: char| !c.is_ascii_alphabetic())?;
    Some(Pt { col: parse_col(&s[..split])?, row: parse_row(&s[split..])? })
}

/// Parse a standalone range like `A1`, `$A$1:B2`, `A:C` or `3:5` (no sheet prefix).
pub fn parse_body(s: &str) -> Option<Body> {
    match s.split_once(':') {
        None => parse_cell(s).map(Body::Cell),
        Some((a, b)) => {
            if let (Some(a), Some(b)) = (parse_cell(a), parse_cell(b)) {
                Some(Body::Area(a, b))
            } else if let (Some(a), Some(b)) = (parse_col(a), parse_col(b)) {
                Some(Body::Cols(a, b))
            } else if let (Some(a), Some(b)) = (parse_row(a), parse_row(b)) {
                Some(Body::Rows(a, b))
            } else {
                None
            }
        }
    }
}

/// Scan formula text (without the leading `=`) for references.
pub fn find_refs(f: &str) -> Vec<Found> {
    let b = f.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    // a sheet prefix waiting for its reference: (name, foreign)
    let mut prefix: Option<(String, bool)> = None;
    while i < b.len() {
        let c = b[i];
        match c {
            b'"' => {
                // string literal, "" escapes
                i += 1;
                while i < b.len() {
                    if b[i] == b'"' {
                        if b.get(i + 1) == Some(&b'"') {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
                prefix = None;
            }
            b'\'' => {
                // quoted sheet name: 'My Sheet'!  (or '[1]Sheet'! / 'A:B'!)
                let mut j = i + 1;
                let mut name = String::new();
                while j < b.len() {
                    if b[j] == b'\'' {
                        if b.get(j + 1) == Some(&b'\'') {
                            name.push('\'');
                            j += 2;
                            continue;
                        }
                        break;
                    }
                    let ch = f[j..].chars().next().unwrap();
                    name.push(ch);
                    j += ch.len_utf8();
                }
                if b.get(j + 1) == Some(&b'!') {
                    let foreign = name.starts_with('[') || name.contains(':');
                    prefix = Some((name, foreign));
                    i = j + 2;
                } else {
                    i = j + 1;
                    prefix = None;
                }
            }
            b'#' => {
                // error literal like #REF! or #N/A: skip so `REF!` isn't read as a sheet
                let mut j = i + 1;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'/') {
                    j += 1;
                }
                if j < b.len() && (b[j] == b'!' || b[j] == b'?') {
                    j += 1;
                }
                i = j;
                prefix = None;
            }
            b'[' => {
                // external workbook index ([1]Sheet1!A1) or a structured reference body: skip
                let mut depth = 0;
                let mut j = i;
                while j < b.len() {
                    match b[j] {
                        b'[' => depth += 1,
                        b']' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        b'\'' => j += 1, // escape inside structured refs
                        _ => {}
                    }
                    j += 1;
                }
                let ext = i == 0 || !is_ident(f[..i].chars().next_back().unwrap_or(' '));
                i = j + 1;
                if ext {
                    // [1]Sheet1!A1: read the sheet name and mark it foreign
                    let start = i;
                    while i < b.len() && is_ident(f[i..].chars().next().unwrap()) {
                        i += f[i..].chars().next().unwrap().len_utf8();
                    }
                    if b.get(i) == Some(&b'!') {
                        prefix = Some((f[start..i].to_string(), true));
                        i += 1;
                    } else {
                        prefix = None;
                    }
                }
            }
            b'0'..=b'9' => {
                // number literal, or a whole-row range like 3:5
                let start = i;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                    i += 1;
                }
                if b.get(i) == Some(&b':') {
                    let k = i + 1;
                    let mut e = k;
                    while e < b.len() && (b[e].is_ascii_digit() || b[e] == b'$') {
                        e += 1;
                    }
                    if let Some(bd @ Body::Rows(..)) = parse_body(&f[start..e]) {
                        let (sheet, foreign) = match prefix.take() {
                            Some((n, fo)) => (Some(n), fo),
                            None => (None, false),
                        };
                        out.push(Found { body_span: (start, e), body: bd, sheet, foreign });
                        i = e;
                        continue;
                    }
                }
                // exponent: 1E+5
                if i < b.len() && (b[i] == b'E' || b[i] == b'e') {
                    i += 1;
                    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
                        i += 1;
                    }
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                prefix = None;
            }
            _ if is_ident(f[i..].chars().next().unwrap()) => {
                let start = i;
                while i < b.len() && is_ident(f[i..].chars().next().unwrap()) {
                    i += f[i..].chars().next().unwrap().len_utf8();
                }
                let word = &f[start..i];
                // sheet prefix: Sheet1!  or 3D Sheet1:Sheet3!
                if b.get(i) == Some(&b'!') && prefix.is_none() {
                    prefix = Some((word.to_string(), false));
                    i += 1;
                    continue;
                }
                if b.get(i) == Some(&b':') && prefix.is_none() {
                    // could be 3D sheet span `S1:S3!`, or a range
                    let k = i + 1;
                    let mut e = k;
                    while e < b.len() && is_ident(f[e..].chars().next().unwrap()) {
                        e += f[e..].chars().next().unwrap().len_utf8();
                    }
                    if b.get(e) == Some(&b'!') && parse_body(&f[start..e]).is_none() {
                        prefix = Some((f[start..e].to_string(), true));
                        i = e + 1;
                        continue;
                    }
                }
                if b.get(i) == Some(&b'(') {
                    prefix = None;
                    continue; // function name
                }
                // try a range `word:word2` first, then a single cell
                let mut end = i;
                let mut body = None;
                if b.get(i) == Some(&b':') {
                    let k = i + 1;
                    // the second half may carry its own (same) sheet prefix: A1:Sheet1!B2 is rare; not handled
                    let mut e = k;
                    while e < b.len() && is_ident(f[e..].chars().next().unwrap()) {
                        e += f[e..].chars().next().unwrap().len_utf8();
                    }
                    if e > k && b.get(e) != Some(&b'(') && b.get(e) != Some(&b'!') {
                        if let Some(bd) = parse_body(&f[start..e]) {
                            body = Some(bd);
                            end = e;
                        }
                    }
                }
                if body.is_none() {
                    body = parse_cell(word).map(Body::Cell);
                }
                if let Some(body) = body {
                    let (sheet, foreign) = match prefix.take() {
                        Some((n, fo)) => (Some(n), fo),
                        None => (None, false),
                    };
                    out.push(Found { body_span: (start, end), body, sheet, foreign });
                    i = end;
                } else {
                    prefix = None;
                }
            }
            _ => {
                i += f[i..].chars().next().map_or(1, char::len_utf8);
                prefix = None;
            }
        }
    }
    out
}

// ----------------------------------------------------------------------------- shifting

fn col_letters(mut n: i32) -> String {
    let mut s = Vec::new();
    while n > 0 {
        s.push((b'A' + ((n - 1) % 26) as u8) as char);
        n = (n - 1) / 26;
    }
    s.iter().rev().collect()
}

fn fmt_col(p: Part) -> String {
    format!("{}{}", if p.abs { "$" } else { "" }, col_letters(p.v))
}

fn fmt_row(p: Part) -> String {
    format!("{}{}", if p.abs { "$" } else { "" }, p.v)
}

pub fn fmt_body(b: &Body) -> String {
    match b {
        Body::Cell(p) => format!("{}{}", fmt_col(p.col), fmt_row(p.row)),
        Body::Area(a, c) => format!("{}{}:{}{}", fmt_col(a.col), fmt_row(a.row), fmt_col(c.col), fmt_row(c.row)),
        Body::Cols(a, c) => format!("{}:{}", fmt_col(*a), fmt_col(*c)),
        Body::Rows(a, c) => format!("{}:{}", fmt_row(*a), fmt_row(*c)),
    }
}

/// Shift one reference body. `None` means it no longer exists (`#REF!`).
pub fn shift_body(b: &Body, s: &Shift) -> Option<Body> {
    let one = |p: Part| s.pos(p.v).map(|v| Part { v, ..p });
    let two = |a: Part, c: Part| -> Option<(Part, Part)> {
        let swap = a.v > c.v;
        let (lo, hi) = if swap { (c, a) } else { (a, c) };
        let (l, h) = s.span(lo.v, hi.v)?;
        let (lo, hi) = (Part { v: l, ..lo }, Part { v: h, ..hi });
        Some(if swap { (hi, lo) } else { (lo, hi) })
    };
    Some(match (*b, s.axis) {
        (Body::Cell(p), Axis::Row) => Body::Cell(Pt { row: one(p.row)?, ..p }),
        (Body::Cell(p), Axis::Col) => Body::Cell(Pt { col: one(p.col)?, ..p }),
        (Body::Area(a, c), Axis::Row) => {
            let (r1, r2) = two(a.row, c.row)?;
            Body::Area(Pt { row: r1, ..a }, Pt { row: r2, ..c })
        }
        (Body::Area(a, c), Axis::Col) => {
            let (c1, c2) = two(a.col, c.col)?;
            Body::Area(Pt { col: c1, ..a }, Pt { col: c2, ..c })
        }
        (Body::Rows(a, c), Axis::Row) => {
            let (a, c) = two(a, c)?;
            Body::Rows(a, c)
        }
        (Body::Cols(a, c), Axis::Col) => {
            let (a, c) = two(a, c)?;
            Body::Cols(a, c)
        }
        (other, _) => other, // whole columns don't move when rows do, and vice versa
    })
}

/// Does this reference (found in a formula evaluated on `current`) point at the shifted sheet?
pub fn targets(r: &Found, current: Option<&str>, s: &Shift) -> bool {
    if r.foreign {
        return false;
    }
    match (&r.sheet, current) {
        (Some(n), _) => s.is_sheet(n),
        (None, Some(c)) => s.is_sheet(c),
        (None, None) => false,
    }
}

/// Rewrite every reference to the shifted sheet in formula text. `current` is the sheet
/// unqualified references resolve to (`None` for defined names and chart series).
/// Returns `None` if nothing changed, so callers can keep the original bytes.
pub fn shift_formula(f: &str, current: Option<&str>, s: &Shift) -> Option<String> {
    let mut out = String::with_capacity(f.len() + 8);
    let mut cursor = 0;
    let mut changed = false;
    for r in find_refs(f) {
        if !targets(&r, current, s) {
            continue;
        }
        let new = match shift_body(&r.body, s) {
            Some(b) if b == r.body => continue,
            Some(b) => fmt_body(&b),
            None => "#REF!".to_string(),
        };
        out.push_str(&f[cursor..r.body_span.0]);
        out.push_str(&new);
        cursor = r.body_span.1;
        changed = true;
    }
    if !changed {
        return None;
    }
    out.push_str(&f[cursor..]);
    Some(out)
}

/// Shift a space-separated list of ranges on the edited sheet (`sqref`, `ref`).
/// `None` = unchanged; `Some(None)` = every range was deleted.
pub fn shift_sqref(list: &str, s: &Shift) -> Option<Option<String>> {
    let mut parts = Vec::new();
    let mut changed = false;
    for item in list.split_whitespace() {
        match parse_body(item) {
            Some(b) => match shift_body(&b, s) {
                Some(n) if n == b => parts.push(item.to_string()),
                Some(n) => {
                    // a range shrunk to one cell is written as that cell, as Excel does
                    let n = match n {
                        Body::Area(a, c) if a.row.v == c.row.v && a.col.v == c.col.v => Body::Cell(a),
                        n => n,
                    };
                    parts.push(fmt_body(&n));
                    changed = true;
                }
                None => changed = true,
            },
            None => parts.push(item.to_string()),
        }
    }
    if !changed {
        return None;
    }
    Some((!parts.is_empty()).then(|| parts.join(" ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(at: i32, count: i32) -> Shift {
        Shift { sheet: "Data".into(), axis: Axis::Row, at, count }
    }
    fn cols(at: i32, count: i32) -> Shift {
        Shift { sheet: "Data".into(), axis: Axis::Col, at, count }
    }
    fn sh(f: &str, cur: Option<&str>, s: &Shift) -> String {
        shift_formula(f, cur, s).unwrap_or_else(|| f.to_string())
    }

    #[test]
    fn insert_rows_cells_and_ranges() {
        let s = rows(5, 2);
        let d = Some("Data");
        assert_eq!(sh("A4+A5+$B$10", d, &s), "A4+A7+$B$12");
        assert_eq!(sh("SUM(A1:A10)", d, &s), "SUM(A1:A12)", "grows when inserted inside");
        assert_eq!(sh("SUM(A5:A10)", d, &s), "SUM(A7:A12)", "moves when inserted at its start");
        assert_eq!(sh("SUM(A1:A4)", d, &s), "SUM(A1:A4)", "untouched above");
        assert_eq!(sh("SUM(A:A)+SUM(3:6)+SUM($5:$5)", d, &s), "SUM(A:A)+SUM(3:8)+SUM($7:$7)");
        assert_eq!(sh("A1048575", d, &s), "#REF!");
    }

    #[test]
    fn delete_rows_shrinks_and_refs() {
        let s = rows(5, -2); // deletes 5..6
        let d = Some("Data");
        assert_eq!(sh("A4+A5+A6+A7", d, &s), "A4+#REF!+#REF!+A5");
        assert_eq!(sh("SUM(A1:A10)", d, &rows(5, -2)), "SUM(A1:A8)");
        assert_eq!(sh("SUM(A5:A6)", d, &rows(5, -2)), "SUM(#REF!)");
        assert_eq!(sh("SUM(A6:A9)", d, &rows(5, -2)), "SUM(A5:A7)");
        assert_eq!(sh("A4+A5+A6+A7", d, &rows(5, -2)), "A4+#REF!+#REF!+A5");
    }

    #[test]
    fn columns() {
        let d = Some("Data");
        assert_eq!(sh("B1+$C$2+SUM(B:D)+SUM(1:1)", d, &cols(2, 1)), "C1+$D$2+SUM(C:E)+SUM(1:1)");
        assert_eq!(sh("Z9+AA9", d, &cols(26, -1)), "#REF!+Z9");
        assert_eq!(sh("SUM(A1:XFD1)", d, &cols(3, 1)), "SUM(A1:XFD1)", "clamped at the last column");
    }

    #[test]
    fn sheet_qualification() {
        let s = rows(2, 1);
        assert_eq!(sh("Data!A3+A3", Some("Other"), &s), "Data!A4+A3");
        assert_eq!(sh("'Data'!A3:B4+data!C3", Some("Other"), &s), "'Data'!A4:B5+data!C4");
        assert_eq!(sh("'My ''Q'' Sheet'!A3", Some("x"), &Shift { sheet: "My 'Q' Sheet".into(), ..s.clone() }), "'My ''Q'' Sheet'!A4");
        assert_eq!(sh("[1]Data!A3+'[2]Data'!A3", Some("Data"), &s), "[1]Data!A3+'[2]Data'!A3", "external");
        assert_eq!(sh("SUM(Data:Other!A3)", Some("Data"), &s), "SUM(Data:Other!A3)", "3D left alone");
        assert_eq!(sh("A3", None, &s), "A3", "unqualified with no current sheet");
        assert_eq!(sh("Data!#REF!+Data!A3", None, &s), "Data!#REF!+Data!A4");
    }

    #[test]
    fn not_references() {
        let s = rows(1, 1);
        let d = Some("Data");
        assert_eq!(sh(r#"LOG10(A1)&"A1"&"say ""B2"""&Table1[Col A1]&_xlfn.XLOOKUP(1,B1:B2,C1:C2)"#, d, &s),
            r#"LOG10(A2)&"A1"&"say ""B2"""&Table1[Col A1]&_xlfn.XLOOKUP(1,B2:B3,C2:C3)"#);
        assert_eq!(sh("1.5E+3+TaxRate+A1B+ROW()", d, &s), "1.5E+3+TaxRate+A1B+ROW()");
        assert_eq!(sh("Résumé!A1", d, &Shift { sheet: "Résumé".into(), ..s.clone() }), "Résumé!A2");
    }

    #[test]
    fn sqref_lists() {
        assert_eq!(shift_sqref("A1:B2 D5 F9:F12", &rows(5, 3)), Some(Some("A1:B2 D8 F12:F15".into())));
        assert_eq!(shift_sqref("A1:B2", &rows(5, 3)), None);
        assert_eq!(shift_sqref("D5 A1", &rows(5, -1)), Some(Some("A1".into())));
        assert_eq!(shift_sqref("D5", &rows(5, -1)), Some(None));
    }

    #[test]
    fn uniform_delta() {
        let s = rows(5, 2);
        assert_eq!(s.uniform_delta(1, 4), Some(0));
        assert_eq!(s.uniform_delta(5, 9), Some(2));
        assert_eq!(s.uniform_delta(4, 5), None);
        let d = rows(5, -2);
        assert_eq!(d.uniform_delta(7, 9), Some(-2));
        assert_eq!(d.uniform_delta(4, 7), None);
        assert_eq!(d.uniform_delta(5, 5), None);
    }
}
