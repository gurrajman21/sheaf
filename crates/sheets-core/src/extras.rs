//! Features beyond IronCalc's model: charts (read from and written to the
//! package), data-validation dropdowns, templates, remove-duplicates and the
//! format painter.

use ironcalc_base::cell::CellValue;
use ooxml::charts::{Anchor, ChartEdits, ChartInfo, NewChart, NewSeries, EMU_PER_PX, PALETTE};
use serde::Serialize;
use serde_json::{json, Value};

use crate::{Rect, Session};

/// A chart as the session tracks it: from the file, or added in this session.
#[derive(Clone, Debug)]
pub struct ChartState {
    pub id: String,
    pub sheet: String,
    pub kind: String,
    pub grouping: Option<String>,
    pub title: Option<String>,
    pub legend: bool,
    /// (name, name_ref, cat_ref, val_ref, color)
    pub series: Vec<(Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)>,
    pub anchor: Option<Anchor>,
    /// (drawing part, index) for charts that came from the file
    pub origin: Option<(String, usize)>,
    pub moved: bool,
    pub deleted: bool,
}

impl ChartState {
    pub fn from_file(c: ChartInfo) -> Self {
        ChartState {
            id: format!("{}#{}", c.drawing, c.index),
            sheet: c.sheet,
            kind: c.kind,
            grouping: c.grouping,
            title: c.title,
            legend: c.legend,
            series: c.series.into_iter().map(|s| (s.name, s.name_ref, s.cat_ref, s.val_ref, s.color)).collect(),
            anchor: c.anchor,
            origin: Some((c.drawing, c.index)),
            moved: false,
            deleted: false,
        }
    }
}

#[derive(Serialize)]
struct Values {
    labels: Vec<String>,
    numbers: Vec<Option<f64>>,
}

/// Split `Sheet!$A$1:$B$2` into (sheet name, rest), handling quoted names.
fn split_sheet(r: &str) -> (Option<String>, &str) {
    let r = r.trim().trim_start_matches('=');
    match r.rfind('!') {
        Some(i) => {
            let s = r[..i].trim();
            let s = s.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')).map(|x| x.replace("''", "'")).unwrap_or_else(|| s.to_string());
            (Some(s), &r[i + 1..])
        }
        None => (None, r),
    }
}

pub fn quote_sheet(name: &str) -> String {
    if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        name.to_string()
    } else {
        format!("'{}'", name.replace('\'', "''"))
    }
}

impl Session {
    pub(crate) fn load_file_meta(&mut self, bytes: &[u8]) {
        self.objects = ooxml::inspect::sheet_objects(bytes).unwrap_or_default();
        self.validations = ooxml::inspect::validations(bytes).unwrap_or_default();
        self.charts = ooxml::charts::read_charts(bytes).unwrap_or_default().into_iter().map(ChartState::from_file).collect();
    }

    fn sheet_index(&self, name: &str) -> Option<u32> {
        self.model.get_worksheets_properties().iter().position(|p| p.name == name).map(|i| i as u32)
    }

    /// Resolve a reference (or a defined name) to an area: (sheet, r0, c0, r1, c1).
    fn resolve_area(&self, reference: &str, default_sheet: u32) -> Option<(u32, i32, i32, i32, i32)> {
        let (sheet, rest) = split_sheet(reference);
        if sheet.is_none() && !rest.contains(':') && ooxml::sheet::parse_ref(rest).is_none() {
            // a defined name
            let (_, _, f) = self.model.get_defined_name_list().into_iter().find(|(n, _, _)| n.eq_ignore_ascii_case(rest))?;
            return self.resolve_area(&f, default_sheet);
        }
        let s = match sheet {
            Some(n) => self.sheet_index(&n)?,
            None => default_sheet,
        };
        let (a, b) = rest.split_once(':').unwrap_or((rest, rest));
        let (r0, c0) = ooxml::sheet::parse_ref(a)?;
        let (r1, c1) = ooxml::sheet::parse_ref(b)?;
        Some((s, r0.min(r1), c0.min(c1), r0.max(r1), c0.max(c1)))
    }

    fn values(&self, reference: &str, default_sheet: u32) -> Values {
        let mut out = Values { labels: vec![], numbers: vec![] };
        let Some((s, r0, c0, r1, c1)) = self.resolve_area(reference, default_sheet) else { return out };
        let model = self.model.get_model();
        for r in r0..=r1.min(r0 + 5000) {
            for c in c0..=c1.min(c0 + 200) {
                out.labels.push(self.model.get_formatted_cell_value(s, r, c).unwrap_or_default());
                out.numbers.push(match model.get_cell_value_by_index(s, r, c) {
                    Ok(CellValue::Number(n)) => Some(n),
                    _ => None,
                });
            }
        }
        out
    }

    /// Charts on a sheet, with their data resolved from the current cells.
    pub(crate) fn charts_json(&self, sheet: u32) -> Value {
        let name = self.model.get_worksheets_properties().get(sheet as usize).map(|p| p.name.clone()).unwrap_or_default();
        let list: Vec<Value> = self
            .charts
            .iter()
            .filter(|c| c.sheet == name && !c.deleted)
            .map(|c| {
                let series: Vec<Value> = c
                    .series
                    .iter()
                    .enumerate()
                    .map(|(i, (n, nref, cref, vref, color))| {
                        let label = nref.as_ref().and_then(|r| self.values(r, sheet).labels.into_iter().next()).or(n.clone()).unwrap_or(format!("Series {}", i + 1));
                        let vals = vref.as_ref().map(|r| self.values(r, sheet)).unwrap_or(Values { labels: vec![], numbers: vec![] });
                        let cats = cref.as_ref().map(|r| self.values(r, sheet));
                        json!({
                            "name": label,
                            "color": color.clone().map(|c| format!("#{c}")),
                            "values": vals.numbers,
                            "cats": cats.as_ref().map(|v| v.labels.clone()).unwrap_or_default(),
                            "xs": cats.map(|v| v.numbers).unwrap_or_default(),
                        })
                    })
                    .collect();
                let a = c.anchor;
                json!({
                    "id": c.id, "kind": c.kind, "grouping": c.grouping, "title": c.title, "legend": c.legend, "series": series,
                    "movable": a.is_some(),
                    "anchor": a.map(|a| json!({
                        "from_col": a.from_col, "from_row": a.from_row, "from_dx": a.from_dx / EMU_PER_PX, "from_dy": a.from_dy / EMU_PER_PX,
                        "to_col": a.to_col, "to_row": a.to_row, "to_dx": a.to_dx / EMU_PER_PX, "to_dy": a.to_dy / EMU_PER_PX,
                    })),
                })
            })
            .collect();
        json!(list)
    }

    /// New chart from a selected range: first row = series names if it's text,
    /// first column = categories if it's text; one series per remaining column.
    pub(crate) fn chart_add(&mut self, rect: &Rect, kind: &str) -> Result<Value, String> {
        let s = rect.sheet;
        let sheet_name = self.model.get_worksheets_properties().get(s as usize).ok_or("no such sheet")?.name.clone();
        let q = quote_sheet(&sheet_name);
        let model = self.model.get_model();
        let is_num = |r: i32, c: i32| matches!(model.get_cell_value_by_index(s, r, c), Ok(CellValue::Number(_)));
        let (r0, c0, r1, c1) = (rect.row, rect.col, rect.row + rect.height.max(1) - 1, rect.col + rect.width.max(1) - 1);
        let header = rect.height > 1 && (c0..=c1).any(|c| !is_num(r0, c)) && (c0..=c1).any(|c| is_num(r0 + 1, c) || !is_num(r0, c));
        let label_col = rect.width > 1 && (r0 + header as i32..=r1).all(|r| !is_num(r, c0));
        let dr0 = r0 + header as i32;
        if dr0 > r1 {
            return Err("Select the data to chart, including at least one row of numbers".into());
        }
        let cat_ref = label_col.then(|| format!("{q}!${}${dr0}:${}${r1}", col_letters(c0), col_letters(c0)));
        let cats: Vec<String> = match &cat_ref {
            Some(r) => self.values(r, s).labels,
            None => (dr0..=r1).map(|r| (r - dr0 + 1).to_string()).collect(),
        };
        let mut series = Vec::new();
        for c in (c0 + label_col as i32)..=c1 {
            let val_ref = format!("{q}!${}${dr0}:${}${r1}", col_letters(c), col_letters(c));
            let raw = self.values(&val_ref, s).numbers;
            if raw.iter().all(Option::is_none) {
                continue; // a column with no numbers isn't a series
            }
            let values: Vec<f64> = raw.into_iter().map(|v| v.unwrap_or(0.0)).collect();
            let name_ref = header.then(|| format!("{q}!${}${r0}", col_letters(c)));
            let name = match &name_ref {
                Some(r) => self.values(r, s).labels.into_iter().next().unwrap_or_default(),
                None => format!("Series {}", series.len() + 1),
            };
            series.push(NewSeries { name, name_ref, cat_ref: cat_ref.clone(), val_ref, cats: cats.clone(), values });
        }
        if series.is_empty() {
            return Err("There are no numbers to chart in the selection. Select a range that includes a column of numbers.".into());
        }
        // a pie shows one series, so it's titled by that series
        let title = if series.len() == 1 || kind == "pie" || kind == "doughnut" { Some(series[0].name.clone()) } else { None };
        if kind == "pie" || kind == "doughnut" {
            series.truncate(1);
        }
        // place it right of everything on the sheet so it never covers data, about 6 x 14 cells
        let last_col = self.model.get_model().workbook.worksheet(s).map(|w| w.dimension().max_column).unwrap_or(c1).max(c1);
        let anchor = Anchor { from_col: last_col, from_row: r0 - 1, from_dx: 16 * EMU_PER_PX, from_dy: 0, to_col: last_col + 6, to_row: r0 + 13, to_dx: 0, to_dy: 0 };
        self.next_chart += 1;
        let id = format!("new#{}", self.next_chart);
        self.charts.push(ChartState {
            id: id.clone(),
            sheet: sheet_name,
            kind: kind.to_string(),
            grouping: None,
            title: title.clone(),
            legend: true,
            series: series.iter().enumerate().map(|(i, x)| (Some(x.name.clone()), x.name_ref.clone(), x.cat_ref.clone(), Some(x.val_ref.clone()), Some(PALETTE[i % PALETTE.len()].to_string()))).collect(),
            anchor: Some(anchor),
            origin: None,
            moved: false,
            deleted: false,
        });
        self.new_charts.push((id.clone(), NewChart { sheet: self.charts.last().unwrap().sheet.clone(), kind: kind.into(), title, series, anchor }));
        self.dirty = true;
        Ok(json!(id))
    }

    pub(crate) fn chart_move(&mut self, id: &str, a: &Value) -> Result<(), String> {
        let n = |k: &str| a.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        let anchor = Anchor {
            from_col: n("from_col") as i32, from_row: n("from_row") as i32,
            from_dx: (n("from_dx") * EMU_PER_PX as f64) as i64, from_dy: (n("from_dy") * EMU_PER_PX as f64) as i64,
            to_col: n("to_col") as i32, to_row: n("to_row") as i32,
            to_dx: (n("to_dx") * EMU_PER_PX as f64) as i64, to_dy: (n("to_dy") * EMU_PER_PX as f64) as i64,
        };
        let c = self.charts.iter_mut().find(|c| c.id == id).ok_or("no such chart")?;
        if c.anchor.is_none() {
            return Err("This chart can't be moved yet".into());
        }
        c.anchor = Some(anchor);
        c.moved = true;
        if let Some((_, nc)) = self.new_charts.iter_mut().find(|(i, _)| i == id) {
            nc.anchor = anchor;
        }
        self.dirty = true;
        Ok(())
    }

    pub(crate) fn chart_delete(&mut self, id: &str) -> Result<(), String> {
        let c = self.charts.iter_mut().find(|c| c.id == id).ok_or("no such chart")?;
        c.deleted = true;
        self.new_charts.retain(|(i, _)| i != id);
        self.dirty = true;
        Ok(())
    }

    /// Chart edits to write on save. `with_file_charts` is false for a full rewrite,
    /// where the file's own charts are gone anyway.
    pub(crate) fn chart_edits(&self, with_file_charts: bool) -> ChartEdits {
        let mut e = ChartEdits::default();
        let sheets: Vec<String> = self.model.get_worksheets_properties().into_iter().map(|p| p.name).collect();
        if with_file_charts {
            // a deleted sheet's drawing goes with it
            for c in self.charts.iter().filter(|c| sheets.contains(&c.sheet)) {
                let Some((d, i)) = &c.origin else { continue };
                if c.deleted {
                    e.deletes.push((d.clone(), *i));
                } else if c.moved {
                    if let Some(a) = c.anchor {
                        e.moves.push((d.clone(), *i, a));
                    }
                }
            }
        }
        e.adds = self.new_charts.iter().filter(|(_, c)| sheets.contains(&c.sheet)).map(|(_, c)| c.clone()).collect();
        e
    }

    /// Keep chart positions in step with row/column inserts and deletes.
    pub(crate) fn shift_charts(&mut self, sheet: u32, rows: bool, at: i32, count: i32) {
        let name = self.model.get_worksheets_properties().get(sheet as usize).map(|p| p.name.clone()).unwrap_or_default();
        let at0 = at - 1; // anchors are 0-based
        let shift = |v: &mut i32| {
            if count > 0 && *v >= at0 {
                *v += count;
            } else if count < 0 && *v >= at0 {
                *v = (*v + count).max(at0);
            }
        };
        for c in self.charts.iter_mut().filter(|c| c.sheet == name) {
            if let Some(a) = c.anchor.as_mut() {
                if rows { shift(&mut a.from_row); shift(&mut a.to_row); } else { shift(&mut a.from_col); shift(&mut a.to_col); }
            }
        }
        for (_, nc) in self.new_charts.iter_mut().filter(|(_, c)| c.sheet == name) {
            let a = &mut nc.anchor;
            if rows { shift(&mut a.from_row); shift(&mut a.to_row); } else { shift(&mut a.from_col); shift(&mut a.to_col); }
        }
    }

    /// Items of a list validation that covers the cell, if any.
    pub(crate) fn validation_list(&self, rect: &Rect) -> Value {
        let name = self.model.get_worksheets_properties().get(rect.sheet as usize).map(|p| p.name.clone()).unwrap_or_default();
        let Some((_, rules)) = self.validations.iter().find(|(n, _)| *n == name) else { return Value::Null };
        for v in rules.iter().filter(|v| v.kind == "list") {
            let covers = v.sqref.split_whitespace().any(|part| {
                let (a, b) = part.split_once(':').unwrap_or((part, part));
                match (ooxml::sheet::parse_ref(a), ooxml::sheet::parse_ref(b)) {
                    (Some((r0, c0)), Some((r1, c1))) => rect.row >= r0.min(r1) && rect.row <= r0.max(r1) && rect.col >= c0.min(c1) && rect.col <= c0.max(c1),
                    _ => false,
                }
            });
            if !covers {
                continue;
            }
            let f = v.formula1.trim();
            let items: Vec<String> = if let Some(list) = f.strip_prefix('"').and_then(|x| x.strip_suffix('"')) {
                list.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
            } else {
                self.values(f, rect.sheet).labels.into_iter().filter(|s| !s.is_empty()).collect()
            };
            return json!({ "items": items });
        }
        Value::Null
    }

    /// Remove rows of a range that repeat an earlier row; returns how many went.
    pub(crate) fn dedupe(&mut self, rect: &Rect) -> Result<usize, String> {
        let s = rect.sheet;
        let key = |me: &Self, r: i32| -> String {
            (rect.col..rect.col + rect.width).map(|c| me.model.get_formatted_cell_value(s, r, c).unwrap_or_default()).collect::<Vec<_>>().join("\u{1f}")
        };
        let mut seen = std::collections::HashSet::new();
        let mut keep = Vec::new();
        for r in rect.row..rect.row + rect.height {
            if seen.insert(key(self, r)) {
                keep.push(r);
            }
        }
        let removed = rect.height as usize - keep.len();
        if removed == 0 {
            return Ok(0);
        }
        self.reorder_rows(rect, &keep)?;
        let tail = Rect { sheet: s, row: rect.row + keep.len() as i32, col: rect.col, width: rect.width, height: removed as i32 };
        self.model.range_clear_all(&tail.area())?;
        Ok(removed)
    }

    /// Format painter: copy the styles of `source` onto `target` (tiled like Excel).
    pub(crate) fn paste_formats(&mut self, source: &Rect, target: &Rect) -> Result<(), String> {
        let mut styles = Vec::new();
        for r in source.row..source.row + source.height.max(1) {
            let mut row = Vec::new();
            for c in source.col..source.col + source.width.max(1) {
                row.push(self.model.get_model().get_style_for_cell(source.sheet, r, c)?);
            }
            styles.push(row);
        }
        self.select_range(target)?;
        self.model.on_paste_styles(&styles)
    }

    pub(crate) fn hide_rows_list(&mut self, sheet: u32, rows: &mut [i32], hidden: bool) -> Result<(), String> {
        rows.sort_unstable();
        let mut i = 0;
        while i < rows.len() {
            let start = rows[i];
            let mut end = start;
            while i + 1 < rows.len() && rows[i + 1] == end + 1 {
                i += 1;
                end = rows[i];
            }
            self.model.set_rows_hidden(sheet, start, end, hidden)?;
            i += 1;
        }
        Ok(())
    }

    /// Fill a new workbook with a starter template.
    pub(crate) fn apply_template(&mut self, name: &str) -> Result<(), String> {
        let m = &mut self.model;
        let set = |m: &mut ironcalc_base::UserModel, r: i32, c: i32, v: &str| m.set_user_input(0, r, c, v);
        let style = |m: &mut ironcalc_base::UserModel, r: i32, c: i32, w: i32, h: i32, path: &str, v: &str| {
            m.update_range_style(&ironcalc_base::expressions::types::Area { sheet: 0, row: r, column: c, width: w, height: h }, path, v)
        };
        let header = |m: &mut ironcalc_base::UserModel, r: i32, c0: i32, w: i32| -> Result<(), String> {
            style(m, r, c0, w, 1, "font.b", "true")?;
            style(m, r, c0, w, 1, "fill.color", "#CCF1EA")?;
            style(m, r, c0, w, 1, "font.color", "#0A6457")
        };
        let title = |m: &mut ironcalc_base::UserModel, text: &str| -> Result<(), String> {
            m.set_user_input(0, 1, 1, text)?;
            m.set_rows_height(0, 1, 1, 34.0)?;
            style(m, 1, 1, 1, 1, "font.size", "18")?;
            style(m, 1, 1, 1, 1, "font.b", "true")
        };
        m.pause_evaluation();
        match name {
            "budget" => {
                title(m, "Monthly budget")?;
                set(m, 2, 1, "Plan against what you actually spent this month.")?;
                style(m, 2, 1, 1, 1, "font.color", "#5B6672")?;
                for (c, h) in ["Category", "Planned", "Actual", "Difference"].iter().enumerate() {
                    set(m, 4, c as i32 + 1, h)?;
                }
                header(m, 4, 1, 4)?;
                let rows = [("Rent", 1500, 1500), ("Groceries", 450, 512), ("Utilities", 180, 164), ("Transport", 120, 98), ("Dining out", 200, 260), ("Entertainment", 150, 95), ("Savings", 600, 600)];
                for (i, (cat, p, a)) in rows.iter().enumerate() {
                    let r = 5 + i as i32;
                    set(m, r, 1, cat)?;
                    set(m, r, 2, &p.to_string())?;
                    set(m, r, 3, &a.to_string())?;
                    set(m, r, 4, &format!("=B{r}-C{r}"))?;
                }
                let t = 5 + rows.len() as i32;
                set(m, t, 1, "Total")?;
                for (c, col) in [(2, "B"), (3, "C"), (4, "D")] {
                    set(m, t, c, &format!("=SUM({col}5:{col}{})", t - 1))?;
                }
                style(m, t, 1, 4, 1, "font.b", "true")?;
                style(m, 5, 2, 3, rows.len() as i32 + 1, "num_fmt", "$#,##0")?;
                m.set_columns_width(0, 1, 1, 180.0)?;
                m.set_columns_width(0, 2, 4, 110.0)?;
            }
            "invoice" => {
                title(m, "Invoice")?;
                set(m, 3, 1, "Bill to")?;
                set(m, 4, 1, "Customer name")?;
                set(m, 5, 1, "Street address, city")?;
                set(m, 3, 4, "Invoice no.")?;
                set(m, 3, 5, "1001")?;
                set(m, 4, 4, "Date")?;
                set(m, 4, 5, "=TODAY()")?;
                style(m, 4, 5, 1, 1, "num_fmt", "yyyy-mm-dd")?;
                style(m, 3, 1, 1, 1, "font.b", "true")?;
                for (c, h) in ["Description", "Qty", "Unit price", "", "Amount"].iter().enumerate() {
                    set(m, 7, c as i32 + 1, h)?;
                }
                header(m, 7, 1, 5)?;
                let items = [("Design work", 12, 85), ("Development", 30, 95), ("Hosting (12 months)", 1, 240)];
                for (i, (d, q, p)) in items.iter().enumerate() {
                    let r = 8 + i as i32;
                    set(m, r, 1, d)?;
                    set(m, r, 2, &q.to_string())?;
                    set(m, r, 3, &p.to_string())?;
                    set(m, r, 5, &format!("=B{r}*C{r}"))?;
                }
                set(m, 13, 4, "Subtotal")?;
                set(m, 13, 5, "=SUM(E8:E12)")?;
                set(m, 14, 4, "Tax (8%)")?;
                set(m, 14, 5, "=E13*0.08")?;
                set(m, 15, 4, "Total")?;
                set(m, 15, 5, "=E13+E14")?;
                style(m, 15, 4, 2, 1, "font.b", "true")?;
                style(m, 8, 3, 1, 5, "num_fmt", "$#,##0.00")?;
                style(m, 8, 5, 1, 8, "num_fmt", "$#,##0.00")?;
                m.set_columns_width(0, 1, 1, 220.0)?;
                m.set_columns_width(0, 4, 5, 110.0)?;
            }
            "sales" => {
                title(m, "Sales tracker")?;
                for (c, h) in ["Month", "Region", "Units", "Price", "Revenue"].iter().enumerate() {
                    set(m, 3, c as i32 + 1, h)?;
                }
                header(m, 3, 1, 5)?;
                let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
                let regions = ["North", "South", "East", "West"];
                for (i, mo) in months.iter().enumerate() {
                    let r = 4 + i as i32;
                    set(m, r, 1, mo)?;
                    set(m, r, 2, regions[i % 4])?;
                    set(m, r, 3, &(120 + (i * 37) % 90).to_string())?;
                    set(m, r, 4, "24.5")?;
                    set(m, r, 5, &format!("=C{r}*D{r}"))?;
                }
                set(m, 16, 1, "Total")?;
                set(m, 16, 3, "=SUM(C4:C15)")?;
                set(m, 16, 5, "=SUM(E4:E15)")?;
                style(m, 16, 1, 5, 1, "font.b", "true")?;
                style(m, 4, 4, 2, 13, "num_fmt", "$#,##0.00")?;
                m.set_columns_width(0, 1, 5, 110.0)?;
            }
            "todo" => {
                title(m, "To-do list")?;
                for (c, h) in ["Task", "Owner", "Due", "Status"].iter().enumerate() {
                    set(m, 3, c as i32 + 1, h)?;
                }
                header(m, 3, 1, 4)?;
                let tasks = [("Draft the proposal", "Sam", "2026-10-20", "In progress"), ("Review budget", "Alex", "2026-10-22", "Not started"), ("Book venue", "Priya", "2026-10-25", "Done"), ("Send invites", "Sam", "2026-10-28", "Not started")];
                for (i, (t, o, d, st)) in tasks.iter().enumerate() {
                    let r = 4 + i as i32;
                    set(m, r, 1, t)?;
                    set(m, r, 2, o)?;
                    set(m, r, 3, d)?;
                    set(m, r, 4, st)?;
                }
                style(m, 4, 3, 1, 10, "num_fmt", "yyyy-mm-dd")?;
                m.set_columns_width(0, 1, 1, 240.0)?;
                m.set_columns_width(0, 2, 4, 120.0)?;
            }
            other => {
                m.resume_evaluation();
                return Err(format!("unknown template {other}"));
            }
        }
        m.resume_evaluation();
        m.evaluate();
        Ok(())
    }
}

fn col_letters(c: i32) -> String {
    ooxml::sheet::col_name(c)
}
