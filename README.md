# Sheaf

Free, open-source desktop editors for Office files that don't break them.

- **Sheaf** edits Excel workbooks (`.xlsx`, `.xlsm`), calculated by the
  [IronCalc](https://github.com/ironcalc/IronCalc) engine.
- **Quire** edits Word documents (`.docx`) on real pages laid out by the
  [Oxi](https://gitlab.com/Ryujiyasu/oxi) engine.

Most editors rebuild the whole file on save, which drops charts, pivot tables,
comments, tracked changes or macros they can't handle. Both apps patch your edits into
the original file instead, so everything else stays exactly as Office wrote it. They run
as native desktop apps (Tauri + React) and share one look.

## Features

### Sheaf (spreadsheets)

- Ribbon UI with a ⌘K command palette, a start screen with templates, and dark mode
- About 400 Excel functions with live recalculation, formula autocomplete and argument hints
- Charts from the file drawn with live data; insert, move, resize and delete charts
- Header filters, dropdown lists from data validation, sort, find and replace
- Formatting: fonts, colors, borders, alignment, merges, number formats, conditional formatting display
- Fill handle, copy/paste with formula shifting, format painter, remove duplicates, text to columns
- Insert and delete rows, columns and sheets, freeze panes, hide/unhide, print to PDF

See [apps/sheets/README.md](apps/sheets/README.md) for the full feature list and known limits.

### Quire (documents)

- Real pages with Word-accurate layout: fonts, tables, images, columns, sections,
  headers, footers, footnotes and text boxes, in light or dark mode
- Edit anywhere on the page: body text, table cells (nested too), headers, footers,
  footnotes, endnotes and text boxes
- Fonts, colors, highlight, paragraph styles, alignment, lists, indents, line and
  paragraph spacing, format painter, copy/paste with formatting to and from other apps
- Page setup (size, orientation, margins, columns), table tools (rows, columns, merge,
  shading), table of contents, footnotes, page numbers, hyperlinks, headings pane
- Comments with replies and resolve, track changes saved as real Word revisions, accept
  or reject one change or all
- Spell check with suggestions, find and replace, a `/` slash menu for quick inserts
- Autosave with crash recovery, version history, print and save as PDF

#### How Quire works

- **Layout:** Oxi, a Rust engine that aims to match Word's line breaking and pagination,
  lays out every page. Quire draws the result on a canvas and maps clicks and the caret back
  to positions in the document.
- **Saving:** Quire doesn't convert the document to its own model. Each edit is spliced into
  the original XML, at the paragraph, run or table cell it changes. Parts it never touches
  stay byte for byte as Word wrote them: charts, fields, SmartArt, content controls, custom
  XML and macros.
- **Speed:** after an edit, only the pages from the last clean page break onward are laid
  out again. A full layout runs when you pause typing.
- **Fonts:** metric-compatible replacements for Calibri, Cambria, Arial, Times New Roman and
  Courier New are bundled, so line breaks match Word on machines without Microsoft's fonts.
- **Fidelity is measured:** the harness in `tools/fidelity` opens, edits and saves a corpus
  of real and generated `.docx` files. Saved files are checked by reopening them in Oxi,
  rdocx and LibreOffice, and tracked-change accept/reject is compared against expected
  results.

See [apps/docs/README.md](apps/docs/README.md) for the full feature list, keyboard
shortcuts and known limits.

## Install

There are no prebuilt downloads yet, so you build it from source. It takes a few
minutes the first time.

### 1. Install the prerequisites

- **Rust** (stable): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **Node.js** 18 or newer: [nodejs.org](https://nodejs.org) or `brew install node`
- **macOS:** Xcode command line tools: `xcode-select --install`
- **Windows / Linux:** the system packages listed in Tauri's
  [prerequisites guide](https://v2.tauri.app/start/prerequisites/)

### 2. Build the apps

```bash
git clone https://github.com/gurrajman21/sheaf.git
cd sheaf/apps/sheets && npm install && npx tauri build --bundles app
cd ../docs && npm install && npx tauri build --bundles app
```

The apps are written to `target/release/bundle/macos/Sheaf.app` and `Quire.app` at the
repository root. Quire's first build also downloads and compiles Oxi, which takes a few
minutes.
On Windows or Linux, drop `--bundles app` to get that platform's installer in
`target/release/bundle/`.

### 3. Install it (macOS)

```bash
cp -R ../../target/release/bundle/macos/Sheaf.app ../../target/release/bundle/macos/Quire.app /Applications/
```

Then open them from Spotlight, Launchpad or the Applications folder. The apps are not
signed with an Apple developer certificate. If macOS says one can't be opened,
right-click it in Applications, choose **Open**, then confirm.

To make them the defaults, right-click an `.xlsx` (or `.docx`) file in Finder, choose
**Get Info**, set **Open with** to Sheaf (or Quire) and click **Change All**.

To update, pull the latest code and repeat steps 2 and 3.

## Development

```bash
cd apps/sheets && npx tauri dev    # or apps/docs: native window with hot reload
```

Both UIs also run in a normal browser against the real Rust backend, which is handy
for UI work:

```bash
cargo run -p sheets-core --bin sheets_dev_server -- 7878 path/to/book.xlsx
cd apps/sheets && npx vite      # open http://localhost:1420

cargo run --release -p docs-core --bin docs_dev_server -- 7879 path/to/doc.docx
cd apps/docs && npx vite        # open http://localhost:1421
```

Run the tests with `cargo test --workspace` and type-check the UI with
`cd apps/sheets && npx tsc --noEmit`.

## How it's organized

| Path | What it is |
|---|---|
| `apps/sheets/` | Sheaf: React UI in `src/`, Tauri shell in `src-tauri/` |
| `apps/docs/` | Quire: canvas page view and editor UI in `src/`, Tauri shell in `src-tauri/` |
| `crates/sheets-core/` | Workbook session and every spreadsheet command; decides how to save |
| `crates/docs-core/` | Document session, editing commands, undo, page layout through Oxi |
| `crates/ooxml/` | The patch-in-place writers: `.xlsx` (diff and splice changed cells) and `.docx` (`docx/`: paragraph splicing, text and formatting edits, tracked changes, stories) |
| `tools/fidelity/` | Test harnesses that measure how well `.xlsx` and `.docx` files survive open, edit and save ([README](tools/fidelity/README.md)) |
| `corpus/` | Test workbooks and documents, generated or fetched locally (only the source lock files are committed) |

## Contributing

Issues and pull requests are welcome. For changes to saving, please run the fidelity
harness in `tools/fidelity` before and after, and include the numbers in your pull request.

## Roadmap

Spreadsheets (Sheaf) came first, the word processor (Quire) second. A presentation editor
built on the same approach is planned.

## License

Sheaf and Quire are licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option. This covers all code in this repository: both apps, `crates/` and
`tools/`.

Unless you explicitly state otherwise, any contribution you intentionally submit for
inclusion in this project, as defined in the Apache-2.0 license, is dual licensed as
above, without any additional terms or conditions.

### Third-party components bundled with Quire

Quire's app includes some files that are not covered by the license above. Each keeps its
own license, and its license text ships next to it:

| Component | Where | License |
|---|---|---|
| Carlito and Caladea fonts (metric-compatible with Calibri and Cambria) | `apps/docs/public/fonts/` | SIL Open Font License 1.1 (`OFL-Carlito.txt`, `OFL-Caladea.txt`) |
| Liberation Sans, Serif and Mono fonts (metric-compatible with Arial, Times New Roman and Courier New) | `apps/docs/public/fonts/` | SIL Open Font License 1.1 (`LICENSE-Liberation.txt`) |
| en_US Hunspell dictionary from SCOWL | `crates/docs-core/dict/` | SCOWL's permissive license (`README_en_US.txt`) |

Two of Quire's Rust dependencies, Oxi and spellbook, are under the Mozilla Public License
2.0. The MPL's copyleft applies only to those libraries' own source files. Quire uses them
unmodified as dependencies fetched from their upstream repositories, so it doesn't change
the license of Quire's code. Anyone redistributing a Quire build must keep the MPL-2.0
libraries' source available, which their public repositories already provide.

## Acknowledgments

Sheaf and Quire are built on the work of other open-source projects.

**Spreadsheets (Sheaf)**

- [IronCalc](https://github.com/ironcalc/IronCalc): the spreadsheet engine that parses and
  calculates workbooks (MIT OR Apache-2.0)

**Documents (Quire)**

- [Oxi](https://gitlab.com/Ryujiyasu/oxi) (`oxidocs-core`): the document layout engine
  behind Quire's pages (MPL-2.0, used unmodified, pinned to a known commit)
- [spellbook](https://github.com/helix-editor/spellbook): Hunspell-compatible spell
  checking (MPL-2.0)
- [SCOWL and the en_US Hunspell dictionary](http://wordlist.aspell.net) by Kevin Atkinson
  and contributors: the word list behind spell check
- [Carlito](https://github.com/googlefonts/carlito) and
  [Caladea](https://github.com/huertatipografica/Caladea) by tyPoland and Huerta
  Tipográfica, and the [Liberation fonts](https://github.com/liberationfonts) by Red Hat:
  bundled fonts (OFL-1.1)
- [base64](https://github.com/marshallpierce/rust-base64) for images and clipboard data, and
  [libc](https://github.com/rust-lang/libc) (MIT OR Apache-2.0)

**Both apps**

- [Tauri](https://tauri.app) and its dialog, clipboard-manager and opener plugins: the
  native desktop shell (MIT OR Apache-2.0)
- [React](https://react.dev) (MIT), [Vite](https://vite.dev) (MIT) and
  [TypeScript](https://www.typescriptlang.org) (Apache-2.0)
- [Lucide](https://lucide.dev) icons (ISC)
- [zip](https://github.com/zip-rs/zip2) and [quick-xml](https://github.com/tafia/quick-xml)
  for reading and writing the file package (MIT)
- [serde_json](https://github.com/serde-rs/json) and
  [tiny_http](https://github.com/tiny-http/tiny-http) for the in-browser development servers
  (MIT OR Apache-2.0)

**Testing only (not part of the apps)**

The fidelity harness in `tools/fidelity` compares against and renders with
[LibreOffice](https://www.libreoffice.org) (MPL-2.0),
[rdocx](https://crates.io/crates/rdocx) (MIT OR Apache-2.0),
[Apache POI](https://poi.apache.org) and python-docx test files, and Python libraries
including [PyMuPDF](https://pymupdf.readthedocs.io) (AGPL-3.0), openpyxl, XlsxWriter and
Pillow. They're installed separately on the developer's machine. None of them is bundled
with or linked into Sheaf or Quire.

Each dependency is distributed under its own license, which you can find in its
package. The libraries the apps ship with are under permissive licenses (MIT,
Apache-2.0, BSD, ISC, Zlib, OFL) or MPL-2.0.
