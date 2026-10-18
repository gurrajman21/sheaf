# Sheaf

A sheaf is a bundle of sheets.

Sheaf is a desktop spreadsheet editor for .xlsx files: IronCalc does the calculating, and
the patch-in-place writer (`crates/ooxml`) saves your edits back into the
original file, so charts, pivot tables, comments and macros it doesn't edit stay intact.

## Run it

```bash
# release build: produces target/release/bundle/macos/Sheaf.app
cd apps/sheets && npm install && npx tauri build --bundles app

# development: native window with hot reload
cd apps/sheets && npx tauri dev
```

You can also pass a file: `Sheaf.app/Contents/MacOS/sheaf path/to/book.xlsx`,
drop an .xlsx onto the window, or use ⌘O.

### In a browser (for UI work and testing)

The same UI runs in a browser against the real Rust backend through a small HTTP bridge:

```bash
cargo run -p sheets-core --bin sheets_dev_server -- 7878 path/to/book.xlsx
cd apps/sheets && npx vite      # http://localhost:1420, proxies /api to :7878
```

## What works

- Open, edit and save .xlsx/.xlsm. Saving keeps everything the app doesn't edit,
  and only the cells you changed are rewritten.
- Formulas (IronCalc's ~400 functions), live recalculation, undo/redo.
- Formatting: bold/italic/underline/strikethrough, font size, text and fill
  color, alignment, wrap, number formats.
- Rows and columns: resize by dragging header edges, insert/delete, freeze panes.
- Sheets: add, rename (double-click a tab), delete (right-click a tab).
- Keyboard: arrows, ⇧+arrows, ⌘+arrows, Enter/Tab, F2, typing to edit,
  ⌘C/⌘X/⌘V, ⌘Z/⇧⌘Z, ⌘B/⌘I/⌘U, ⌘A, ⌘S/⇧⌘S/⌘O/⌘N, Delete.
- Selection summary (sum, average, count), cell address box, formula bar.
- Renders merged cells, borders, frozen panes, hidden rows/columns, text overflow.

## Known limits

- **Structural edits** (insert/delete rows or columns, add/rename/delete sheets) can't
  yet be patched into the original file. Saving after one asks before rewriting the
  file, which drops charts, pivots etc. The `refs/heads/claude/goofy-mestorf-fd3d21`
  branch adds row/column support to the patch writer; it isn't merged yet.
- Charts, images and comments are kept but not displayed. The status bar says when a
  sheet has them.
- Copy/paste moves values as text; formulas aren't adjusted when pasted. No fill
  handle, find/replace, sorting, filtering or printing yet.
- IronCalc gaps: AGGREGATE, 3D references, named constants and some legacy array
  formulas calculate wrong; about 7% of unusual files don't open.

## Layout

- `src/` React UI: `Grid.tsx` (canvas grid), `App.tsx` (shell, commands), `Toolbar.tsx`
- `src-tauri/` Tauri shell: one `api` command forwarding to `sheets_core::Session::dispatch`
- `../../crates/sheets-core` backend: workbook session, UI commands, save logic
