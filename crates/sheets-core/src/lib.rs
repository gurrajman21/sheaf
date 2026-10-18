//! The spreadsheet app's backend: one open workbook (an IronCalc `UserModel`)
//! plus everything the UI asks of it, behind a single JSON `dispatch`.
//!
//! Both the Tauri app and the dev HTTP bridge call `dispatch`, so the desktop
//! app and the browser test setup run exactly the same code.
//!
//! Saving uses the patch-in-place writer (`ooxml::save_patched`) whenever the
//! workbook came from an .xlsx, so charts, pivots, comments, macros etc. in the
//! original survive. If the patch writer refuses an edit (a structural change it
//! can't express yet), the UI is asked to confirm a lossy save instead.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use ironcalc_base::expressions::types::Area;
use ironcalc_base::types::{Color, HorizontalAlignment, VerticalAlignment};
use ironcalc_base::{Model, UserModel};
use ooxml::structural::StructuralOp;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const LANG: &str = "en";
const MIN_ROWS: i32 = 200;
const MIN_COLS: i32 = 30;

pub struct Session {
    model: UserModel<'static>,
    /// Bytes of the file as last opened/saved: the base for patch saves.
    original: Option<Vec<u8>>,
    path: Option<PathBuf>,
    dirty: bool,
    sheet: u32,
    /// Per-sheet objects found in the file at open/save time.
    objects: Vec<(String, ooxml::inspect::SheetObjects)>,
    /// Edits since the last open/save that the patch writer can't express
    /// (sheet add/rename/delete). Saving after one needs a confirmed full rewrite.
    structural: Vec<&'static str>,
    /// Row/column inserts and deletes since the last open/save, in order. The patch
    /// writer replays them on its baseline and shifts every reference in the file.
    ops: Vec<StructuralOp>,
    /// Any row/column/sheet operation since the file was opened.
    structural_ever: bool,
    /// Undo/redo after a structural edit: the op log may no longer match the
    /// model (a moved block of cells looks like plain edits to the diff), so the
    /// next save must not be a patch.
    ops_uncertain: bool,
}

#[derive(Serialize)]
pub struct SheetInfo {
    name: String,
    state: String,
    color: Option<String>,
    /// Charts, images etc. from the file that are kept on save but not drawn.
    #[serde(skip_serializing_if = "Option::is_none")]
    objects: Option<ooxml::inspect::SheetObjects>,
}

#[derive(Serialize)]
pub struct WorkbookInfo {
    name: String,
    path: Option<String>,
    dirty: bool,
    sheet: u32,
    sheets: Vec<SheetInfo>,
    can_undo: bool,
    can_redo: bool,
}

#[derive(Serialize)]
pub struct Layout {
    rows: i32,
    cols: i32,
    row_heights: Vec<f64>,
    col_widths: Vec<f64>,
    frozen_rows: i32,
    frozen_cols: i32,
    merged: Vec<[i32; 4]>, // row, col, width, height
    show_grid: bool,
}

#[derive(Serialize, Default)]
pub struct CellView {
    r: i32,
    c: i32,
    text: String,
    #[serde(skip_serializing_if = "is_false")]
    b: bool,
    #[serde(skip_serializing_if = "is_false")]
    i: bool,
    #[serde(skip_serializing_if = "is_false")]
    u: bool,
    #[serde(skip_serializing_if = "is_false")]
    s: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fill: Option<String>,
    size: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    font: Option<String>,
    /// left | center | right (resolved: numbers default right, text left)
    align: &'static str,
    valign: &'static str,
    #[serde(skip_serializing_if = "is_false")]
    wrap: bool,
    /// border edges present: top, right, bottom, left
    #[serde(skip_serializing_if = "Option::is_none")]
    borders: Option<[bool; 4]>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Deserialize)]
struct Rect {
    sheet: u32,
    row: i32,
    col: i32,
    #[serde(default = "one")]
    width: i32,
    #[serde(default = "one")]
    height: i32,
}

fn one() -> i32 {
    1
}

impl Rect {
    fn area(&self) -> Area {
        Area { sheet: self.sheet, row: self.row, column: self.col, width: self.width.max(1), height: self.height.max(1) }
    }
}

fn arg<T: for<'de> Deserialize<'de>>(args: &Value, key: &str) -> Result<T, String> {
    serde_json::from_value(args.get(key).cloned().unwrap_or(Value::Null)).map_err(|e| format!("argument `{key}`: {e}"))
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        let model = UserModel::new_empty("Book1", "en", "UTC", LANG).expect("empty workbook");
        Session { model, original: None, path: None, dirty: false, sheet: 0, objects: Vec::new(), structural: Vec::new(), ops: Vec::new(), structural_ever: false, ops_uncertain: false }
    }

    pub fn open(&mut self, path: &Path) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Book").to_string();
        let model = load_model(&bytes, &name)?;
        self.model = UserModel::from_model(model);
        self.objects = ooxml::inspect::sheet_objects(&bytes).unwrap_or_default();
        self.structural.clear();
        self.ops.clear();
        self.structural_ever = false;
        self.ops_uncertain = false;
        self.original = Some(bytes);
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        self.sheet = 0;
        Ok(())
    }

    /// Save to `path` (or the current path). Returns `needs_confirmation` with a
    /// reason when only a lossy save is possible and `allow_lossy` is false.
    pub fn save(&mut self, path: Option<PathBuf>, allow_lossy: bool) -> Result<Value, String> {
        let path = path.or_else(|| self.path.clone()).ok_or("no file name yet")?;
        let (bytes, mode, report) = match &self.original {
            Some(_) if self.blocked().is_some() && !allow_lossy => {
                return Ok(json!({"saved": false, "needs_confirmation": true, "reason": self.blocked()}));
            }
            Some(_) if self.blocked().is_some() => (native_bytes(self.model.get_model())?, "native", None),
            Some(orig) => {
                let mut base = load_model(orig, "base")?;
                base.evaluate();
                match ooxml::patch::save_patched_ops(orig, &base, self.model.get_model(), &self.ops) {
                    Ok((bytes, report)) => (bytes, "patch", serde_json::to_value(report).ok()),
                    Err(reason) if !allow_lossy => {
                        return Ok(json!({"saved": false, "needs_confirmation": true, "reason": reason}));
                    }
                    Err(_) => (native_bytes(self.model.get_model())?, "native", None),
                }
            }
            None => (native_bytes(self.model.get_model())?, "native", None),
        };
        // write atomically so a failed save never leaves a half-written file
        let tmp = path.with_extension("xlsx.saving");
        std::fs::write(&tmp, &bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
        // the saved file is the new baseline for the next patch save
        self.objects = ooxml::inspect::sheet_objects(&bytes).unwrap_or_default();
        self.structural.clear();
        self.ops.clear();
        self.ops_uncertain = false;
        self.original = Some(bytes);
        self.path = Some(path.clone());
        self.dirty = false;
        Ok(json!({"saved": true, "mode": mode, "path": path.display().to_string(), "report": report}))
    }

    /// Why the next save can't be a patch, if it can't.
    fn blocked(&self) -> Option<String> {
        if !self.structural.is_empty() {
            return Some(format!("{} can't be saved into the original file yet", self.structural.join(", ")));
        }
        if self.ops_uncertain {
            return Some("Undoing or redoing after inserting or deleting rows, columns or sheets can't be saved into the original file yet".into());
        }
        None
    }

    /// Insert/delete rows or columns with undo, keeping the op log and defined names in step.
    fn structural_op(&mut self, op: StructuralOp) -> Result<Value, String> {
        let (sheet, at, count) = match op {
            StructuralOp::InsertRows { sheet, at, count }
            | StructuralOp::DeleteRows { sheet, at, count }
            | StructuralOp::InsertCols { sheet, at, count }
            | StructuralOp::DeleteCols { sheet, at, count } => (sheet, at, count),
        };
        match op {
            StructuralOp::InsertRows { .. } => self.model.insert_rows(sheet, at, count)?,
            StructuralOp::DeleteRows { .. } => self.model.delete_rows(sheet, at, count)?,
            StructuralOp::InsertCols { .. } => self.model.insert_columns(sheet, at, count)?,
            StructuralOp::DeleteCols { .. } => self.model.delete_columns(sheet, at, count)?,
        }
        // IronCalc shifts cell formulas but not defined names; shift them like the file will be
        let name = self.model.get_worksheets_properties()[sheet as usize].name.clone();
        let shift = op.to_shift(&name);
        for (n, scope, formula) in self.model.get_defined_name_list() {
            if let Some(f) = ooxml::refs::shift_formula(&formula, None, &shift) {
                if f != formula {
                    let _ = self.model.update_defined_name(&n, scope, &n, scope, &f);
                }
            }
        }
        self.ops.push(op);
        self.structural_ever = true;
        Ok(Value::Null)
    }

    pub fn info(&self) -> WorkbookInfo {
        WorkbookInfo {
            name: self
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .map(str::to_string)
                .unwrap_or_else(|| "Untitled.xlsx".into()),
            path: self.path.as_ref().map(|p| p.display().to_string()),
            dirty: self.dirty,
            sheet: self.sheet,
            sheets: self
                .model
                .get_worksheets_properties()
                .into_iter()
                .map(|p| SheetInfo {
                    objects: self
                        .objects
                        .iter()
                        .find(|(n, _)| *n == p.name)
                        .map(|(_, o)| o.clone())
                        .filter(|o| *o != ooxml::inspect::SheetObjects::default()),
                    name: p.name,
                    state: p.state,
                    color: match p.color {
                        Color::None => None,
                        c => Some(self.model.resolve_color(&c)),
                    },
                })
                .collect(),
            can_undo: self.model.can_undo(),
            can_redo: self.model.can_redo(),
        }
    }

    fn layout(&self, sheet: u32) -> Result<Layout, String> {
        let model = self.model.get_model();
        let ws = model.workbook.worksheet(sheet)?;
        let dim = ws.dimension();
        let rows = (dim.max_row + 100).max(MIN_ROWS).min(1_048_576);
        let cols = (dim.max_column + 10).max(MIN_COLS).min(16_384);
        let mut row_heights = Vec::with_capacity(rows as usize);
        for r in 1..=rows {
            row_heights.push(self.model.get_row_height(sheet, r)?);
        }
        for row in &ws.rows {
            if row.hidden && row.r >= 1 && row.r <= rows {
                row_heights[(row.r - 1) as usize] = 0.0;
            }
        }
        let mut col_widths = Vec::with_capacity(cols as usize);
        for c in 1..=cols {
            col_widths.push(self.model.get_column_width(sheet, c)?);
        }
        for col in &ws.cols {
            if col.hidden {
                for c in col.min.max(1)..=col.max.min(cols) {
                    col_widths[(c - 1) as usize] = 0.0;
                }
            }
        }
        Ok(Layout {
            rows,
            cols,
            row_heights,
            col_widths,
            frozen_rows: ws.frozen_rows,
            frozen_cols: ws.frozen_columns,
            merged: ws.merged_cells.iter().map(|m| [m.row, m.column, m.width, m.height]).collect(),
            show_grid: self.model.get_show_grid_lines(sheet).unwrap_or(true),
        })
    }

    fn cells(&self, sheet: u32, r0: i32, c0: i32, r1: i32, c1: i32) -> Result<Vec<CellView>, String> {
        let model = self.model.get_model();
        let ws = model.workbook.worksheet(sheet)?;
        let theme = &model.workbook.theme;
        let mut out = Vec::new();
        for r in ws.sheet_data.rows() {
            if r < r0 || r > r1 {
                continue;
            }
            for (c, _) in ws.sheet_data.cells_in_row_between(r, c0, c1) {
                let style = self.model.get_cell_style(sheet, r, c)?;
                let text = self.model.get_formatted_cell_value(sheet, r, c).unwrap_or_default();
                let numeric = matches!(
                    self.model.get_cell_type(sheet, r, c),
                    Ok(ironcalc_base::types::CellType::Number)
                );
                let (align, valign, wrap) = match &style.alignment {
                    Some(a) => (
                        match a.horizontal {
                            HorizontalAlignment::Center | HorizontalAlignment::CenterContinuous => "center",
                            HorizontalAlignment::Right => "right",
                            HorizontalAlignment::Left | HorizontalAlignment::Fill | HorizontalAlignment::Justify | HorizontalAlignment::Distributed => "left",
                            HorizontalAlignment::General => if numeric { "right" } else { "left" },
                        },
                        match a.vertical {
                            VerticalAlignment::Top => "top",
                            VerticalAlignment::Center => "middle",
                            _ => "bottom",
                        },
                        a.wrap_text,
                    ),
                    None => (if numeric { "right" } else { "left" }, "bottom", false),
                };
                let color = match &style.font.color {
                    Color::None => None,
                    c => Some(c.to_rgb(theme)).filter(|c| !c.eq_ignore_ascii_case("#000000")),
                };
                let fill = match &style.fill.color {
                    Color::None => None,
                    c => Some(c.to_rgb(theme)),
                };
                let b = &style.border;
                let edge = |e: &Option<ironcalc_base::types::BorderItem>| e.is_some();
                let borders = [edge(&b.top), edge(&b.right), edge(&b.bottom), edge(&b.left)];
                let font = (!style.font.name.is_empty() && style.font.name != "Calibri").then(|| style.font.name.clone());
                if text.is_empty() && fill.is_none() && !borders.iter().any(|x| *x) {
                    continue;
                }
                out.push(CellView {
                    r,
                    c,
                    text,
                    b: style.font.b,
                    i: style.font.i,
                    u: style.font.u,
                    s: style.font.strike,
                    color,
                    fill,
                    size: style.font.sz as f64,
                    font,
                    align,
                    valign,
                    wrap,
                    borders: borders.iter().any(|x| *x).then_some(borders),
                });
            }
        }
        Ok(out)
    }

    /// Tab-separated text of a range, for the clipboard.
    fn copy(&self, rect: &Rect) -> Result<String, String> {
        let mut lines = Vec::new();
        for r in rect.row..rect.row + rect.height {
            let mut cols = Vec::new();
            for c in rect.col..rect.col + rect.width {
                cols.push(self.model.get_formatted_cell_value(rect.sheet, r, c).unwrap_or_default());
            }
            lines.push(cols.join("\t"));
        }
        Ok(lines.join("\n"))
    }

    fn paste(&mut self, sheet: u32, row: i32, col: i32, text: &str) -> Result<Value, String> {
        let text = text.strip_suffix('\n').unwrap_or(text).replace("\r\n", "\n");
        let mut width = 0;
        let lines: Vec<&str> = text.split('\n').collect();
        self.model.pause_evaluation();
        let mut result = Ok(());
        for (dr, line) in lines.iter().enumerate() {
            let cells: Vec<&str> = line.split('\t').collect();
            width = width.max(cells.len() as i32);
            for (dc, v) in cells.iter().enumerate() {
                if let Err(e) = self.model.set_user_input(sheet, row + dr as i32, col + dc as i32, v) {
                    result = Err(e);
                }
            }
        }
        self.model.resume_evaluation();
        self.model.evaluate();
        self.dirty = true;
        result?;
        Ok(json!({"width": width, "height": lines.len()}))
    }

    fn selection_stats(&self, rect: &Rect) -> Value {
        let (mut sum, mut count, mut numbers) = (0.0, 0usize, 0usize);
        let model = self.model.get_model();
        let max_cells = 200_000;
        'outer: for r in rect.row..rect.row + rect.height {
            for c in rect.col..rect.col + rect.width {
                if count > max_cells {
                    break 'outer;
                }
                match model.get_cell_value_by_index(rect.sheet, r, c) {
                    Ok(ironcalc_base::cell::CellValue::Number(n)) => {
                        sum += n;
                        numbers += 1;
                        count += 1;
                    }
                    Ok(ironcalc_base::cell::CellValue::None) | Err(_) => {}
                    Ok(_) => count += 1,
                }
            }
        }
        json!({"sum": sum, "count": count, "numbers": numbers,
               "average": if numbers > 0 { Some(sum / numbers as f64) } else { None }})
    }

    /// Run one UI command. Every mutating command marks the workbook dirty.
    pub fn dispatch(&mut self, cmd: &str, args: &Value) -> Result<Value, String> {
        let mutating = !matches!(
            cmd,
            "info" | "layout" | "cells" | "input" | "style" | "copy" | "stats" | "save" | "open" | "new" | "select_sheet"
        );
        let result = match cmd {
            "new" => {
                *self = Session::new();
                Ok(Value::Null)
            }
            "open" => self.open(Path::new(&arg::<String>(args, "path")?)).map(|_| Value::Null),
            "save" => {
                let path: Option<String> = arg(args, "path")?;
                let lossy: Option<bool> = arg(args, "allow_lossy")?;
                self.save(path.map(PathBuf::from), lossy.unwrap_or(false))
            }
            "info" => Ok(serde_json::to_value(self.info()).unwrap()),
            "select_sheet" => {
                let s: u32 = arg(args, "sheet")?;
                if (s as usize) < self.model.get_worksheets_properties().len() {
                    self.sheet = s;
                }
                Ok(Value::Null)
            }
            "layout" => Ok(serde_json::to_value(self.layout(arg(args, "sheet")?)?).unwrap()),
            "cells" => {
                let r: [i32; 4] = arg(args, "range")?;
                Ok(serde_json::to_value(self.cells(arg(args, "sheet")?, r[0], r[1], r[2], r[3])?).unwrap())
            }
            "input" => {
                let rect: Rect = arg(args, "cell")?;
                Ok(json!(self.model.get_cell_content(rect.sheet, rect.row, rect.col)?))
            }
            "style" => {
                let rect: Rect = arg(args, "cell")?;
                let s = self.model.get_cell_style(rect.sheet, rect.row, rect.col)?;
                Ok(json!({
                    "b": s.font.b, "i": s.font.i, "u": s.font.u, "s": s.font.strike,
                    "num_fmt": s.num_fmt, "size": s.font.sz,
                    "align": s.alignment.as_ref().map(|a| format!("{:?}", a.horizontal).to_lowercase()).unwrap_or("general".into()),
                    "wrap": s.alignment.as_ref().map(|a| a.wrap_text).unwrap_or(false),
                }))
            }
            "set_input" => {
                let rect: Rect = arg(args, "cell")?;
                let value: String = arg(args, "value")?;
                self.model.set_user_input(rect.sheet, rect.row, rect.col, &value).map(|_| Value::Null)
            }
            "set_style" => {
                let rect: Rect = arg(args, "range")?;
                let path: String = arg(args, "path")?;
                let value: String = arg(args, "value")?;
                self.model.update_range_style(&rect.area(), &path, &value).map(|_| Value::Null)
            }
            "clear" => {
                let rect: Rect = arg(args, "range")?;
                self.model.range_clear_contents(&rect.area()).map(|_| Value::Null)
            }
            "clear_all" => {
                let rect: Rect = arg(args, "range")?;
                self.model.range_clear_all(&rect.area()).map(|_| Value::Null)
            }
            "copy" => Ok(json!(self.copy(&arg(args, "range")?)?)),
            "paste" => {
                let rect: Rect = arg(args, "cell")?;
                self.paste(rect.sheet, rect.row, rect.col, &arg::<String>(args, "text")?)
            }
            "stats" => Ok(self.selection_stats(&arg(args, "range")?)),
            "undo" | "redo" => {
                if self.structural_ever {
                    self.ops_uncertain = true;
                }
                if cmd == "undo" { self.model.undo() } else { self.model.redo() }.map(|_| Value::Null)
            }
            "add_sheet" => {
                let r = self.model.new_sheet();
                self.sheet = self.model.get_worksheets_properties().len() as u32 - 1;
                r.map(|_| Value::Null)
            }
            "rename_sheet" => self.model.rename_sheet(arg(args, "sheet")?, &arg::<String>(args, "name")?).map(|_| Value::Null),
            "delete_sheet" => {
                let s: u32 = arg(args, "sheet")?;
                let r = self.model.delete_sheet(s);
                let n = self.model.get_worksheets_properties().len() as u32;
                self.sheet = self.sheet.min(n.saturating_sub(1));
                r.map(|_| Value::Null)
            }
            "insert_rows" | "delete_rows" | "insert_cols" | "delete_cols" => {
                let (sheet, at, count) = (arg(args, "sheet")?, arg(args, "at")?, arg(args, "count")?);
                self.structural_op(match cmd {
                    "insert_rows" => StructuralOp::InsertRows { sheet, at, count },
                    "delete_rows" => StructuralOp::DeleteRows { sheet, at, count },
                    "insert_cols" => StructuralOp::InsertCols { sheet, at, count },
                    _ => StructuralOp::DeleteCols { sheet, at, count },
                })
            }
            "set_col_width" => {
                let (s, c0, c1, w): (u32, i32, i32, f64) = (arg(args, "sheet")?, arg(args, "from")?, arg(args, "to")?, arg(args, "width")?);
                self.model.set_columns_width(s, c0, c1, w).map(|_| Value::Null)
            }
            "set_row_height" => {
                let (s, r0, r1, h): (u32, i32, i32, f64) = (arg(args, "sheet")?, arg(args, "from")?, arg(args, "to")?, arg(args, "height")?);
                self.model.set_rows_height(s, r0, r1, h).map(|_| Value::Null)
            }
            "freeze" => {
                let (s, rows, cols): (u32, i32, i32) = (arg(args, "sheet")?, arg(args, "rows")?, arg(args, "cols")?);
                self.model.set_frozen_rows_count(s, rows).and_then(|_| self.model.set_frozen_columns_count(s, cols)).map(|_| Value::Null)
            }
            other => Err(format!("unknown command {other}")),
        };
        if mutating && result.is_ok() {
            self.dirty = true;
            if matches!(cmd, "add_sheet" | "rename_sheet" | "delete_sheet") {
                self.structural_ever = true;
                let what = "Adding, renaming or deleting sheets";
                if !self.structural.contains(&what) {
                    self.structural.push(what);
                }
            }
        }
        result
    }
}

fn load_model(bytes: &[u8], name: &str) -> Result<Model<'static>, String> {
    let workbook = ironcalc::import::load_from_xlsx_bytes(bytes, name, "en", "UTC").map_err(|e| format!("could not open: {e:?}"))?;
    let mut model = Model::from_workbook(workbook, LANG)?;
    model.evaluate();
    Ok(model)
}

fn native_bytes(model: &Model) -> Result<Vec<u8>, String> {
    Ok(ironcalc::export::save_xlsx_to_writer(model, Cursor::new(Vec::new())).map_err(|e| format!("{e:?}"))?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(s: &mut Session, cmd: &str, args: Value) -> Value {
        s.dispatch(cmd, &args).unwrap_or_else(|e| panic!("{cmd}: {e}"))
    }

    #[test]
    fn edit_evaluate_and_render() {
        let mut s = Session::new();
        call(&mut s, "set_input", json!({"cell": {"sheet": 0, "row": 1, "col": 1}, "value": "2"}));
        call(&mut s, "set_input", json!({"cell": {"sheet": 0, "row": 2, "col": 1}, "value": "=A1*21"}));
        call(&mut s, "set_style", json!({"range": {"sheet": 0, "row": 2, "col": 1}, "path": "font.b", "value": "true"}));
        let cells = call(&mut s, "cells", json!({"sheet": 0, "range": [1, 1, 10, 10]}));
        let a2 = cells.as_array().unwrap().iter().find(|c| c["r"] == 2).unwrap();
        assert_eq!(a2["text"], "42");
        assert_eq!(a2["b"], true);
        assert_eq!(a2["align"], "right");
        assert_eq!(call(&mut s, "input", json!({"cell": {"sheet": 0, "row": 2, "col": 1}})), "=A1*21");
        assert!(call(&mut s, "info", json!({}))["dirty"].as_bool().unwrap());
        call(&mut s, "undo", json!({}));
        call(&mut s, "undo", json!({}));
        let cells = call(&mut s, "cells", json!({"sheet": 0, "range": [1, 1, 10, 10]}));
        assert_eq!(cells.as_array().unwrap().len(), 1);
    }

    #[test]
    fn paste_copy_and_stats() {
        let mut s = Session::new();
        call(&mut s, "paste", json!({"cell": {"sheet": 0, "row": 1, "col": 1}, "text": "1\t2\n3\t=A1+B1\n"}));
        assert_eq!(call(&mut s, "copy", json!({"range": {"sheet": 0, "row": 1, "col": 1, "width": 2, "height": 2}})), "1\t2\n3\t3");
        let st = call(&mut s, "stats", json!({"range": {"sheet": 0, "row": 1, "col": 1, "width": 2, "height": 2}}));
        assert_eq!(st["sum"], 9.0);
        assert_eq!(st["numbers"], 4);
    }

    #[test]
    fn row_inserts_are_patched_and_undo_forces_a_rewrite() {
        let dir = std::env::temp_dir().join(format!("sheets-core-struct-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.xlsx");
        let mut s = Session::new();
        call(&mut s, "set_input", json!({"cell": {"sheet": 0, "row": 1, "col": 1}, "value": "a"}));
        call(&mut s, "set_input", json!({"cell": {"sheet": 0, "row": 2, "col": 1}, "value": "=A1&\"!\""}));
        call(&mut s, "save", json!({"path": path.display().to_string()}));
        // inserted row is patched, and the formula follows its input
        call(&mut s, "insert_rows", json!({"sheet": 0, "at": 2, "count": 1}));
        let r = call(&mut s, "save", json!({}));
        assert_eq!(r["mode"], "patch", "{r}");
        let mut t = Session::new();
        call(&mut t, "open", json!({"path": path.display().to_string()}));
        assert_eq!(call(&mut t, "input", json!({"cell": {"sheet": 0, "row": 3, "col": 1}})), "=A1&\"!\"");
        // undo after a structural edit: the op log can't be trusted, so ask first
        call(&mut s, "undo", json!({}));
        assert_eq!(call(&mut s, "save", json!({}))["needs_confirmation"], true);
        assert_eq!(call(&mut s, "save", json!({"allow_lossy": true}))["mode"], "native");
        // sheet changes still need a rewrite
        call(&mut s, "add_sheet", json!({}));
        assert_eq!(call(&mut s, "save", json!({}))["needs_confirmation"], true);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn new_workbook_save_then_patch_save_roundtrip() {
        let dir = std::env::temp_dir().join(format!("sheets-core-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.xlsx");
        let mut s = Session::new();
        call(&mut s, "set_input", json!({"cell": {"sheet": 0, "row": 1, "col": 1}, "value": "hello"}));
        let r = call(&mut s, "save", json!({"path": path.display().to_string()}));
        assert_eq!(r["mode"], "native");
        call(&mut s, "set_input", json!({"cell": {"sheet": 0, "row": 2, "col": 1}, "value": "=LEN(A1)"}));
        let r = call(&mut s, "save", json!({}));
        assert_eq!(r["mode"], "patch", "{r}");
        let mut t = Session::new();
        call(&mut t, "open", json!({"path": path.display().to_string()}));
        let cells = call(&mut t, "cells", json!({"sheet": 0, "range": [1, 1, 5, 5]}));
        assert_eq!(cells[1]["text"], "5");
        std::fs::remove_dir_all(dir).ok();
    }
}
