import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Plus } from "lucide-react";
import { call, isTauri, type CellStyle, type Layout, type SaveResult, type SheetObjects, type Stats, type WorkbookInfo } from "./api";
import { writeClipboard } from "./clipboard";
import { confirmAction, pickOpenPath, pickSavePath } from "./files";
import { makeGeometry, normRect, parseCellName, selectionName, type Selection } from "./geometry";
import { Grid, type Editing } from "./Grid";
import { Toolbar } from "./Toolbar";

type Toast = { text: string; kind: "info" | "error" };
type Menu = { x: number; y: number; items: { label: string; shortcut?: string; action?: () => void; danger?: boolean }[] };

function ObjectsNotice({ objects }: { objects?: SheetObjects }) {
  if (!objects) return null;
  const parts: string[] = [];
  const n = (count: number, one: string, many: string) => count && parts.push(`${count} ${count === 1 ? one : many}`);
  n(objects.charts, "chart", "charts");
  n(objects.images, "image", "images");
  n(objects.pivot_tables, "pivot table", "pivot tables");
  n(objects.comments, "comment", "comments");
  if (!parts.length) return null;
  const list = parts.length > 1 ? `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}` : parts[0];
  return (
    <div className="objects-notice" title="These aren't displayed yet, but they're saved back into the file unchanged.">
      {list} on this sheet aren't shown yet. They stay in the file when you save.
    </div>
  );
}

const START: Selection = { anchor: { r: 1, c: 1 }, focus: { r: 1, c: 1 } };
const fmtNumber = (n: number) => n.toLocaleString(undefined, { maximumFractionDigits: 6 });

export default function App() {
  const [info, setInfo] = useState<WorkbookInfo | null>(null);
  const [layout, setLayout] = useState<Layout | null>(null);
  const [sel, setSel] = useState<Selection>(START);
  const [editing, setEditing] = useState<Editing | null>(null);
  const [version, setVersion] = useState(0);
  const [input, setInput] = useState("");
  const [style, setStyle] = useState<CellStyle | null>(null);
  const [stats, setStats] = useState<Stats | null>(null);
  const [menu, setMenu] = useState<Menu | null>(null);
  const [toast, setToast] = useState<Toast | null>(null);
  const [renaming, setRenaming] = useState<number | null>(null);
  const [focusToken, setFocusToken] = useState(0);
  const [nameBox, setNameBox] = useState<string | null>(null);
  const geom = useMemo(() => (layout ? makeGeometry(layout) : null), [layout]);
  const sheet = info?.sheet ?? 0;
  const toastTimer = useRef<number>(undefined);

  const say = useCallback((text: string, kind: Toast["kind"] = "info") => {
    setToast({ text, kind });
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), kind === "error" ? 6000 : 3000);
  }, []);

  const refresh = useCallback(async (opts: { layout?: boolean } = {}) => {
    const i = await call<WorkbookInfo>("info");
    setInfo(i);
    if (opts.layout) setLayout(await call<Layout>("layout", { sheet: i.sheet }));
    setVersion((v) => v + 1);
  }, []);

  useEffect(() => { refresh({ layout: true }).catch((e) => say(String(e), "error")); }, [refresh, say]);

  // run a backend command, then redraw; structural commands also reload the layout
  const run = useCallback(async (cmd: string, args: object, opts: { layout?: boolean } = {}) => {
    try {
      const out = await call(cmd, args);
      await refresh(opts);
      return out;
    } catch (e) {
      say(e instanceof Error ? e.message : String(e), "error");
      return undefined;
    }
  }, [refresh, say]);

  // active cell details for the formula bar and toolbar
  useEffect(() => {
    if (!info) return;
    const cell = { sheet, row: sel.anchor.r, col: sel.anchor.c };
    call<string>("input", { cell }).then(setInput).catch(() => setInput(""));
    call<CellStyle>("style", { cell }).then(setStyle).catch(() => setStyle(null));
    const { r0, r1, c0, c1 } = normRect(sel);
    if (r0 === r1 && c0 === c1) setStats(null);
    else call<Stats>("stats", { range: { sheet, row: r0, col: c0, width: c1 - c0 + 1, height: r1 - r0 + 1 } }).then(setStats).catch(() => setStats(null));
  }, [sel, version, sheet, info !== null]); // eslint-disable-line react-hooks/exhaustive-deps

  const range = useCallback(() => {
    const { r0, r1, c0, c1 } = normRect(sel);
    return { sheet, row: r0, col: c0, width: c1 - c0 + 1, height: r1 - r0 + 1 };
  }, [sel, sheet]);

  // ---- window title
  useEffect(() => {
    if (!info) return;
    const title = `${info.dirty ? "• " : ""}${info.name}`;
    document.title = title;
    if (isTauri) import("@tauri-apps/api/window").then(({ getCurrentWindow }) => getCurrentWindow().setTitle(title)).catch(() => {});
  }, [info?.name, info?.dirty]); // eslint-disable-line react-hooks/exhaustive-deps

  // ---- editing
  const startEdit = useCallback((initial?: string, source: Editing["source"] = "cell") => {
    // typed text can arrive in several input events before the editor takes focus: append
    setEditing((ed) => ed && initial !== undefined
      ? { ...ed, text: ed.text + initial }
      : { cell: sel.anchor, text: initial ?? input, caretAtEnd: true, source });
  }, [sel.anchor, input]);

  const commitEdit = useCallback(async (move: [number, number]) => {
    if (!editing || !layout) return;
    const { cell, text } = editing;
    setEditing(null);
    // move first, so typing that follows lands in the next cell, not this one
    if (move[0] || move[1]) {
      const to = { r: Math.min(Math.max(cell.r + move[0], 1), layout.rows), c: Math.min(Math.max(cell.c + move[1], 1), layout.cols) };
      setSel({ anchor: to, focus: to });
    }
    setFocusToken((t) => t + 1);
    if (text !== input || cell.r !== sel.anchor.r || cell.c !== sel.anchor.c) {
      await run("set_input", { cell: { sheet, row: cell.r, col: cell.c }, value: text });
    }
  }, [editing, input, layout, run, sel.anchor, sheet]);

  const cancelEdit = useCallback(() => { setEditing(null); setFocusToken((t) => t + 1); }, []);

  // ---- files
  const guardUnsaved = useCallback(async () => {
    if (!info?.dirty) return true;
    return confirmAction(`Your changes to ${info.name} haven't been saved.`, "Discard unsaved changes?", "Discard changes");
  }, [info]);

  const save = useCallback(async (saveAs = false) => {
    if (!info) return;
    let path = saveAs || !info.path ? await pickSavePath(info.path ?? info.name) : info.path;
    if (!path) return;
    try {
      let res = await call<SaveResult>("save", { path });
      if (res.needs_confirmation) {
        const ok = await confirmAction(
          `${res.reason}. Saving now rewrites the whole file, which drops charts, pivot tables, comments and other content this app doesn't edit yet.\n\nTo keep the original untouched, choose Cancel, undo that change, and save again.`,
          "Save by rewriting the file?",
          "Rewrite and save",
        );
        if (!ok) return;
        res = await call<SaveResult>("save", { path, allow_lossy: true });
      }
      path = res.path ?? path;
      await refresh();
      say(res.mode === "patch" ? "Saved. Everything else in the file was kept as it was." : "Saved");
    } catch (e) {
      say(`Couldn't save: ${e instanceof Error ? e.message : e}`, "error");
    }
  }, [info, refresh, say]);

  const openPath = useCallback(async (path: string) => {
    try {
      await call("open", { path });
      setSel(START);
      setEditing(null);
      await refresh({ layout: true });
    } catch (e) {
      say(e instanceof Error ? e.message : String(e), "error");
    }
  }, [refresh, say]);

  const fileAction = useCallback(async (action: "new" | "open" | "save" | "save_as") => {
    if (action === "save" || action === "save_as") return save(action === "save_as");
    if (!(await guardUnsaved())) return;
    if (action === "new") {
      await call("new");
      setSel(START);
      setEditing(null);
      await refresh({ layout: true });
    } else {
      const path = await pickOpenPath();
      if (path) await openPath(path);
    }
  }, [guardUnsaved, openPath, refresh, save]);

  // close guard + drag-and-drop open (desktop app only)
  const infoRef = useRef(info);
  infoRef.current = info;
  // registered once; handlers read the latest state through refs
  const dropRef = useRef<(file: string) => void>(() => {});
  dropRef.current = (file) => { guardUnsaved().then((ok) => { if (ok) openPath(file); }); };
  useEffect(() => {
    if (!isTauri) return;
    let cancelled = false;
    const unlisten: (() => void)[] = [];
    import("@tauri-apps/api/window").then(async ({ getCurrentWindow }) => {
      const win = getCurrentWindow();
      const subs = [
        // Tauri destroys the window after this handler unless it calls preventDefault
        await win.onCloseRequested(async (e) => {
          const i = infoRef.current;
          if (i?.dirty && !(await confirmAction(`Your changes to ${i.name} haven't been saved.`, "Close without saving?", "Close without saving"))) e.preventDefault();
        }),
        await win.onDragDropEvent((ev) => {
          if (ev.payload.type !== "drop") return;
          const file = ev.payload.paths.find((p) => /\.xls[xm]$/i.test(p));
          if (file) dropRef.current(file);
        }),
      ];
      // the effect may have been cleaned up (StrictMode) before registration finished
      if (cancelled) subs.forEach((u) => u());
      else unlisten.push(...subs);
    });
    return () => { cancelled = true; unlisten.forEach((u) => u()); };
  }, []);

  // global shortcuts (work from the formula bar too)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey)) return;
      const k = e.key.toLowerCase();
      if (k === "s") { e.preventDefault(); fileAction(e.shiftKey ? "save_as" : "save"); }
      else if (k === "o") { e.preventDefault(); fileAction("open"); }
      else if (k === "n") { e.preventDefault(); fileAction("new"); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [fileAction]);

  // ---- commands
  const setCellStyle = useCallback((path: string, value: string) => run("set_style", { range: range(), path, value }), [range, run]);
  const undo = useCallback(() => run("undo", {}, { layout: true }), [run]);
  const redo = useCallback(() => run("redo", {}, { layout: true }), [run]);
  const clear = useCallback(() => run("clear", { range: range() }), [range, run]);

  const copy = useCallback(async (cut: boolean) => {
    try {
      const text = await call<string>("copy", { range: range() });
      await writeClipboard(text);
      if (cut) await run("clear", { range: range() });
      say(cut ? "Cut" : "Copied");
    } catch (e) {
      say(`Couldn't copy: ${e instanceof Error ? e.message : e}`, "error");
    }
  }, [range, run, say]);

  const paste = useCallback(async (text: string) => {
    const r = range();
    const out = await run("paste", { cell: { sheet, row: r.row, col: r.col }, text }) as { width: number; height: number } | undefined;
    if (out) setSel({ anchor: { r: r.row, c: r.col }, focus: { r: r.row + out.height - 1, c: r.col + out.width - 1 } });
  }, [range, run, sheet]);

  const structural = useCallback((cmd: string, at: number, count: number) => run(cmd, { sheet, at, count }, { layout: true }), [run, sheet]);

  const insert = useCallback((what: "row" | "col") => {
    const { r0, r1, c0, c1 } = normRect(sel);
    return what === "row" ? structural("insert_rows", r0, r1 - r0 + 1) : structural("insert_cols", c0, c1 - c0 + 1);
  }, [sel, structural]);

  const freeze = useCallback(() => {
    if (!layout) return;
    const frozen = layout.frozen_rows > 0 || layout.frozen_cols > 0;
    return run("freeze", { sheet, rows: frozen ? 0 : sel.anchor.r - 1, cols: frozen ? 0 : sel.anchor.c - 1 }, { layout: true });
  }, [layout, run, sel.anchor, sheet]);

  const onGridKey = useCallback((e: React.KeyboardEvent) => {
    const mod = e.metaKey || e.ctrlKey;
    const k = e.key.toLowerCase();
    if (e.key === "Delete" || e.key === "Backspace") { e.preventDefault(); clear(); return true; }
    if (!mod) return false;
    if (k === "z") { e.preventDefault(); (e.shiftKey ? redo : undo)(); return true; }
    if (k === "y") { e.preventDefault(); redo(); return true; }
    if (k === "b") { e.preventDefault(); setCellStyle("font.b", String(!style?.b)); return true; }
    if (k === "i") { e.preventDefault(); setCellStyle("font.i", String(!style?.i)); return true; }
    if (k === "u") { e.preventDefault(); setCellStyle("font.u", String(!style?.u)); return true; }
    if (k === "a" && layout) { e.preventDefault(); setSel({ anchor: { r: 1, c: 1 }, focus: { r: layout.rows, c: layout.cols } }); return true; }
    return false;
  }, [clear, layout, redo, setCellStyle, style, undo]);

  const onClipboard = useCallback((kind: "copy" | "cut" | "paste", e: React.ClipboardEvent) => {
    e.preventDefault();
    if (kind === "paste") {
      const text = e.clipboardData.getData("text/plain");
      if (text) paste(text);
    } else copy(kind === "cut");
  }, [copy, paste]);

  const openContextMenu = useCallback((x: number, y: number) => {
    const { r0, r1, c0, c1 } = normRect(sel);
    const rows = r1 - r0 + 1;
    const cols = c1 - c0 + 1;
    const rl = rows === 1 ? "row" : `${rows} rows`;
    const cl = cols === 1 ? "column" : `${cols} columns`;
    setMenu({
      x, y, items: [
        { label: "Cut", shortcut: "⌘X", action: () => copy(true) },
        { label: "Copy", shortcut: "⌘C", action: () => copy(false) },
        { label: "Paste", shortcut: "⌘V", action: async () => { try { paste(await navigator.clipboard.readText()); } catch { say("Use ⌘V to paste here", "info"); } } },
        { label: "" },
        { label: `Insert ${rl} above`, action: () => structural("insert_rows", r0, rows) },
        { label: `Insert ${rl} below`, action: () => structural("insert_rows", r1 + 1, rows) },
        { label: `Insert ${cl} left`, action: () => structural("insert_cols", c0, cols) },
        { label: `Insert ${cl} right`, action: () => structural("insert_cols", c1 + 1, cols) },
        { label: "" },
        { label: `Delete ${rl}`, danger: true, action: () => structural("delete_rows", r0, rows) },
        { label: `Delete ${cl}`, danger: true, action: () => structural("delete_cols", c0, cols) },
        { label: "Clear contents", shortcut: "⌫", action: clear },
        { label: "Clear formatting and contents", action: () => run("clear_all", { range: range() }) },
      ],
    });
  }, [clear, copy, paste, range, run, say, sel, structural]);

  const onResize = useCallback((kind: "col" | "row", index: number, size: number) => {
    const { r0, r1, c0, c1 } = normRect(sel);
    if (kind === "col") {
      const [from, to] = index >= c0 && index <= c1 && r0 === 1 && r1 === layout?.rows ? [c0, c1] : [index, index];
      run("set_col_width", { sheet, from, to, width: size }, { layout: true });
    } else {
      const [from, to] = index >= r0 && index <= r1 && c0 === 1 && c1 === layout?.cols ? [r0, r1] : [index, index];
      run("set_row_height", { sheet, from, to, height: size }, { layout: true });
    }
  }, [layout, run, sel, sheet]);

  // ---- sheets
  const selectSheet = useCallback(async (i: number) => {
    if (editing) await commitEdit([0, 0]);
    await call("select_sheet", { sheet: i });
    setSel(START);
    await refresh({ layout: true });
  }, [commitEdit, editing, refresh]);

  const sheetMenu = (i: number, x: number, y: number) => setMenu({
    x, y, items: [
      { label: "Rename", action: () => setRenaming(i) },
      {
        label: "Delete sheet", danger: true, action: async () => {
          if (!info || info.sheets.length <= 1) return say("A workbook needs at least one sheet", "error");
          if (await confirmAction(`Delete "${info.sheets[i].name}" and everything on it?`, "Delete sheet?", "Delete")) {
            await run("delete_sheet", { sheet: i }, { layout: true });
          }
        },
      },
    ],
  });

  if (!info || !geom) return <div className="app loading">Opening…</div>;

  const frozen = (layout?.frozen_rows ?? 0) > 0 || (layout?.frozen_cols ?? 0) > 0;
  const barValue = editing ? editing.text : input;

  return (
    <div className="app" onMouseDown={() => menu && setMenu(null)}>
      <header className="titlebar" data-tauri-drag-region>
        <span className="doc-name" data-tauri-drag-region>{info.name}</span>
        <span className={`doc-state${info.dirty ? " is-dirty" : ""}`} data-tauri-drag-region>
          {info.dirty ? "Edited" : info.path ? "Saved" : "Not saved yet"}
        </span>
      </header>
      <Toolbar
        style={style}
        canUndo={info.can_undo}
        canRedo={info.can_redo}
        frozen={frozen}
        onFile={fileAction}
        onUndo={undo}
        onRedo={redo}
        onStyle={setCellStyle}
        onInsert={insert}
        onFreeze={freeze}
      />
      <div className="formula-bar">
        <input
          className="name-box"
          aria-label="Selected cell"
          value={nameBox ?? selectionName(sel)}
          onFocus={(e) => { setNameBox(selectionName(sel)); e.currentTarget.select(); }}
          onChange={(e) => setNameBox(e.target.value)}
          onBlur={() => setNameBox(null)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              const c = parseCellName(nameBox ?? "");
              if (c && layout && c.r <= layout.rows && c.c <= layout.cols) setSel({ anchor: c, focus: c });
              else say(`"${nameBox}" isn't a cell address. Try something like B12.`, "error");
              setNameBox(null);
              setFocusToken((t) => t + 1);
            } else if (e.key === "Escape") {
              setNameBox(null);
              setFocusToken((t) => t + 1);
            }
          }}
        />
        <span className="fx" aria-hidden>ƒx</span>
        <input
          className="formula-input"
          aria-label="Cell contents"
          value={barValue}
          spellCheck={false}
          onFocus={() => { if (!editing) startEdit(undefined, "bar"); }}
          onChange={(e) => setEditing((ed) => (ed ? { ...ed, text: e.target.value, caretAtEnd: false } : { cell: sel.anchor, text: e.target.value, caretAtEnd: false, source: "bar" }))}
          onKeyDown={(e) => {
            if (e.key === "Enter") { e.preventDefault(); commitEdit([1, 0]); }
            else if (e.key === "Tab") { e.preventDefault(); commitEdit([0, 1]); }
            else if (e.key === "Escape") { e.preventDefault(); cancelEdit(); }
          }}
        />
      </div>
      <Grid
        sheet={sheet}
        geom={geom}
        version={version}
        sel={sel}
        onSelect={(s) => { setSel(s); if (editing && editing.source === "bar") commitEdit([0, 0]); }}
        editing={editing}
        onEditStart={(initial) => startEdit(initial, "cell")}
        onEditChange={(text) => setEditing((ed) => (ed ? { ...ed, text, caretAtEnd: false } : ed))}
        onEditCommit={commitEdit}
        onEditCancel={cancelEdit}
        onKey={onGridKey}
        onContextMenu={openContextMenu}
        onResize={onResize}
        onClipboard={onClipboard}
        focusToken={focusToken}
      />
      <footer className="statusbar">
        <nav className="tabs" aria-label="Sheets">
          {info.sheets.map((s, i) => s.state !== "visible" ? null : (
            renaming === i ? (
              <input
                key={i}
                className="tab tab-rename"
                defaultValue={s.name}
                autoFocus
                onFocus={(e) => e.currentTarget.select()}
                onBlur={async (e) => {
                  setRenaming(null);
                  const name = e.currentTarget.value.trim();
                  if (name && name !== s.name) await run("rename_sheet", { sheet: i, name });
                  setFocusToken((t) => t + 1);
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter") e.currentTarget.blur();
                  if (e.key === "Escape") { e.currentTarget.value = s.name; e.currentTarget.blur(); }
                }}
              />
            ) : (
              <button
                key={i}
                type="button"
                className={`tab${i === sheet ? " is-active" : ""}`}
                aria-current={i === sheet ? "page" : undefined}
                style={s.color ? { boxShadow: `inset 0 -3px 0 ${s.color}` } : undefined}
                onClick={() => i !== sheet && selectSheet(i)}
                onDoubleClick={() => setRenaming(i)}
                onContextMenu={(e) => { e.preventDefault(); sheetMenu(i, e.clientX, e.clientY); }}
              >
                {s.name}
              </button>
            )
          ))}
          <button type="button" className="tab-add" aria-label="Add sheet" title="Add sheet" onClick={() => run("add_sheet", {}, { layout: true })}>
            <Plus size={15} />
          </button>
        </nav>
        <ObjectsNotice objects={info.sheets[sheet]?.objects} />
        <div className="stats" aria-live="polite">
          {stats && stats.count > 0 && (
            <>
              {stats.numbers > 0 && <span>Sum <b>{fmtNumber(stats.sum)}</b></span>}
              {stats.average !== null && <span>Average <b>{fmtNumber(stats.average)}</b></span>}
              <span>Count <b>{stats.count}</b></span>
            </>
          )}
        </div>
      </footer>
      {menu && (
        <div className="menu" role="menu" style={{ left: Math.min(menu.x, window.innerWidth - 260), top: Math.min(menu.y, window.innerHeight - 30 * menu.items.length) }} onMouseDown={(e) => e.stopPropagation()}>
          {menu.items.map((it, i) => it.label === "" ? <div key={i} className="menu-sep" role="separator" /> : (
            <button key={i} type="button" role="menuitem" className={`menu-item${it.danger ? " is-danger" : ""}`}
              onClick={() => { setMenu(null); it.action?.(); setFocusToken((t) => t + 1); }}>
              <span>{it.label}</span>{it.shortcut && <kbd>{it.shortcut}</kbd>}
            </button>
          ))}
        </div>
      )}
      {toast && <div className={`toast is-${toast.kind}`} role="status">{toast.text}</div>}
    </div>
  );
}
