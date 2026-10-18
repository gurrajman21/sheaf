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
