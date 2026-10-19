// One way to reach the Rust backend (`docs_core::Session::dispatch`):
// through Tauri in the desktop app, or the dev HTTP bridge in a browser.

import { invoke } from "@tauri-apps/api/core";

export const isTauri = "__TAURI_INTERNALS__" in window;

export async function call<T = unknown>(cmd: string, args: object = {}): Promise<T> {
  if (isTauri) return invoke<T>("api", { cmd, args });
  const res = await fetch(`/api/${cmd}`, { method: "POST", body: JSON.stringify(args) });
  const body = await res.json();
  if (!body.ok) throw new Error(body.error);
  return body.data as T;
}

export interface DocInfo {
  name: string;
  path: string | null;
  dirty: boolean;
  can_undo: boolean;
  can_redo: boolean;
  track: boolean;
  author: string;
  blocks: number;
  words: number;
  page?: { size: string; orientation: string; margins: string; cols: number };
  toc?: boolean;
}

/** A position: story (absent/"" = body, "h:word/header1.xml", "fn:3", ...), path of block
 *  indices (through tables and text boxes; the last is the paragraph), visible offset. */
export interface Pos { s?: string; p: number[]; o: number }

export const keyOf = (q: { s?: string; p: number[] }) => `${q.s ?? ""}|${q.p.join(",")}`;
export const containerOf = (q: { s?: string; p: number[] }) => `${q.s ?? ""}|${q.p.slice(0, -1).join(",")}`;
export const idxOf = (q: { p: number[] }) => q.p[q.p.length - 1];
export const sameContainer = (a: Pos, b: Pos) => containerOf(a) === containerOf(b);
export const samePos = (a: Pos, b: Pos) => keyOf(a) === keyOf(b) && a.o === b.o;
export const at = (q: { s?: string; p: number[] }, o: number): Pos => ({ s: q.s, p: q.p, o });
export const isTop = (q: Pos) => !q.s && q.p.length === 1;

/** Document order (within a story; paths compared element by element). */
export function before(a: Pos, b: Pos): boolean {
  if ((a.s ?? "") !== (b.s ?? "")) return (a.s ?? "") < (b.s ?? "");
  for (let i = 0; i < Math.min(a.p.length, b.p.length); i++) {
    if (a.p[i] !== b.p[i]) return a.p[i] < b.p[i];
  }
  if (a.p.length !== b.p.length) return a.p.length < b.p.length;
  return a.o < b.o;
}
export const order = (a: Pos, b: Pos): [Pos, Pos] => (before(b, a) ? [b, a] : [a, b]);

export interface TextItem {
  k: "t"; x: number; y: number; w: number; h: number; text: string; font: string; size: number; base: number;
  b?: boolean; i?: boolean; u?: boolean; s?: boolean; color?: string; hl?: string; vert?: boolean;
  /** Anchored text: story, path, visible offset and length (deleted text: `del`). */
  st?: string; p?: number[]; blk?: number; off?: number; n?: number; del?: boolean;
}
export type Item =
  | TextItem
  | { k: "img"; x: number; y: number; w: number; h: number; id: string; crop: number[] | null }
  | { k: "line"; x1: number; y1: number; x2: number; y2: number; w: number; color: string | null; dash: string | null }
  | { k: "rect"; x: number; y: number; w: number; h: number; fill: string | null; stroke?: string | null; sw?: number; r?: number }
  | { k: "wm"; x: number; y: number; w: number; h: number; text: string; color: string | null; rot: number }
  | { k: "clip"; x: number; y: number; w: number; h: number }
  | { k: "unclip" };

export interface Page { w: number; h: number; items: Item[] }
export interface LayoutData { pages: Page[]; images: Record<string, string> }

export interface ParaInfo { style: string; align: string; list: boolean; len: number | null }

export interface Revision { id: string; kind: string; author: string; date: string; part: string; text: string; at?: Pos | null }

export interface RunFmt {
  bold?: boolean; italic?: boolean; underline?: boolean; strike?: boolean;
  size?: number | null; font?: string | null; color?: string | null; highlight?: string | null;
}
