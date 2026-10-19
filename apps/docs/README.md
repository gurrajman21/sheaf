# Quire

A quire is a gathering of folded sheets, the unit books are bound from.

Quire is a desktop word processor for .docx files, built the same way as Sheaf: the
[Oxi](https://gitlab.com/Ryujiyasu/oxi) engine lays out real pages, and Sheaf's
patch-in-place writer (`crates/ooxml/src/docx`) saves your edits back into the original
file, so everything it doesn't edit (headers, footers, footnotes, comments, charts, fields,
macros, custom XML) stays exactly as Word wrote it.

## Run it

```bash
# release build: produces target/release/bundle/macos/Quire.app
cd apps/docs && npm install && npx tauri build --bundles app

# development: native window with hot reload
cd apps/docs && npx tauri dev
```

Pass a file to open it: `Quire.app/Contents/MacOS/quire path/to/doc.docx`, drop a .docx
onto the window, or use ⌘O.

### In a browser (for UI work and testing)

```bash
cargo run --release -p docs-core --bin docs_dev_server -- 7879 path/to/doc.docx
cd apps/docs && npx vite      # http://localhost:1421, proxies /api to :7879
```

## What works

- **Real pages:** every page drawn from Oxi's layout (fonts, tables, images, headers and
  footers, footnotes, text boxes, page and section breaks), on a desk in light or dark mode,
  zoom 40 to 300%. Metric-compatible fonts (Carlito, Caladea, Liberation, OFL) are bundled,
  so documents look right without Microsoft's fonts.
- **Edit everywhere:** body text, table cells (nested too; Tab moves between cells, Tab in the
  last cell adds a row), headers, footers, footnotes, endnotes and text boxes.
- **Editing:** click, drag and double-click selection, typing (keystrokes batch while a layout
  is in flight and show immediately), Enter, Backspace/Delete, ⌥ word movement and deletion,
  arrows by line, Home/End, ⌘A, cut/copy/paste with formatting (to and from other apps),
  undo/redo, find and replace (⌥⌘F), format painter.
- **Formatting:** font, size, bold, italic, underline, strikethrough, color, highlight, clear
  formatting; paragraph styles, alignment, bulleted and numbered lists, indent, line spacing
  and space before/after.
- **Layout:** page size (Letter, Legal, A4, A5, Tabloid), orientation, margins, columns,
  page breaks. **Tables:** insert from a grid picker, add/delete rows and columns, merge,
  shade, delete (a Table tab appears when the caret is in one).
- **Structure:** headings pane, table of contents (insert and update with page numbers),
  footnotes, page numbers in the header or footer, hyperlinks (⌥⌘K; ⌘-click opens).
- **Review:** comments in a side panel (add with ⌥⌘M, reply, resolve, edit, delete; threads
  are Word-compatible), commented text highlighted; track changes recorded as Word revisions
  with change bars; accept or reject one change or all.
- **Spelling:** red squiggles from a bundled Hunspell dictionary (en_US), suggestions and
  "Add to dictionary" on right-click.
- **Slash menu:** type `/` on an empty line for headings, lists, quote, table, picture, page
  break, table of contents, footnote, page numbers and comments.
- **Never lose work:** a recovery copy is autosaved every 20 seconds while there are unsaved
  changes and offered back after a crash; version history keeps a snapshot on every save and
  every few minutes, restorable (and undoable) from File → Version history.
- **Speed:** after an edit only the affected pages are laid out again (a full layout follows
  when you pause), and Oxi's hot environment-flag lookups are served from a snapshot, which
  made layout about 3.5× faster.
- ⌘K palette, start screen with templates, native menu bar, print / save as PDF (⌘P).

## Known limits

- The windowed relayout needs a page that starts cleanly at a paragraph; inside one long table
  that spans many pages it falls back to a full layout (about 10 ms per page).
- Selections stay inside one container (a cell, a header, a note); in the body they can span
  tables. Header/footer text is matched to its part by text, so two variants with identical
  text (first-page vs default) edit the first one.
- Spell check is English (US) only; custom words are stored per user profile in the app.
- Comments can't be added inside headers or footnotes; replies sit on the parent's range.
- Find and replace is plain text (no regular expressions or formatting search).
- Oxi (layout) and spellbook (spell check) are MPL-2.0 dependencies used unmodified; the
  en_US dictionary is from SCOWL (see `crates/docs-core/dict/README_en_US.txt`). Quire's own
  code is MIT OR Apache-2.0.

## Layout

- `src/` React UI: `PageView.tsx` (canvas pages, caret, input), `geometry.ts` (measuring,
  hit-testing, caret placement), `App.tsx` (shell, commands, typing queue), `Ribbon.tsx`,
  `ribbonKit.tsx` (shared with Sheaf's ribbon), `SidePanel.tsx` (comments and changes),
  `NavPane.tsx`, `SlashMenu.tsx`, `Versions.tsx`, `richtext.ts` (HTML paste), `StartScreen.tsx`
- `public/fonts/` bundled metric-compatible fonts (OFL)
- `src-tauri/` Tauri shell: one `api` command forwarding to `docs_core::Session::dispatch`
- `../../crates/docs-core` backend: session, editing commands (`edit.rs`) on any container
  (`blocks.rs`: stories and paths), undo, layout display list and windowed relayout
  (`layout.rs`), spell check (`spell.rs`), autosave and versions (`autosave.rs`)
