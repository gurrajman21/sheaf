//! OOXML package handling for the office suite.
//!
//! Today this is the spreadsheet *patch-in-place* writer: saving an edited
//! IronCalc model back into the original .xlsx so everything IronCalc doesn't
//! model (charts, pivots, comments, validation, macros, ...) survives.

pub mod inspect;
pub mod package;
pub mod patch;
pub mod refs;
pub mod sheet;
pub mod structural;
pub mod tables;
pub mod xml;

pub use patch::{save_patched, PatchReport};
