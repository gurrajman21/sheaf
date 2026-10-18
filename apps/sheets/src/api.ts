// One way to reach the Rust backend (`sheets_core::Session::dispatch`):
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

export interface SheetObjects { charts: number; images: number; comments: number; pivot_tables: number; tables: number }

export interface SheetInfo { name: string; state: string; color: string | null; objects?: SheetObjects }

export interface WorkbookInfo {
  name: string;
  path: string | null;
  dirty: boolean;
  sheet: number;
  sheets: SheetInfo[];
  can_undo: boolean;
  can_redo: boolean;
}

export interface Layout {
  rows: number;
  cols: number;
  row_heights: number[];
  col_widths: number[];
  frozen_rows: number;
  frozen_cols: number;
  merged: [number, number, number, number][]; // row, col, width, height
  show_grid: boolean;
}

export interface CellView {
  r: number;
  c: number;
  text: string;
  b?: boolean;
  i?: boolean;
  u?: boolean;
  s?: boolean;
  color?: string;
  fill?: string;
  size: number;
  font?: string;
  align: "left" | "center" | "right";
  valign: "top" | "middle" | "bottom";
  wrap?: boolean;
  borders?: [boolean, boolean, boolean, boolean]; // top right bottom left
  bar?: { value: number; axis: number; gradient: boolean; color: string; neg: string };
  icon?: { name: string; color: string; count?: number; max?: number };
  comment?: string;
  formula?: boolean;
}

export interface CellStyle {
  b: boolean;
  i: boolean;
  u: boolean;
  s: boolean;
  num_fmt: string;
  size: number;
  align: string;
  valign: string;
  wrap: boolean;
}

export interface SaveResult {
  saved: boolean;
  needs_confirmation?: boolean;
  reason?: string;
  mode?: "patch" | "native";
  path?: string;
}

export interface Stats { sum: number; count: number; numbers: number; average: number | null; min: number | null; max: number | null }

export interface FindHit { sheet: number; row: number; col: number; text: string }

export interface Rect { sheet: number; row: number; col: number; width: number; height: number }
