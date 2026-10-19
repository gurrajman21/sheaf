
docs_core::fast_oxi_env!();
fn main() {
    let path = std::env::args().nth(1).expect("file");
    let bytes = std::fs::read(&path).unwrap();
    let doc = oxidocs_core::parse_docx(&bytes).unwrap();
    for _ in 0..200 {
        let _ = oxidocs_core::layout::LayoutEngine::for_document(&doc).layout(&doc);
    }
}
