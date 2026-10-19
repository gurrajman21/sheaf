import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Command, Minus, Plus } from "lucide-react";
import { call, isTauri, type CellStyle, type CellView, type FindHit, type Layout, type SaveResult, type SheetObjects, type Stats, type WorkbookInfo } from "./api";
import type { ChartData } from "./ChartView";
import { readClipboard, writeClipboard } from "./clipboard";
import { confirmAction, pickOpenPath, pickSavePath } from "./files";
import { FindPanel } from "./FindPanel";
import { FUNCTIONS, REF_COLORS, adjustDecimals, callAtCaret, formulaRefs, suggest, tokenAtCaret, type FnInfo } from "./functions";
import { cellName, colName, makeGeometry, normRect, parseCellName, selectionName, type Selection } from "./geometry";
import { Grid, type Editing, type Ref } from "./Grid";
import { AppMark } from "./icons";
import { nativeMenu, onMenuAction, onOpenedFiles, printWindow, type MenuEntry } from "./native";
import { Palette, type PaletteItem } from "./Palette";
import { FilterPopover, ListPopover } from "./Popovers";
import { Ribbon, type Action } from "./Ribbon";
import { StartScreen, addRecent, loadRecents, removeRecent, type Recent } from "./StartScreen";

type Toast = { text: string; kind: "info" | "error" };
type Menu = { x: number; y: number; items: MenuEntry[] };
type Rect = { sheet: number; row: number; col: number; width: number; height: number };
interface FilterState { r0: number; r1: number; c0: number; c1: number; criteria: Record<number, string[]> }

const START: Selection = { anchor: { r: 1, c: 1 }, focus: { r: 1, c: 1 } };
const fmtNumber = (n: number) => n.toLocaleString(undefined, { maximumFractionDigits: 6 });
const NUMERIC = /^[-+(]?[$€£¥]?\s?[\d,]*\.?\d+(E[+-]?\d+)?%?\)?$/i;
const POINTABLE = /[=(,+\-*/^&<>:;]\s*$/;
const ptToPx = (pt: number) => (pt * 4) / 3;

function ObjectsNotice({ objects }: { objects?: SheetObjects }) {
  if (!objects) return null;
  const parts: string[] = [];
  const n = (count: number, one: string, many: string) => count && parts.push(`${count} ${count === 1 ? one : many}`);
  n(objects.images, "image", "images");
  n(objects.pivot_tables, "pivot table", "pivot tables");
  if (!parts.length) return null;
  return (
    <div className="objects-notice" title="These aren't displayed yet, but they're saved back into the file unchanged.">
      {parts.join(" and ")} on this sheet {parts.length > 1 || /s$/.test(parts[0]) ? "aren't" : "isn't"} shown yet. Kept when you save.
    </div>
  );
}

function Assist({ suggestions, index, onPick, hint }: {
  suggestions: FnInfo[]; index: number; onPick: (f: FnInfo) => void; hint: { fn: FnInfo; arg: number } | null;
}) {
  if (suggestions.length) {
    return (
      <div className="assist" role="listbox" aria-label="Function suggestions">
        {suggestions.map((f, i) => (
          <button key={f.name} type="button" role="option" aria-selected={i === index} className={`assist-item${i === index ? " is-active" : ""}`}
            onMouseDown={(e) => { e.preventDefault(); onPick(f); }}>
            <b>{f.name}</b><span>{f.desc}</span>
          </button>
        ))}
        <div className="assist-foot">Tab to insert</div>
      </div>
    );
  }
  if (!hint) return null;
  const { fn, arg } = hint;
  const last = fn.args.length - 1;
  const current = fn.args[last]?.includes("…") && arg > last ? last : arg;
  return (
    <div className="assist assist-hint" role="tooltip">
      <div>
        <b>{fn.name}</b>(
        {fn.args.map((a, i) => (
          <span key={i}>{i > 0 && ", "}<span className={i === current ? "arg-active" : ""}>{a}</span></span>
        ))})
      </div>
      <div className="assist-desc">{fn.desc}</div>
    </div>
  );
}

/** Rows of the used range, rendered as a plain table for printing / PDF. */
function PrintView({ cells, rows, cols, title }: { cells: CellView[]; rows: number; cols: number; title: string }) {
  const map = new Map(cells.map((c) => [`${c.r},${c.c}`, c]));
  return (
    <div id="print-root">
      <h1>{title}</h1>
      <table>
        <tbody>
          {Array.from({ length: rows }, (_, i) => (
            <tr key={i}>
              {Array.from({ length: cols }, (_, j) => {
                const c = map.get(`${i + 1},${j + 1}`);
                return (
                  <td key={j} style={c ? { textAlign: c.align, fontWeight: c.b ? 600 : undefined, fontStyle: c.i ? "italic" : undefined, background: c.fill, color: c.color } : undefined}>
                    {c?.text}
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export default function App() {
  const [info, setInfo] = useState<WorkbookInfo | null>(null);
  const [layout, setLayout] = useState<Layout | null>(null);
  const [sel, setSel] = useState<Selection>(START);
  const [editing, setEditing] = useState<Editing | null>(null);
  const [caret, setCaret] = useState(0);
  const [assistIndex, setAssistIndex] = useState(0);
  const [assistDismissed, setAssistDismissed] = useState<string | null>(null);
  const pointRef = useRef<{ start: number; end: number; ref: string } | null>(null);
  const [version, setVersion] = useState(0);
  const [input, setInput] = useState("");
  const [style, setStyle] = useState<CellStyle | null>(null);
  const [stats, setStats] = useState<Stats | null>(null);
  const [menu, setMenu] = useState<Menu | null>(null);
  const [toast, setToast] = useState<Toast | null>(null);
  const [renaming, setRenaming] = useState<number | null>(null);
  const [focusToken, setFocusToken] = useState(0);
  const [nameBox, setNameBox] = useState<string | null>(null);
  const nameBoxRef = useRef<HTMLInputElement>(null);
  const [zoom, setZoom] = useState(1);
  const [showFormulas, setShowFormulas] = useState(false);
  const [find, setFind] = useState<{ replace: boolean } | null>(null);
  const [start, setStart] = useState(false);
  const [recents, setRecents] = useState<Recent[]>(loadRecents);
  const [palette, setPalette] = useState(false);
  const [charts, setCharts] = useState<ChartData[]>([]);
  const [selectedChart, setSelectedChart] = useState<string | null>(null);
  const [filters, setFilters] = useState<Record<number, FilterState>>({});
  const [filterPop, setFilterPop] = useState<{ col: number; x: number; y: number; values: string[] } | null>(null);
  const [listItems, setListItems] = useState<string[] | null>(null);
  const [listPop, setListPop] = useState<{ x: number; y: number; w: number } | null>(null);
  const [painter, setPainter] = useState<Rect | null>(null);
  const [printing, setPrinting] = useState<{ cells: CellView[]; rows: number; cols: number } | null>(null);
  const cutPending = useRef(false);
  const geom = useMemo(() => (layout ? makeGeometry(layout, zoom) : null), [layout, zoom]);
  const sheet = info?.sheet ?? 0;
  const toastTimer = useRef<number>(undefined);
  const refocus = () => setFocusToken((t) => t + 1);

  const say = useCallback((text: string, kind: Toast["kind"] = "info") => {
    setToast({ text, kind });
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), kind === "error" ? 6000 : 2600);
  }, []);

  const refresh = useCallback(async (opts: { layout?: boolean } = {}) => {
    const i = await call<WorkbookInfo>("info");
    setInfo(i);
    if (opts.layout) setLayout(await call<Layout>("layout", { sheet: i.sheet }));
    setVersion((v) => v + 1);
    return i;
  }, []);

  useEffect(() => {
    refresh({ layout: true }).then((i) => { if (!i.path && !i.dirty) setStart(true); }).catch((e) => say(String(e), "error"));
  }, [refresh, say]);

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

  // active cell details for the formula bar, ribbon, status bar and dropdown lists
  useEffect(() => {
    if (!info) return;
    const cell = { sheet, row: sel.anchor.r, col: sel.anchor.c };
    call<string>("input", { cell }).then(setInput).catch(() => setInput(""));
    call<CellStyle>("style", { cell }).then(setStyle).catch(() => setStyle(null));
    call<{ items: string[] } | null>("validation", { cell }).then((v) => setListItems(v?.items?.length ? v.items : null)).catch(() => setListItems(null));
    const { r0, r1, c0, c1 } = normRect(sel);
    if (r0 === r1 && c0 === c1) setStats(null);
    else call<Stats>("stats", { range: { sheet, row: r0, col: c0, width: c1 - c0 + 1, height: r1 - r0 + 1 } }).then(setStats).catch(() => setStats(null));
  }, [sel, version, sheet, info !== null]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (!info) return;
    call<ChartData[]>("charts", { sheet }).then(setCharts).catch(() => setCharts([]));
  }, [version, sheet, info !== null]); // eslint-disable-line react-hooks/exhaustive-deps

  const range = useCallback((): Rect => {
    const { r0, r1, c0, c1 } = normRect(sel);
    return { sheet, row: r0, col: c0, width: c1 - c0 + 1, height: r1 - r0 + 1 };
  }, [sel, sheet]);

  useEffect(() => {
    if (!info) return;
    const title = `${info.dirty ? "• " : ""}${info.name}`;
    document.title = title;
    if (isTauri) import("@tauri-apps/api/window").then(({ getCurrentWindow }) => getCurrentWindow().setTitle(title)).catch(() => {});
  }, [info?.name, info?.dirty]); // eslint-disable-line react-hooks/exhaustive-deps

  // ---- editing
  const startEdit = useCallback((initial?: string, source: Editing["source"] = "cell") => {
    pointRef.current = null;
    setAssistDismissed(null);
    setEditing((ed) => ed && initial !== undefined
      ? { ...ed, text: ed.text + initial }
      : { cell: sel.anchor, text: initial ?? input, caretAtEnd: true, source });
  }, [sel.anchor, input]);

  const changeEdit = useCallback((text: string) => {
    pointRef.current = null;
    setAssistIndex(0);
    setEditing((ed) => (ed ? { ...ed, text, caretAtEnd: false } : ed));
  }, []);

  const commitEdit = useCallback(async (move: [number, number]) => {
    if (!editing || !layout) return;
    const { cell, text } = editing;
    setEditing(null);
    pointRef.current = null;
    if (move[0] || move[1]) {
      const to = { r: Math.min(Math.max(cell.r + move[0], 1), layout.rows), c: Math.min(Math.max(cell.c + move[1], 1), layout.cols) };
      setSel({ anchor: to, focus: to });
    }
    refocus();
    if (text !== input || cell.r !== sel.anchor.r || cell.c !== sel.anchor.c) {
      let value = text;
      if (value.startsWith("=")) {
        const open = (value.match(/\(/g)?.length ?? 0) - (value.match(/\)/g)?.length ?? 0);
        if (open > 0) value += ")".repeat(open);
      }
      await run("set_input", { cell: { sheet, row: cell.r, col: cell.c }, value });
    }
  }, [editing, input, layout, run, sel.anchor, sheet]);

  const cancelEdit = useCallback(() => { setEditing(null); pointRef.current = null; refocus(); }, []);

  const formula = editing?.text.startsWith("=") ? editing.text : null;
  const token = formula ? tokenAtCaret(formula, caret) : null;
  const suggestions = token && assistDismissed !== formula ? suggest(token.prefix) : [];
  const hint = formula && !suggestions.length ? callAtCaret(formula, caret) : null;
  const refs: Ref[] = useMemo(() => (formula ? formulaRefs(formula).map((r, i) => ({ ...r, color: REF_COLORS[i % REF_COLORS.length] })) : []),
    [formula]);
  const pointMode = !!formula && (pointRef.current !== null || POINTABLE.test(formula.slice(0, caret)));

  const acceptSuggestion = useCallback((f: FnInfo) => {
    if (!editing || !token) return;
    const text = editing.text;
    const next = text.slice(0, token.start) + f.name + "(" + text.slice(caret).replace(/^\(/, "");
    setEditing({ ...editing, text: next, caretAtEnd: true });
    setAssistIndex(0);
  }, [editing, token, caret]);

  const assistKey = useCallback((e: React.KeyboardEvent) => {
    if (!suggestions.length) return false;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      setAssistIndex((i) => (i + (e.key === "ArrowDown" ? 1 : -1) + suggestions.length) % suggestions.length);
      return true;
    }
    if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey)) {
      e.preventDefault();
      acceptSuggestion(suggestions[Math.min(assistIndex, suggestions.length - 1)]);
      return true;
    }
    if (e.key === "Escape") { e.preventDefault(); setAssistDismissed(formula); return true; }
    return false;
  }, [suggestions, assistIndex, acceptSuggestion, formula]);

  const onPointRef = useCallback((s: Selection) => {
    if (!editing) return;
    const ref = selectionName(s);
    const text = editing.text;
    const prev = pointRef.current;
    let startAt: number, next: string;
    if (prev && text.slice(prev.start, prev.end) === prev.ref) {
      startAt = prev.start;
      next = text.slice(0, prev.start) + ref + text.slice(prev.end);
    } else {
      startAt = caret;
      next = text.slice(0, caret) + ref + text.slice(caret);
    }
    pointRef.current = { start: startAt, end: startAt + ref.length, ref };
    setEditing({ ...editing, text: next, caretAtEnd: true });
  }, [editing, caret]);

  const assist = (suggestions.length || hint)
    ? <Assist suggestions={suggestions} index={Math.min(assistIndex, suggestions.length - 1)} onPick={acceptSuggestion} hint={hint} />
    : null;

  // ---- files
  const guardUnsaved = useCallback(async () => {
    if (!info?.dirty) return true;
    return confirmAction(`Your changes to ${info.name} haven't been saved.`, "Discard unsaved changes?", "Discard changes");
  }, [info]);

  const remember = (i: WorkbookInfo) => { if (i.path) { addRecent(i.path, i.name); setRecents(loadRecents()); } };

  const save = useCallback(async (saveAs = false) => {
    if (!info) return;
    const path = saveAs || !info.path ? await pickSavePath(info.path ?? info.name) : info.path;
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
      remember(await refresh());
      say(res.mode === "patch" ? "Saved. Everything else in the file was kept as it was." : "Saved");
    } catch (e) {
      say(`Couldn't save: ${e instanceof Error ? e.message : e}`, "error");
    }
  }, [info, refresh, say]); // eslint-disable-line react-hooks/exhaustive-deps

  const resetView = () => { setSel(START); setEditing(null); setFilters({}); setSelectedChart(null); setPainter(null); };

  const openPath = useCallback(async (path: string) => {
    try {
      await call("open", { path });
      resetView();
      setStart(false);
      remember(await refresh({ layout: true }));
    } catch (e) {
      say(e instanceof Error ? e.message : String(e), "error");
      return false;
    }
    return true;
  }, [refresh, say]); // eslint-disable-line react-hooks/exhaustive-deps

  const fileAction = useCallback(async (a: "new" | "open" | "save" | "save_as") => {
    if (a === "save" || a === "save_as") return save(a === "save_as");
    if (!(await guardUnsaved())) return;
    if (a === "new") {
      await call("new");
      resetView();
      setStart(false);
      await refresh({ layout: true });
    } else {
      const path = await pickOpenPath();
      if (path) await openPath(path);
    }
  }, [guardUnsaved, openPath, refresh, save]);

  const useTemplate = useCallback(async (id: string) => {
    if (!(await guardUnsaved())) return;
    await call("new");
    if (id !== "blank") await call("template", { name: id });
    resetView();
    setStart(false);
    await refresh({ layout: true });
    refocus();
  }, [guardUnsaved, refresh]);

  const infoRef = useRef(info);
  infoRef.current = info;
  const dropRef = useRef<(file: string) => void>(() => {});
  dropRef.current = (file) => { guardUnsaved().then((ok) => { if (ok) openPath(file); }); };
  useEffect(() => onOpenedFiles((paths) => {
    const file = paths.filter((p) => /\.xls[xm]$/i.test(p)).pop();
    if (file) dropRef.current(file);
  }), []);
  useEffect(() => {
    if (!isTauri) return;
    let cancelled = false;
    const unlisten: (() => void)[] = [];
    import("@tauri-apps/api/window").then(async ({ getCurrentWindow }) => {
      const win = getCurrentWindow();
      const subs = [
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
      if (cancelled) subs.forEach((u) => u());
      else unlisten.push(...subs);
    });
    return () => { cancelled = true; unlisten.forEach((u) => u()); };
  }, []);

  // ---- commands
  const setCellStyle = useCallback((path: string, value: string) => run("set_style", { range: range(), path, value }), [range, run]);
  const undo = useCallback(() => run("undo", {}, { layout: true }), [run]);
  const redo = useCallback(() => run("redo", {}, { layout: true }), [run]);
  const clear = useCallback(() => run("clear", { range: range() }), [range, run]);

  const copy = useCallback(async (cut: boolean) => {
    try {
      const text = await call<string>("clip_copy", { range: range() });
      await writeClipboard(text);
      cutPending.current = cut;
      say(cut ? "Cut. Paste to move it." : "Copied");
    } catch (e) {
      say(`Couldn't copy: ${e instanceof Error ? e.message : e}`, "error");
    }
  }, [range, say]);

  const paste = useCallback(async (text: string, valuesOnly = false) => {
    const r = range();
    const out = await run(valuesOnly ? "paste" : "clip_paste", { cell: { sheet, row: r.row, col: r.col }, text, cut: cutPending.current }) as { width: number; height: number } | undefined;
    if (!valuesOnly) cutPending.current = false;
    if (out) setSel({ anchor: { r: r.row, c: r.col }, focus: { r: r.row + out.height - 1, c: r.col + out.width - 1 } });
  }, [range, run, sheet]);

  const structural = useCallback((cmd: string, at: number, count: number) => run(cmd, { sheet, at, count }, { layout: true }), [run, sheet]);

  /** The table around the active cell (or the selection): bounds, header row and totals row. */
  const detectRegion = useCallback(async () => {
    if (!layout) return null;
    let { r0, r1, c0, c1 } = normRect(sel);
    const maxR = Math.max(1, layout.rows - 100), maxC = Math.max(1, layout.cols - 10);
    const cells = await call<CellView[]>("cells", { sheet, range: [1, 1, maxR, maxC] });
    const byKey = new Map(cells.map((x) => [`${x.r},${x.c}`, x]));
    const filled = (r: number, c: number) => (byKey.get(`${r},${c}`)?.text ?? "") !== "";
    const picked = !(r0 === r1 && c0 === c1);
    if (!picked) {
      if (!filled(r0, c0)) return null;
      const rowHas = (r: number) => { for (let c = c0; c <= c1; c++) if (filled(r, c)) return true; return false; };
      const colHas = (c: number) => { for (let r = r0; r <= r1; r++) if (filled(r, c)) return true; return false; };
      for (let grew = true; grew; ) {
        grew = false;
        if (r0 > 1 && rowHas(r0 - 1)) { r0--; grew = true; }
        if (r1 < maxR && rowHas(r1 + 1)) { r1++; grew = true; }
        if (c0 > 1 && colHas(c0 - 1)) { c0--; grew = true; }
        if (c1 < maxC && colHas(c1 + 1)) { c1++; grew = true; }
      }
    }
    const col = Math.min(Math.max(sel.anchor.c, c0), c1);
    const head = byKey.get(`${r0},${col}`), next = byKey.get(`${r0 + 1},${col}`);
    const header = !!head && !NUMERIC.test(head.text) && (!next || NUMERIC.test(next.text) || !!head.b || picked);
    const hasFormula = (r: number) => { for (let c = c0; c <= c1; c++) if (byKey.get(`${r},${c}`)?.formula) return true; return false; };
    const totals = !picked && r1 - 1 > r0 + (header ? 1 : 0) && hasFormula(r1) && !hasFormula(r1 - 1);
    return { r0, r1, c0, c1, header, totals, cells: byKey };
  }, [layout, sel, sheet]);

  const autoSum = useCallback(async (fn: string) => {
    const { r0, r1, c0, c1 } = normRect(sel);
    if (r0 !== r1 || c0 !== c1) {
      for (let c = c0; c <= c1; c++) {
        await call("set_input", { cell: { sheet, row: r1 + 1, col: c }, value: `=${fn}(${colName(c)}${r0}:${colName(c)}${r1})` });
      }
      await refresh();
      setSel({ anchor: { r: r1 + 1, c: c0 }, focus: { r: r1 + 1, c: c1 } });
      return;
    }
    const { r, c } = sel.anchor;
    const isNum = (cv?: CellView) => !!cv && NUMERIC.test(cv.text.trim());
    let ref = "";
    if (r > 1) {
      const above = await call<CellView[]>("cells", { sheet, range: [Math.max(1, r - 1000), c, r - 1, c] });
      const byRow = new Map(above.map((x) => [x.r, x]));
      let top = r - 1;
      while (top >= 1 && isNum(byRow.get(top))) top--;
      if (top < r - 1) ref = `${colName(c)}${top + 1}:${colName(c)}${r - 1}`;
    }
    if (!ref && c > 1) {
      const left = await call<CellView[]>("cells", { sheet, range: [r, Math.max(1, c - 200), r, c - 1] });
      const byCol = new Map(left.map((x) => [x.c, x]));
      let lc = c - 1;
      while (lc >= 1 && isNum(byCol.get(lc))) lc--;
      if (lc < c - 1) ref = `${colName(lc + 1)}${r}:${colName(c - 1)}${r}`;
    }
    pointRef.current = null;
    setEditing({ cell: sel.anchor, text: `=${fn}(${ref})`, caretAtEnd: true, source: "cell" });
  }, [refresh, sel, sheet]);

  const fill = useCallback((dir: "down" | "right") => {
    const { r0, r1, c0, c1 } = normRect(sel);
    if (dir === "down") {
      if (r0 === r1) return r0 > 1 && run("autofill", { range: { sheet, row: r0 - 1, col: c0, width: c1 - c0 + 1, height: 1 }, to_row: r0 });
      return run("autofill", { range: { sheet, row: r0, col: c0, width: c1 - c0 + 1, height: 1 }, to_row: r1 });
    }
    if (c0 === c1) return c0 > 1 && run("autofill", { range: { sheet, row: r0, col: c0 - 1, width: 1, height: r1 - r0 + 1 }, to_col: c0 });
    return run("autofill", { range: { sheet, row: r0, col: c0, width: 1, height: r1 - r0 + 1 }, to_col: c1 });
  }, [run, sel, sheet]);

  const sort = useCallback(async (desc: boolean, byCol?: number) => {
    const reg = await detectRegion();
    if (!reg) { say("Select the cells to sort, or a cell inside a table", "error"); return; }
    const r0 = reg.r0 + (reg.header ? 1 : 0), r1 = reg.r1 - (reg.totals ? 1 : 0);
    if (r1 <= r0) return;
    const col = byCol ?? Math.min(Math.max(sel.anchor.c, reg.c0), reg.c1);
    await run("sort", { range: { sheet, row: r0, col: reg.c0, width: reg.c1 - reg.c0 + 1, height: r1 - r0 + 1 }, col, desc });
    say(`Sorted rows ${r0}–${r1} by column ${colName(col)}, ${desc ? "Z to A" : "A to Z"}`);
  }, [detectRegion, run, say, sel.anchor.c, sheet]);

  // ---- filters
  const filter = filters[sheet] ?? null;
  const applyFilter = useCallback(async (f: FilterState) => {
    const cells = await call<CellView[]>("cells", { sheet, range: [f.r0 + 1, f.c0, f.r1, f.c1] });
    const val = new Map(cells.map((c) => [`${c.r},${c.c}`, c.text]));
    const hide: number[] = [];
    for (let r = f.r0 + 1; r <= f.r1; r++) {
      for (const [col, keep] of Object.entries(f.criteria)) {
        if (!keep.includes(val.get(`${r},${col}`) ?? "")) { hide.push(r); break; }
      }
    }
    const all = Array.from({ length: f.r1 - f.r0 }, (_, i) => f.r0 + 1 + i);
    await call("hide_rows_list", { sheet, rows: all, hidden: false });
    if (hide.length) await call("hide_rows_list", { sheet, rows: hide, hidden: true });
    await refresh({ layout: true });
    const shown = all.length - hide.length;
    if (Object.keys(f.criteria).length) say(`Showing ${shown} of ${all.length} rows`);
  }, [refresh, say, sheet]);

  const toggleFilter = useCallback(async () => {
    if (filter) {
      const all = Array.from({ length: filter.r1 - filter.r0 }, (_, i) => filter.r0 + 1 + i);
      await call("hide_rows_list", { sheet, rows: all, hidden: false });
      setFilters((fs) => { const n = { ...fs }; delete n[sheet]; return n; });
      await refresh({ layout: true });
      return;
    }
    const reg = await detectRegion();
    if (!reg) { say("Select a cell inside a table with a header row to filter it", "error"); return; }
    setFilters((fs) => ({ ...fs, [sheet]: { r0: reg.r0, r1: reg.r1 - (reg.totals ? 1 : 0), c0: reg.c0, c1: reg.c1, criteria: {} } }));
    say("Filter on. Use the buttons in the header row.");
  }, [detectRegion, filter, refresh, say, sheet]);

  const openFilterMenu = useCallback(async (col: number, x: number, y: number) => {
    if (!filter) return;
    const cells = await call<CellView[]>("cells", { sheet, range: [filter.r0 + 1, col, filter.r1, col] });
    const byRow = new Map(cells.map((c) => [c.r, c.text]));
    const values = new Set<string>();
    for (let r = filter.r0 + 1; r <= filter.r1; r++) values.add(byRow.get(r) ?? "");
    const sorted = [...values].sort((a, b) => {
      const na = parseFloat(a.replace(/[^0-9.-]/g, "")), nb = parseFloat(b.replace(/[^0-9.-]/g, ""));
      if (a === "") return 1;
      if (b === "") return -1;
      return NUMERIC.test(a) && NUMERIC.test(b) ? na - nb : a.localeCompare(b);
    });
    setFilterPop({ col, x: Math.max(8, x), y, values: sorted });
  }, [filter, sheet]);

  // ---- charts
  const addChart = useCallback(async (kind: string) => {
    const reg = await detectRegion();
    if (!reg) { say("Select the data to chart first: a column of labels and one or more columns of numbers", "error"); return; }
    const r1 = reg.r1 - (reg.totals ? 1 : 0);
    try {
      const id = await call<string>("chart_add", { range: { sheet, row: reg.r0, col: reg.c0, width: reg.c1 - reg.c0 + 1, height: r1 - reg.r0 + 1 }, kind });
      await refresh();
      setSelectedChart(id);
    } catch (e) {
      say(e instanceof Error ? e.message : String(e), "error");
    }
  }, [detectRegion, refresh, say, sheet]);

  // ---- power tools
  const autofit = useCallback(async (cols?: number[]) => {
    if (!layout) return;
    const { c0, c1 } = normRect(sel);
    const list = cols ?? Array.from({ length: c1 - c0 + 1 }, (_, i) => c0 + i);
    const maxR = Math.max(1, layout.rows - 100);
    const ctx = document.createElement("canvas").getContext("2d")!;
    for (const c of list) {
      const cells = await call<CellView[]>("cells", { sheet, range: [1, c, maxR, c] });
      let w = 0;
      for (const cv of cells) {
        if (!cv.text || cv.wrap) continue;
        ctx.font = `${cv.i ? "italic " : ""}${cv.b ? "600 " : ""}${ptToPx(cv.size || 11)}px -apple-system, system-ui, sans-serif`;
        w = Math.max(w, ctx.measureText(cv.text).width);
      }
      if (w > 0) await call("set_col_width", { sheet, from: c, to: c, width: Math.ceil(Math.min(w + 14, 600)) });
    }
    await refresh({ layout: true });
  }, [layout, refresh, sel, sheet]);

  const dedupe = useCallback(async () => {
    const reg = await detectRegion();
    if (!reg) { say("Select the rows to check for duplicates", "error"); return; }
    const r0 = reg.r0 + (reg.header ? 1 : 0), r1 = reg.r1 - (reg.totals ? 1 : 0);
    const n = await run("dedupe", { range: { sheet, row: r0, col: reg.c0, width: reg.c1 - reg.c0 + 1, height: r1 - r0 + 1 } }) as number | undefined;
    if (n !== undefined) say(n ? `Removed ${n} duplicate row${n === 1 ? "" : "s"}` : "No duplicate rows found");
  }, [detectRegion, run, say, sheet]);

  const splitColumn = useCallback(async () => {
    const { r0, r1, c0, c1 } = normRect(sel);
    if (c0 !== c1) { say("Select one column of text to split", "error"); return; }
    let end = r1;
    if (r0 === r1) { // one cell: take the filled run below it
      const below = await call<CellView[]>("cells", { sheet, range: [r0, c0, r0 + 5000, c0] });
      const rows = new Set(below.filter((x) => x.text).map((x) => x.r));
      while (rows.has(end + 1)) end++;
    }
    const cells = await call<CellView[]>("cells", { sheet, range: [r0, c0, end, c0] });
    const texts = Array.from({ length: end - r0 + 1 }, (_, i) => cells.find((x) => x.r === r0 + i)?.text ?? "");
    const delim = ["\t", ",", ";", "|"].find((d) => texts.some((t) => t.includes(d))) ?? " ";
    const tsv = texts.map((t) => t.split(delim).map((s) => s.trim()).join("\t")).join("\n");
    await run("paste", { cell: { sheet, row: r0, col: c0 }, text: tsv });
    const name = { "\t": "tabs", ",": "commas", ";": "semicolons", "|": "bars", " ": "spaces" }[delim];
    say(`Split ${texts.length} cell${texts.length === 1 ? "" : "s"} at ${name}`);
  }, [run, say, sel, sheet]);

  const print = useCallback(async () => {
    if (!layout || !info) return;
    const rows = Math.max(1, layout.rows - 100), cols = Math.max(1, layout.cols - 10);
    const cells = await call<CellView[]>("cells", { sheet, range: [1, 1, rows, cols] });
    const usedR = Math.max(1, ...cells.map((c) => c.r)), usedC = Math.max(1, ...cells.map((c) => c.c));
    setPrinting({ cells, rows: usedR, cols: usedC });
    window.setTimeout(async () => { await printWindow(); setPrinting(null); }, 60);
  }, [info, layout, sheet]);

  const sheetDelete = useCallback(async (i: number) => {
    if (!info || info.sheets.length <= 1) { say("A workbook needs at least one sheet", "error"); return; }
    if (await confirmAction(`Delete "${info.sheets[i].name}" and everything on it?`, "Delete sheet?", "Delete")) {
      await run("delete_sheet", { sheet: i }, { layout: true });
    }
  }, [info, run, say]);

  const act = useCallback(async (a: Action) => {
    const { r0, r1, c0, c1 } = normRect(sel);
    switch (a.t) {
      case "file": return fileAction(a.a);
      case "undo": return undo();
      case "redo": return redo();
      case "cut": return copy(true);
      case "copy": return copy(false);
      case "paste":
        try { await paste(await readClipboard()); } catch { say("Use ⌘V to paste here"); }
        return;
      case "paste_values":
        try { await paste(await readClipboard(), true); } catch { say("Couldn't read the clipboard", "error"); }
        return;
      case "style": return setCellStyle(a.path, a.value);
      case "border": return run("border", { range: range(), kind: a.kind });
      case "merge":
        return a.mode === "unmerge"
          ? run("unmerge", { range: range() }, { layout: true })
          : run("merge", { range: range(), center: a.mode === "center" }, { layout: true });
      case "decimals": return setCellStyle("num_fmt", adjustDecimals(style?.num_fmt ?? "general", a.delta));
      case "insert":
        if (a.what === "sheet") return run("add_sheet", {}, { layout: true });
        return a.what === "rows" ? structural("insert_rows", r0, r1 - r0 + 1) : structural("insert_cols", c0, c1 - c0 + 1);
      case "delete":
        if (a.what === "sheet") return sheetDelete(sheet);
        return a.what === "rows" ? structural("delete_rows", r0, r1 - r0 + 1) : structural("delete_cols", c0, c1 - c0 + 1);
      case "autosum": return autoSum(a.fn);
      case "fill": return fill(a.dir);
      case "clear": return run(a.what === "contents" ? "clear" : a.what === "formats" ? "clear_formats" : "clear_all", { range: range() });
      case "sort": return sort(a.desc);
      case "find": setFind({ replace: a.replace }); return;
      case "freeze": {
        const rows = a.mode === "row" ? 1 : a.mode === "selection" ? sel.anchor.r - 1 : 0;
        const cols = a.mode === "col" ? 1 : a.mode === "selection" ? sel.anchor.c - 1 : 0;
        return run("freeze", { sheet, rows, cols }, { layout: true });
      }
      case "zoom": setZoom(a.value); return;
      case "toggle":
        if (a.what === "formulas") { setShowFormulas((v) => !v); return; }
        return run("grid_lines", { sheet, show: !(layout?.show_grid ?? true) }, { layout: true });
      case "hide":
        return a.kind === "rows"
          ? run("hide", { sheet, kind: "rows", from: r0, to: r1, hidden: a.hidden }, { layout: true })
          : run("hide", { sheet, kind: "cols", from: c0, to: c1, hidden: a.hidden }, { layout: true });
      case "function":
        pointRef.current = null;
        setEditing({ cell: sel.anchor, text: `=${a.name}(`, caretAtEnd: true, source: "cell" });
        return;
      case "stamp": {
        const d = new Date();
        const pad = (n: number) => String(n).padStart(2, "0");
        const value = a.what === "date" ? `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}` : `${pad(d.getHours())}:${pad(d.getMinutes())}`;
        return run("set_input", { cell: { sheet, row: sel.anchor.r, col: sel.anchor.c }, value });
      }
      case "chart": return addChart(a.kind);
      case "filter": return toggleFilter();
      case "painter":
        if (painter) { setPainter(null); return; }
        setPainter(range());
        say("Select the cells to paint this format onto");
        return;
      case "autofit": return autofit();
      case "goto": nameBoxRef.current?.focus(); return;
      case "dedupe": return dedupe();
      case "split": return splitColumn();
      case "print": return print();
      case "palette": setPalette(true); return;
      case "start": setStart(true); return;
    }
  }, [addChart, autoSum, autofit, copy, dedupe, fileAction, fill, layout, paste, painter, print, range, redo, run, say, sel, setCellStyle, sheet, sheetDelete, sort, splitColumn, structural, style, toggleFilter, undo]);

  // menu bar (desktop app)
  const actRef = useRef(act);
  actRef.current = act;
  const styleRef = useRef(style);
  styleRef.current = style;
  useEffect(() => onMenuAction((id) => {
    const a = actRef.current;
    const st = styleRef.current;
    const map: Record<string, Action> = {
      "file:new": { t: "file", a: "new" }, "file:start": { t: "start" }, "file:open": { t: "file", a: "open" },
      "file:save": { t: "file", a: "save" }, "file:save_as": { t: "file", a: "save_as" }, "file:print": { t: "print" },
      "edit:undo": { t: "undo" }, "edit:redo": { t: "redo" }, "edit:paste_values": { t: "paste_values" },
      "edit:fill_down": { t: "fill", dir: "down" }, "edit:fill_right": { t: "fill", dir: "right" },
      "edit:clear": { t: "clear", what: "contents" }, "edit:find": { t: "find", replace: false }, "edit:replace": { t: "find", replace: true },
      "edit:goto": { t: "goto" }, "view:palette": { t: "palette" },
      "view:gridlines": { t: "toggle", what: "grid" }, "view:formulas": { t: "toggle", what: "formulas" },
      "view:freeze": { t: "freeze", mode: "selection" }, "view:unfreeze": { t: "freeze", mode: "none" },
      "insert:rows": { t: "insert", what: "rows" }, "insert:cols": { t: "insert", what: "cols" }, "insert:sheet": { t: "insert", what: "sheet" },
      "insert:chart": { t: "chart", kind: "column" }, "insert:function": { t: "palette" }, "insert:sum": { t: "autosum", fn: "SUM" },
      "insert:date": { t: "stamp", what: "date" },
      "format:bold": { t: "style", path: "font.b", value: String(!st?.b) }, "format:italic": { t: "style", path: "font.i", value: String(!st?.i) },
      "format:underline": { t: "style", path: "font.u", value: String(!st?.u) }, "format:painter": { t: "painter" },
      "format:clear": { t: "clear", what: "formats" }, "format:merge": { t: "merge", mode: "center" }, "format:autofit": { t: "autofit" },
      "data:sort_asc": { t: "sort", desc: false }, "data:sort_desc": { t: "sort", desc: true }, "data:filter": { t: "filter" },
      "data:dedupe": { t: "dedupe" }, "data:split": { t: "split" },
    };
    if (id === "view:zoom_in") setZoom((z) => Math.min(2, +(z + 0.1).toFixed(2)));
    else if (id === "view:zoom_out") setZoom((z) => Math.max(0.5, +(z - 0.1).toFixed(2)));
    else if (id === "view:zoom_reset") setZoom(1);
    else if (map[id]) a(map[id]);
  }), []);

  // shortcuts in the browser build (the desktop menu bar owns them there)
  useEffect(() => {
    if (isTauri) return;
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey)) return;
      const k = e.key.toLowerCase();
      const a = actRef.current;
      if (k === "s") { e.preventDefault(); a({ t: "file", a: e.shiftKey ? "save_as" : "save" }); }
      else if (k === "o") { e.preventDefault(); a({ t: "file", a: "open" }); }
      else if (k === "k") { e.preventDefault(); setPalette(true); }
      else if (k === "f") { e.preventDefault(); setFind({ replace: false }); }
      else if (k === "h") { e.preventDefault(); setFind({ replace: true }); }
      else if (k === "g") { e.preventDefault(); nameBoxRef.current?.focus(); }
      else if (k === "l" && e.shiftKey) { e.preventDefault(); a({ t: "filter" }); }
      else if (k === "v" && e.shiftKey) { e.preventDefault(); a({ t: "paste_values" }); }
      else if (e.key === "=" || e.key === "+") { e.preventDefault(); setZoom((z) => Math.min(2, +(z + 0.1).toFixed(2))); }
      else if (e.key === "-") { e.preventDefault(); setZoom((z) => Math.max(0.5, +(z - 0.1).toFixed(2))); }
      else if (e.key === "0") { e.preventDefault(); setZoom(1); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const onGridKey = useCallback((e: React.KeyboardEvent) => {
    const mod = e.metaKey || e.ctrlKey;
    const k = e.key.toLowerCase();
    if (selectedChart && (e.key === "Delete" || e.key === "Backspace")) {
      e.preventDefault();
      const id = selectedChart;
      setSelectedChart(null);
      run("chart_delete", { id });
      return true;
    }
    if (e.key === "Escape" && (painter || selectedChart)) { setPainter(null); setSelectedChart(null); return true; }
    if (e.key === "Delete" || e.key === "Backspace") { e.preventDefault(); clear(); return true; }
    if (!mod || isTauri) return false; // the menu bar handles shortcuts in the desktop app
    if (k === "z") { e.preventDefault(); (e.shiftKey ? redo : undo)(); return true; }
    if (k === "y") { e.preventDefault(); redo(); return true; }
    if (k === "b") { e.preventDefault(); setCellStyle("font.b", String(!style?.b)); return true; }
    if (k === "i") { e.preventDefault(); setCellStyle("font.i", String(!style?.i)); return true; }
    if (k === "u") { e.preventDefault(); setCellStyle("font.u", String(!style?.u)); return true; }
    if (k === "d") { e.preventDefault(); fill("down"); return true; }
    if (k === "r") { e.preventDefault(); fill("right"); return true; }
    if (e.key === ";") { e.preventDefault(); act({ t: "stamp", what: "date" }); return true; }
    if (k === "a" && layout) { e.preventDefault(); setSel({ anchor: { r: 1, c: 1 }, focus: { r: layout.rows, c: layout.cols } }); return true; }
    return false;
  }, [act, clear, fill, layout, painter, redo, run, selectedChart, setCellStyle, style, undo]);

  const onClipboard = useCallback((kind: "copy" | "cut" | "paste", e: React.ClipboardEvent) => {
    e.preventDefault();
    if (kind === "paste") {
      const text = e.clipboardData.getData("text/plain");
      if (text) paste(text);
    } else copy(kind === "cut");
  }, [copy, paste]);

  const showMenu = useCallback(async (x: number, y: number, items: MenuEntry[]) => {
    if (!(await nativeMenu(items))) setMenu({ x, y, items });
  }, []);

  const openContextMenu = useCallback((x: number, y: number) => {
    if (!layout) return;
    const { r0, r1, c0, c1 } = normRect(sel);
    const rows = r1 - r0 + 1, cols = c1 - c0 + 1;
    const rl = rows === 1 ? "row" : `${rows} rows`;
    const cl = cols === 1 ? "column" : `${cols} columns`;
    const wholeRows = c0 === 1 && c1 === layout.cols;
    const wholeCols = r0 === 1 && r1 === layout.rows;
    showMenu(x, y, [
      { label: "Cut", shortcut: "⌘X", action: () => copy(true) },
      { label: "Copy", shortcut: "⌘C", action: () => copy(false) },
      { label: "Paste", shortcut: "⌘V", action: () => act({ t: "paste" }) },
      { label: "Paste values only", shortcut: "⇧⌘V", action: () => act({ t: "paste_values" }) },
      { label: "" },
      { label: `Insert ${rl} above`, action: () => structural("insert_rows", r0, rows) },
      { label: `Insert ${rl} below`, action: () => structural("insert_rows", r1 + 1, rows) },
      { label: `Insert ${cl} left`, action: () => structural("insert_cols", c0, cols) },
      { label: `Insert ${cl} right`, action: () => structural("insert_cols", c1 + 1, cols) },
      { label: `Delete ${rl}`, danger: true, action: () => structural("delete_rows", r0, rows) },
      { label: `Delete ${cl}`, danger: true, action: () => structural("delete_cols", c0, cols) },
      { label: "" },
      { label: "Clear contents", action: clear },
      { label: "Clear formats", action: () => act({ t: "clear", what: "formats" }) },
      { label: "Format painter", action: () => act({ t: "painter" }) },
      { label: "" },
      ...(wholeCols || !wholeRows ? [
        { label: `Hide ${cl}`, action: () => act({ t: "hide", kind: "cols", hidden: true }) },
        { label: "Unhide columns", action: () => act({ t: "hide", kind: "cols", hidden: false }) },
        { label: "Auto-fit column width", action: () => autofit() },
      ] : []),
      ...(wholeRows || !wholeCols ? [
        { label: `Hide ${rl}`, action: () => act({ t: "hide", kind: "rows", hidden: true }) },
        { label: "Unhide rows", action: () => act({ t: "hide", kind: "rows", hidden: false }) },
      ] : []),
      { label: "" },
      { label: "Sort A to Z", action: () => sort(false) },
      { label: "Sort Z to A", action: () => sort(true) },
      { label: filter ? "Remove filter" : "Filter", action: () => toggleFilter() },
      { label: "Insert chart", action: () => addChart("column") },
    ]);
  }, [act, addChart, autofit, clear, copy, filter, layout, sel, showMenu, sort, structural, toggleFilter]);

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

  const selectSheet = useCallback(async (i: number) => {
    if (editing) await commitEdit([0, 0]);
    await call("select_sheet", { sheet: i });
    setSel(START);
    setSelectedChart(null);
    await refresh({ layout: true });
  }, [commitEdit, editing, refresh]);

  const sheetMenu = (i: number, x: number, y: number) => showMenu(x, y, [
    { label: "Rename", action: () => setRenaming(i) },
    { label: "Insert sheet", action: () => run("add_sheet", {}, { layout: true }) },
    { label: "Delete sheet", danger: true, action: () => sheetDelete(i) },
  ]);

  const goToHit = useCallback(async (h: FindHit) => {
    if (h.sheet !== sheet) {
      await call("select_sheet", { sheet: h.sheet });
      await refresh({ layout: true });
    }
    setSel({ anchor: { r: h.row, c: h.col }, focus: { r: h.row, c: h.col } });
  }, [refresh, sheet]);

  // ⌘K palette contents
  const paletteItems: PaletteItem[] = useMemo(() => {
    const A = (group: string, label: string, a: Action, shortcut?: string): PaletteItem => ({ id: `${group}:${label}`, group, label, shortcut, run: () => act(a) });
    return [
      A("File", "New workbook", { t: "file", a: "new" }, "⌘N"), A("File", "New from template", { t: "start" }, "⇧⌘N"),
      A("File", "Open…", { t: "file", a: "open" }, "⌘O"), A("File", "Save", { t: "file", a: "save" }, "⌘S"),
      A("File", "Save as…", { t: "file", a: "save_as" }, "⇧⌘S"), A("File", "Print or save as PDF", { t: "print" }, "⌘P"),
      A("Edit", "Undo", { t: "undo" }, "⌘Z"), A("Edit", "Redo", { t: "redo" }, "⇧⌘Z"),
      A("Edit", "Copy", { t: "copy" }, "⌘C"), A("Edit", "Cut", { t: "cut" }, "⌘X"), A("Edit", "Paste", { t: "paste" }, "⌘V"),
      A("Edit", "Paste values only", { t: "paste_values" }, "⇧⌘V"), A("Edit", "Fill down", { t: "fill", dir: "down" }, "⌘D"),
      A("Edit", "Fill right", { t: "fill", dir: "right" }, "⌘R"), A("Edit", "Clear contents", { t: "clear", what: "contents" }),
      A("Edit", "Find", { t: "find", replace: false }, "⌘F"), A("Edit", "Replace", { t: "find", replace: true }, "⌘H"), A("Edit", "Go to cell", { t: "goto" }, "⌘G"),
      A("Format", "Bold", { t: "style", path: "font.b", value: String(!style?.b) }, "⌘B"),
      A("Format", "Italic", { t: "style", path: "font.i", value: String(!style?.i) }, "⌘I"),
      A("Format", "Underline", { t: "style", path: "font.u", value: String(!style?.u) }, "⌘U"),
      A("Format", "Strikethrough", { t: "style", path: "font.strike", value: String(!style?.s) }),
      A("Format", "Wrap text", { t: "style", path: "alignment.wrap_text", value: String(!style?.wrap) }),
      A("Format", "Merge & center", { t: "merge", mode: "center" }), A("Format", "Unmerge cells", { t: "merge", mode: "unmerge" }),
      A("Format", "All borders", { t: "border", kind: "All" }), A("Format", "Outside borders", { t: "border", kind: "Outer" }),
      A("Format", "Bottom border", { t: "border", kind: "Bottom" }), A("Format", "No borders", { t: "border", kind: "None" }),
      A("Format", "Currency format", { t: "style", path: "num_fmt", value: "$#,##0.00" }), A("Format", "Percent format", { t: "style", path: "num_fmt", value: "0%" }),
      A("Format", "Number format", { t: "style", path: "num_fmt", value: "#,##0.00" }), A("Format", "Date format", { t: "style", path: "num_fmt", value: "yyyy-mm-dd" }),
      A("Format", "Increase decimals", { t: "decimals", delta: 1 }), A("Format", "Decrease decimals", { t: "decimals", delta: -1 }),
      A("Format", "Align left", { t: "style", path: "alignment.horizontal", value: "left" }),
      A("Format", "Align center", { t: "style", path: "alignment.horizontal", value: "center" }),
      A("Format", "Align right", { t: "style", path: "alignment.horizontal", value: "right" }),
      A("Format", "Format painter", { t: "painter" }), A("Format", "Clear formats", { t: "clear", what: "formats" }),
      A("Format", "Auto-fit column width", { t: "autofit" }),
      A("Insert", "Rows above", { t: "insert", what: "rows" }), A("Insert", "Columns left", { t: "insert", what: "cols" }),
      A("Insert", "Sheet", { t: "insert", what: "sheet" }),
      A("Insert", "Column chart", { t: "chart", kind: "column" }), A("Insert", "Line chart", { t: "chart", kind: "line" }),
      A("Insert", "Pie chart", { t: "chart", kind: "pie" }), A("Insert", "Bar chart", { t: "chart", kind: "bar" }),
      A("Insert", "Area chart", { t: "chart", kind: "area" }), A("Insert", "Scatter chart", { t: "chart", kind: "scatter" }),
      A("Insert", "Doughnut chart", { t: "chart", kind: "doughnut" }),
      A("Insert", "Today's date", { t: "stamp", what: "date" }, "⌘;"), A("Insert", "Current time", { t: "stamp", what: "time" }),
      A("Formulas", "AutoSum", { t: "autosum", fn: "SUM" }), A("Formulas", "Average", { t: "autosum", fn: "AVERAGE" }),
      A("Formulas", "Count", { t: "autosum", fn: "COUNT" }), A("Formulas", "Max", { t: "autosum", fn: "MAX" }), A("Formulas", "Min", { t: "autosum", fn: "MIN" }),
      A("Formulas", "Show formulas", { t: "toggle", what: "formulas" }),
      A("Data", "Sort A to Z", { t: "sort", desc: false }), A("Data", "Sort Z to A", { t: "sort", desc: true }),
      A("Data", filter ? "Remove filter" : "Filter", { t: "filter" }, "⇧⌘L"), A("Data", "Remove duplicates", { t: "dedupe" }),
      A("Data", "Text to columns", { t: "split" }),
      A("Data", "Hide rows", { t: "hide", kind: "rows", hidden: true }), A("Data", "Unhide rows", { t: "hide", kind: "rows", hidden: false }),
      A("Data", "Hide columns", { t: "hide", kind: "cols", hidden: true }), A("Data", "Unhide columns", { t: "hide", kind: "cols", hidden: false }),
      A("View", "Zoom in", { t: "zoom", value: Math.min(2, +(zoom + 0.1).toFixed(2)) }, "⌘+"),
      A("View", "Zoom out", { t: "zoom", value: Math.max(0.5, +(zoom - 0.1).toFixed(2)) }, "⌘−"),
      A("View", "Actual size", { t: "zoom", value: 1 }, "⌘0"), A("View", "Gridlines", { t: "toggle", what: "grid" }),
      A("View", "Freeze panes at selection", { t: "freeze", mode: "selection" }), A("View", "Freeze top row", { t: "freeze", mode: "row" }),
      A("View", "Freeze first column", { t: "freeze", mode: "col" }), A("View", "Unfreeze panes", { t: "freeze", mode: "none" }),
    ];
  }, [act, filter, style, zoom]);

  const paletteExtra = useCallback((q: string): PaletteItem[] => {
    const out: PaletteItem[] = [];
    const cell = parseCellName(q.replace(/^go to\s+/i, ""));
    if (cell && layout && cell.r <= layout.rows && cell.c <= layout.cols) {
      out.push({ id: "goto", group: "Go to", label: `Cell ${cellName(cell)}`, run: () => { setSel({ anchor: cell, focus: cell }); refocus(); } });
    }
    if (!q) return out;
    const lq = q.toLowerCase();
    info?.sheets.forEach((s, i) => {
      if (s.name.toLowerCase().includes(lq) && i !== sheet) out.push({ id: `sheet:${i}`, group: "Sheet", label: s.name, run: () => selectSheet(i) });
    });
    FUNCTIONS.filter((f) => f.name.toLowerCase().startsWith(lq)).slice(0, 5).forEach((f) => out.push({
      id: `fn:${f.name}`, group: "Function", label: f.name, hint: f.desc, run: () => act({ t: "function", name: f.name }),
    }));
    recents.filter((r) => r.name.toLowerCase().includes(lq)).slice(0, 4).forEach((r) => out.push({
      id: `recent:${r.path}`, group: "Open", label: r.name, hint: r.path.replace(/^\/Users\/[^/]+/, "~"), run: () => { guardUnsaved().then((ok) => ok && openPath(r.path)); },
    }));
    return out;
  }, [act, guardUnsaved, info, layout, openPath, recents, selectSheet, sheet]);

  if (!info || !geom || !layout) return <div className="app loading">Opening…</div>;

  const frozen = layout.frozen_rows > 0 || layout.frozen_cols > 0;
  const barValue = editing ? editing.text : input;
  const filterProp = filter ? { r0: filter.r0, c0: filter.c0, c1: filter.c1, active: new Set(Object.keys(filter.criteria).map(Number)) } : null;

  return (
    <div className="app" onMouseDown={() => menu && setMenu(null)}>
      <header className="titlebar" data-tauri-drag-region>
        <AppMark />
        <span className="doc-name" data-tauri-drag-region>{info.name.replace(/\.xlsx$/i, "")}</span>
        <span className={`doc-state${info.dirty ? " is-dirty" : ""}`} data-tauri-drag-region>
          {info.dirty ? "Edited" : info.path ? "Saved" : "Not saved yet"}
        </span>
        <span className="title-spacer" data-tauri-drag-region />
        <button type="button" className="title-search" onClick={() => setPalette(true)} title="Search commands (⌘K)">
          <Command size={13} /> <span>Search commands</span> <kbd>⌘K</kbd>
        </button>
      </header>
      <Ribbon
        act={act}
        s={{ style, canUndo: info.can_undo, canRedo: info.can_redo, frozen, showGrid: layout.show_grid, showFormulas, zoom, painter: !!painter, filterOn: !!filter }}
      />
      <div className="formula-bar">
        <input
          ref={nameBoxRef}
          className="name-box"
          aria-label="Selected cell"
          value={nameBox ?? selectionName(sel)}
          onFocus={(e) => { setNameBox(selectionName(sel)); e.currentTarget.select(); }}
          onChange={(e) => setNameBox(e.target.value)}
          onBlur={() => setNameBox(null)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              const [a, b] = (nameBox ?? "").split(":");
              const ca = parseCellName(a ?? ""), cb = b ? parseCellName(b) : ca;
              if (ca && cb && Math.max(ca.r, cb.r) <= layout.rows && Math.max(ca.c, cb.c) <= layout.cols) setSel({ anchor: ca, focus: cb });
              else say(`"${nameBox}" isn't a cell address. Try something like B12 or A1:C5.`, "error");
              setNameBox(null);
              refocus();
            } else if (e.key === "Escape") {
              setNameBox(null);
              refocus();
            }
          }}
        />
        <span className="fx" aria-hidden>ƒx</span>
        <div className="formula-wrap">
          <input
            className="formula-input"
            aria-label="Cell contents"
            value={barValue}
            spellCheck={false}
            onFocus={() => { if (!editing) startEdit(undefined, "bar"); }}
            onChange={(e) => {
              setCaret(e.target.selectionStart ?? e.target.value.length);
              if (editing) changeEdit(e.target.value);
              else setEditing({ cell: sel.anchor, text: e.target.value, caretAtEnd: false, source: "bar" });
            }}
            onSelect={(e) => setCaret(e.currentTarget.selectionStart ?? 0)}
            onKeyDown={(e) => {
              if (assistKey(e)) return;
              if (e.key === "Enter") { e.preventDefault(); commitEdit([1, 0]); }
              else if (e.key === "Tab") { e.preventDefault(); commitEdit([0, 1]); }
              else if (e.key === "Escape") { e.preventDefault(); cancelEdit(); }
            }}
          />
          {editing?.source === "bar" && assist && <div className="assist-anchor bar">{assist}</div>}
        </div>
      </div>
      <div className="grid-area">
        <Grid
          sheet={sheet}
          geom={geom}
          version={version}
          sel={sel}
          onSelect={(s) => { setSel(s); if (editing && editing.source === "bar") commitEdit([0, 0]); }}
          editing={editing}
          onEditStart={(initial) => startEdit(initial, "cell")}
          onEditChange={changeEdit}
          onEditCommit={commitEdit}
          onEditCancel={cancelEdit}
          onKey={onGridKey}
          onContextMenu={openContextMenu}
          onResize={onResize}
          onClipboard={onClipboard}
          focusToken={focusToken}
          showFormulas={showFormulas}
          refs={refs}
          pointMode={pointMode}
          onPointRef={onPointRef}
          onAutofill={(target) => run("autofill", { range: range(), ...target })}
          onCaret={setCaret}
          assist={assist}
          onEditorKey={assistKey}
          filter={filterProp}
          onFilterButton={openFilterMenu}
          dropdown={!!listItems}
          onDropdown={(x, y, w) => setListPop({ x, y, w })}
          charts={charts}
          selectedChart={selectedChart}
          onChartSelect={setSelectedChart}
          onChartMove={(id, anchor) => run("chart_move", { id, anchor })}
          onChartDelete={(id) => { setSelectedChart(null); run("chart_delete", { id }); }}
          painter={!!painter}
          onSelectEnd={() => {
            if (!painter) return;
            const target = range();
            setPainter(null);
            run("paste_formats", { source: painter, target });
          }}
          onAutofit={(col) => autofit([col])}
        />
        {find && (
          <FindPanel
            replace={find.replace}
            sheets={info.sheets.map((s) => s.name)}
            version={version}
            onClose={() => { setFind(null); refocus(); }}
            onGo={goToHit}
            onReplaced={async (n) => { await refresh(); say(n ? `Replaced ${n} cell${n === 1 ? "" : "s"}` : "Nothing to replace"); }}
          />
        )}
        {start && (
          <StartScreen
            recents={recents}
            onTemplate={useTemplate}
            onOpen={() => fileAction("open")}
            onOpenRecent={async (r) => { if (!(await openPath(r.path))) { removeRecent(r.path); setRecents(loadRecents()); } }}
            onForget={(r) => { removeRecent(r.path); setRecents(loadRecents()); }}
            onClose={() => { setStart(false); refocus(); }}
          />
        )}
      </div>
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
                  refocus();
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
                onClick={() => i !== sheet && selectSheet(i)}
                onDoubleClick={() => setRenaming(i)}
                onContextMenu={(e) => { e.preventDefault(); sheetMenu(i, e.clientX, e.clientY); }}
              >
                {s.color && <span className="tab-dot" style={{ background: s.color }} />}
                {s.name}
              </button>
            )
          ))}
          <button type="button" className="tab-add" aria-label="Add sheet" title="Add sheet" onClick={() => run("add_sheet", {}, { layout: true })}>
            <Plus size={14} />
          </button>
        </nav>
        <ObjectsNotice objects={info.sheets[sheet]?.objects} />
        <div className="stats" aria-live="polite">
          {painter && <span className="mode accent">Format painter: select cells</span>}
          {stats && stats.count > 0 && (
            <>
              {stats.average !== null && <span>Average <b>{fmtNumber(stats.average)}</b></span>}
              <span>Count <b>{stats.count}</b></span>
              {stats.numbers > 0 && <span>Sum <b>{fmtNumber(stats.sum)}</b></span>}
              {stats.min !== null && stats.numbers > 1 && <span>Min <b>{fmtNumber(stats.min)}</b></span>}
              {stats.max !== null && stats.numbers > 1 && <span>Max <b>{fmtNumber(stats.max)}</b></span>}
            </>
          )}
          {!stats && !painter && <span className="mode">{editing ? (pointMode ? "Point" : "Edit") : "Ready"}</span>}
        </div>
        <div className="zoom" role="group" aria-label="Zoom">
          <button type="button" className="zoom-btn" aria-label="Zoom out" onClick={() => setZoom((z) => Math.max(0.5, +(z - 0.1).toFixed(2)))}><Minus size={12} /></button>
          <input type="range" min={50} max={200} step={10} value={Math.round(zoom * 100)} aria-label="Zoom level" onChange={(e) => setZoom(+e.target.value / 100)} />
          <button type="button" className="zoom-btn" aria-label="Zoom in" onClick={() => setZoom((z) => Math.min(2, +(z + 0.1).toFixed(2)))}><Plus size={12} /></button>
          <button type="button" className="zoom-value" title="Reset zoom (⌘0)" onClick={() => setZoom(1)}>{Math.round(zoom * 100)}%</button>
        </div>
      </footer>
      {menu && (
        <div className="menu" role="menu" style={{ left: Math.min(menu.x, window.innerWidth - 260), top: Math.max(8, Math.min(menu.y, window.innerHeight - 30 * menu.items.length - 10)) }} onMouseDown={(e) => e.stopPropagation()}>
          {menu.items.map((it, i) => it.label === "" ? <div key={i} className="menu-sep" role="separator" /> : (
            <button key={i} type="button" role="menuitem" className={`menu-item${it.danger ? " is-danger" : ""}`}
              onClick={() => { setMenu(null); it.action?.(); refocus(); }}>
              <span className="menu-text">{it.label}</span>{it.shortcut && <kbd>{it.shortcut}</kbd>}
            </button>
          ))}
        </div>
      )}
      {filterPop && filter && (
        <FilterPopover
          x={filterPop.x} y={filterPop.y}
          title={colName(filterPop.col)}
          values={filterPop.values}
          selected={filter.criteria[filterPop.col] ? new Set(filter.criteria[filterPop.col]) : null}
          onClose={() => { setFilterPop(null); refocus(); }}
          onSort={async (desc) => {
            await run("sort", { range: { sheet, row: filter.r0 + 1, col: filter.c0, width: filter.c1 - filter.c0 + 1, height: filter.r1 - filter.r0 }, col: filterPop.col, desc });
            await applyFilter(filter);
          }}
          onApply={async (keep) => {
            const criteria = { ...filter.criteria };
            if (keep) criteria[filterPop.col] = [...keep]; else delete criteria[filterPop.col];
            const next = { ...filter, criteria };
            setFilters((fs) => ({ ...fs, [sheet]: next }));
            await applyFilter(next);
          }}
        />
      )}
      {listPop && listItems && (
        <ListPopover
          x={listPop.x} y={listPop.y} width={listPop.w}
          items={listItems}
          current={input}
          onClose={() => { setListPop(null); refocus(); }}
          onPick={async (v) => { setListPop(null); await run("set_input", { cell: { sheet, row: sel.anchor.r, col: sel.anchor.c }, value: v }); refocus(); }}
        />
      )}
      {palette && <Palette items={paletteItems} extra={paletteExtra} onClose={() => { setPalette(false); refocus(); }} />}
      {printing && createPortal(<PrintView {...printing} title={info.name} />, document.body)}
      {toast && <div className={`toast is-${toast.kind}`} role="status">{toast.text}</div>}
    </div>
  );
}
