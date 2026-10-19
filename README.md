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

- Real pages with Word-accurate layout; edit body text, tables, headers, footers, footnotes and text boxes
- Comments with replies, track changes with per-change accept/reject, spell check
- Styles, lists, spacing, format painter, find and replace, copy/paste with formatting
- Page setup, table tools, table of contents, footnotes, page numbers, links, headings pane
- Slash menu for quick inserts, autosave with crash recovery, version history, print to PDF

See [apps/docs/README.md](apps/docs/README.md) for details and known limits.

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
| `corpus/` | Test workbooks, generated or fetched locally (not committed) |

## Contributing

Issues and pull requests are welcome. For changes to saving, please run the fidelity
harness in `tools/fidelity` before and after, and include the numbers in your pull request.

## Roadmap

Spreadsheets (Sheaf) came first, the word processor (Quire) second. A presentation editor
built on the same approach is planned.

## License

Sheaf is licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution you intentionally submit for
inclusion in Sheaf, as defined in the Apache-2.0 license, is dual licensed as above,
without any additional terms or conditions.

## Acknowledgments

Sheaf is built on the work of other open-source projects, including:

- [IronCalc](https://github.com/ironcalc/IronCalc): the spreadsheet engine that parses
  and calculates workbooks (MIT OR Apache-2.0)
- [Oxi](https://gitlab.com/Ryujiyasu/oxi): the document layout engine behind Quire's pages
  (MPL-2.0, used unmodified as a dependency)
- [Tauri](https://tauri.app): the native desktop shell (MIT OR Apache-2.0)
- [React](https://react.dev) (MIT) and [Lucide](https://lucide.dev) icons (ISC)
- [zip](https://github.com/zip-rs/zip2) and [quick-xml](https://github.com/tafia/quick-xml)
  for reading and writing the file package (MIT)

Each dependency is distributed under its own license, which you can find in its
package. All of them are permissive (MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0 and
similar).
