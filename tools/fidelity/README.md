# Fidelity harness (Phase 0)

Compares spreadsheet engines on real .xlsx files: does a file open, do formulas
calculate the same as Excel, and what survives a save?

## Setup

```bash
python3 -m venv tools/fidelity/.venv
tools/fidelity/.venv/bin/pip install xlsxwriter openpyxl pymupdf pillow
brew install --cask libreoffice   # for the LibreOffice engine and --visual
cargo build --release             # builds target/release/ooxml_runner (IronCalc engines)
```

The harness is a development tool and is not part of the Sheaf app. Some of what it
installs uses other licenses: PyMuPDF (for the `--visual` diff) is AGPL-3.0, and
LibreOffice is MPL-2.0. Neither is bundled with or linked into Sheaf, so they don't
affect Sheaf's own license.

## Engines

| Engine | What it is |
|---|---|
| `ironcalc` | IronCalc 0.9 with its own xlsx writer (regenerates the whole file) |
| `ironcalc-patch` | IronCalc 0.9 + our patch-in-place writer (`crates/ooxml`): only changed cells are rewritten, every other part is copied from the original |
| `libreoffice` | LibreOffice headless, driven by an in-process Python macro |

On macOS, LibreOffice's bundled Python is signed so only LibreOffice can launch
it. Running it from a shell is killed by AMFI with "Launch Constraint Violation",
exit 137. So `engines/lo_runner.py` runs as a Python macro inside soffice:
`run.py` copies it into a throwaway profile per worker and sends it jobs through a
folder. You never run LibreOffice's Python directly.

## Corpus

| Folder | What | Committed? |
|---|---|---|
| `corpus/synthetic/` | One file per feature, from `gen_synthetic.py`. Formula files carry known answers. | No (regenerate) |
| `corpus/public/` | Apache POI + LibreOffice regression files, via `fetch_public.py`, pinned in `SOURCES.lock` | Only the lock file |
| `corpus/real/` | Your own files: drop .xlsx/.xlsm here | Never |

```bash
cd tools/fidelity
.venv/bin/python gen_synthetic.py
.venv/bin/python fetch_public.py
```

## Run

```bash
.venv/bin/python run.py                                   # everything, all three engines
.venv/bin/python run.py --corpus synthetic,real --visual  # + PDF pixel diff via LibreOffice
.venv/bin/python run.py --edits --visual                  # same scripted edits through every engine
.venv/bin/python run.py --edits --insert-rows --visual    # ... starting with a 2-row insert mid-data
.venv/bin/python run.py --engines ironcalc-patch --jobs 8
.venv/bin/python report.py ../../results/<run> [../../results/<other-run>]
```

## What's measured

- **Opens / saves / crashes / timeouts**: each engine runs in a subprocess (120–180s limit).
- **Formula cells correct**: the engine recalculates, and results are compared with the
  values Excel cached in the file. Only files Excel wrote (detected via `calcChain.xml`
  + app name) or synthetic files with known answers count. Volatile functions are skipped.
- **Round-trip fidelity** (raw XML, engine-independent): constants, formula presence,
  resolved styles (number format, font, fill, border, alignment), merged cells, column
  widths, row heights, hidden rows/cols, frozen panes, sheet names/states, defined
  names, and a feature inventory (charts, images, pivots, tables, comments, conditional
  formatting, data validation, sparklines, VBA, ...).
- **Edits** (`--edits`): every engine applies the same script per file: change a number,
  a text cell and a cell's font, type a number over a formula (a shared-formula master
  when there is one), and add a dependent formula and a new text cell. Checked: each edit
  landed (with a correct cached value for the formula), nothing else changed (edited cells
  are excluded from the fidelity comparison), and no shared formulas were orphaned.
- **Row insert** (`--edits --insert-rows`): the script first inserts two rows in the middle of
  the edited sheet's data (below the header and frozen rows, never splitting an array formula);
  the cell edits after it are addressed post-insert. Fidelity is then scored against the
  original with rows below the insertion point moved down, so every value, formula, style,
  merge, row height and feature must have moved with its cell. The insert itself counts as
  an edit: the new rows must exist and be empty.
- **Reload check** (`ironcalc-patch`): the runner re-opens its saved file in IronCalc,
  recalculates, and compares every formula's value with the edited model's. A reference the
  patch writer shifted wrongly shows up here. Mismatches can also be IronCalc's own model
  being wrong (it re-enters moved text cells as input, so a string like `=x` becomes a
  formula; it doesn't move table ranges inside already-parsed structured references).
- **Visual** (`--visual`): LibreOffice renders original and round-trip to PDF; first 5 pages pixel-diffed.

## Known biases

- LibreOffice's own test files favor LibreOffice. The report breaks results down by source.
- XlsxWriter synthetic files aren't real Excel output. Real files in `corpus/real/` matter most.
- Built-in date formats (e.g. id 14) are locale-dependent and can show as numfmt drift.

## Files

- `ooxml.py`: raw .xlsx reader (cells, cached values, styles, layout, inventory)
- `compare.py`: formula scoring and original-vs-round-trip comparison
- `../../crates/ooxml/src/bin/ooxml_runner.rs`: IronCalc runner (native or patch mode), JSON out
- `engines/lo_runner.py`: LibreOffice job-server macro
- `rescore.py`: recompute fidelity for a finished run after changing `compare.py`
- `run.py`: parallel driver, one headless soffice per worker
- `report.py`: `summary.md` scorecard

# .docx harness (Phase D0)

Same idea for Word files: which engine opens them, lays them out like Word, and saves
edits without damaging anything else?

## Setup

```bash
tools/fidelity/.venv/bin/pip install lxml
cd tools/fidelity
.venv/bin/python fetch_public_docx.py        # POI, LibreOffice Writer and python-docx test files
.venv/bin/python gen_synthetic_docx.py       # one file per Word feature
git clone --depth 1 https://gitlab.com/Ryujiyasu/oxi.git .engines/oxi   # Oxi source (path dependency)
(cd docx_bench && cargo build --release)     # Rust runner for rdocx (crates.io) and Oxi
```

## Engines

| Engine | What it is |
|---|---|
| `rdocx` | rdocx 0.16 (MIT/Apache): own model, native layout, keeps unknown XML |
| `oxi` | Oxi (MPL-2.0): Rust layout engine, patches edits into the original zip |
| `oxi+hints` | Oxi laying out the original file, *with* Word's cached page breaks (see below) |
| `libreoffice` | LibreOffice Writer, via the in-process macro `engines/lo_docx_runner.py` |
| `sheaf` | Sheaf's patch-in-place writer (`crates/ooxml/src/docx`) for saving, Oxi for layout. Edited files are reopened in rdocx, Oxi and LibreOffice |
| `sheaf-track` | Same edits recorded as tracked changes (`w:ins`/`w:del`/`w:rPrChange`) |
| `sheaf-accept` / `sheaf-reject` | Accept / reject every tracked change (files that have any). Checked: none left, and the text equals Word's "No Markup" / "Original" view of the original |
| `sheaf-stories` | Append to the first header, add a footnote, make the edit paragraph a numbered list item, insert an inline picture |

## Ground truth without Word

- Word-saved files: `docProps/app.xml` `<Pages>` is Word's page count, and
  `<w:lastRenderedPageBreak/>` marks where Word broke each page. When both agree
  (pages = markers + 1) the file is *trusted* and every page start is checked.
- Synthetic files with only hard breaks list their pages in `corpus/docx/synthetic/expected.json`.
- **Blind layout:** Oxi reads `lastRenderedPageBreak` as a layout input, so every engine lays
  out a copy with those markers removed. `oxi+hints` shows the score with them.

## Run

```bash
.venv/bin/python run_docx.py --jobs 8                      # rdocx, Oxi, LibreOffice
.venv/bin/python run_docx.py --engines sheaf,sheaf-track   # Sheaf's own writer
.venv/bin/python run_docx.py --engines sheaf-accept,sheaf-reject,sheaf-stories
.venv/bin/python run_docx.py --corpus synthetic --keep     # keep each engine's output files
.venv/bin/python report_docx.py ../../results/docx-<run>   # summary.md scorecard
```

## What's measured

- **Opens**, crashes and timeouts (every engine runs in its own process).
- **Page count** vs Word, and **page starts**: the first 12 non-space characters of each page
  must match the text after Word's page break.
- **Round trip** (open, save, no edits): every top-level body element compared as exclusive C14N
  with rsids, `proofErr` and cached page breaks removed; other parts byte-equal or C14N-equal;
  dropped parts and relationships; a feature inventory (tables, drawings, fields, comments,
  tracked changes, footnotes, content controls, equations, ...).
- **Edits**: bold the first run of a plain paragraph, append text to it, insert a paragraph
  after it, retype the first table's first cell. Each must land, and everything else must
  survive as in the round trip.

## Files

- `docx_xml.py`: text in reading order, Word's hints, inventory, canonical XML
- `docx_compare.py`: page matching, edit target and checks, survival
- `docx_bench/`: Rust runner (`docx_bench <job.json>`), its own cargo workspace
- `engines/lo_docx_runner.py`: LibreOffice Writer job-server macro
- `run_docx.py`, `report_docx.py`: driver and scorecard
