// The spreadsheet grid: a canvas renderer with a transparent scroller on top.
// The canvas never scrolls; it redraws the visible window from cached cell
// views, fetched from the backend in blocks around the viewport.

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { call, type CellView } from "./api";
import {
  HEADER_H, HEADER_W, colName, colScreenX, hitTest, mergeIndex, normRect, rowScreenY,
  visibleRange, type Cell, type Geometry, type Selection,
} from "./geometry";

export interface Editing { cell: Cell; text: string; caretAtEnd: boolean; source: "cell" | "bar" }

interface Props {
  sheet: number;
  geom: Geometry;
  version: number;
  sel: Selection;
  onSelect: (sel: Selection) => void;
  editing: Editing | null;
  onEditStart: (initial?: string) => void;
  onEditChange: (text: string) => void;
  onEditCommit: (move: [number, number]) => void;
  onEditCancel: () => void;
  onKey: (e: React.KeyboardEvent) => boolean;
  onContextMenu: (x: number, y: number) => void;
  onResize: (kind: "col" | "row", index: number, size: number) => void;
  onClipboard: (kind: "copy" | "cut" | "paste", e: React.ClipboardEvent) => void;
  focusToken: number;
}

const FONT_STACK = `-apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif`;

interface Theme {
  grid: string; gridStrong: string; header: string; headerText: string; headerLine: string;
  accent: string; accentTint: string; accentHeader: string; ink: string; paper: string;
}

function readTheme(el: HTMLElement): Theme {
  const s = getComputedStyle(el);
  const v = (n: string) => s.getPropertyValue(n).trim();
  return {
    grid: v("--grid-line"), gridStrong: v("--grid-line-strong"), header: v("--grid-header"),
    headerText: v("--grid-header-text"), headerLine: v("--grid-header-line"), accent: v("--accent"),
    accentTint: v("--accent-tint"), accentHeader: v("--accent-header"), ink: v("--grid-ink"), paper: v("--grid-paper"),
  };
}

const ptToPx = (pt: number) => (pt * 4) / 3;

export function Grid(p: Props) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<HTMLTextAreaElement>(null);
  // Focus lives in a hidden text field under the active cell, so typed text,
  // IME composition and dictation all arrive as input and open the editor there.
  const sinkRef = useRef<HTMLTextAreaElement>(null);
  const composing = useRef(false);
  const [size, setSize] = useState({ w: 800, h: 600 });
  const [scroll, setScroll] = useState({ left: 0, top: 0 });
  const [resizeGuide, setResizeGuide] = useState<{ kind: "col" | "row"; pos: number } | null>(null);
  const store = useRef(new Map<string, CellView>());
  const fetched = useRef<{ gen: number; rects: number[][]; pending: Set<string> }>({ gen: 0, rects: [], pending: new Set() });
  const [dataTick, setDataTick] = useState(0);
  const themeRef = useRef<Theme | null>(null);
  const { geom, sel } = p;
  const merges = useRef(mergeIndex(geom.layout));

  useEffect(() => { merges.current = mergeIndex(geom.layout); }, [geom]);

  // ---- viewport size + theme
  useLayoutEffect(() => {
    const el = wrapRef.current!;
    themeRef.current = readTheme(el);
    const ro = new ResizeObserver(() => setSize({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    const mq = matchMedia("(prefers-color-scheme: dark)");
    const onTheme = () => { themeRef.current = readTheme(el); setDataTick((t) => t + 1); };
    mq.addEventListener("change", onTheme);
    return () => { ro.disconnect(); mq.removeEventListener("change", onTheme); };
  }, []);

  // ---- data: refetch everything when the sheet or data version changes
  useEffect(() => {
    fetched.current = { gen: fetched.current.gen + 1, rects: [], pending: new Set() };
  }, [p.sheet, p.version, geom]);

  const ensureData = useCallback(() => {
    const g = geom;
    const v = visibleRange(g, scroll.left, scroll.top, size.w, size.h);
    const L = g.layout;
    const pad = (a: number, b: number, n: number, max: number) => [Math.max(1, a - n), Math.min(max, b + n)];
    const [r0, r1] = pad(v.r0, v.r1, 40, L.rows);
    const [c0, c1] = pad(v.c0, v.c1, 8, L.cols);
    const want: number[][] = [[r0, c0, r1, c1]];
    if (L.frozen_rows) want.push([1, c0, L.frozen_rows, c1]);
    if (L.frozen_cols) want.push([r0, 1, r1, L.frozen_cols]);
    if (L.frozen_rows && L.frozen_cols) want.push([1, 1, L.frozen_rows, L.frozen_cols]);
    const f = fetched.current;
    const gen = f.gen;
    const inside = (a: number[], b: number[]) => a[0] >= b[0] && a[1] >= b[1] && a[2] <= b[2] && a[3] <= b[3];
    for (const rect of want) {
      const key = rect.join(",");
      if (f.rects.some((have) => inside(rect, have)) || f.pending.has(key)) continue;
      f.pending.add(key);
      call<CellView[]>("cells", { sheet: p.sheet, range: rect }).then((cells) => {
        const cur = fetched.current;
        if (cur.gen !== gen) return;
        if (cur.rects.length === 0) store.current = new Map(); // first block of a new generation
        for (const c of cells) store.current.set(`${c.r},${c.c}`, c);
        cur.rects.push(rect);
        cur.pending.delete(key);
        setDataTick((t) => t + 1);
      }).catch(() => fetched.current.pending.delete(key));
    }
  }, [geom, scroll, size, p.sheet]);

  useEffect(() => { ensureData(); }, [ensureData, p.version]);

  // ---- drawing
  useEffect(() => {
    const canvas = canvasRef.current!;
    const theme = themeRef.current;
    if (!theme) return;
    const dpr = window.devicePixelRatio || 1;
    if (canvas.width !== Math.round(size.w * dpr) || canvas.height !== Math.round(size.h * dpr)) {
      canvas.width = Math.round(size.w * dpr);
      canvas.height = Math.round(size.h * dpr);
    }
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    draw(ctx, theme, geom, store.current, merges.current, sel, scroll, size, p.editing?.cell ?? null);
  }, [geom, sel, scroll, size, dataTick, p.editing]);

  // ---- keep the active cell visible
  useEffect(() => {
    const el = scrollRef.current!;
    const g = geom;
    const { r, c } = sel.focus;
    const vw = el.clientWidth - HEADER_W - g.frozenW; // clientWidth excludes the scrollbar
    const vh = el.clientHeight - HEADER_H - g.frozenH;
    let { scrollLeft, scrollTop } = el;
    if (c > g.layout.frozen_cols) {
      const x0 = g.colX[c - 1] - g.frozenW;
      const x1 = g.colX[c] - g.frozenW;
      if (x0 < scrollLeft) scrollLeft = x0;
      else if (x1 > scrollLeft + vw) scrollLeft = x1 - vw;
    }
    if (r > g.layout.frozen_rows) {
      const y0 = g.rowY[r - 1] - g.frozenH;
      const y1 = g.rowY[r] - g.frozenH;
      if (y0 < scrollTop) scrollTop = y0;
      else if (y1 > scrollTop + vh) scrollTop = y1 - vh;
    }
    if (scrollLeft !== el.scrollLeft || scrollTop !== el.scrollTop) el.scrollTo(scrollLeft, scrollTop);
  }, [sel.focus.r, sel.focus.c, geom]); // eslint-disable-line react-hooks/exhaustive-deps

  // ---- focus management
  useEffect(() => {
    if (p.editing) {
      if (p.editing.source === "cell") editorRef.current?.focus();
    } else sinkRef.current?.focus({ preventScroll: true });
  }, [p.editing === null, p.focusToken]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    const ed = editorRef.current;
    if (p.editing && ed && document.activeElement === ed && p.editing.caretAtEnd) {
      ed.selectionStart = ed.selectionEnd = ed.value.length;
    }
  }, [p.editing?.cell.r, p.editing?.cell.c]); // eslint-disable-line react-hooks/exhaustive-deps

  // ---- mouse
  const local = (e: { clientX: number; clientY: number }) => {
    const r = scrollRef.current!.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };

  const resizeTarget = (x: number, y: number): { kind: "col" | "row"; index: number } | null => {
    const g = geom;
    if (y < HEADER_H && x > HEADER_W) {
      const hit = hitTest(g, x, HEADER_H + 1, scroll.left, scroll.top);
      for (const c of [hit.c - 1, hit.c]) {
        if (c >= 1 && Math.abs(colScreenX(g, c + 1, scroll.left) - x) <= 4) return { kind: "col", index: c };
      }
    }
    if (x < HEADER_W && y > HEADER_H) {
      const hit = hitTest(g, HEADER_W + 1, y, scroll.left, scroll.top);
      for (const r of [hit.r - 1, hit.r]) {
        if (r >= 1 && Math.abs(rowScreenY(g, r + 1, scroll.top) - y) <= 3) return { kind: "row", index: r };
      }
    }
    return null;
  };

  const onMouseDown = (e: React.MouseEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    sinkRef.current?.focus({ preventScroll: true });
    const { x, y } = local(e);
    const g = geom;
    const L = g.layout;
    if (p.editing) p.onEditCommit([0, 0]);
    const rz = resizeTarget(x, y);
    if (rz) {
      e.preventDefault();
      const start = rz.kind === "col" ? colScreenX(g, rz.index, scroll.left) : rowScreenY(g, rz.index, scroll.top);
      let pos = rz.kind === "col" ? x : y;
      const move = (ev: MouseEvent) => {
        const l = local(ev);
        pos = Math.max(start + 8, rz.kind === "col" ? l.x : l.y);
        setResizeGuide({ kind: rz.kind, pos });
      };
      const up = () => {
        window.removeEventListener("mousemove", move);
        window.removeEventListener("mouseup", up);
        setResizeGuide(null);
        p.onResize(rz.kind, rz.index, Math.round(pos - start));
      };
      window.addEventListener("mousemove", move);
      window.addEventListener("mouseup", up);
      return;
    }
    const hit = hitTest(g, x, y, scroll.left, scroll.top);
    let mode: "cell" | "col" | "row";
    let next: Selection;
    if (hit.r === 0 && hit.c === 0) {
      p.onSelect({ anchor: { r: 1, c: 1 }, focus: { r: L.rows, c: L.cols } });
      return;
    } else if (hit.r === 0) {
      mode = "col";
      next = e.shiftKey ? { anchor: { r: 1, c: sel.anchor.c }, focus: { r: L.rows, c: hit.c } } : { anchor: { r: 1, c: hit.c }, focus: { r: L.rows, c: hit.c } };
    } else if (hit.c === 0) {
      mode = "row";
      next = e.shiftKey ? { anchor: { r: sel.anchor.r, c: 1 }, focus: { r: hit.r, c: L.cols } } : { anchor: { r: hit.r, c: 1 }, focus: { r: hit.r, c: L.cols } };
    } else {
      mode = "cell";
      next = e.shiftKey ? { anchor: sel.anchor, focus: hit } : { anchor: hit, focus: hit };
    }
    p.onSelect(next);
    const anchor = next.anchor;
    const move = (ev: MouseEvent) => {
      const l = local(ev);
      const h = hitTest(g, Math.max(l.x, HEADER_W + 1), Math.max(l.y, HEADER_H + 1), scrollRef.current!.scrollLeft, scrollRef.current!.scrollTop);
      if (mode === "col") p.onSelect({ anchor, focus: { r: L.rows, c: h.c } });
      else if (mode === "row") p.onSelect({ anchor, focus: { r: h.r, c: L.cols } });
      else p.onSelect({ anchor, focus: h });
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  const onMouseMove = (e: React.MouseEvent) => {
    const { x, y } = local(e);
    const rz = resizeTarget(x, y);
    scrollRef.current!.style.cursor = rz ? (rz.kind === "col" ? "col-resize" : "row-resize") : "cell";
  };

  const onDoubleClick = (e: React.MouseEvent) => {
    const { x, y } = local(e);
    if (x > HEADER_W && y > HEADER_H) p.onEditStart();
  };

  const onContextMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    const { x, y } = local(e);
    const hit = hitTest(geom, x, y, scroll.left, scroll.top);
    const { r0, r1, c0, c1 } = normRect(sel);
    if (hit.r > 0 && hit.c > 0 && (hit.r < r0 || hit.r > r1 || hit.c < c0 || hit.c > c1)) {
      p.onSelect({ anchor: hit, focus: hit });
    }
    p.onContextMenu(e.clientX, e.clientY);
  };

  // ---- keyboard (grid focused, not editing)
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (p.onKey(e)) return;
    const L = geom.layout;
    const mod = e.metaKey || e.ctrlKey;
    const clamp = (c: Cell): Cell => ({ r: Math.min(Math.max(c.r, 1), L.rows), c: Math.min(Math.max(c.c, 1), L.cols) });
    const pageRows = Math.max(1, Math.floor((size.h - HEADER_H - geom.frozenH) / 24) - 1);
    const moves: Record<string, [number, number]> = {
      ArrowUp: [-1, 0], ArrowDown: [1, 0], ArrowLeft: [0, -1], ArrowRight: [0, 1],
      PageUp: [-pageRows, 0], PageDown: [pageRows, 0],
    };
    if (e.key in moves) {
      e.preventDefault();
      let [dr, dc] = moves[e.key];
      const from = e.shiftKey ? sel.focus : sel.anchor;
      if (mod && e.key.startsWith("Arrow")) {
        dr = dr < 0 ? -L.rows : dr > 0 ? L.rows : 0;
        dc = dc < 0 ? -L.cols : dc > 0 ? L.cols : 0;
      }
      const to = clamp({ r: from.r + dr, c: from.c + dc });
      p.onSelect(e.shiftKey ? { anchor: sel.anchor, focus: to } : { anchor: to, focus: to });
      return;
    }
    if (e.key === "Enter" || e.key === "Tab") {
      e.preventDefault();
      const back = e.shiftKey ? -1 : 1;
      const to = clamp(e.key === "Enter" ? { r: sel.anchor.r + back, c: sel.anchor.c } : { r: sel.anchor.r, c: sel.anchor.c + back });
      p.onSelect({ anchor: to, focus: to });
      return;
    }
    if (e.key === "Home") {
      e.preventDefault();
      const to = mod ? { r: 1, c: 1 } : { r: sel.anchor.r, c: 1 };
      p.onSelect({ anchor: to, focus: to });
      return;
    }
    if (e.key === "F2") {
      e.preventDefault();
      p.onEditStart();
      return;
    }
    // printable keys fall through to the sink's input event (see onSinkInput)
  };

  const onSinkInput = (e: React.FormEvent<HTMLTextAreaElement>) => {
    if (composing.current) return;
    const v = e.currentTarget.value;
    e.currentTarget.value = "";
    if (v) p.onEditStart(v);
  };

  // ---- editor keys
  const onEditorKey = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.altKey) {
      e.preventDefault();
      p.onEditCommit(e.shiftKey ? [-1, 0] : [1, 0]);
    } else if (e.key === "Tab") {
      e.preventDefault();
      p.onEditCommit(e.shiftKey ? [0, -1] : [0, 1]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      p.onEditCancel();
    } else if (e.key === "Enter" && e.altKey) {
      e.preventDefault();
      const ed = e.currentTarget;
      const v = ed.value;
      const at = ed.selectionStart;
      p.onEditChange(v.slice(0, at) + "\n" + v.slice(ed.selectionEnd));
      requestAnimationFrame(() => { ed.selectionStart = ed.selectionEnd = at + 1; });
    }
  };

  // ---- editor placement
  let editorStyle: React.CSSProperties | undefined;
  if (p.editing) {
    const { r, c } = p.editing.cell;
    const m = merges.current.anchors.get(`${r},${c}`);
    const x = colScreenX(geom, c, scroll.left);
    const y = rowScreenY(geom, r, scroll.top);
    const w = m ? geom.colX[c - 1 + m.w] - geom.colX[c - 1] : geom.layout.col_widths[c - 1];
    const h = m ? geom.rowY[r - 1 + m.h] - geom.rowY[r - 1] : geom.layout.row_heights[r - 1];
    const cv = store.current.get(`${r},${c}`);
    editorStyle = {
      left: x, top: y, minWidth: Math.max(w, 40) + 1, minHeight: h + 1,
      maxWidth: Math.max(size.w - x - 8, w + 1),
      font: `${cv?.i ? "italic " : ""}${cv?.b ? "600 " : ""}${ptToPx(cv?.size || 11)}px ${FONT_STACK}`,
      width: Math.max(w + 1, Math.min(size.w - x - 8, measureText(p.editing.text, ptToPx(cv?.size || 11)) + 24)),
    };
  }

  const sinkStyle: React.CSSProperties = {
    left: Math.min(Math.max(colScreenX(geom, sel.anchor.c, scroll.left), HEADER_W), size.w - 4),
    top: Math.min(Math.max(rowScreenY(geom, sel.anchor.r, scroll.top), HEADER_H), size.h - 4),
  };
  const L = geom.layout;
  return (
    <div className="grid" ref={wrapRef}>
      <canvas ref={canvasRef} style={{ width: size.w, height: size.h }} />
      <div
        className="grid-scroller"
        ref={scrollRef}
        tabIndex={-1}
        onScroll={(e) => setScroll({ left: e.currentTarget.scrollLeft, top: e.currentTarget.scrollTop })}
        onMouseDown={onMouseDown}
        onMouseMove={onMouseMove}
        onDoubleClick={onDoubleClick}
        onContextMenu={onContextMenu}
      >
        <div style={{ width: HEADER_W + geom.colX[L.cols], height: HEADER_H + geom.rowY[L.rows] }} />
      </div>
      <textarea
        ref={sinkRef}
        className="input-sink"
        aria-label="Spreadsheet grid. Type to edit the selected cell."
        style={sinkStyle}
        autoCapitalize="off"
        autoComplete="off"
        spellCheck={false}
        onKeyDown={onKeyDown}
        onInput={onSinkInput}
        onCompositionStart={() => { composing.current = true; }}
        onCompositionEnd={(e) => { composing.current = false; onSinkInput(e); }}
        onCopy={(e) => p.onClipboard("copy", e)}
        onCut={(e) => p.onClipboard("cut", e)}
        onPaste={(e) => p.onClipboard("paste", e)}
      />
      {resizeGuide && (
        <div
          className="resize-guide"
          style={resizeGuide.kind === "col" ? { left: resizeGuide.pos, top: 0, bottom: 0, width: 1 } : { top: resizeGuide.pos, left: 0, right: 0, height: 1 }}
        />
      )}
      {p.editing && (
        <textarea
          ref={editorRef}
          className="cell-editor"
          style={editorStyle}
          value={p.editing.text}
          spellCheck={false}
          onChange={(e) => p.onEditChange(e.target.value)}
          onKeyDown={onEditorKey}
        />
      )}
    </div>
  );
}

let measureCtx: CanvasRenderingContext2D | null = null;
function measureText(text: string, px: number): number {
  measureCtx ??= document.createElement("canvas").getContext("2d");
  measureCtx!.font = `${px}px ${FONT_STACK}`;
  return Math.max(...text.split("\n").map((l) => measureCtx!.measureText(l).width));
}

// --------------------------------------------------------------------------- renderer

function draw(
  ctx: CanvasRenderingContext2D, t: Theme, g: Geometry, cells: Map<string, CellView>,
  merges: ReturnType<typeof mergeIndex>, sel: Selection, scroll: { left: number; top: number },
  size: { w: number; h: number }, editingCell: Cell | null,
) {
  const L = g.layout;
  const { w, h } = size;
  ctx.fillStyle = t.paper;
  ctx.fillRect(0, 0, w, h);
  const v = visibleRange(g, scroll.left, scroll.top, w, h);

  const rowSpans: { r0: number; r1: number; oy: number; y0: number; y1: number }[] = [];
  if (L.frozen_rows) rowSpans.push({ r0: 1, r1: L.frozen_rows, oy: 0, y0: HEADER_H, y1: HEADER_H + g.frozenH });
  rowSpans.push({ r0: v.r0, r1: v.r1, oy: -scroll.top, y0: HEADER_H + g.frozenH, y1: h });
  const colSpans: { c0: number; c1: number; ox: number; x0: number; x1: number }[] = [];
  if (L.frozen_cols) colSpans.push({ c0: 1, c1: L.frozen_cols, ox: 0, x0: HEADER_W, x1: HEADER_W + g.frozenW });
  colSpans.push({ c0: v.c0, c1: v.c1, ox: -scroll.left, x0: HEADER_W + g.frozenW, x1: w });

  for (const rs of rowSpans) {
    for (const cs of colSpans) {
      if (rs.r0 > rs.r1 || cs.c0 > cs.c1) continue;
      ctx.save();
      ctx.beginPath();
      ctx.rect(cs.x0, rs.y0, cs.x1 - cs.x0, rs.y1 - rs.y0);
      ctx.clip();
      drawCells(ctx, t, g, cells, merges, rs.r0, rs.r1, cs.c0, cs.c1, cs.ox, rs.oy, editingCell);
      drawSelection(ctx, t, g, sel, cs.ox, rs.oy, merges);
      ctx.restore();
    }
  }

  // frozen pane dividers
  ctx.fillStyle = t.gridStrong;
  if (L.frozen_rows) ctx.fillRect(0, HEADER_H + g.frozenH - 1, w, 1);
  if (L.frozen_cols) ctx.fillRect(HEADER_W + g.frozenW - 1, 0, 1, h);

  drawHeaders(ctx, t, g, sel, colSpans, rowSpans, w, h);
}

function cellRect(g: Geometry, r: number, c: number, ox: number, oy: number, m?: { w: number; h: number }) {
  const x = HEADER_W + g.colX[c - 1] + ox;
  const y = HEADER_H + g.rowY[r - 1] + oy;
  const cw = g.colX[Math.min(c - 1 + (m?.w ?? 1), g.layout.cols)] - g.colX[c - 1];
  const ch = g.rowY[Math.min(r - 1 + (m?.h ?? 1), g.layout.rows)] - g.rowY[r - 1];
  return { x, y, w: cw, h: ch };
}

function drawCells(
  ctx: CanvasRenderingContext2D, t: Theme, g: Geometry, cells: Map<string, CellView>,
  merges: ReturnType<typeof mergeIndex>, r0: number, r1: number, c0: number, c1: number, ox: number, oy: number,
  editingCell: Cell | null,
) {
  const L = g.layout;
  const xs = (c: number) => HEADER_W + g.colX[c - 1] + ox;
  const ys = (r: number) => HEADER_H + g.rowY[r - 1] + oy;
  const top = ys(r0), bottom = ys(r1 + 1), left = xs(c0), right = xs(c1 + 1);

  // gridlines
  if (L.show_grid) {
    ctx.fillStyle = t.grid;
    for (let c = c0; c <= c1 + 1; c++) if (c === 1 || L.col_widths[c - 2] > 0) ctx.fillRect(Math.round(xs(c)) - 1, top, 1, bottom - top);
    for (let r = r0; r <= r1 + 1; r++) if (r === 1 || L.row_heights[r - 2] > 0) ctx.fillRect(left, Math.round(ys(r)) - 1, right - left, 1);
  }

  const visible: CellView[] = [];
  for (let r = r0; r <= r1; r++) {
    for (let c = c0; c <= c1; c++) {
      const cv = cells.get(`${r},${c}`);
      if (cv) visible.push(cv);
    }
  }
  // merged ranges whose anchor is outside the window still need painting
  const seenMerge = new Set<string>();
  for (const [key, m] of merges.covered) {
    const [r, c] = key.split(",").map(Number);
    if (r < r0 || r > r1 || c < c0 || c > c1) continue;
    const ak = `${m.r},${m.c}`;
    if (seenMerge.has(ak)) continue;
    seenMerge.add(ak);
    const rect = cellRect(g, m.r, m.c, ox, oy, m);
    const cv = cells.get(ak);
    ctx.fillStyle = cv?.fill ?? t.paper;
    ctx.fillRect(rect.x, rect.y, rect.w - 1, rect.h - 1);
    if (cv && (m.r < r0 || m.c < c0)) visible.push(cv);
  }

  // fills
  for (const cv of visible) {
    if (!cv.fill || merges.covered.has(`${cv.r},${cv.c}`)) continue;
    const rc = cellRect(g, cv.r, cv.c, ox, oy);
    ctx.fillStyle = cv.fill;
    ctx.fillRect(rc.x - 1, rc.y - 1, rc.w + 1, rc.h + 1);
  }

  // borders
  ctx.fillStyle = t.ink;
  for (const cv of visible) {
    if (!cv.borders) continue;
    const rc = cellRect(g, cv.r, cv.c, ox, oy, merges.anchors.get(`${cv.r},${cv.c}`));
    const [bt, br, bb, bl] = cv.borders;
    if (bt) ctx.fillRect(rc.x - 1, Math.round(rc.y) - 1, rc.w + 1, 1);
    if (bb) ctx.fillRect(rc.x - 1, Math.round(rc.y + rc.h) - 1, rc.w + 1, 1);
    if (bl) ctx.fillRect(Math.round(rc.x) - 1, rc.y - 1, 1, rc.h + 1);
    if (br) ctx.fillRect(Math.round(rc.x + rc.w) - 1, rc.y - 1, 1, rc.h + 1);
  }

  // text
  ctx.textBaseline = "alphabetic";
  const isEmpty = (r: number, c: number) => {
    const cv = cells.get(`${r},${c}`);
    return (!cv || cv.text === "") && !merges.covered.has(`${r},${c}`);
  };
  for (const cv of visible) {
    if (!cv.text) continue;
    const key = `${cv.r},${cv.c}`;
    const m = merges.anchors.get(key);
    if (!m && merges.covered.has(key)) continue;
    if (editingCell && editingCell.r === cv.r && editingCell.c === cv.c) continue;
    const rc = cellRect(g, cv.r, cv.c, ox, oy, m);
    if (rc.w <= 0 || rc.h <= 0) continue;
    const px = ptToPx(cv.size || 11);
    ctx.font = `${cv.i ? "italic " : ""}${cv.b ? "600 " : ""}${px}px ${cv.font ? `"${cv.font}", ` : ""}${FONT_STACK}`;
    ctx.fillStyle = cv.color ?? t.ink;
    const padX = 4;
    const lines = cv.wrap ? wrapLines(ctx, cv.text, rc.w - padX * 2) : cv.text.split("\n").slice(0, 1);
    const lineH = px * 1.25;
    const blockH = lineH * lines.length;
    let y0: number;
    if (cv.valign === "top") y0 = rc.y + px + 3;
    else if (cv.valign === "middle") y0 = rc.y + (rc.h - blockH) / 2 + px;
    else y0 = rc.y + rc.h - blockH + px * 1.0 - 4 + (lineH - px) * 0.2;

    // overflow into empty neighbours for single-line text (like Excel)
    let clipL = rc.x, clipR = rc.x + rc.w;
    const textW = Math.max(...lines.map((l) => ctx.measureText(l).width));
    if (!cv.wrap && !m && textW > rc.w - padX * 2) {
      const numeric = cv.align === "right" && /^[-+$€£(]?[\d.,%E+-]+\)?$/.test(cv.text);
      if (numeric) {
        // like Excel: drop decimals until it fits, and only show ### if even the integer part doesn't
        const room = rc.w - padX * 2;
        const m = /^(-?\d+)\.(\d+)$/.exec(cv.text);
        let fitted: string | null = null;
        for (let d = (m?.[2].length ?? 0) - 1; m && d >= 0 && !fitted; d--) {
          const t = String(Number(Number(cv.text).toFixed(d))); // String() drops trailing zeros
          if (ctx.measureText(t).width <= room) fitted = t;
        }
        lines[0] = fitted ?? "#".repeat(Math.max(1, Math.floor(room / ctx.measureText("#").width)));
      } else {
        if (cv.align !== "right") for (let c = cv.c + 1; c <= L.cols && clipR - rc.x < textW + padX * 2 && isEmpty(cv.r, c); c++) clipR += L.col_widths[c - 1];
        if (cv.align !== "left") for (let c = cv.c - 1; c >= 1 && clipR - clipL < textW + padX * 2 && isEmpty(cv.r, c); c--) clipL -= L.col_widths[c - 1];
      }
    }
    ctx.save();
    ctx.beginPath();
    ctx.rect(clipL, rc.y, clipR - clipL - 1, rc.h - 1);
    ctx.clip();
    lines.forEach((line, i) => {
      const lw = ctx.measureText(line).width;
      const x = cv.align === "right" ? rc.x + rc.w - padX - lw : cv.align === "center" ? rc.x + (rc.w - lw) / 2 : rc.x + padX;
      const y = y0 + i * lineH;
      ctx.fillText(line, x, y);
      if (cv.u) ctx.fillRect(x, y + 2, lw, 1);
      if (cv.s) ctx.fillRect(x, y - px * 0.32, lw, 1);
    });
    ctx.restore();
  }
}

function wrapLines(ctx: CanvasRenderingContext2D, text: string, maxW: number): string[] {
  const out: string[] = [];
  for (const para of text.split("\n")) {
    let line = "";
    for (const word of para.split(/(\s+)/)) {
      const next = line + word;
      if (line && ctx.measureText(next).width > maxW) {
        out.push(line.trimEnd());
        line = word.trimStart();
      } else line = next;
    }
    out.push(line);
  }
  return out;
}

function drawSelection(ctx: CanvasRenderingContext2D, t: Theme, g: Geometry, sel: Selection, ox: number, oy: number, merges: ReturnType<typeof mergeIndex>) {
  const { r0, r1, c0, c1 } = normRect(sel);
  const x = HEADER_W + g.colX[c0 - 1] + ox;
  const y = HEADER_H + g.rowY[r0 - 1] + oy;
  const w = g.colX[c1] - g.colX[c0 - 1];
  const h = g.rowY[r1] - g.rowY[r0 - 1];
  const a = sel.anchor;
  const am = merges.anchors.get(`${a.r},${a.c}`);
  const ar = cellRect(g, a.r, a.c, ox, oy, am);
  if (r0 !== r1 || c0 !== c1) {
    ctx.fillStyle = t.accentTint;
    ctx.beginPath();
    ctx.rect(x, y, w, h);
    ctx.rect(ar.x + ar.w, ar.y, -ar.w, ar.h); // hole for the active cell (opposite winding)
    ctx.fill("evenodd");
  }
  ctx.strokeStyle = t.accent;
  ctx.lineWidth = 2;
  ctx.strokeRect(Math.round(x) - 1, Math.round(y) - 1, Math.round(w), Math.round(h));
  if (r0 !== r1 || c0 !== c1) {
    ctx.lineWidth = 1;
    ctx.strokeRect(Math.round(ar.x) + 0.5, Math.round(ar.y) + 0.5, Math.round(ar.w) - 2, Math.round(ar.h) - 2);
  }
}

function drawHeaders(
  ctx: CanvasRenderingContext2D, t: Theme, g: Geometry, sel: Selection,
  colSpans: { c0: number; c1: number; ox: number; x0: number; x1: number }[],
  rowSpans: { r0: number; r1: number; oy: number; y0: number; y1: number }[], w: number, h: number,
) {
  const L = g.layout;
  const { r0, r1, c0, c1 } = normRect(sel);
  const wholeCols = r0 === 1 && r1 === L.rows;
  const wholeRows = c0 === 1 && c1 === L.cols;
  ctx.font = `12px ${FONT_STACK}`;
  ctx.textBaseline = "middle";
  ctx.textAlign = "center";

  ctx.fillStyle = t.header;
  ctx.fillRect(0, 0, w, HEADER_H);
  ctx.fillRect(0, 0, HEADER_W, h);

  for (const cs of colSpans) {
    ctx.save();
    ctx.beginPath();
    ctx.rect(cs.x0, 0, cs.x1 - cs.x0, HEADER_H);
    ctx.clip();
    for (let c = cs.c0; c <= cs.c1; c++) {
      const cw = L.col_widths[c - 1];
      if (cw <= 0) continue;
      const x = HEADER_W + g.colX[c - 1] + cs.ox;
      const selected = c >= c0 && c <= c1;
      if (selected) {
        ctx.fillStyle = wholeCols ? t.accent : t.accentHeader;
        ctx.fillRect(x, 0, cw, HEADER_H);
        ctx.fillStyle = t.accent;
        ctx.fillRect(x, HEADER_H - 2, cw, 2);
      }
      ctx.fillStyle = t.headerLine;
      ctx.fillRect(Math.round(x + cw) - 1, 0, 1, HEADER_H);
      ctx.fillStyle = selected ? (wholeCols ? "#fff" : t.accent) : t.headerText;
      ctx.fillText(colName(c), x + cw / 2, HEADER_H / 2 + 0.5);
    }
    ctx.restore();
  }
  for (const rs of rowSpans) {
    ctx.save();
    ctx.beginPath();
    ctx.rect(0, rs.y0, HEADER_W, rs.y1 - rs.y0);
    ctx.clip();
    for (let r = rs.r0; r <= rs.r1; r++) {
      const rh = L.row_heights[r - 1];
      if (rh <= 0) continue;
      const y = HEADER_H + g.rowY[r - 1] + rs.oy;
      const selected = r >= r0 && r <= r1;
      if (selected) {
        ctx.fillStyle = wholeRows ? t.accent : t.accentHeader;
        ctx.fillRect(0, y, HEADER_W, rh);
        ctx.fillStyle = t.accent;
        ctx.fillRect(HEADER_W - 2, y, 2, rh);
      }
      ctx.fillStyle = t.headerLine;
      ctx.fillRect(0, Math.round(y + rh) - 1, HEADER_W, 1);
      ctx.fillStyle = selected ? (wholeRows ? "#fff" : t.accent) : t.headerText;
      ctx.fillText(String(r), HEADER_W / 2, y + rh / 2 + 0.5);
    }
    ctx.restore();
  }
  ctx.fillStyle = t.headerLine;
  ctx.fillRect(0, HEADER_H - 1, w, 1);
  ctx.fillRect(HEADER_W - 1, 0, 1, h);
  ctx.fillStyle = t.header;
  ctx.fillRect(0, 0, HEADER_W - 1, HEADER_H - 1);
  // corner: a small triangle, like the select-all button it is
  ctx.fillStyle = t.headerLine;
  ctx.beginPath();
  ctx.moveTo(HEADER_W - 6, HEADER_H - 6);
  ctx.lineTo(HEADER_W - 6, HEADER_H - 16);
  ctx.lineTo(HEADER_W - 16, HEADER_H - 6);
  ctx.fill();
  ctx.textAlign = "start";
}
