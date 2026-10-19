docs_core::fast_oxi_env!();
fn main() {
    let path = std::env::args().nth(1).expect("file");
    let want: Vec<String> = std::env::args().skip(2).collect();
    let bytes = std::fs::read(&path).unwrap();
    let doc = oxidocs_core::parse_docx(&bytes).unwrap();
    let res = oxidocs_core::layout::LayoutEngine::for_document(&doc).layout(&doc);
    for (pi, p) in res.pages.iter().enumerate() {
        for e in &p.elements {
            if let oxidocs_core::layout::LayoutContent::Text { text, .. } = &e.content {
                if want.is_empty() || want.iter().any(|w| text.contains(w.as_str())) {
                    println!("p{pi} {:?} y={:.0} para={:?} run={:?} off={:?} cont={:?} row={:?} col={:?} cpi={:?} anc={:?} prefix={:?}",
                        text, e.y, e.paragraph_index, e.run_index, e.char_offset, e.source_container_index, e.cell_row_index, e.cell_col_index, e.cell_paragraph_index, e.cell_ancestor_path, e.source_paragraph_prefix.as_deref().map(|s| s.chars().take(20).collect::<String>()));
                }
            }
        }
    }
}
