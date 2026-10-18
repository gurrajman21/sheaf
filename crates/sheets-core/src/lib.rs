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

mod extras;
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
    /// Row/column inserts and deletes and sheet adds/renames/deletes since the last
    /// open/save, in order, each with the id of the history entry that made it. The
    /// patch writer replays them on its baseline and rewrites every reference in the file.
    ops: Vec<(StructuralOp, u64)>,
    /// IronCalc's undo history as the session sees it, so undo/redo keep `ops` in step.
    history: Mirror,
    /// Undo/redo the mirror couldn't follow: the op log may no longer match the
    /// model (a moved block of cells looks like plain edits to the diff), so the
    /// next save must not be a patch.
    ops_uncertain: bool,
    /// Last copy made in this app (IronCalc's clipboard, serialized) and the text
    /// put on the system clipboard with it. Pasting that same text uses this, so
    /// formulas shift and formatting comes along, like Excel.
    clipboard: Option<(Value, String)>,
    /// data validations per sheet, read from the file
    validations: Vec<(String, Vec<ooxml::inspect::Validation>)>,
    /// charts from the file plus charts added in this session
    charts: Vec<extras::ChartState>,
    new_charts: Vec<(String, ooxml::charts::NewChart)>,
    next_chart: usize,
}

/// One IronCalc history entry, as the session knows it.
#[derive(Clone, Debug)]
enum Entry {
    /// Anything that isn't a structural op.
    Plain,
    /// A run of entries too long to count (a huge paste): where it ends is unknown.
    Unknown,
    /// A structural op, and the op that takes it back (none for a sheet delete).
    Op { id: u64, op: StructuralOp, inverse: Option<StructuralOp> },
}

/// A mirror of IronCalc's undo and redo stacks, grouped by command: one undo takes
/// back everything one command did (a paste of many cells, an insert and the defined
/// names it shifted), like Excel. IronCalc doesn't expose its history, but every entry
/// it pushes also goes to its send queue, which `dispatch` drains around each command
/// to count them.
#[derive(Default)]
struct Mirror {
    /// One frame per command, its entries oldest first.
    undo: Vec<Vec<Entry>>,
    redo: Vec<Vec<Entry>>,
    next_id: u64,
    /// The op the running command applied, to tag its history entry, and how many
    /// entries the command made before it (`None`: too many to count).
    pending: Option<(Entry, Option<usize>)>,
    /// Lost track of IronCalc's history: any undo/redo after a structural op is a guess.
    lost: bool,
    /// A structural op happened since the file was opened.
    seen_ops: bool,
}

/// History entries in a drained send queue. bitcode writes the queue's length first:
/// one byte below 255, else 255 and a packed int (width code, little-endian bytes).
/// `None` for a layout we don't expect (`queue_len_matches_bitcode` pins it).
fn queue_len(q: &[u8]) -> Option<usize> {
    match *q {
        [b, ..] if b < 255 => Some(b as usize),
        [255, code, ref rest @ ..] => {
            let width = match code {
                6 => 1,
                4 => 2,
                2 => 4,
                0 => 8,
                _ => return None,
            };
            let n = rest.get(..width)?.iter().rev().fold(0u64, |a, &b| a << 8 | b as u64);
            usize::try_from(n).ok().filter(|&n| n >= 255)
        }
        _ => None,
    }
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
    /// conditional-format data bar: fill fraction, axis position, colors
    #[serde(skip_serializing_if = "Option::is_none")]
    bar: Option<Value>,
    /// conditional-format icon (icon sets and ratings)
    #[serde(skip_serializing_if = "Option::is_none")]
    icon: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    comment: Option<String>,
    /// the cell holds a formula (for "show formulas" and editing hints)
    #[serde(skip_serializing_if = "is_false")]
    formula: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Deserialize)]
pub(crate) struct Rect {
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
        Session { model, original: None, path: None, dirty: false, sheet: 0, objects: Vec::new(), ops: Vec::new(), history: Mirror::default(), ops_uncertain: false, clipboard: None,
            validations: Vec::new(), charts: Vec::new(), new_charts: Vec::new(), next_chart: 0 }
    }

    pub fn open(&mut self, path: &Path) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Book").to_string();
        let model = load_model(&bytes, &name)?;
        self.model = UserModel::from_model(model);
        self.load_file_meta(&bytes);
        self.new_charts.clear();
        self.ops.clear();
        self.history = Mirror::default();
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
                let ops: Vec<StructuralOp> = self.ops.iter().map(|(op, _)| op.clone()).collect();
                match ooxml::patch::save_patched_ops(orig, &base, self.model.get_model(), &ops) {
                    Ok((bytes, report)) => (bytes, "patch", serde_json::to_value(report).ok()),
                    Err(reason) if !allow_lossy => {
                        return Ok(json!({"saved": false, "needs_confirmation": true, "reason": reason}));
                    }
                    Err(_) => (native_bytes(self.model.get_model())?, "native", None),
                }
            }
            None => (native_bytes(self.model.get_model())?, "native", None),
        };
        let bytes = ooxml::charts::apply_chart_edits(&bytes, &self.chart_edits(mode == "patch"))?;
        // write atomically so a failed save never leaves a half-written file
        let tmp = path.with_extension("xlsx.saving");
        std::fs::write(&tmp, &bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
        // the saved file is the new baseline for the next patch save
        self.load_file_meta(&bytes);
        self.new_charts.clear();
        // ops already in the file: undoing one now logs its inverse
        self.ops.clear();
        self.ops_uncertain = false;
        self.original = Some(bytes);
        self.path = Some(path.clone());
        self.dirty = false;
        Ok(json!({"saved": true, "mode": mode, "path": path.display().to_string(), "report": report}))
    }

    /// Why the next save can't be a patch, if it can't.
    fn blocked(&self) -> Option<String> {
        if self.ops_uncertain {
            return Some("Undoing or redoing after inserting or deleting rows, columns or sheets can't be saved into the original file yet".into());
        }
        None
    }

    /// Insert/delete rows or columns with undo, keeping the op log and defined names in step.
    fn structural_op(&mut self, op: StructuralOp) -> Result<Value, String> {
        let (sheet, at, count, inverse) = match op {
            StructuralOp::InsertRows { sheet, at, count } => (sheet, at, count, StructuralOp::DeleteRows { sheet, at, count }),
            StructuralOp::DeleteRows { sheet, at, count } => (sheet, at, count, StructuralOp::InsertRows { sheet, at, count }),
            StructuralOp::InsertCols { sheet, at, count } => (sheet, at, count, StructuralOp::DeleteCols { sheet, at, count }),
            StructuralOp::DeleteCols { sheet, at, count } => (sheet, at, count, StructuralOp::InsertCols { sheet, at, count }),
            _ => return Err("not a row/column operation".into()),
        };
        match op {
            StructuralOp::InsertRows { .. } => self.model.insert_rows(sheet, at, count)?,
            StructuralOp::DeleteRows { .. } => self.model.delete_rows(sheet, at, count)?,
            StructuralOp::InsertCols { .. } => self.model.insert_columns(sheet, at, count)?,
            _ => self.model.delete_columns(sheet, at, count)?,
        }
        // the op is IronCalc's first history entry; the name updates below come after it
        self.log_op(op.clone(), Some(inverse));
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
        self.follow(&op, None);
        Ok(Value::Null)
    }

    /// Add, rename or delete a sheet, logging it for the patch writer.
    fn sheet_op(&mut self, cmd: &str, args: &Value) -> Result<Value, String> {
        match cmd {
            "add_sheet" => {
                self.model.new_sheet()?;
                let props = self.model.get_worksheets_properties();
                let index = props.len() as u32 - 1;
                self.log_op(StructuralOp::AddSheet { name: props[index as usize].name.clone() }, Some(StructuralOp::DeleteSheet { sheet: index }));
                self.sheet = index;
            }
            "rename_sheet" => {
                let (sheet, name): (u32, String) = (arg(args, "sheet")?, arg(args, "name")?);
                let old = self.model.get_worksheets_properties().get(sheet as usize).ok_or("no such sheet")?.name.clone();
                if old == name {
                    return Ok(Value::Null); // IronCalc records nothing either
                }
                self.model.rename_sheet(sheet, &name)?;
                let op = StructuralOp::RenameSheet { sheet, name };
                let inverse = StructuralOp::RenameSheet { sheet, name: old.clone() };
                self.follow(&op, Some(&inverse));
                self.log_op(op, Some(inverse));
            }
            _ => {
                let sheet: u32 = arg(args, "sheet")?;
                if self.model.get_worksheets_properties().len() < 2 {
                    return Err("Cannot delete only sheet".into());
                }
                // names scoped to the sheet go with it, as in Excel (IronCalc would keep them
                // pointing at a missing sheet, and its exporter can't write those)
                for (n, scope, _) in self.model.get_defined_name_list() {
                    if scope == Some(sheet) {
                        self.model.delete_defined_name(&n, scope)?;
                    }
                }
                let before = queue_len(&self.model.flush_send_queue());
                self.model.delete_sheet(sheet)?;
                // putting a deleted sheet back would need its content: no inverse op
                self.log_op(StructuralOp::DeleteSheet { sheet }, None);
                if let Some((_, pre)) = &mut self.history.pending {
                    *pre = before;
                }
                let n = self.model.get_worksheets_properties().len() as u32;
                self.sheet = self.sheet.min(n.saturating_sub(1));
            }
        }
        Ok(Value::Null)
    }

    /// Append an op the model just applied to the log, tagging the history entry it made.
    fn log_op(&mut self, op: StructuralOp, inverse: Option<StructuralOp>) {
        let id = self.history.next_id;
        self.history.next_id += 1;
        self.ops.push((op.clone(), id));
        self.history.seen_ops = true;
        self.history.pending = Some((Entry::Op { id, op, inverse }, Some(0)));
    }

    /// Keep what the session tracks besides the model (chart positions, per-sheet
    /// file objects) in step with an op the model just applied. `other` is the op's
    /// counterpart (for a rename, the one that holds the previous name).
    fn follow(&mut self, op: &StructuralOp, other: Option<&StructuralOp>) {
        match *op {
            StructuralOp::InsertRows { sheet, at, count } => self.shift_charts(sheet, true, at, count),
            StructuralOp::DeleteRows { sheet, at, count } => self.shift_charts(sheet, true, at, -count),
            StructuralOp::InsertCols { sheet, at, count } => self.shift_charts(sheet, false, at, count),
            StructuralOp::DeleteCols { sheet, at, count } => self.shift_charts(sheet, false, at, -count),
            StructuralOp::RenameSheet { ref name, .. } => {
                if let Some(StructuralOp::RenameSheet { name: old, .. }) = other {
                    self.sheet_renamed(old, name);
                }
            }
            // charts and objects of a deleted sheet stay keyed by its name, so undo brings them back
            StructuralOp::AddSheet { .. } | StructuralOp::DeleteSheet { .. } => {}
        }
    }

    /// After a dispatch: put the history entries it made (`pushed`, from the send queue) on the mirror.
    fn record_history(&mut self, pushed: Option<usize>) {
        let pending = self.history.pending.take();
        let entries: Vec<Entry> = match (pushed, pending) {
            (Some(0), None) => return,
            (Some(n), None) => vec![Entry::Plain; n],
            // the op's own entry comes first among those counted after `pre`
            (Some(n), Some((op, Some(pre)))) if n > 0 => {
                std::iter::repeat_n(Entry::Plain, pre).chain(std::iter::once(op)).chain(std::iter::repeat_n(Entry::Plain, n - 1)).collect()
            }
            // an op without its history entry, or too many to count
            (_, pending) => {
                self.history.seen_ops |= pending.is_some();
                vec![Entry::Unknown]
            }
        };
        self.history.redo.clear(); // IronCalc drops its redo stack on any new entry
        self.history.undo.push(entries);
    }

    /// Undo or redo one command (all the history entries it made), keeping the op log
    /// in step with the model.
    fn undo_redo(&mut self, undo: bool) -> Result<Value, String> {
        let can = if undo { self.model.can_undo() } else { self.model.can_redo() };
        if !can {
            return Ok(Value::Null);
        }
        let h = &mut self.history;
        let stack = if undo { &mut h.undo } else { &mut h.redo };
        // a command whose entries couldn't be counted: one entry at a time, and the mirror is lost
        if h.lost || stack.last().is_none_or(|f| f.iter().any(|e| matches!(e, Entry::Unknown))) {
            h.lost = true;
            self.ops_uncertain |= h.seen_ops;
            return if undo { self.model.undo() } else { self.model.redo() }.map(|_| Value::Null);
        }
        let frame = stack.pop().unwrap();
        // undo takes entries back newest first; redo replays them oldest first
        let order: Vec<&Entry> = if undo { frame.iter().rev().collect() } else { frame.iter().collect() };
        for entry in order {
            if let Err(e) = if undo { self.model.undo() } else { self.model.redo() } {
                self.history.lost = true;
                self.ops_uncertain |= self.history.seen_ops;
                return Err(e);
            }
            if let Entry::Op { id, op, inverse } = entry {
                // what the model just did, and its counterpart
                let (did, back) = if undo { (inverse.as_ref(), Some(op)) } else { (Some(op), inverse.as_ref()) };
                match (self.ops.last(), did) {
                    // taking back the op just logged cancels it
                    (Some((last, last_id)), _) if last_id == id && Some(last) == back => {
                        self.ops.pop();
                    }
                    // it's in the saved file already: log what the model did
                    (_, Some(d)) => self.ops.push((d.clone(), *id)),
                    // a deleted sheet coming back after a save
                    (_, None) => self.ops_uncertain = true,
                }
                if let Some(d) = did {
                    self.follow(&d.clone(), back.cloned().as_ref());
                }
            }
        }
        self.model.flush_send_queue();
        if undo { self.history.redo.push(frame) } else { self.history.undo.push(frame) };
        Ok(Value::Null)
    }

    /// Sheet renamed: carry the per-sheet state the session keeps by name.
    fn sheet_renamed(&mut self, old: &str, new: &str) {
        let fix = |r: &mut Option<String>| {
            if let Some(n) = r.as_deref().and_then(|x| ooxml::refs::rename_sheet_in_formula(x, old, new)) {
                *r = Some(n);
            }
        };
        for c in &mut self.charts {
            if c.sheet == old {
                c.sheet = new.to_string();
            }
            for s in &mut c.series {
                fix(&mut s.1);
                fix(&mut s.2);
                fix(&mut s.3);
            }
        }
        for (_, c) in &mut self.new_charts {
            if c.sheet == old {
                c.sheet = new.to_string();
            }
            for s in &mut c.series {
                fix(&mut s.name_ref);
                fix(&mut s.cat_ref);
                if let Some(n) = ooxml::refs::rename_sheet_in_formula(&s.val_ref, old, new) {
                    s.val_ref = n;
                }
            }
        }
        for (n, _) in &mut self.objects {
            if n == old {
                *n = new.to_string();
            }
        }
        for (n, rules) in &mut self.validations {
            if n == old {
                *n = new.to_string();
            }
            for v in rules.iter_mut() {
                if let Some(f) = ooxml::refs::rename_sheet_in_formula(&v.formula1, old, new) {
                    v.formula1 = f;
                }
            }
        }
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

    fn cells(&self, sheet: u32, r0: i32, c0: i32, r1: i32, c1: i32, show_formulas: bool) -> Result<Vec<CellView>, String> {
        let model = self.model.get_model();
        let ws = model.workbook.worksheet(sheet)?;
        let theme = &model.workbook.theme;
        let mut comments: std::collections::HashMap<(i32, i32), String> = ws
            .comments
            .iter()
            .filter_map(|c| {
                let (r, col) = ooxml::sheet::parse_ref(&c.cell_ref)?;
                (r >= r0 && r <= r1 && col >= c0 && col <= c1).then(|| ((r, col), c.text.trim().to_string()))
            })
            .collect();
        let mut out = Vec::new();
        for r in ws.sheet_data.rows() {
            if r < r0 || r > r1 {
                continue;
            }
            for (c, _) in ws.sheet_data.cells_in_row_between(r, c0, c1) {
                let ext = self.model.get_extended_cell_style(sheet, r, c)?;
                let style = &ext.style;
                let content = self.model.get_cell_content(sheet, r, c).unwrap_or_default();
                let is_formula = content.starts_with('=');
                let mut text = if show_formulas && is_formula {
                    content
                } else {
                    self.model.get_formatted_cell_value(sheet, r, c).unwrap_or_default()
                };
                let bar = ext.data_bar.as_ref().map(|d| json!({
                    "value": d.value, "axis": d.axis_position, "gradient": d.is_gradient,
                    "color": d.positive_color.to_rgb(theme), "neg": d.negative_color.to_rgb(theme),
                }));
                let icon = match (&ext.icon, &ext.rating) {
                    (Some(i), _) => Some(json!({"name": format!("{:?}", i.icon), "color": i.color.to_rgb(theme)})),
                    (None, Some(rt)) => Some(json!({"name": format!("{:?}", rt.icon), "color": rt.color.to_rgb(theme), "count": rt.count, "max": rt.max})),
                    _ => None,
                };
                let hide_value = ext.data_bar.as_ref().is_some_and(|d| !d.show_value)
                    || ext.icon.as_ref().is_some_and(|i| !i.show_value)
                    || ext.rating.as_ref().is_some_and(|i| !i.show_value);
                if hide_value && !(show_formulas && is_formula) {
                    text.clear();
                }
                let comment = comments.remove(&(r, c));
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
                if text.is_empty() && fill.is_none() && !borders.iter().any(|x| *x) && bar.is_none() && icon.is_none() && comment.is_none() {
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
                    bar,
                    icon,
                    comment,
                    formula: is_formula,
                });
            }
        }
        // comments on empty cells
        for ((r, c), text) in comments {
            out.push(CellView { r, c, size: 11.0, align: "left", valign: "bottom", comment: Some(text), ..Default::default() });
        }
        Ok(out)
    }

    /// Point IronCalc's selection at a range (its clipboard and paste work on the selection).
    fn select_range(&mut self, rect: &Rect) -> Result<(), String> {
        self.model.set_selected_sheet(rect.sheet)?;
        self.model.set_selected_cell(rect.row, rect.col)?;
        self.model.set_selected_range(rect.row, rect.col, rect.row + rect.height.max(1) - 1, rect.col + rect.width.max(1) - 1)
    }

    fn clip_copy(&mut self, rect: &Rect) -> Result<Value, String> {
        self.select_range(rect)?;
        let clip = serde_json::to_value(self.model.copy_to_clipboard()?).map_err(|e| e.to_string())?;
        let csv = clip["csv"].as_str().unwrap_or_default().to_string();
        self.clipboard = Some((clip, csv.clone()));
        Ok(json!(csv))
    }

    /// Paste the app's own last copy at `at`, if the system clipboard still holds its text.
    fn clip_paste(&mut self, at: &Rect, text: &str, cut: bool) -> Result<Option<Value>, String> {
        let Some((clip, csv)) = &self.clipboard else { return Ok(None) };
        let norm = |s: &str| s.replace("\r\n", "\n").trim_end_matches('\n').to_string();
        if norm(csv) != norm(text) {
            return Ok(None);
        }
        let source_sheet = clip["sheet"].as_u64().unwrap_or(0) as u32;
        let range: (i32, i32, i32, i32) = serde_json::from_value(clip["range"].clone()).map_err(|e| e.to_string())?;
        let data: ironcalc_base::ClipboardData = serde_json::from_value(clip["data"].clone()).map_err(|e| e.to_string())?;
        self.select_range(&Rect { sheet: at.sheet, row: at.row, col: at.col, width: 1, height: 1 })?;
        self.model.paste_from_clipboard(source_sheet, range, &data, cut)?;
        if cut {
            self.clipboard = None;
        }
        Ok(Some(json!({"width": range.3 - range.1 + 1, "height": range.2 - range.0 + 1})))
    }

    /// Sort the rows of a range by one column, as one undoable step. Rows move whole,
    /// with their formatting, through IronCalc's own copy/paste.
    fn sort(&mut self, rect: &Rect, key_col: i32, desc: bool) -> Result<(), String> {
        let model = self.model.get_model();
        let mut rows: Vec<i32> = (rect.row..rect.row + rect.height).collect();
        use ironcalc_base::cell::CellValue as V;
        let key = |r: i32| model.get_cell_value_by_index(rect.sheet, r, key_col).unwrap_or(V::None);
        rows.sort_by(|&a, &b| {
            use std::cmp::Ordering::*;
            let rank = |v: &V| match v { V::Number(_) => 0, V::String(_) => 1, V::Boolean(_) => 2, V::None => 3 };
            let (ka, kb) = (key(a), key(b));
            // empty cells always sort last, either direction
            match (rank(&ka), rank(&kb)) {
                (3, 3) => return Equal,
                (3, _) => return Greater,
                (_, 3) => return Less,
                _ => {}
            }
            let ord = match (&ka, &kb) {
                (V::Number(x), V::Number(y)) => x.partial_cmp(y).unwrap_or(Equal),
                (V::String(x), V::String(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
                (V::Boolean(x), V::Boolean(y)) => x.cmp(y),
                _ => rank(&ka).cmp(&rank(&kb)),
            };
            if desc { ord.reverse() } else { ord }
        });
        self.reorder_rows(rect, &rows)
    }

    /// Rewrite a range's rows in the given order (source row numbers), starting at
    /// its top, as one undoable paste. Rows keep their formatting.
    fn reorder_rows(&mut self, rect: &Rect, order: &[i32]) -> Result<(), String> {
        self.select_range(rect)?;
        let mut clip = serde_json::to_value(self.model.copy_to_clipboard()?).map_err(|e| e.to_string())?;
        let old = clip["data"].clone();
        let mut new = serde_json::Map::new();
        for (i, src) in order.iter().enumerate() {
            if let Some(row) = old.get(src.to_string()) {
                new.insert((rect.row + i as i32).to_string(), row.clone());
            }
        }
        clip["data"] = Value::Object(new);
        let range: (i32, i32, i32, i32) = serde_json::from_value(clip["range"].clone()).map_err(|e| e.to_string())?;
        let data: ironcalc_base::ClipboardData = serde_json::from_value(clip["data"].clone()).map_err(|e| e.to_string())?;
        self.select_range(&Rect { sheet: rect.sheet, row: rect.row, col: rect.col, width: 1, height: 1 })?;
        self.model.paste_from_clipboard(rect.sheet, range, &data, false)
    }

    fn find(&self, query: &str, match_case: bool) -> Value {
        let model = self.model.get_model();
        let q = if match_case { query.to_string() } else { query.to_lowercase() };
        let mut hits = Vec::new();
        if q.is_empty() {
            return json!(hits);
        }
        for (si, ws) in model.workbook.worksheets.iter().enumerate() {
            let mut cells: Vec<(i32, i32)> = ws.sheet_data.cells().map(|(r, c, _)| (r, c)).collect();
            cells.sort();
            for (r, c) in cells {
                let shown = self.model.get_formatted_cell_value(si as u32, r, c).unwrap_or_default();
                let content = self.model.get_cell_content(si as u32, r, c).unwrap_or_default();
                let norm = |s: &str| if match_case { s.to_string() } else { s.to_lowercase() };
                if norm(&shown).contains(&q) || norm(&content).contains(&q) {
                    hits.push(json!({"sheet": si, "row": r, "col": c, "text": shown}));
                    if hits.len() >= 5000 {
                        return json!(hits);
                    }
                }
            }
        }
        json!(hits)
    }

    fn replace(&mut self, query: &str, with: &str, match_case: bool, cells: &[(u32, i32, i32)]) -> Result<usize, String> {
        let mut n = 0;
        for &(sheet, r, c) in cells {
            let content = self.model.get_cell_content(sheet, r, c)?;
            let replaced = if match_case {
                content.replace(query, with)
            } else {
                replace_ci(&content, query, with)
            };
            if replaced != content {
                self.model.set_user_input(sheet, r, c, &replaced)?;
                n += 1;
            }
        }
        Ok(n)
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
        let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
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
                        min = min.min(n);
                        max = max.max(n);
                        numbers += 1;
                        count += 1;
                    }
                    Ok(ironcalc_base::cell::CellValue::None) | Err(_) => {}
                    Ok(_) => count += 1,
                }
            }
        }
        json!({"sum": sum, "count": count, "numbers": numbers,
               "average": if numbers > 0 { Some(sum / numbers as f64) } else { None },
               "min": (numbers > 0).then_some(min), "max": (numbers > 0).then_some(max)})
    }

    /// Run one UI command. Every mutating command marks the workbook dirty.
    pub fn dispatch(&mut self, cmd: &str, args: &Value) -> Result<Value, String> {
        let mutating = !matches!(
            cmd,
            "info" | "layout" | "cells" | "input" | "style" | "copy" | "clip_copy" | "stats" | "save" | "open" | "new"
                | "select_sheet" | "complete" | "find" | "charts" | "validation"
        );
        // count the history entries this command makes (undo/redo and open/new/save manage the mirror themselves)
        let counted = !matches!(cmd, "undo" | "redo" | "new" | "open" | "save");
        if counted {
            self.model.flush_send_queue();
            self.history.pending = None;
        }
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
                let show: Option<bool> = arg(args, "show_formulas")?;
                Ok(serde_json::to_value(self.cells(arg(args, "sheet")?, r[0], r[1], r[2], r[3], show.unwrap_or(false))?).unwrap())
            }
            "complete" => {
                let rect: Rect = arg(args, "cell")?;
                let formula: String = arg(args, "formula")?;
                let cursor: usize = arg(args, "cursor")?;
                let ctx = self.model.formula_completion(rect.sheet, rect.row, rect.col, &formula, cursor)?;
                Ok(serde_json::to_value(ctx).unwrap())
            }
            "clip_copy" => self.clip_copy(&arg(args, "range")?),
            "clip_paste" => {
                let rect: Rect = arg(args, "cell")?;
                let text: String = arg(args, "text")?;
                let cut: Option<bool> = arg(args, "cut")?;
                match self.clip_paste(&rect, &text, cut.unwrap_or(false))? {
                    Some(v) => Ok(v),
                    None => self.paste(rect.sheet, rect.row, rect.col, &text),
                }
            }
            "autofill" => {
                let rect: Rect = arg(args, "range")?;
                let to_row: Option<i32> = arg(args, "to_row")?;
                let to_col: Option<i32> = arg(args, "to_col")?;
                match (to_row, to_col) {
                    (Some(r), _) => self.model.auto_fill_rows(&rect.area(), r),
                    (_, Some(c)) => self.model.auto_fill_columns(&rect.area(), c),
                    _ => Err("autofill needs to_row or to_col".into()),
                }
                .map(|_| Value::Null)
            }
            "border" => {
                let rect: Rect = arg(args, "range")?;
                let kind: String = arg(args, "kind")?;
                let style: Option<String> = arg(args, "style")?;
                let color: Option<String> = arg(args, "color")?;
                let area: ironcalc_base::BorderArea = serde_json::from_value(json!({
                    "item": {"style": style.unwrap_or("thin".into()), "color": color.unwrap_or("#000000".into())},
                    "type": kind,
                }))
                .map_err(|e| format!("border: {e}"))?;
                self.model.set_area_with_border(&rect.area(), &area).map(|_| Value::Null)
            }
            "merge" => {
                let rect: Rect = arg(args, "range")?;
                let center: Option<bool> = arg(args, "center")?;
                if center.unwrap_or(false) { self.model.merge_cells_center(&rect.area()) } else { self.model.merge_cells(&rect.area()) }
                    .map(|_| Value::Null)
            }
            "unmerge" => self.model.unmerge_cells(&arg::<Rect>(args, "range")?.area()).map(|_| Value::Null),
            "sort" => {
                let rect: Rect = arg(args, "range")?;
                let desc: Option<bool> = arg(args, "desc")?;
                self.sort(&rect, arg(args, "col")?, desc.unwrap_or(false)).map(|_| Value::Null)
            }
            "find" => {
                let mc: Option<bool> = arg(args, "match_case")?;
                Ok(self.find(&arg::<String>(args, "query")?, mc.unwrap_or(false)))
            }
            "replace" => {
                let mc: Option<bool> = arg(args, "match_case")?;
                let cells: Vec<(u32, i32, i32)> = arg(args, "cells")?;
                self.replace(&arg::<String>(args, "query")?, &arg::<String>(args, "with")?, mc.unwrap_or(false), &cells).map(|n| json!(n))
            }
            "hide" => {
                let (s, kind, from, to, hidden): (u32, String, i32, i32, bool) =
                    (arg(args, "sheet")?, arg(args, "kind")?, arg(args, "from")?, arg(args, "to")?, arg(args, "hidden")?);
                if kind == "rows" { self.model.set_rows_hidden(s, from, to, hidden) } else { self.model.set_columns_hidden(s, from, to, hidden) }
                    .map(|_| Value::Null)
            }
            "charts" => Ok(self.charts_json(arg(args, "sheet")?)),
            "chart_add" => self.chart_add(&arg::<Rect>(args, "range")?, &arg::<String>(args, "kind")?),
            "chart_move" => self.chart_move(&arg::<String>(args, "id")?, args.get("anchor").unwrap_or(&Value::Null)).map(|_| Value::Null),
            "chart_delete" => self.chart_delete(&arg::<String>(args, "id")?).map(|_| Value::Null),
            "validation" => Ok(self.validation_list(&arg(args, "cell")?)),
            "template" => self.apply_template(&arg::<String>(args, "name")?).map(|_| Value::Null),
            "dedupe" => self.dedupe(&arg(args, "range")?).map(|n| json!(n)),
            "paste_formats" => self.paste_formats(&arg(args, "source")?, &arg(args, "target")?).map(|_| Value::Null),
            "hide_rows_list" => {
                let mut rows: Vec<i32> = arg(args, "rows")?;
                self.hide_rows_list(arg(args, "sheet")?, &mut rows, arg(args, "hidden")?).map(|_| Value::Null)
            }
            "grid_lines" => self.model.set_show_grid_lines(arg(args, "sheet")?, arg(args, "show")?).map(|_| Value::Null),
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
                    "valign": s.alignment.as_ref().map(|a| format!("{:?}", a.vertical).to_lowercase()).unwrap_or("bottom".into()),
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
            "clear_formats" => {
                let rect: Rect = arg(args, "range")?;
                self.model.range_clear_formatting(&rect.area()).map(|_| Value::Null)
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
            "undo" | "redo" => self.undo_redo(cmd == "undo"),
            "add_sheet" | "rename_sheet" | "delete_sheet" => self.sheet_op(cmd, args),
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
        if counted {
            let pushed = queue_len(&self.model.flush_send_queue());
            self.record_history(pushed);
        }
        if mutating && result.is_ok() {
            self.dirty = true;
        }
        result
    }
}

/// Case-insensitive replace of every occurrence of `from`.
fn replace_ci(text: &str, from: &str, to: &str) -> String {
    if from.is_empty() {
        return text.to_string();
    }
    let lower = text.to_lowercase();
    let needle = from.to_lowercase();
    if lower.len() != text.len() {
        return text.replace(from, to); // casing changed byte lengths; fall back to exact match
    }
    let mut out = String::new();
    let mut i = 0;
    while let Some(p) = lower[i..].find(&needle) {
        out.push_str(&text[i..i + p]);
        out.push_str(to);
        i += p + needle.len();
    }
    out.push_str(&text[i..]);
    out
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
    fn copy_paste_shifts_formulas_and_sort_and_find() {
        let mut s = Session::new();
        let set = |s: &mut Session, r: i32, c: i32, v: &str| { call(s, "set_input", json!({"cell": {"sheet": 0, "row": r, "col": c}, "value": v})); };
        set(&mut s, 1, 1, "3"); set(&mut s, 2, 1, "1"); set(&mut s, 3, 1, "2");
        set(&mut s, 1, 2, "=A1*10");
        // internal copy/paste moves the formula's relative reference
        let csv = call(&mut s, "clip_copy", json!({"range": {"sheet": 0, "row": 1, "col": 2}}));
        call(&mut s, "clip_paste", json!({"cell": {"sheet": 0, "row": 3, "col": 2}, "text": csv}));
        assert_eq!(call(&mut s, "input", json!({"cell": {"sheet": 0, "row": 3, "col": 2}})), "=A3*10");
        // foreign text pastes as plain values
        call(&mut s, "clip_paste", json!({"cell": {"sheet": 0, "row": 5, "col": 1}, "text": "x\ty"}));
        assert_eq!(call(&mut s, "input", json!({"cell": {"sheet": 0, "row": 5, "col": 2}})), "y");
        // sort A1:A3 descending
        call(&mut s, "sort", json!({"range": {"sheet": 0, "row": 1, "col": 1, "width": 1, "height": 3}, "col": 1, "desc": true}));
        let col: Vec<Value> = (1..=3).map(|r| call(&mut s, "input", json!({"cell": {"sheet": 0, "row": r, "col": 1}}))).collect();
        assert_eq!(col, vec![json!("3"), json!("2"), json!("1")]);
        // autofill a series
        call(&mut s, "autofill", json!({"range": {"sheet": 0, "row": 1, "col": 4, "height": 2}, "to_row": 4}));
        let hits = call(&mut s, "find", json!({"query": "A3"}));
        assert!(hits.as_array().unwrap().iter().any(|h| h["row"] == 3 && h["col"] == 2), "{hits}");
        let n = call(&mut s, "replace", json!({"query": "a3", "with": "A2", "cells": [[0, 3, 2]]}));
        assert_eq!(n, 1);
        assert_eq!(call(&mut s, "input", json!({"cell": {"sheet": 0, "row": 3, "col": 2}})), "=A2*10");
        call(&mut s, "border", json!({"range": {"sheet": 0, "row": 1, "col": 1, "width": 2, "height": 2}, "kind": "All"}));
        call(&mut s, "merge", json!({"range": {"sheet": 0, "row": 8, "col": 1, "width": 3}, "center": true}));
        assert_eq!(call(&mut s, "layout", json!({"sheet": 0}))["merged"][0], json!([8, 1, 3, 1]));
    }

    #[test]
    fn layout_edits_are_patched() {
        let dir = std::env::temp_dir().join(format!("sheets-core-layout-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("l.xlsx");
        let mut s = Session::new();
        call(&mut s, "set_input", json!({"cell": {"sheet": 0, "row": 1, "col": 1}, "value": "title"}));
        call(&mut s, "save", json!({"path": path.display().to_string()}));
        call(&mut s, "merge", json!({"range": {"sheet": 0, "row": 1, "col": 1, "width": 3}, "center": true}));
        call(&mut s, "set_col_width", json!({"sheet": 0, "from": 2, "to": 2, "width": 200.0}));
        call(&mut s, "set_row_height", json!({"sheet": 0, "from": 3, "to": 3, "height": 60.0}));
        call(&mut s, "freeze", json!({"sheet": 0, "rows": 1, "cols": 0}));
        call(&mut s, "hide", json!({"sheet": 0, "kind": "rows", "from": 5, "to": 5, "hidden": true}));
        let r = call(&mut s, "save", json!({}));
        assert_eq!(r["mode"], "patch", "{r}");
        let mut t = Session::new();
        call(&mut t, "open", json!({"path": path.display().to_string()}));
        let l = call(&mut t, "layout", json!({"sheet": 0}));
        assert_eq!(l["merged"][0], json!([1, 1, 3, 1]));
        assert_eq!(l["frozen_rows"], 1);
        assert!((l["col_widths"][1].as_f64().unwrap() - 200.0).abs() < 2.0, "{}", l["col_widths"][1]);
        assert!((l["row_heights"][2].as_f64().unwrap() - 60.0).abs() < 2.0, "{}", l["row_heights"][2]);
        assert_eq!(l["row_heights"][4], 0.0, "row 5 hidden");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn charts_add_move_save_and_reload() {
        let dir = std::env::temp_dir().join(format!("sheets-core-charts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.xlsx");
        let mut s = Session::new();
        call(&mut s, "paste", json!({"cell": {"sheet": 0, "row": 1, "col": 1}, "text": "Month\tSales\nJan\t10\nFeb\t20\nMar\t15\n"}));
        call(&mut s, "save", json!({"path": path.display().to_string()}));
        let id = call(&mut s, "chart_add", json!({"range": {"sheet": 0, "row": 1, "col": 1, "width": 2, "height": 4}, "kind": "column"}));
        let charts = call(&mut s, "charts", json!({"sheet": 0}));
        assert_eq!(charts[0]["series"][0]["name"], "Sales");
        assert_eq!(charts[0]["series"][0]["values"], json!([10.0, 20.0, 15.0]));
        assert_eq!(charts[0]["series"][0]["cats"], json!(["Jan", "Feb", "Mar"]));
        let r = call(&mut s, "save", json!({}));
        assert_eq!(r["mode"], "patch");
        // reopened, the chart is in the file and still reads the cells
        let mut t = Session::new();
        call(&mut t, "open", json!({"path": path.display().to_string()}));
        let charts = call(&mut t, "charts", json!({"sheet": 0}));
        assert_eq!(charts.as_array().unwrap().len(), 1, "{charts}");
        assert_eq!(charts[0]["kind"], "column");
        assert_eq!(charts[0]["series"][0]["values"], json!([10.0, 20.0, 15.0]));
        // move it, save, reopen: the new position sticks
        let fid = charts[0]["id"].as_str().unwrap().to_string();
        call(&mut t, "chart_move", json!({"id": fid, "anchor": {"from_col": 5, "from_row": 2, "to_col": 11, "to_row": 16}}));
        call(&mut t, "save", json!({}));
        let mut u = Session::new();
        call(&mut u, "open", json!({"path": path.display().to_string()}));
        assert_eq!(call(&mut u, "charts", json!({"sheet": 0}))[0]["anchor"]["from_col"], 5);
        // delete it
        let fid = call(&mut u, "charts", json!({"sheet": 0}))[0]["id"].as_str().unwrap().to_string();
        call(&mut u, "chart_delete", json!({"id": fid}));
        call(&mut u, "save", json!({}));
        let mut v = Session::new();
        call(&mut v, "open", json!({"path": path.display().to_string()}));
        assert_eq!(call(&mut v, "charts", json!({"sheet": 0})), json!([]));
        let _ = id;
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn queue_len_matches_bitcode() {
        // the history-entry count read from IronCalc's send queue (see `queue_len`)
        let mut m = UserModel::new_empty("t", "en", "UTC", LANG).unwrap();
        for n in [0usize, 1, 3, 254, 255, 256, 1000] {
            m.flush_send_queue();
            for i in 0..n {
                m.set_user_input(0, i as i32 + 1, 1, "1").unwrap();
            }
            assert_eq!(queue_len(&m.flush_send_queue()), Some(n));
        }
    }

    fn part_names(path: &Path) -> Vec<String> {
        let bytes = std::fs::read(path).unwrap();
        ooxml::package::Package::open(&bytes).unwrap().names
    }

    fn read_part(path: &Path, part: &str) -> String {
        let bytes = std::fs::read(path).unwrap();
        ooxml::package::Package::open(&bytes).unwrap().read(part).unwrap()
    }

    fn reopen(path: &Path) -> Session {
        let mut s = Session::new();
        call(&mut s, "open", json!({"path": path.display().to_string()}));
        s
    }

    fn sheet_names(s: &mut Session) -> Vec<String> {
        call(s, "info", json!({}))["sheets"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap().to_string()).collect()
    }

    fn text(s: &mut Session, sheet: u32, row: i32, col: i32) -> String {
        let cells = call(s, "cells", json!({"sheet": sheet, "range": [row, col, row, col]}));
        cells.as_array().unwrap().iter().find(|c| c["r"] == row && c["c"] == col).map(|c| c["text"].as_str().unwrap().to_string()).unwrap_or_default()
    }

    #[test]
    fn sheet_rename_add_delete_are_patched() {
        let dir = std::env::temp_dir().join(format!("sheets-core-sheetops-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.xlsx");
        let p = || json!({"path": dir.join("s.xlsx").display().to_string()});
        let mut s = Session::new();
        call(&mut s, "paste", json!({"cell": {"sheet": 0, "row": 1, "col": 1}, "text": "Month\tSales\nJan\t10\nFeb\t20\nMar\t15\n"}));
        call(&mut s, "add_sheet", json!({}));
        call(&mut s, "set_input", json!({"cell": {"sheet": 1, "row": 1, "col": 1}, "value": "=SUM(Sheet1!B2:B4)"}));
        s.model.new_defined_name("Rate", Some(1), "Sheet2!$A$1").unwrap();
        s.model.new_defined_name("Total", None, "Sheet2!$A$1").unwrap();
        call(&mut s, "save", p());
        call(&mut s, "chart_add", json!({"range": {"sheet": 0, "row": 1, "col": 1, "width": 2, "height": 4}, "kind": "column"}));
        assert_eq!(call(&mut s, "save", json!({}))["mode"], "patch");

        // rename the data sheet: formulas on the other sheet and the chart follow it
        let mut t = reopen(&path);
        call(&mut t, "rename_sheet", json!({"sheet": 0, "name": "Sales Data"}));
        assert_eq!(call(&mut t, "charts", json!({"sheet": 0}))[0]["series"][0]["values"], json!([10.0, 20.0, 15.0]));
        let r = call(&mut t, "save", json!({}));
        assert_eq!(r["mode"], "patch", "{r}");
        let mut u = reopen(&path);
        assert_eq!(sheet_names(&mut u), ["Sales Data", "Sheet2"]);
        assert_eq!(call(&mut u, "input", json!({"cell": {"sheet": 1, "row": 1, "col": 1}})), "=SUM('Sales Data'!B2:B4)");
        assert_eq!(text(&mut u, 1, 1, 1), "45");
        let charts = call(&mut u, "charts", json!({"sheet": 0}));
        assert_eq!(charts.as_array().unwrap().len(), 1, "{charts}");
        assert_eq!(charts[0]["series"][0]["values"], json!([10.0, 20.0, 15.0]), "{charts}");
        let chart_part = part_names(&path).into_iter().find(|n| n.starts_with("xl/charts/chart")).unwrap();
        let chart = read_part(&path, &chart_part);
        assert!(chart.contains("'Sales Data'!$B$2:$B$4") && !chart.contains("Sheet1!"), "{chart}");

        // add a sheet with content
        call(&mut u, "add_sheet", json!({}));
        let added = sheet_names(&mut u)[2].clone();
        call(&mut u, "set_input", json!({"cell": {"sheet": 2, "row": 1, "col": 1}, "value": "=Sheet2!A1*2"}));
        call(&mut u, "set_input", json!({"cell": {"sheet": 2, "row": 2, "col": 2}, "value": "hello"}));
        call(&mut u, "set_style", json!({"range": {"sheet": 2, "row": 2, "col": 2}, "path": "font.b", "value": "true"}));
        let r = call(&mut u, "save", json!({}));
        assert_eq!(r["mode"], "patch", "{r}");
        let mut v = reopen(&path);
        assert_eq!(sheet_names(&mut v), ["Sales Data", "Sheet2", added.as_str()]);
        assert_eq!(text(&mut v, 2, 1, 1), "90");
        assert_eq!(text(&mut v, 2, 2, 2), "hello");
        assert_eq!(call(&mut v, "style", json!({"cell": {"sheet": 2, "row": 2, "col": 2}}))["b"], true);
        assert_eq!(call(&mut v, "charts", json!({"sheet": 0})).as_array().unwrap().len(), 1);

        // delete the middle sheet: references to it become #REF!, its part goes
        call(&mut v, "delete_sheet", json!({"sheet": 1}));
        let r = call(&mut v, "save", json!({}));
        assert_eq!(r["mode"], "patch", "{r}");
        let mut w = reopen(&path);
        assert_eq!(sheet_names(&mut w), ["Sales Data", added.as_str()]);
        // its scoped name went with it; a global one pointing at it is #REF!
        assert_eq!(w.model.get_defined_name_list(), vec![("Total".to_string(), None, "#REF!".to_string())]);
        assert_eq!(text(&mut w, 1, 1, 1), "#REF!");
        assert_eq!(text(&mut w, 1, 2, 2), "hello");
        assert_eq!(call(&mut w, "charts", json!({"sheet": 0}))[0]["series"][0]["values"], json!([10.0, 20.0, 15.0]));
        let wb = read_part(&path, "xl/workbook.xml");
        assert!(!wb.contains("Sheet2"), "{wb}");
        let rels = read_part(&path, "xl/_rels/workbook.xml.rels");
        let ct = read_part(&path, "[Content_Types].xml");
        let parts = part_names(&path);
        for line in rels.split("<Relationship ").filter(|x| x.contains("worksheet")) {
            let target = line.split("Target=\"").nth(1).unwrap().split('"').next().unwrap();
            assert!(parts.contains(&format!("xl/{target}")), "dangling {target}");
            assert!(ct.contains(&format!("/xl/{target}")), "no content type for {target}");
        }
        assert_eq!(parts.iter().filter(|n| n.starts_with("xl/worksheets/sheet")).count(), 2, "{parts:?}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn undo_keeps_saves_patched() {
        let dir = std::env::temp_dir().join(format!("sheets-core-undo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("u.xlsx");
        let mut s = Session::new();
        call(&mut s, "paste", json!({"cell": {"sheet": 0, "row": 1, "col": 1}, "text": "1\n2\n3\n=SUM(A1:A3)\n"}));
        call(&mut s, "add_sheet", json!({}));
        call(&mut s, "set_input", json!({"cell": {"sheet": 1, "row": 1, "col": 1}, "value": "=Sheet1!A4"}));
        s.model.new_defined_name("Third", None, "Sheet1!$A$3").unwrap();
        call(&mut s, "save", json!({"path": path.display().to_string()}));
        let original = std::fs::read(&path).unwrap();
        let saved = |s: &mut Session| {
            let r = call(s, "save", json!({}));
            assert_eq!(r["mode"], "patch", "{r}");
            r
        };

        // insert (which also shifts the name) -> one undo: nothing to do, the file stays byte for byte
        let mut t = reopen(&path);
        call(&mut t, "insert_rows", json!({"sheet": 0, "at": 2, "count": 1}));
        assert_eq!(t.model.get_defined_name_list()[0].2, "Sheet1!$A$4");
        call(&mut t, "undo", json!({}));
        assert_eq!(t.model.get_defined_name_list()[0].2, "Sheet1!$A$3");
        saved(&mut t);
        assert_eq!(std::fs::read(&path).unwrap(), original);

        // a four-cell paste on top of an insert: one undo takes back the whole paste
        call(&mut t, "insert_rows", json!({"sheet": 0, "at": 2, "count": 1}));
        call(&mut t, "paste", json!({"cell": {"sheet": 0, "row": 10, "col": 3}, "text": "a\tb\nc\td\n"}));
        call(&mut t, "undo", json!({}));
        assert_eq!(text(&mut t, 0, 10, 3), "");
        assert_eq!(text(&mut t, 0, 11, 4), "");
        assert!(t.ops.len() == 1, "the insert is still in");
        call(&mut t, "undo", json!({}));
        assert!(t.ops.is_empty());
        call(&mut t, "redo", json!({}));
        assert_eq!(t.ops.len(), 1);
        call(&mut t, "redo", json!({}));
        assert_eq!(text(&mut t, 0, 11, 4), "d", "the whole paste comes back in one redo");
        call(&mut t, "undo", json!({}));
        saved(&mut t);
        let mut u = reopen(&path);
        assert_eq!(call(&mut u, "input", json!({"cell": {"sheet": 0, "row": 5, "col": 1}})), "=SUM(A1:A4)");
        assert_eq!(call(&mut u, "input", json!({"cell": {"sheet": 1, "row": 1, "col": 1}})), "=Sheet1!A5");

        // undo after a save logs the inverse: the row comes back out of the file
        call(&mut t, "undo", json!({}));
        saved(&mut t);
        let mut v = reopen(&path);
        assert_eq!(call(&mut v, "input", json!({"cell": {"sheet": 0, "row": 4, "col": 1}})), "=SUM(A1:A3)");
        assert_eq!(call(&mut v, "input", json!({"cell": {"sheet": 1, "row": 1, "col": 1}})), "=Sheet1!A4");
        assert_eq!(text(&mut v, 1, 1, 1), "6");

        // sheet ops: rename and add undone before saving leave the file alone
        let before = std::fs::read(&path).unwrap();
        call(&mut v, "rename_sheet", json!({"sheet": 0, "name": "Numbers"}));
        call(&mut v, "add_sheet", json!({}));
        call(&mut v, "undo", json!({}));
        call(&mut v, "undo", json!({}));
        assert_eq!(sheet_names(&mut v), ["Sheet1", "Sheet2"]);
        saved(&mut v);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        // delete undone before saving: same
        call(&mut v, "delete_sheet", json!({"sheet": 1}));
        call(&mut v, "undo", json!({}));
        saved(&mut v);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        // rename saved, then undone and saved again: back to the old name everywhere
        call(&mut v, "rename_sheet", json!({"sheet": 0, "name": "Numbers"}));
        saved(&mut v);
        call(&mut v, "undo", json!({}));
        saved(&mut v);
        let mut w = reopen(&path);
        assert_eq!(sheet_names(&mut w), ["Sheet1", "Sheet2"]);
        assert_eq!(call(&mut w, "input", json!({"cell": {"sheet": 1, "row": 1, "col": 1}})), "=Sheet1!A4");
        // a delete can't be taken back out of a saved file by a patch
        call(&mut w, "delete_sheet", json!({"sheet": 1}));
        saved(&mut w);
        call(&mut w, "undo", json!({}));
        let r = call(&mut w, "save", json!({}));
        assert_eq!(r["needs_confirmation"], true, "{r}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn templates_dedupe_and_painter() {
        let mut s = Session::new();
        call(&mut s, "template", json!({"name": "budget"}));
        assert_eq!(call(&mut s, "input", json!({"cell": {"sheet": 0, "row": 12, "col": 2}})), "=SUM(B5:B11)");
        let mut d = Session::new();
        call(&mut d, "paste", json!({"cell": {"sheet": 0, "row": 1, "col": 1}, "text": "a\t1\nb\t2\na\t1\nc\t3\n"}));
        let n = call(&mut d, "dedupe", json!({"range": {"sheet": 0, "row": 1, "col": 1, "width": 2, "height": 4}}));
        assert_eq!(n, 1);
        assert_eq!(call(&mut d, "copy", json!({"range": {"sheet": 0, "row": 1, "col": 1, "width": 2, "height": 4}})), "a\t1\nb\t2\nc\t3\n\t");
        call(&mut d, "set_style", json!({"range": {"sheet": 0, "row": 1, "col": 1}, "path": "font.b", "value": "true"}));
        call(&mut d, "paste_formats", json!({"source": {"sheet": 0, "row": 1, "col": 1}, "target": {"sheet": 0, "row": 2, "col": 1, "width": 2, "height": 2}}));
        assert_eq!(call(&mut d, "style", json!({"cell": {"sheet": 0, "row": 3, "col": 2}}))["b"], true);
    }

    #[test]
    fn row_inserts_are_patched_and_undo_stays_a_patch() {
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
        // undo after the save: the mirror logs the delete that takes the row back out
        call(&mut s, "undo", json!({}));
        assert_eq!(call(&mut s, "save", json!({}))["mode"], "patch");
        let mut t = Session::new();
        call(&mut t, "open", json!({"path": path.display().to_string()}));
        assert_eq!(call(&mut t, "input", json!({"cell": {"sheet": 0, "row": 2, "col": 1}})), "=A1&\"!\"");
        // sheet changes are patched too
        call(&mut s, "add_sheet", json!({}));
        assert_eq!(call(&mut s, "save", json!({}))["mode"], "patch");
        // when the mirror loses track of IronCalc's history, ask before saving
        call(&mut s, "insert_rows", json!({"sheet": 0, "at": 1, "count": 1}));
        s.history.undo.push(vec![Entry::Unknown]);
        call(&mut s, "undo", json!({}));
        assert_eq!(call(&mut s, "save", json!({}))["needs_confirmation"], true);
        assert_eq!(call(&mut s, "save", json!({"allow_lossy": true}))["mode"], "native");
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

