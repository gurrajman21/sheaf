//! Harness runner: load an xlsx with IronCalc, apply scripted edits, evaluate,
//! and save either with IronCalc's own writer (`native`) or the patch-in-place
//! writer (`patch`). Prints one JSON object (same shape as the Python runners).
//!
//! Usage: ooxml_runner <in.xlsx> <out.xlsx> [--mode patch|native] [--edits edits.json] [--reload-check]
//!
//! `--reload-check` (patch mode) re-opens the saved file and compares every formula's value
//! with the edited model's.
//!
//! Edits file: [{"op": "number"|"text"|"formula"|"bold", "sheet": "Sheet1", "cell": "B3", "value": ...}]
//! Structural edits: {"op": "insert_rows"|"delete_rows"|"insert_cols"|"delete_cols", "sheet": "Sheet1",
//! "at": 5 (row number, or column number/letters), "count": 2}. Edits apply in order, so cells in
//! later edits are addressed after the shift.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Instant;

use ironcalc_base::types::{Cell, FormulaValue};
use ironcalc_base::Model;
use ooxml::sheet::{col_name, parse_ref};
use ooxml::structural::StructuralOp;
use serde_json::{json, Map, Value};

struct Run {
    out: Map<String, Value>,
    stages: Map<String, Value>,
}

impl Run {
    fn stage<T>(&mut self, name: &str, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        let t = Instant::now();
        let res = match catch_unwind(AssertUnwindSafe(f)) {
            Ok(r) => r,
            Err(p) => Err(format!(
                "PanicException: {}",
                p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()
            )),
        };
        let ms = (t.elapsed().as_secs_f64() * 10000.0).round() / 10.0;
        self.stages.insert(name.into(), json!({"ok": res.is_ok(), "ms": ms}));
        res.map_err(|e| format!("{name}: {e}"))
    }
}

fn apply_edits(model: &mut Model, edits: &[Value], ops: &mut Vec<StructuralOp>) -> Vec<String> {
    let mut errors = Vec::new();
    for e in edits {
        let res = (|| -> Result<(), String> {
            let sheet_name = e["sheet"].as_str().ok_or("edit without sheet")?;
            let sheet = model
                .workbook
                .worksheets
                .iter()
                .position(|w| w.name == sheet_name)
                .ok_or(format!("no sheet {sheet_name}"))? as u32;
            let op = e["op"].as_str().unwrap_or("");
            if op.starts_with("insert_") || op.starts_with("delete_") {
                let at = match &e["at"] {
                    Value::Number(n) => n.as_i64().ok_or("bad at")? as i32,
                    Value::String(c) => parse_ref(&format!("{c}1")).ok_or("bad column")?.1,
                    _ => return Err("structural edit without at".into()),
                };
                let count = e["count"].as_i64().unwrap_or(1) as i32;
                let s = serde_json::from_value::<StructuralOp>(json!({"op": op, "sheet": sheet, "at": at, "count": count}))
                    .map_err(|err| err.to_string())?;
                s.apply(model)?;
                ops.push(s);
                return Ok(());
            }
            let (row, col) = parse_ref(e["cell"].as_str().unwrap_or("")).ok_or("bad cell ref")?;
            match op {
                "number" => model.update_cell_with_number(sheet, row, col, e["value"].as_f64().ok_or("number")?),
                "text" => model.update_cell_with_text(sheet, row, col, e["value"].as_str().ok_or("text")?),
                "formula" => model.update_cell_with_formula(sheet, row, col, e["value"].as_str().ok_or("formula")?.to_string()),
                "bold" => {
                    let mut style = model.get_style_for_cell(sheet, row, col)?;
                    style.font.b = true;
                    model.set_cell_style(sheet, row, col, &style)
                }
                op => Err(format!("unknown op {op}")),
            }
        })();
        if let Err(msg) = res {
            errors.push(format!("{}: {msg}", e.get("cell").unwrap_or(&e["op"])));
        }
    }
    errors
}

fn formula_values(model: &Model) -> Map<String, Value> {
    let mut out = Map::new();
    for ws in &model.workbook.worksheets {
        for (r, c, cell) in ws.sheet_data.cells() {
            let v = match cell.as_ref() {
                Cell::CellFormula { v, .. } | Cell::ArrayFormula { v, .. } => v,
                _ => continue,
            };
            let val = match v {
                FormulaValue::Number(n) => json!({"t": "n", "v": n}),
                FormulaValue::Text(s) => json!({"t": "s", "v": s}),
                FormulaValue::Boolean(b) => json!({"t": "b", "v": b}),
                FormulaValue::Error { ei, .. } => json!({"t": "e", "v": ei.to_string()}),
                FormulaValue::Unevaluated => json!({"t": "empty", "v": null}),
            };
            out.insert(format!("{}!{}{r}", ws.name, col_name(c)), val);
        }
    }
    out
}

/// Re-open the saved file and recalculate: every formula should give what the edited
/// model gave. A wrongly shifted reference (or a broken cell) shows up here.
fn reload_check(saved: &[u8], work: &Model, expected: &Map<String, Value>) -> Value {
    let reloaded = ironcalc::import::load_from_xlsx_bytes(saved, "workbook", "en", "UTC")
        .map_err(|e| format!("{e:?}"))
        .and_then(|wb| Model::from_workbook(wb, "en"));
    let mut m = match reloaded {
        Ok(m) => m,
        Err(e) => return json!({"error": e.chars().take(300).collect::<String>()}),
    };
    m.evaluate();
    let got = formula_values(&m);
    let volatile = |key: &str| -> bool {
        let Some((sheet, cell)) = key.rsplit_once('!') else { return false };
        let Some(i) = work.workbook.worksheets.iter().position(|w| w.name == sheet) else { return false };
        let Some((r, c)) = parse_ref(cell) else { return false };
        let f = work.get_cell_formula(i as u32, r, c).ok().flatten().unwrap_or_default().to_uppercase();
        ["RAND", "NOW(", "TODAY(", "OFFSET(", "INDIRECT(", "CELL(", "INFO("].iter().any(|v| f.contains(v))
    };
    let same = |a: &Value, b: &Value| match (a["v"].as_f64(), b["v"].as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() <= 1e-9 * x.abs().max(1.0),
        _ => a == b,
    };
    let (mut checked, mut bad) = (0, 0);
    let mut samples = Vec::new();
    for (k, v) in expected {
        if volatile(k) {
            continue;
        }
        checked += 1;
        let g = got.get(k);
        if !g.is_some_and(|g| same(v, g)) {
            bad += 1;
            if samples.len() < 5 {
                samples.push(json!({"cell": k, "expected": v, "got": g}));
            }
        }
    }
    let extra = got.keys().filter(|k| !expected.contains_key(*k)).count();
    json!({"formulas": checked, "mismatched": bad, "extra": extra, "samples": samples})
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let (src, dst) = (args[1].clone(), args[2].clone());
    let mode = flag("--mode").unwrap_or_else(|| "patch".into());
    let edits: Vec<Value> = flag("--edits")
        .map(|p| serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_default()).unwrap_or_default())
        .unwrap_or_default();

    let mut run = Run { out: Map::new(), stages: Map::new() };
    let engine = if mode == "native" { "ironcalc" } else { "ironcalc-patch" };
    run.out.insert("engine".into(), json!(engine));
    run.out.insert("version".into(), json!("ironcalc 0.9.0"));

    let result = (|| -> Result<(), String> {
        let bytes = std::fs::read(&src).map_err(|e| e.to_string())?;
        let workbook = run.stage("load", || {
            ironcalc::import::load_from_xlsx_bytes(&bytes, "workbook", "en", "UTC").map_err(|e| format!("{e:?}"))
        })?;
        // The baseline (same file, evaluated, never edited) is what the patch writer diffs against.
        let mut base = if mode == "patch" {
            Some(Model::from_workbook(workbook.clone(), "en")?)
        } else {
            None
        };
        let mut work = Model::from_workbook(workbook, "en")?;
        let mut ops = Vec::new();
        let edit_errors = run.stage("edits", || Ok(apply_edits(&mut work, &edits, &mut ops)))?;
        if !edit_errors.is_empty() {
            run.out.insert("edit_errors".into(), json!(edit_errors));
        }
        run.stage("evaluate", || {
            work.evaluate();
            if let Some(b) = base.as_mut() {
                b.evaluate();
            }
            Ok(())
        })?;
        let values = run.stage("read", || Ok(formula_values(&work)))?;
        let saved = run.stage("save", || match &base {
            Some(b) => {
                let (out, report) = ooxml::patch::save_patched_ops(&bytes, b, &work, &ops)?;
                run_report(&report);
                std::fs::write(&dst, &out).map_err(|e| e.to_string())?;
                Ok(Some(out))
            }
            None => ironcalc::export::save_to_xlsx(&work, &dst).map(|_| None).map_err(|e| format!("{e:?}")),
        })?;
        if let Some(out) = saved {
            if args.iter().any(|a| a == "--reload-check") {
                let check = run.stage("reload", || Ok(reload_check(&out, &work, &values)))?;
                run.out.insert("reload".into(), check);
            }
        }
        run.out.insert("values".into(), Value::Object(values));
        Ok(())
    })();
    run.out.insert("stages".into(), Value::Object(run.stages));
    match result {
        Ok(()) => {
            run.out.insert("ok".into(), json!(true));
        }
        Err(e) => {
            run.out.insert("ok".into(), json!(false));
            run.out.insert("error".into(), json!(e.chars().take(800).collect::<String>()));
        }
    }
    if let Some(r) = REPORT.with(|r| r.borrow_mut().take()) {
        run.out.insert("patch".into(), r);
    }
    println!("{}", Value::Object(run.out));
}

thread_local! {
    static REPORT: std::cell::RefCell<Option<Value>> = const { std::cell::RefCell::new(None) };
}

fn run_report(report: &ooxml::PatchReport) {
    REPORT.with(|r| *r.borrow_mut() = serde_json::to_value(report).ok());
}
