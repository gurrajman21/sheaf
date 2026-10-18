# Sheaf

A free, open-source desktop spreadsheet editor for Excel files.

Sheaf opens and edits `.xlsx` and `.xlsm` workbooks and saves them back without
damaging what it doesn't understand. Most editors rebuild the whole file on save,
which drops charts, pivot tables, comments or macros they can't handle. Sheaf
patches your edits into the original file instead, so everything else stays exactly
as Excel wrote it.

It runs as a native desktop app (Tauri + React) with the
[IronCalc](https://github.com/ironcalc/IronCalc) engine for calculation.

## Features

- Ribbon UI with a ⌘K command palette, a start screen with templates, and dark mode
- About 400 Excel functions with live recalculation, formula autocomplete and argument hints
- Charts from the file drawn with live data; insert, move, resize and delete charts
- Header filters, dropdown lists from data validation, sort, find and replace
- Formatting: fonts, colors, borders, alignment, merges, number formats, conditional formatting display
- Fill handle, copy/paste with formula shifting, format painter, remove duplicates, text to columns
- Insert and delete rows, columns and sheets, freeze panes, hide/unhide, print to PDF

See [apps/sheets/README.md](apps/sheets/README.md) for the full feature list and known limits.

## Install

There are no prebuilt downloads yet, so you build it from source. It takes a few
minutes the first time.

### 1. Install the prerequisites

- **Rust** (stable): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **Node.js** 18 or newer: [nodejs.org](https://nodejs.org) or `brew install node`
- **macOS:** Xcode command line tools: `xcode-select --install`
- **Windows / Linux:** the system packages listed in Tauri's
  [prerequisites guide](https://v2.tauri.app/start/prerequisites/)

### 2. Build the app

```bash
git clone https://github.com/gurrajman21/sheaf.git
cd sheaf/apps/sheets
npm install
npx tauri build --bundles app
```

The app is written to `target/release/bundle/macos/Sheaf.app` at the repository root.
On Windows or Linux, drop `--bundles app` to get that platform's installer in
`target/release/bundle/`.

### 3. Install it (macOS)

```bash
cp -R ../../target/release/bundle/macos/Sheaf.app /Applications/
```

Then open Sheaf from Spotlight, Launchpad or the Applications folder. The app is not
signed with an Apple developer certificate. If macOS says it can't be opened,
right-click Sheaf in Applications, choose **Open**, then confirm.

To make Sheaf the default for spreadsheets, right-click any `.xlsx` file in Finder,
choose **Get Info**, set **Open with** to Sheaf and click **Change All**.

To update, pull the latest code and repeat steps 2 and 3.

## Development

```bash
cd apps/sheets
npx tauri dev          # native window with hot reload
```

The UI also runs in a normal browser against the real Rust backend, which is handy
for UI work:

```bash
cargo run -p sheets-core --bin sheets_dev_server -- 7878 path/to/book.xlsx
cd apps/sheets && npx vite      # open http://localhost:1420
```

Run the tests with `cargo test --workspace` and type-check the UI with
`cd apps/sheets && npx tsc --noEmit`.

## How it's organized

| Path | What it is |
|---|---|
| `apps/sheets/` | The desktop app: React UI in `src/`, Tauri shell in `src-tauri/` |
| `crates/sheets-core/` | Workbook session and every UI command; decides how to save |
| `crates/ooxml/` | The patch-in-place `.xlsx` writer: diffs the workbook and splices only changed XML |
| `tools/fidelity/` | Test harness that measures how well files survive open, edit and save ([README](tools/fidelity/README.md)) |
| `corpus/` | Test workbooks, generated or fetched locally (not committed) |

## Contributing

Issues and pull requests are welcome. For changes to saving, please run the fidelity
harness in `tools/fidelity` before and after, and include the numbers in your pull request.

## Roadmap

Spreadsheets come first. A word processor and a presentation editor built on the same
approach are planned.

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
- [Tauri](https://tauri.app): the native desktop shell (MIT OR Apache-2.0)
- [React](https://react.dev) (MIT) and [Lucide](https://lucide.dev) icons (ISC)
- [zip](https://github.com/zip-rs/zip2) and [quick-xml](https://github.com/tafia/quick-xml)
  for reading and writing the file package (MIT)

Each dependency is distributed under its own license, which you can find in its
package. All of them are permissive (MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0 and
similar).
