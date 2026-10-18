// Native platform features in the desktop app, with browser fallbacks:
// context menus, the menu bar's actions, and printing.

import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "./api";

export interface MenuEntry { label: string; shortcut?: string; action?: () => void; danger?: boolean; disabled?: boolean }

/** Show a native context menu at the pointer. Returns false when not available (browser). */
export async function nativeMenu(items: MenuEntry[]): Promise<boolean> {
  if (!isTauri) return false;
  const { Menu, MenuItem, PredefinedMenuItem } = await import("@tauri-apps/api/menu");
  const built = await Promise.all(items.map((it) => (
    it.label === ""
      ? PredefinedMenuItem.new({ item: "Separator" })
      : MenuItem.new({ text: it.label, enabled: !it.disabled, accelerator: toAccelerator(it.shortcut), action: () => it.action?.() })
  )));
  const menu = await Menu.new({ items: built });
  await menu.popup();
  return true;
}

/** "⌘⇧V" -> "CmdOrCtrl+Shift+V" (shown in the menu only; the menu bar owns the real shortcuts). */
function toAccelerator(s?: string): string | undefined {
  if (!s) return undefined;
  const map: Record<string, string> = { "⌘": "CmdOrCtrl+", "⇧": "Shift+", "⌥": "Alt+", "⌃": "Ctrl+" };
  let out = "";
  for (const ch of s) out += map[ch] ?? ch.toUpperCase();
  return /\+$/.test(out) || out.length === 0 ? undefined : out.replace("⌫", "Backspace");
}

/** Subscribe to menu bar actions (desktop app only). */
export function onMenuAction(handler: (id: string) => void): () => void {
  if (!isTauri) return () => {};
  let unlisten: (() => void) | null = null;
  let cancelled = false;
  import("@tauri-apps/api/event").then(({ listen }) => listen<string>("menu", (e) => handler(e.payload))).then((u) => {
    if (cancelled) u();
    else unlisten = u;
  });
  return () => { cancelled = true; unlisten?.(); };
}

export async function printWindow(): Promise<void> {
  if (isTauri) await invoke("print_window");
  else window.print();
}
