//! Where layout time goes: `cargo run --release -p docs-core --example layout_timing -- file.docx`

docs_core::fast_oxi_env!();
use std::time::Instant;
fn main() {
    let path = std::env::args().nth(1).expect("file");
    let bytes = std::fs::read(&path).unwrap();
    for round in 0..3 {
        let t = Instant::now();
        let doc = oxidocs_core::parse_docx(&bytes).unwrap();
        let parse = t.elapsed();
        let t = Instant::now();
        let res = oxidocs_core::layout::LayoutEngine::for_document(&doc).layout(&doc);
        let layout = t.elapsed();
        let t = Instant::now();
        let s = docs_core::Session::new();
        let _ = s;
        println!("round {round}: parse {:?} layout {:?} pages {} (session {:?})", parse, layout, res.pages.len(), t.elapsed());
    }
}
