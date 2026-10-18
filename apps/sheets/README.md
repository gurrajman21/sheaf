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

- **Files:** open, edit and save .xlsx/.xlsm. Saving rewrites only what changed: cells,
  merges, column widths, row heights, hidden rows/columns, frozen panes, row/column
  inserts and deletes, sheet add/rename/delete and chart edits are patched into the
  original, so charts, pivots, comments and macros stay.
- **Native Mac app:** full menu bar with shortcuts, native right-click menus, translucent
  window chrome, dark mode with a dark grid, print / save as PDF (⌘P).
- **Ribbon** with Home, Insert, Formulas, Data and View tabs; tinted tiles for key commands.
- **Start screen** with templates (budget, invoice, sales tracker, to-do) and recent files;
  **⌘K command palette** for every command, sheets, functions, go-to and recent files.
- **Charts:** charts in the file are drawn with live data (column, bar, stacked, line, area,
  pie, doughnut, scatter); insert new ones, drag to move, resize, delete. All saved into the file.
- **Filters:** filter buttons on a table's header row, with value lists, search and sort;
  **dropdown lists** from the file's data validation.
- **Formulas:** IronCalc's ~400 functions with live recalculation. While typing a formula
  you get function autocomplete (Tab to insert), argument hints, colored reference
  highlights, and click/drag to insert references. Missing `)` are closed on Enter.
- **Formatting:** font size, B/I/U/S, text and fill colors, borders (all, outside,
  inside, each edge, none), horizontal and vertical alignment, wrap, merge & center,
  number formats, currency/percent/thousands, increase/decrease decimals, clear formats.
- **Editing:** fill handle (drag to continue series like Jan, Feb… or 1, 2…), ⌘D/⌘R fill,
  copy/paste that shifts formula references and keeps formatting, cut & paste to move,
  AutoSum (sum, average, count, max, min), sort A→Z/Z→A (auto-detects the table, its header
  and totals row), find & replace across all sheets (⌘F/⌘H), go to (⌘G), insert date/time,
  format painter, paste values only (⇧⌘V), auto-fit columns (double-click a column edge),
  remove duplicates, text to columns.
- **Rows, columns, sheets:** insert/delete, resize by dragging, hide/unhide, freeze panes
  (at selection, top row, first column); add/rename/delete sheets.
- **Display:** conditional formatting (color scales, data bars, icon sets, rule colors),
  comment markers with hover text, merged cells, borders, text overflow, zoom (50–200%),
  gridlines and show-formulas toggles, Excel-style number fitting.
- **Status bar:** average, count, sum, min and max of the selection; zoom slider.
- **Undo/redo:** ⌘Z undoes one whole command, as in Excel: a multi-cell paste, or an
  insert along with the names and references it shifted, is a single step.
- **Keyboard:** arrows, ⇧/⌘+arrows, Enter/Tab, F2, typing to edit, ⌘C/X/V, ⌘Z/⇧⌘Z,
  ⌘B/I/U, ⌘D/R, ⌘;, ⌘A, ⌘F/H, ⌘S/⇧⌘S/⌘O/⌘N, ⌘+/⌘−/⌘0 zoom, Delete.

## Known limits

- Row/column inserts and deletes and sheet add/rename/delete are patched into the
  original file: references in formulas, charts, conditional formatting, validation,
  merges, tables, pivot sources and names follow them, and references to a deleted
  sheet become `#REF!`, as in Excel. Undo and redo of these are patched too. The save
  still asks before a full rewrite (which drops charts, pivots etc.) in a few cases:
  undoing a sheet delete that was already saved, deleting a sheet whose table is used
  by formulas elsewhere or whose tables/pivots have slicers, 3D references ending on a
  deleted sheet, and column inserts inside a table.
- Images, pivot tables and slicers are kept but not displayed. The status bar says when a
  sheet has them.
- No font family picker or pivot table editing yet. Comments can be read but not edited.
  Chart edits aren't part of undo.
- Filters hide rows in the saved file but don't write Excel's filter definition, so Excel
  shows the hidden rows without filter buttons.
- IronCalc gaps: AGGREGATE, 3D references, named constants and some legacy array
  formulas calculate wrong; about 7% of unusual files don't open.

## Layout

- `src/` React UI: `Grid.tsx` (canvas grid and chart layer), `App.tsx` (shell, commands),
  `Ribbon.tsx`, `ChartView.tsx` (SVG charts), `Palette.tsx` (⌘K), `StartScreen.tsx`,
  `Popovers.tsx` (filter and dropdown lists), `native.ts` (menu bar, context menus, print)
- `src-tauri/` Tauri shell: one `api` command forwarding to `sheets_core::Session::dispatch`
- `../../crates/sheets-core` backend: workbook session, UI commands, save logic, charts,
  templates and data tools (`extras.rs`)
- `../../crates/ooxml` patch-in-place writer: cell/style splicing, structural ops
  (`structural.rs`, `sheetops.rs`), charts (`charts.rs`)
