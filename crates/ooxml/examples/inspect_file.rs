//! Print the charts and validations ooxml finds in a workbook: `cargo run --example inspect_file -- book.xlsx`
fn main() {
    let path = std::env::args().nth(1).expect("path");
    let bytes = std::fs::read(&path).expect("read");
    for c in ooxml::charts::read_charts(&bytes).expect("charts") {
        println!("chart {} #{} {} {:?} title={:?} series={} anchor={:?}", c.sheet, c.index, c.kind, c.grouping, c.title, c.series.len(), c.anchor.map(|a| (a.from_col, a.from_row, a.to_col, a.to_row)));
        if let Some(s) = c.series.first() { println!("   first series: {:?} cat={:?} val={:?}", s.name, s.cat_ref, s.val_ref); }
    }
    for (sheet, rules) in ooxml::inspect::validations(&bytes).expect("validations") {
        for r in rules { println!("validation {sheet} {} {} {}", r.kind, r.sqref, r.formula1); }
    }
}
