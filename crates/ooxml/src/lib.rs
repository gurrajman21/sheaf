//! OOXML package handling for the office suite.
//!
//! *Patch-in-place* writers: saving an edited IronCalc model back into the
//! original .xlsx so everything IronCalc doesn't model (charts, pivots,
//! comments, validation, macros, ...) survives, and the same for .docx in
//! [`docx`] (paragraph-level splicing, text/format edits, tracked changes).

pub mod charts;
pub mod docx;
pub mod inspect;
pub mod package;
pub mod patch;
pub mod refs;
pub mod sheet;
pub mod sheetops;
pub mod structural;
pub mod tables;
pub mod xml;

pub use patch::{save_patched, PatchReport};
