//! Harness runner: load an xlsx with IronCalc, apply scripted edits, evaluate,
//! and save either with IronCalc's own writer (`native`) or the patch-in-place
//! writer (`patch`). Prints one JSON object (same shape as the Python runners).
//!
//! Usage: ooxml_runner <in.xlsx> <out.xlsx> [--mode patch|native] [--edits edits.json]
//!
//! Edits file: [{"op": "number"|"text"|"formula"|"bold", "sheet": "Sheet1", "cell": "B3", "value": ...}]

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Instant;

use ironcalc_base::types::{Cell, FormulaValue};
use ironcalc_base::Model;
use ooxml::sheet::{col_name, parse_ref};
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

fn apply_edits(model: &mut Model, edits: &[Value]) -> Vec<String> {
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
            let (row, col) = parse_ref(e["cell"].as_str().unwrap_or("")).ok_or("bad cell ref")?;
            match e["op"].as_str().unwrap_or("") {
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
            errors.push(format!("{}: {msg}", e["cell"]));
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
        let edit_errors = run.stage("edits", || Ok(apply_edits(&mut work, &edits)))?;
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
        run.out.insert("values".into(), Value::Object(values));
        run.stage("save", || match &base {
            Some(b) => {
                let (out, report) = ooxml::save_patched(&bytes, b, &work)?;
                run_report(&report);
                std::fs::write(&dst, out).map_err(|e| e.to_string())
            }
            None => ironcalc::export::save_to_xlsx(&work, &dst).map_err(|e| format!("{e:?}")),
        })?;
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
