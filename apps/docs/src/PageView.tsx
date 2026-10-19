// The page view: every page drawn on its own canvas from Oxi's layout, stacked
// on a desk. Only pages near the viewport are drawn. The caret is a DOM element
// (so it can blink without redrawing) and a hidden textarea at the caret takes
// keyboard and IME input.

import { forwardRef, useCallback, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Item, LayoutData, Page, Pos, TextItem } from "./api";
import { before, keyOf, order, samePos } from "./api";
import { anchoredItems, caretRect, charX, contentWidth, cssFont, hit, pageLeft, pageTops, type Placed } from "./geometry";

export interface Mark { s?: string; p: number[]; o: number; len: number }

export interface PageViewHandle {
  focus: () => void;
  scrollToCaret: () => void;
  items: () => Placed[][];
  /** Render every page to PNG data URLs (for printing). */
  snapshot: (scale: number) => string[];
  /** The caret's box on screen (for popovers at the caret). */
  caretBox: () => DOMRect | null;
}

interface Props {
  layout: LayoutData;
  zoom: number;
  caret: Pos | null;
  anchor: Pos | null;
  ghost: string;
  marks: Mark[];
  current: number;
  /** Commented ranges to highlight; `active` is drawn stronger. */
  comments: { from: Pos; to: Pos; active: boolean }[];
  /** Misspelled words (red squiggles). */
  squiggles: Mark[];
  onSelectEnd?: () => void;
  onPoint: (p: Pos, extend: boolean, meta: boolean) => void;
  onDrag: (p: Pos) => void;
  onWord: (p: Pos) => void;
  onKeyDown: (e: React.KeyboardEvent<HTMLTextAreaElement>) => void;
  onText: (text: string) => void;
  onPaste: (text: string, html: string) => void;
  onCopy: (cut: boolean) => void;
  onContext: (e: React.MouseEvent, p: Pos | null) => void;
  onScroll?: (page: number) => void;
}

const images = new Map<string, HTMLImageElement>();

function image(id: string, src: string, onLoad: () => void): HTMLImageElement | null {
  let img = images.get(id);
  if (!img) {
    img = new Image();
    img.onload = onLoad;
    img.src = src;
    images.set(id, img);
  }
  return img.complete && img.naturalWidth ? img : null;
}

function dash(style: string | null): number[] {
  switch (style) {
    case "dotted": return [1, 2];
    case "dashed": case "dashSmallGap": return [4, 2];
    case "dotDash": case "dashDotStroked": return [4, 2, 1, 2];
    default: return [];
  }
}

interface Range { from: Pos; to: Pos }

function drawText(ctx: CanvasRenderingContext2D, it: TextItem) {
  if (!it.text) return;
  ctx.font = cssFont(it);
  const measured = ctx.measureText(it.text).width;
  const sx = measured > 0 && it.w > 0 ? Math.min(1.6, Math.max(0.5, it.w / measured)) : 1;
  ctx.fillStyle = it.color ?? "#000";
  ctx.save();
  ctx.translate(it.x, it.y + it.base);
  if (it.vert) ctx.rotate(Math.PI / 2);
  ctx.scale(sx, 1);
  ctx.fillText(it.text, 0, 0);
  ctx.restore();
  const lineAt = (dy: number) => {
    ctx.fillRect(it.x, it.y + dy, it.w, Math.max(0.5, it.size / 18));
  };
  if (it.u) lineAt(it.base + it.size * 0.12);
  if (it.s) lineAt(it.base - it.size * 0.28);
}

type CommentRange = { from: Pos; to: Pos; active: boolean };

/** The part of a text item inside `from..to`, as character offsets in the item. */
function overlap(it: TextItem, from: Pos, to: Pos): [number, number] | null {
  if (it.p === undefined || it.del || (it.st ?? "") !== (from.s ?? "")) return null;
  const n = it.n ?? 0;
  const start: Pos = { s: it.st, p: it.p, o: it.off! }, end: Pos = { s: it.st, p: it.p, o: it.off! + n };
  const k = keyOf(start);
  const lo = !before(start, from) ? 0 : keyOf(from) === k ? from.o - it.off! : n + 1;
  const hi = !before(to, end) ? n : keyOf(to) === k ? to.o - it.off! : -1;
  return lo > n || hi < 0 || hi <= lo ? null : [lo, hi];
}

function squiggle(ctx: CanvasRenderingContext2D, x0: number, x1: number, y: number) {
  ctx.save();
  ctx.strokeStyle = "rgba(220, 38, 38, 0.9)";
  ctx.lineWidth = 0.8;
  ctx.beginPath();
  for (let x = x0, up = true; x < x1; x += 1.6, up = !up) ctx.lineTo(x, y + (up ? 0 : 1.2));
  ctx.stroke();
  ctx.restore();
}

function drawPage(ctx: CanvasRenderingContext2D, page: Page, imgs: Record<string, string>, sel: Range | null, marks: Mark[], current: number, redraw: () => void, comments: CommentRange[] = [], squiggles: Mark[] = []) {
  ctx.fillStyle = "#fff";
  ctx.fillRect(0, 0, page.w, page.h);
  const inSel = (it: TextItem): [number, number] | null => {
    if (!sel || it.p === undefined || it.del || (it.st ?? "") !== (sel.from.s ?? "")) return null;
    const n = it.n ?? 0;
    const start: Pos = { s: it.st, p: it.p, o: it.off! }, end: Pos = { s: it.st, p: it.p, o: it.off! + n };
    const k = keyOf(start);
    const lo = !before(start, sel.from) ? 0 : keyOf(sel.from) === k ? sel.from.o - it.off! : n + 1;
    const hi = !before(sel.to, end) ? n : keyOf(sel.to) === k ? sel.to.o - it.off! : -1;
    if (lo > n || hi < 0 || hi <= lo) {
      // A selection running through an empty paragraph still marks it.
      return n === 0 && before(sel.from, start) && before(end, sel.to) ? [0, 0] : null;
    }
    return [lo, hi];
  };
  for (const item of page.items as Item[]) {
    switch (item.k) {
      case "rect":
        if (item.fill) { ctx.fillStyle = item.fill; ctx.fillRect(item.x, item.y, item.w, item.h); }
        if (item.stroke && item.sw) { ctx.strokeStyle = item.stroke; ctx.lineWidth = item.sw; ctx.strokeRect(item.x, item.y, item.w, item.h); }
        break;
      case "line":
        ctx.save();
        ctx.strokeStyle = item.color ?? "#000";
        ctx.lineWidth = Math.max(0.5, item.w);
        ctx.setLineDash(dash(item.dash));
        ctx.beginPath();
        ctx.moveTo(item.x1, item.y1);
        ctx.lineTo(item.x2, item.y2);
        ctx.stroke();
        ctx.restore();
        break;
      case "img": {
        const src = imgs[item.id];
        const img = src ? image(item.id, src, redraw) : null;
        if (img) {
          const [t, r, b, l] = item.crop ?? [0, 0, 0, 0];
          const sx = (l / 100) * img.naturalWidth, sy = (t / 100) * img.naturalHeight;
          const sw = img.naturalWidth * (1 - (l + r) / 100), sh = img.naturalHeight * (1 - (t + b) / 100);
          ctx.drawImage(img, sx, sy, Math.max(1, sw), Math.max(1, sh), item.x, item.y, item.w, item.h);
        } else {
          ctx.fillStyle = "#f1f3f5";
          ctx.fillRect(item.x, item.y, item.w, item.h);
        }
        break;
      }
      case "wm":
        ctx.save();
        ctx.globalAlpha = 0.35;
        ctx.translate(item.x + item.w / 2, item.y + item.h / 2);
        ctx.rotate((item.rot * Math.PI) / 180);
        ctx.font = `${item.h * 0.8}px ${cssFont({ font: "Calibri", size: 1 }).split("px ")[1]}`;
        ctx.fillStyle = item.color ?? "#c0c0c0";
        ctx.textAlign = "center";
        ctx.textBaseline = "middle";
        ctx.fillText(item.text, 0, 0, item.w);
        ctx.restore();
        break;
      case "clip":
        ctx.save();
        ctx.beginPath();
        ctx.rect(item.x, item.y, item.w, item.h);
        ctx.clip();
        break;
      case "unclip":
        ctx.restore();
        break;
      case "t": {
        if (item.hl) { ctx.fillStyle = item.hl; ctx.fillRect(item.x, item.y, item.w, item.h); }
        for (const c of comments) {
          const r = overlap(item, c.from, c.to);
          if (!r) continue;
          const x0 = charX(item, r[0]), x1 = charX(item, r[1]);
          ctx.fillStyle = c.active ? "rgba(255, 176, 32, 0.45)" : "rgba(255, 200, 60, 0.22)";
          ctx.fillRect(item.x + x0, item.y, x1 - x0, item.h);
          if (c.active) { ctx.fillStyle = "rgba(214, 120, 0, 0.9)"; ctx.fillRect(item.x + x0, item.y + item.h - 1.2, x1 - x0, 1.2); }
        }
        // Find matches.
        if (item.p !== undefined && !item.del && marks.length) {
          const k = keyOf({ s: item.st, p: item.p });
          marks.forEach((m, mi) => {
            if (keyOf(m) !== k) return;
            const a = Math.max(m.o, item.off!), z = Math.min(m.o + m.len, item.off! + (item.n ?? 0));
            if (z <= a) return;
            ctx.fillStyle = mi === current ? "rgba(255, 170, 0, 0.75)" : "rgba(255, 214, 0, 0.45)";
            const x0 = charX(item, a - item.off!), x1 = charX(item, z - item.off!);
            ctx.fillRect(item.x + x0, item.y, x1 - x0, item.h);
          });
        }
        drawText(ctx, item);
        if (item.p !== undefined && !item.del && squiggles.length) {
          const k = keyOf({ s: item.st, p: item.p });
          for (const m of squiggles) {
            if (keyOf(m) !== k) continue;
            const a = Math.max(m.o, item.off!), z = Math.min(m.o + m.len, item.off! + (item.n ?? 0));
            if (z <= a) continue;
            squiggle(ctx, item.x + charX(item, a - item.off!), item.x + charX(item, z - item.off!), item.y + item.base + Math.max(1.5, item.size * 0.12));
          }
        }
        const range = inSel(item);
        if (range) {
          const x0 = charX(item, range[0]), x1 = range[1] === range[0] ? x0 + 4 : charX(item, range[1]);
          ctx.fillStyle = "rgba(47, 91, 211, 0.28)";
          ctx.fillRect(item.x + x0, item.y, x1 - x0, item.h);
        }
        break;
      }
    }
  }
}

export const PageView = forwardRef<PageViewHandle, Props>(function PageView(p, ref) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const sinkRef = useRef<HTMLTextAreaElement>(null);
  const canvases = useRef(new Map<number, HTMLCanvasElement>());
  const [view, setView] = useState({ top: 0, height: 800, width: 1000 });
  const [tick, setTick] = useState(0);
  const dragging = useRef(false);
  const composing = useRef(false);

  const items = useMemo(() => anchoredItems(p.layout), [p.layout]);
  const tops = useMemo(() => pageTops(p.layout, p.zoom), [p.layout, p.zoom]);
  const width = Math.max(view.width, contentWidth(p.layout, p.zoom));
  const sel: Range | null = p.caret && p.anchor && !samePos(p.caret, p.anchor) ? (([from, to]) => ({ from, to }))(order(p.anchor, p.caret)) : null;
  const caret = p.caret ? caretRect(items, p.caret) : null;

  // Redraw once web fonts finish loading (first paint may use a fallback).
  useEffect(() => {
    const done = () => setTick((t) => t + 1);
    document.fonts.addEventListener("loadingdone", done);
    const families = new Set(p.layout.pages.flatMap((pg) => pg.items.filter((i): i is TextItem => i.k === "t").map((i) => cssFont({ ...i, size: 12 }))));
    Promise.all([...families].slice(0, 40).map((f) => document.fonts.load(f).catch(() => []))).then(done);
    return () => document.fonts.removeEventListener("loadingdone", done);
  }, [p.layout]);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const update = () => setView({ top: el.scrollTop, height: el.clientHeight, width: el.clientWidth });
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const visible = useMemo(() => {
    const out: number[] = [];
    p.layout.pages.forEach((_, i) => {
      if (tops[i + 1] >= view.top - 400 && tops[i] <= view.top + view.height + 400) out.push(i);
    });
    return out;
  }, [p.layout, tops, view]);

  // Draw visible pages.
  useEffect(() => {
    const dpr = window.devicePixelRatio || 1;
    for (const i of visible) {
      const c = canvases.current.get(i);
      const page = p.layout.pages[i];
      if (!c || !page) continue;
      const w = Math.round(page.w * p.zoom * dpr), h = Math.round(page.h * p.zoom * dpr);
      if (c.width !== w || c.height !== h) { c.width = w; c.height = h; }
      const ctx = c.getContext("2d")!;
      ctx.setTransform(p.zoom * dpr, 0, 0, p.zoom * dpr, 0, 0);
      drawPage(ctx, page, p.layout.images, sel, p.marks, p.current, () => setTick((t) => t + 1), p.comments, p.squiggles);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible, p.layout, p.zoom, sel ? keyOf(sel.from) + sel.from.o : "", sel ? keyOf(sel.to) + sel.to.o : "", p.marks, p.current, tick, p.comments, p.squiggles]);

  const pointToPos = useCallback((clientX: number, clientY: number): Pos | null => {
    const el = scrollRef.current!;
    const r = el.getBoundingClientRect();
    const x = clientX - r.left + el.scrollLeft, y = clientY - r.top + el.scrollTop;
    let page = tops.findIndex((_, i) => i < p.layout.pages.length && y < tops[i + 1] - GAP_HALF);
    if (page < 0) page = p.layout.pages.length - 1;
    const pg = p.layout.pages[page];
    if (!pg) return null;
    const left = pageLeft(pg, width, p.zoom);
    return hit(items, page, (x - left) / p.zoom, (y - tops[page]) / p.zoom);
  }, [items, tops, width, p.layout, p.zoom]);

  const scrollToCaret = useCallback(() => {
    const el = scrollRef.current;
    if (!el || !caret) return;
    const y = tops[caret.page] + caret.y * p.zoom;
    const h = caret.h * p.zoom;
    if (y < el.scrollTop + 40) el.scrollTop = y - 40;
    else if (y + h > el.scrollTop + el.clientHeight - 40) el.scrollTop = y + h - el.clientHeight + 40;
  }, [caret, tops, p.zoom]);

  useImperativeHandle(ref, () => ({
    focus: () => sinkRef.current?.focus({ preventScroll: true }),
    scrollToCaret,
    items: () => items,
    caretBox: () => (document.querySelector(".caret, .ghost") as HTMLElement | null)?.getBoundingClientRect() ?? null,
    snapshot: (scale: number) => p.layout.pages.map((page) => {
      const c = document.createElement("canvas");
      c.width = Math.round(page.w * scale);
      c.height = Math.round(page.h * scale);
      const ctx = c.getContext("2d")!;
      ctx.scale(scale, scale);
      drawPage(ctx, page, p.layout.images, null, [], -1, () => {});
      return c.toDataURL("image/png");
    }),
  }), [scrollToCaret, items, p.layout]);

  useEffect(() => {
    const up = () => { if (dragging.current) p.onSelectEnd?.(); dragging.current = false; };
    const move = (e: MouseEvent) => {
      if (!dragging.current) return;
      const pos = pointToPos(e.clientX, e.clientY);
      if (pos) p.onDrag(pos);
    };
    window.addEventListener("mouseup", up);
    window.addEventListener("mousemove", move);
    return () => { window.removeEventListener("mouseup", up); window.removeEventListener("mousemove", move); };
  }, [pointToPos, p]);

  // Where the caret (and the ghost of text still being sent) is drawn.
  let caretStyle: React.CSSProperties | null = null;
  let ghostStyle: React.CSSProperties | null = null;
  if (caret) {
    const pg = p.layout.pages[caret.page];
    const left = pageLeft(pg, width, p.zoom);
    let x = left + caret.x * p.zoom;
    const y = tops[caret.page] + caret.y * p.zoom;
    if (p.ghost) {
      ghostStyle = { left: x, top: y + (caret.it.base - caret.it.size) * p.zoom, font: cssFont(caret.it, p.zoom), lineHeight: `${caret.it.size * 1.25 * p.zoom}px`, color: caret.it.color ?? "#000" };
      const m = document.createElement("canvas").getContext("2d")!;
      m.font = cssFont(caret.it, p.zoom);
      x += m.measureText(p.ghost).width;
    }
    caretStyle = { left: x, top: y, height: Math.max(10, caret.h * p.zoom) };
  }

  return (
    <div className="page-scroll" ref={scrollRef}
      onScroll={(e) => {
        const el = e.currentTarget;
        setView({ top: el.scrollTop, height: el.clientHeight, width: el.clientWidth });
        if (p.onScroll) {
          const mid = el.scrollTop + el.clientHeight / 3;
          p.onScroll(Math.max(0, tops.findIndex((_, i) => i < p.layout.pages.length && mid < tops[i + 1])));
        }
      }}
      onMouseDown={(e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        const pos = pointToPos(e.clientX, e.clientY);
        sinkRef.current?.focus({ preventScroll: true });
        if (!pos) return;
        if (e.detail === 2) { p.onWord(pos); return; }
        dragging.current = true;
        p.onPoint(pos, e.shiftKey, e.metaKey || e.ctrlKey);
      }}
      onContextMenu={(e) => { e.preventDefault(); p.onContext(e, pointToPos(e.clientX, e.clientY)); }}>
      <div className="page-desk" style={{ height: tops[tops.length - 1] + PAD_BOTTOM, width }}>
        {p.layout.pages.map((pg, i) => visible.includes(i) ? (
          <canvas key={i} className="page" aria-label={`Page ${i + 1}`}
            ref={(c) => { if (c) canvases.current.set(i, c); else canvases.current.delete(i); }}
            style={{ left: pageLeft(pg, width, p.zoom), top: tops[i], width: pg.w * p.zoom, height: pg.h * p.zoom }} />
        ) : (
          <div key={i} className="page page-placeholder" style={{ left: pageLeft(pg, width, p.zoom), top: tops[i], width: pg.w * p.zoom, height: pg.h * p.zoom }} />
        ))}
        {ghostStyle && <span className="ghost" style={ghostStyle}>{p.ghost}</span>}
        {caretStyle && !sel && <div className="caret" style={caretStyle} key={`${p.caret ? keyOf(p.caret) : ""}:${p.caret?.o}:${p.ghost.length}`} />}
        <textarea
          ref={sinkRef}
          className="input-sink"
          aria-label="Document"
          autoCorrect="off"
          autoCapitalize="off"
          spellCheck={false}
          style={caretStyle ? { left: caretStyle.left, top: caretStyle.top } : undefined}
          onKeyDown={(e) => { if (!composing.current) p.onKeyDown(e); }}
          onCompositionStart={() => { composing.current = true; }}
          onCompositionEnd={(e) => {
            composing.current = false;
            const t = e.currentTarget;
            if (t.value) p.onText(t.value);
            t.value = "";
          }}
          onInput={(e) => {
            if (composing.current) return;
            const t = e.currentTarget;
            if (t.value) p.onText(t.value);
            t.value = "";
          }}
          onPaste={(e) => { e.preventDefault(); const text = e.clipboardData.getData("text/plain"); const html = e.clipboardData.getData("text/html"); if (text || html) p.onPaste(text, html); }}
          onCopy={(e) => { e.preventDefault(); p.onCopy(false); }}
          onCut={(e) => { e.preventDefault(); p.onCopy(true); }}
        />
      </div>
    </div>
  );
});

const GAP_HALF = 14;
const PAD_BOTTOM = 60;
