// Grid geometry: cumulative row/column positions, hit testing and frozen panes.
// Rows and columns are 1-based like the spreadsheet; positions are CSS pixels.

import type { Layout } from "./api";

export const HEADER_W = 52;
export const HEADER_H = 26;

export interface Cell { r: number; c: number }
export interface Selection { anchor: Cell; focus: Cell }

export interface Geometry {
  layout: Layout;
  rowY: Float64Array; // rowY[r-1] = top of row r; rowY[rows] = total height
  colX: Float64Array;
  frozenH: number; // height of frozen rows
  frozenW: number;
}

export function makeGeometry(layout: Layout): Geometry {
  const rowY = new Float64Array(layout.rows + 1);
  for (let i = 0; i < layout.rows; i++) rowY[i + 1] = rowY[i] + layout.row_heights[i];
  const colX = new Float64Array(layout.cols + 1);
  for (let i = 0; i < layout.cols; i++) colX[i + 1] = colX[i] + layout.col_widths[i];
  return {
    layout,
    rowY,
    colX,
    frozenH: rowY[Math.min(layout.frozen_rows, layout.rows)],
    frozenW: colX[Math.min(layout.frozen_cols, layout.cols)],
  };
}

/** Index (1-based) of the row/column containing content position `p`. */
export function indexAt(edges: Float64Array, p: number): number {
  let lo = 0;
  let hi = edges.length - 2;
  if (p <= 0) return 1;
  if (p >= edges[edges.length - 1]) return edges.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (edges[mid] <= p) lo = mid;
    else hi = mid - 1;
  }
  return lo + 1;
}

/** Screen x (relative to the grid viewport) of the left edge of column c. */
export function colScreenX(g: Geometry, c: number, scrollLeft: number): number {
  const frozen = c <= g.layout.frozen_cols;
  return HEADER_W + g.colX[c - 1] - (frozen ? 0 : scrollLeft);
}

export function rowScreenY(g: Geometry, r: number, scrollTop: number): number {
  const frozen = r <= g.layout.frozen_rows;
  return HEADER_H + g.rowY[r - 1] - (frozen ? 0 : scrollTop);
}

/** Which cell is under a viewport point (header areas map to row/col 0). */
export function hitTest(g: Geometry, x: number, y: number, scrollLeft: number, scrollTop: number): Cell {
  const cx = x - HEADER_W;
  const cy = y - HEADER_H;
  const c = cx < 0 ? 0 : indexAt(g.colX, cx < g.frozenW ? cx : cx + scrollLeft);
  const r = cy < 0 ? 0 : indexAt(g.rowY, cy < g.frozenH ? cy : cy + scrollTop);
  return { r, c };
}

/** Visible scrollable row/column ranges for a viewport. */
export function visibleRange(g: Geometry, scrollLeft: number, scrollTop: number, w: number, h: number) {
  const r0 = indexAt(g.rowY, g.frozenH + scrollTop);
  const r1 = indexAt(g.rowY, scrollTop + h - HEADER_H);
  const c0 = indexAt(g.colX, g.frozenW + scrollLeft);
  const c1 = indexAt(g.colX, scrollLeft + w - HEADER_W);
  return { r0: Math.max(r0, g.layout.frozen_rows + 1), r1, c0: Math.max(c0, g.layout.frozen_cols + 1), c1 };
}

export function colName(n: number): string {
  let s = "";
  while (n > 0) {
    const r = (n - 1) % 26;
    s = String.fromCharCode(65 + r) + s;
    n = Math.floor((n - 1) / 26);
  }
  return s;
}

export function cellName(c: Cell): string {
  return `${colName(c.c)}${c.r}`;
}

export function parseCellName(s: string): Cell | null {
  const m = /^\s*\$?([A-Za-z]{1,3})\$?(\d+)\s*$/.exec(s);
  if (!m) return null;
  let c = 0;
  for (const ch of m[1].toUpperCase()) c = c * 26 + (ch.charCodeAt(0) - 64);
  return { r: parseInt(m[2], 10), c };
}

export function normRect(sel: Selection) {
  const r0 = Math.min(sel.anchor.r, sel.focus.r);
  const r1 = Math.max(sel.anchor.r, sel.focus.r);
  const c0 = Math.min(sel.anchor.c, sel.focus.c);
  const c1 = Math.max(sel.anchor.c, sel.focus.c);
  return { r0, r1, c0, c1 };
}

export function selectionName(sel: Selection): string {
  const { r0, r1, c0, c1 } = normRect(sel);
  if (r0 === r1 && c0 === c1) return cellName({ r: r0, c: c0 });
  return `${cellName({ r: r0, c: c0 })}:${cellName({ r: r1, c: c1 })}`;
}

export interface Merge { r: number; c: number; w: number; h: number }

export function mergeIndex(layout: Layout) {
  const anchors = new Map<string, Merge>();
  const covered = new Map<string, Merge>();
  for (const [r, c, w, h] of layout.merged) {
    const m = { r, c, w, h };
    anchors.set(`${r},${c}`, m);
    for (let rr = r; rr < r + h; rr++) for (let cc = c; cc < c + w; cc++) covered.set(`${rr},${cc}`, m);
  }
  return { anchors, covered };
}
