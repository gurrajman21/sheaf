// Geometry of the laid-out pages: where pages sit in the scroll area, how wide
// characters are inside a text item, where the caret goes, what a click hits,
// and how arrow keys move between lines.
//
// Oxi positions every text item exactly (in points). The canvas may not have the
// same font, so each item is drawn stretched to its laid-out width and character
// positions are measured proportionally inside it.

import { keyOf, type LayoutData, type Page, type Pos, type TextItem } from "./api";

export const GAP = 28;   // between pages, in screen px
export const PAD = 28;   // above the first page

/** Font stack for a Word font name: the real font if installed, then metric-compatible ones. */
export function fontStack(name: string): string {
  const alt: Record<string, string> = {
    Calibri: '"Calibri", "Carlito"', "Calibri Light": '"Calibri Light", "Carlito"', Cambria: '"Cambria", "Caladea"',
    Arial: '"Arial", "Liberation Sans"', Helvetica: '"Helvetica", "Liberation Sans"', "Times New Roman": '"Times New Roman", "Liberation Serif"',
    Times: '"Times", "Liberation Serif"', "Courier New": '"Courier New", "Liberation Mono"', Courier: '"Courier", "Liberation Mono"',
    Aptos: '"Aptos", "Carlito"', "Segoe UI": '"Segoe UI", "Carlito"', Georgia: '"Georgia", "Caladea"',
  };
  const serif = /times|georgia|garamond|cambria|book|serif|minion|palatino/i.test(name);
  return `${alt[name] ?? `"${name}"`}, ${serif ? '"Liberation Serif", Georgia, serif' : '"Carlito", -apple-system, "Helvetica Neue", sans-serif'}`;
}

export function cssFont(it: Pick<TextItem, "font" | "size" | "b" | "i">, scale = 1): string {
  return `${it.i ? "italic " : ""}${it.b ? "bold " : ""}${(it.size * scale).toFixed(2)}px ${fontStack(it.font)}`;
}

const measurer = document.createElement("canvas").getContext("2d")!;
const widthCache = new Map<string, number>();

function textWidth(font: string, text: string): number {
  const key = `${font}|${text}`;
  let w = widthCache.get(key);
  if (w === undefined) {
    measurer.font = font;
    w = measurer.measureText(text).width;
    if (widthCache.size > 20000) widthCache.clear();
    widthCache.set(key, w);
  }
  return w;
}

/** x (in points, relative to the item) of the boundary before visible character `k`. */
export function charX(it: TextItem, k: number): number {
  const n = it.n ?? it.text.length;
  if (k <= 0 || !it.text) return 0;
  if (k >= n) return it.w;
  const chars = [...it.text];
  // Painted text can differ from source length (caps, field results): map proportionally.
  const kk = chars.length === n ? k : Math.round((k / n) * chars.length);
  const font = cssFont(it);
  const full = textWidth(font, it.text) || 1;
  return (textWidth(font, chars.slice(0, kk).join("")) / full) * it.w;
}

/** Visible character index nearest to x (points, relative to the item). */
export function charAt(it: TextItem, x: number): number {
  const n = it.n ?? 0;
  if (n === 0 || x <= 0) return 0;
  if (x >= it.w) return n;
  let best = 0, bestD = Infinity;
  for (let k = 0; k <= n; k++) {
    const d = Math.abs(charX(it, k) - x);
    if (d < bestD) { best = k; bestD = d; }
  }
  return best;
}

export interface Placed { page: number; it: TextItem }

/** Anchored, editable text items per page (deleted text and unanchored text excluded). */
export function anchoredItems(layout: LayoutData): Placed[][] {
  return layout.pages.map((p, page) => p.items.filter((i): i is TextItem => i.k === "t" && i.p !== undefined && !i.del).map((it) => ({ page, it })));
}

export function pageTops(layout: LayoutData, zoom: number): number[] {
  const tops: number[] = [];
  let y = PAD;
  for (const p of layout.pages) { tops.push(y); y += p.h * zoom + GAP; }
  tops.push(y);
  return tops;
}

export function contentWidth(layout: LayoutData, zoom: number): number {
  return Math.max(0, ...layout.pages.map((p) => p.w * zoom)) + 2 * PAD;
}

export function pageLeft(page: Page, width: number, zoom: number): number {
  return Math.max(PAD, (width - page.w * zoom) / 2);
}

const contains = (it: TextItem, o: number) => it.off! <= o && o <= it.off! + (it.n ?? 0);
const posOf = (it: TextItem, o: number): Pos => ({ s: it.st, p: it.p!, o });

/** The text item the caret at `pos` sits in (first item of the paragraph if none contains it). */
export function caretItem(items: Placed[][], pos: Pos): Placed | null {
  const want = keyOf(pos);
  let fallback: Placed | null = null;
  let best: Placed | null = null;
  for (const pageItems of items) {
    for (const pl of pageItems) {
      const it = pl.it;
      if (keyOf(posOf(it, 0)) !== want) continue;
      if (!fallback) fallback = pl;
      if (contains(it, pos.o)) {
        // At a boundary, prefer the item that starts here (caret at line starts).
        if (!best || (it.off === pos.o && best.it.off! + (best.it.n ?? 0) === pos.o && it.y !== best.it.y)) best = pl;
        if (it.off! < pos.o && pos.o < it.off! + (it.n ?? 0)) return pl;
      }
      if (it.off! + (it.n ?? 0) <= pos.o) fallback = pl;
    }
  }
  return best ?? fallback;
}

export interface CaretRect { page: number; x: number; y: number; h: number; it: TextItem }

export function caretRect(items: Placed[][], pos: Pos): CaretRect | null {
  const pl = caretItem(items, pos);
  if (!pl) return null;
  const k = Math.max(0, Math.min(pos.o - pl.it.off!, pl.it.n ?? 0));
  return { page: pl.page, x: pl.it.x + charX(pl.it, k), y: pl.it.y, h: pl.it.h, it: pl.it };
}

/** Lines of a page: anchored items grouped by their line top. */
function lines(pageItems: Placed[]): Placed[][] {
  const out: Placed[][] = [];
  const sorted = [...pageItems].sort((a, b) => a.it.y - b.it.y || a.it.x - b.it.x);
  for (const pl of sorted) {
    const last = out[out.length - 1];
    if (last && Math.abs(last[0].it.y - pl.it.y) < 1.5) last.push(pl);
    else out.push([pl]);
  }
  return out;
}

function hitLine(line: Placed[], x: number): Pos {
  // In a table row, empty space belongs to the cell on its left: take the nearest text
  // that starts left of the click (Word puts the caret at the end of that cell's text).
  if (line.some((q) => (q.it.p?.length ?? 1) > 1)) {
    const left = line.filter((q) => q.it.x <= x);
    if (left.length) {
      const pick = left.reduce((a, q) => (q.it.x > a.it.x ? q : a));
      return posOf(pick.it, pick.it.off! + charAt(pick.it, x - pick.it.x));
    }
  }
  let best = line[0], bestD = Infinity;
  for (const pl of line) {
    const it = pl.it;
    const d = x < it.x ? it.x - x : x > it.x + it.w ? x - it.x - it.w : 0;
    if (d < bestD) { best = pl; bestD = d; }
  }
  return posOf(best.it, best.it.off! + charAt(best.it, x - best.it.x));
}

/** Document position under (x, y) points on `page`. */
export function hit(items: Placed[][], page: number, x: number, y: number): Pos | null {
  // Search this page first, then neighbours (clicks in margins between pages).
  for (const p of [page, page - 1, page + 1, page - 2, page + 2]) {
    const ls = items[p] ? lines(items[p]) : [];
    if (!ls.length) continue;
    if (p !== page) return p < page ? hitLine(ls[ls.length - 1], x) : hitLine(ls[0], x);
    let best = ls[0], bestD = Infinity;
    for (const l of ls) {
      const top = l[0].it.y, bot = top + Math.max(...l.map((q) => q.it.h));
      const d = y < top ? top - y : y > bot ? y - bot : 0;
      if (d < bestD) { best = l; bestD = d; }
    }
    return hitLine(best, x);
  }
  return null;
}

/** The position one line up (-1) or down (+1) from `pos`, aiming for x `goalX`. */
export function lineNav(items: Placed[][], pos: Pos, dir: 1 | -1, goalX: number): Pos | null {
  const c = caretRect(items, pos);
  if (!c) return null;
  const ls = lines(items[c.page] ?? []);
  const i = ls.findIndex((l) => l.some((q) => q.it === c.it));
  if (i < 0) return null;
  const j = i + dir;
  if (j >= 0 && j < ls.length) return hitLine(ls[j], goalX);
  for (let p = c.page + dir; p >= 0 && p < items.length; p += dir) {
    const other = lines(items[p]);
    if (other.length) return hitLine(dir > 0 ? other[0] : other[other.length - 1], goalX);
  }
  return null;
}

/** Start or end of the visual line holding `pos`. */
export function lineEdge(items: Placed[][], pos: Pos, end: boolean): Pos | null {
  const c = caretRect(items, pos);
  if (!c) return null;
  const line = lines(items[c.page]).find((l) => l.some((q) => q.it === c.it));
  if (!line) return null;
  const want = keyOf(pos);
  const same = line.filter((q) => keyOf(posOf(q.it, 0)) === want);
  if (!same.length) return null;
  if (end) {
    const last = same.reduce((a, q) => (q.it.off! + (q.it.n ?? 0) > a.it.off! + (a.it.n ?? 0) ? q : a));
    return { ...pos, o: last.it.off! + (last.it.n ?? 0) };
  }
  const first = same.reduce((a, q) => (q.it.off! < a.it.off! ? q : a));
  return { ...pos, o: first.it.off! };
}

/** Editable paragraphs in reading order (page by page), as positions at offset 0. */
export function readingOrder(items: Placed[][]): Pos[] {
  const seen = new Set<string>();
  const out: Pos[] = [];
  for (const p of items) {
    for (const q of p) {
      const k = keyOf(posOf(q.it, 0));
      if (!seen.has(k)) { seen.add(k); out.push(posOf(q.it, 0)); }
    }
  }
  return out;
}

/** Visible length of the paragraph at `pos` according to the layout. */
export function paraLength(items: Placed[][], pos: Pos): number {
  const want = keyOf(pos);
  let n = 0;
  for (const p of items) for (const q of p) if (keyOf(posOf(q.it, 0)) === want) n = Math.max(n, q.it.off! + (q.it.n ?? 0));
  return n;
}
