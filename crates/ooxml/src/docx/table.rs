//! Editable table (`w:tbl`): rows and cells as structure, cell contents as raw
//! block XML (edit a cell's paragraphs with [`super::Para`]). Anything that isn't a
//! row or cell (properties, row-level content controls, bookmarks) is kept verbatim.

use super::para::{Para, SegKind};
use super::props::Props;
use crate::xml::{element_end, tokens, Kind, Tok};

#[derive(Debug, Clone, PartialEq)]
pub enum Item<T> {
    Node(T),
    Raw(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub open: String,
    pub tc_pr: Option<String>,
    /// Block-level children: `w:p`, nested `w:tbl`, content controls, ... (raw XML).
    pub blocks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub open: String,
    pub items: Vec<Item<Cell>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub open: String,
    pub items: Vec<Item<Row>>,
}

fn children<'a>(src: &'a str, toks: &[Tok<'a>], from: usize, end: usize) -> Vec<(usize, &'a str, &'a str)> {
    let mut out = Vec::new();
    let mut i = from;
    while i < toks.len() && toks[i].start < end {
        let t = toks[i];
        if matches!(t.kind, Kind::Start | Kind::Empty) {
            let e = element_end(toks, i);
            out.push((i, t.local(), &src[t.start..e]));
            while i < toks.len() && toks[i].start < e {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

fn open_tag(src: &str, t: Tok) -> String {
    if t.kind == Kind::Empty {
        src[t.start..t.end].trim_end_matches("/>").trim_end().to_string() + ">"
    } else {
        src[t.start..t.end].to_string()
    }
}

fn close_of(open: &str) -> String {
    let name = open.trim_start_matches('<').split(|c: char| c.is_whitespace() || c == '>' || c == '/').next().unwrap_or("");
    format!("</{name}>")
}

impl Table {
    pub fn parse(src: &str) -> Result<Table, String> {
        let toks: Vec<Tok> = tokens(src).collect();
        let first = toks.iter().position(|t| matches!(t.kind, Kind::Start | Kind::Empty)).ok_or("empty table xml")?;
        if toks[first].local() != "tbl" {
            return Err("not a table".into());
        }
        let mut table = Table { open: open_tag(src, toks[first]), items: vec![] };
        let end = element_end(&toks, first);
        for (ri, name, raw) in children(src, &toks, first + 1, end) {
            if name != "tr" {
                table.items.push(Item::Raw(raw.to_string()));
                continue;
            }
            let mut row = Row { open: open_tag(src, toks[ri]), items: vec![] };
            let rend = element_end(&toks, ri);
            for (ci, cname, craw) in children(src, &toks, ri + 1, rend) {
                if cname != "tc" {
                    row.items.push(Item::Raw(craw.to_string()));
                    continue;
                }
                let mut cell = Cell { open: open_tag(src, toks[ci]), tc_pr: None, blocks: vec![] };
                let cend = element_end(&toks, ci);
                for (_, bname, braw) in children(src, &toks, ci + 1, cend) {
                    if bname == "tcPr" {
                        cell.tc_pr = Some(braw.to_string());
                    } else {
                        cell.blocks.push(braw.to_string());
                    }
                }
                row.items.push(Item::Node(cell));
            }
            table.items.push(Item::Node(row));
        }
        Ok(table)
    }

    pub fn rows(&self) -> impl Iterator<Item = &Row> {
        self.items.iter().filter_map(|i| match i { Item::Node(r) => Some(r), _ => None })
    }

    pub fn row_mut(&mut self, r: usize) -> Option<&mut Row> {
        self.items.iter_mut().filter_map(|i| match i { Item::Node(r) => Some(r), _ => None }).nth(r)
    }

    pub fn cell_mut(&mut self, r: usize, c: usize) -> Option<&mut Cell> {
        self.row_mut(r)?.cell_mut(c)
    }

    /// Insert a copy of row `r` after it, with every cell emptied (formatting kept).
    pub fn insert_row_after(&mut self, r: usize) -> bool {
        let Some(pos) = self.row_position(r) else { return false };
        let Item::Node(row) = &self.items[pos] else { return false };
        let mut new = row.clone();
        new.open = strip_ids(&new.open);
        for item in new.items.iter_mut() {
            if let Item::Node(cell) = item {
                cell.set_text("");
            }
        }
        self.items.insert(pos + 1, Item::Node(new));
        true
    }

    /// Insert an empty copy of row `r` above it.
    pub fn insert_row_before(&mut self, r: usize) -> bool {
        let Some(pos) = self.row_position(r) else { return false };
        let Item::Node(row) = &self.items[pos] else { return false };
        let mut new = row.clone();
        new.open = strip_ids(&new.open);
        for item in new.items.iter_mut() {
            if let Item::Node(cell) = item {
                cell.set_text("");
            }
        }
        self.items.insert(pos, Item::Node(new));
        true
    }

    /// Grid column widths (`w:tblGrid`), in twips.
    pub fn grid(&self) -> Vec<i64> {
        self.items.iter().find_map(|i| match i {
            Item::Raw(x) if x.trim_start().starts_with("<w:tblGrid") => Some(
                tokens(x).filter(|t| t.local() == "gridCol").map(|t| crate::xml::attr(&x[t.start..t.end], "w:w").and_then(|v| v.parse().ok()).unwrap_or(0)).collect(),
            ),
            _ => None,
        }).unwrap_or_default()
    }

    fn set_grid(&mut self, cols: &[i64]) {
        let xml = format!("<w:tblGrid>{}</w:tblGrid>", cols.iter().map(|w| format!(r#"<w:gridCol w:w="{w}"/>"#)).collect::<String>());
        match self.items.iter_mut().find(|i| matches!(i, Item::Raw(x) if x.trim_start().starts_with("<w:tblGrid"))) {
            Some(i) => *i = Item::Raw(xml),
            None => {
                let at = self.items.iter().position(|i| matches!(i, Item::Node(_))).unwrap_or(self.items.len());
                self.items.insert(at, Item::Raw(xml));
            }
        }
    }

    /// Grid column where cell `c` of row `r` starts.
    pub fn grid_col(&self, r: usize, c: usize) -> Option<usize> {
        let row = self.rows().nth(r)?;
        Some(row.cells().take(c).map(Cell::span).sum())
    }

    /// Insert a column left or right of cell (`r`, `c`): a new grid column and, in every row,
    /// an empty cell next to the cell covering that grid column.
    pub fn insert_col(&mut self, r: usize, c: usize, right: bool) -> bool {
        let Some(g0) = self.grid_col(r, c) else { return false };
        let span = self.rows().nth(r).and_then(|row| row.cells().nth(c)).map(Cell::span).unwrap_or(1);
        let at_grid = if right { g0 + span } else { g0 };
        let mut grid = self.grid();
        let width = grid.get(g0).copied().unwrap_or(1440).max(360);
        if grid.len() >= at_grid { grid.insert(at_grid, width); } else { grid.push(width); }
        for item in self.items.iter_mut() {
            let Item::Node(row) = item else { continue };
            // Find the cell index whose grid range touches `at_grid` from the left/right.
            let mut g = 0;
            let mut insert_at = None;
            let mut template = None;
            for (ci, cell) in row.cells().enumerate() {
                let s = cell.span();
                if right && g + s == at_grid || !right && g == at_grid {
                    insert_at = Some(if right { ci + 1 } else { ci });
                    template = Some(cell.clone());
                    break;
                }
                if g < at_grid && at_grid < g + s {
                    // Inside a merged cell: widen it instead.
                    insert_at = None;
                    template = None;
                    break;
                }
                g += s;
            }
            match (insert_at, template) {
                (Some(ci), Some(mut cell)) => {
                    cell.set_text("");
                    cell.set_span(1);
                    cell.set_width(width);
                    if let Some(tc) = cell.tc_pr.take() {
                        let mut p = super::props::Props::parse(&tc);
                        p.set("vMerge", None);
                        cell.tc_pr = if p.is_empty() { None } else { Some(p.to_xml()) };
                    }
                    row.insert_cell(ci, cell);
                }
                _ => {
                    if let Some(cell) = row.cell_covering(at_grid.saturating_sub(1)) {
                        let s = cell.span();
                        cell.set_span(s + 1);
                    }
                }
            }
        }
        self.set_grid(&grid);
        true
    }

    /// Delete the grid column(s) under cell (`r`, `c`).
    pub fn delete_col(&mut self, r: usize, c: usize) -> bool {
        let Some(g0) = self.grid_col(r, c) else { return false };
        let span = self.rows().nth(r).and_then(|row| row.cells().nth(c)).map(Cell::span).unwrap_or(1);
        let mut grid = self.grid();
        if grid.len() <= span {
            return false; // that would delete the whole table
        }
        for item in self.items.iter_mut() {
            let Item::Node(row) = item else { continue };
            for _ in 0..span {
                row.remove_grid_col(g0);
            }
        }
        grid.drain(g0..(g0 + span).min(grid.len()));
        self.set_grid(&grid);
        true
    }

    /// Merge cell (`r`, `c`) with the cell to its right: spans add up, contents join.
    pub fn merge_right(&mut self, r: usize, c: usize) -> bool {
        let Some(row) = self.row_mut(r) else { return false };
        let cells: Vec<usize> = row.items.iter().enumerate().filter(|(_, i)| matches!(i, Item::Node(_))).map(|(p, _)| p).collect();
        let (Some(&a), Some(&b)) = (cells.get(c), cells.get(c + 1)) else { return false };
        let Item::Node(right) = row.items.remove(b) else { return false };
        let Item::Node(left) = &mut row.items[a] else { return false };
        let span = left.span() + right.span();
        left.set_span(span);
        let w = left.width().unwrap_or(0) + right.width().unwrap_or(0);
        if w > 0 { left.set_width(w); }
        let keep: Vec<String> = right.blocks.into_iter().filter(|b| Para::parse(b).map(|p| !p.is_empty()).unwrap_or(true)).collect();
        if !keep.is_empty() {
            // Drop a lone empty paragraph on the left before appending.
            if left.blocks.len() == 1 && Para::parse(&left.blocks[0]).map(|p| p.is_empty()).unwrap_or(false) {
                left.blocks.clear();
            }
            left.blocks.extend(keep);
        }
        true
    }

    /// Fill cell (`r`, `c`) with a background color (hex without '#'), or clear it.
    pub fn shade(&mut self, r: usize, c: usize, color: Option<&str>) -> bool {
        let Some(cell) = self.cell_mut(r, c) else { return false };
        let mut p = cell.tc_pr.as_deref().map(Props::parse).unwrap_or_else(|| Props::new("w:tcPr"));
        p.set("shd", color.map(|c| format!(r#"<w:shd w:val="clear" w:color="auto" w:fill="{}"/>"#, crate::xml::escape(c))));
        cell.tc_pr = if p.is_empty() { None } else { Some(p.to_xml()) };
        true
    }

    pub fn delete_row(&mut self, r: usize) -> bool {
        match self.row_position(r) {
            Some(pos) if self.rows().count() > 1 => {
                self.items.remove(pos);
                true
            }
            _ => false,
        }
    }

    fn row_position(&self, r: usize) -> Option<usize> {
        self.items.iter().enumerate().filter(|(_, i)| matches!(i, Item::Node(_))).nth(r).map(|(p, _)| p)
    }

    pub fn to_xml(&self) -> String {
        let mut s = self.open.clone();
        for item in &self.items {
            match item {
                Item::Raw(x) => s.push_str(x),
                Item::Node(row) => s.push_str(&row.to_xml()),
            }
        }
        s.push_str(&close_of(&self.open));
        s
    }
}

impl Row {
    fn insert_cell(&mut self, c: usize, cell: Cell) {
        let positions: Vec<usize> = self.items.iter().enumerate().filter(|(_, i)| matches!(i, Item::Node(_))).map(|(p, _)| p).collect();
        let at = positions.get(c).copied().unwrap_or_else(|| positions.last().map(|p| p + 1).unwrap_or(self.items.len()));
        self.items.insert(at, Item::Node(cell));
    }

    /// The cell covering grid column `g`.
    fn cell_covering(&mut self, g: usize) -> Option<&mut Cell> {
        let mut pos = 0;
        for item in self.items.iter_mut() {
            if let Item::Node(cell) = item {
                let s = cell.span();
                if pos <= g && g < pos + s {
                    return Some(cell);
                }
                pos += s;
            }
        }
        None
    }

    /// Remove one grid column: delete the cell there, or narrow a merged cell covering it.
    fn remove_grid_col(&mut self, g: usize) {
        let mut pos = 0;
        for i in 0..self.items.len() {
            if let Item::Node(cell) = &mut self.items[i] {
                let s = cell.span();
                if pos <= g && g < pos + s {
                    if s > 1 {
                        cell.set_span(s - 1);
                    } else if self.cells().count() > 1 {
                        self.items.remove(i);
                    }
                    return;
                }
                pos += s;
            }
        }
    }

    pub fn cells(&self) -> impl Iterator<Item = &Cell> {
        self.items.iter().filter_map(|i| match i { Item::Node(c) => Some(c), _ => None })
    }

    pub fn cell_mut(&mut self, c: usize) -> Option<&mut Cell> {
        self.items.iter_mut().filter_map(|i| match i { Item::Node(c) => Some(c), _ => None }).nth(c)
    }

    fn to_xml(&self) -> String {
        let mut s = self.open.clone();
        for item in &self.items {
            match item {
                Item::Raw(x) => s.push_str(x),
                Item::Node(cell) => s.push_str(&cell.to_xml()),
            }
        }
        s.push_str(&close_of(&self.open));
        s
    }
}

impl Cell {
    /// Grid columns this cell spans (`w:gridSpan`).
    pub fn span(&self) -> usize {
        self.tc_pr.as_deref().map(Props::parse).and_then(|p| p.get("gridSpan").and_then(|x| crate::xml::attr(x, "w:val")).and_then(|v| v.parse().ok())).unwrap_or(1).max(1)
    }

    pub fn set_span(&mut self, n: usize) {
        let mut p = self.tc_pr.as_deref().map(Props::parse).unwrap_or_else(|| Props::new("w:tcPr"));
        p.set("gridSpan", (n > 1).then(|| format!(r#"<w:gridSpan w:val="{n}"/>"#)));
        self.tc_pr = if p.is_empty() { None } else { Some(p.to_xml()) };
    }

    pub fn width(&self) -> Option<i64> {
        self.tc_pr.as_deref().map(Props::parse).and_then(|p| p.get("tcW").and_then(|x| crate::xml::attr(x, "w:w")).and_then(|v| v.parse().ok()))
    }

    pub fn set_width(&mut self, w: i64) {
        let mut p = self.tc_pr.as_deref().map(Props::parse).unwrap_or_else(|| Props::new("w:tcPr"));
        p.set("tcW", Some(format!(r#"<w:tcW w:w="{w}" w:type="dxa"/>"#)));
        self.tc_pr = Some(p.to_xml());
    }

    /// Replace the cell's contents with one paragraph of `text`, keeping the first
    /// paragraph's properties and its first run's formatting.
    pub fn set_text(&mut self, text: &str) {
        let first = self.blocks.iter().find(|b| b.trim_start().starts_with("<w:p")).and_then(|b| Para::parse(b).ok());
        let (ppr, rpr) = match &first {
            Some(p) => {
                let mut ppr = p.ppr.clone();
                if let Some(pp) = ppr.as_mut() {
                    pp.set("pPrChange", None);
                    pp.set("sectPr", None);
                }
                let rpr = p.segs.iter().find_map(|s| match &s.kind {
                    SegKind::Run(r) => Some(r.rpr.clone()),
                    _ => None,
                }).flatten().map(|mut r: Props| {
                    r.set("rPrChange", None);
                    r.set("ins", None);
                    r.set("del", None);
                    r
                });
                (ppr, rpr)
            }
            None => (None, None),
        };
        let mut para = Para::new(text, ppr, rpr);
        if let Some(p) = &first {
            para.open = strip_ids(&p.open);
        }
        self.blocks = vec![para.to_xml()];
    }

    /// Paragraphs of this cell (block-level `w:p` only).
    pub fn paragraphs(&self) -> Vec<Para> {
        self.blocks.iter().filter_map(|b| Para::parse(b).ok()).collect()
    }

    fn to_xml(&self) -> String {
        let mut s = self.open.clone();
        if let Some(p) = &self.tc_pr {
            s.push_str(p);
        }
        for b in &self.blocks {
            s.push_str(b);
        }
        if self.blocks.is_empty() {
            s.push_str("<w:p/>"); // a cell must end with a paragraph
        }
        s.push_str(&close_of(&self.open));
        s
    }
}

fn strip_ids(open: &str) -> String {
    let mut s = open.to_string();
    for a in ["w14:paraId", "w14:textId"] {
        s = crate::xml::set_attr(&s, a, None);
    }
    s
}
