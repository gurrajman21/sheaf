// File pickers and confirmations: native dialogs in the desktop app,
// plain prompts when running against the dev bridge in a browser.

import { isTauri } from "./api";

const FILTERS = [{ name: "Excel workbook", extensions: ["xlsx", "xlsm"] }];

export async function pickOpenPath(): Promise<string | null> {
  if (isTauri) {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const p = await open({ multiple: false, directory: false, filters: FILTERS });
    return typeof p === "string" ? p : null;
  }
  return window.prompt("Open file (absolute path to an .xlsx)") || null;
}

export async function pickSavePath(suggested: string): Promise<string | null> {
  if (isTauri) {
    const { save } = await import("@tauri-apps/plugin-dialog");
    const p = await save({ defaultPath: suggested, filters: FILTERS });
    if (!p) return null;
    return /\.xls[xm]$/i.test(p) ? p : `${p}.xlsx`;
  }
  return window.prompt("Save as (absolute path)", suggested) || null;
}

export async function confirmAction(message: string, title: string, okLabel: string): Promise<boolean> {
  if (isTauri) {
    const { ask } = await import("@tauri-apps/plugin-dialog");
    return ask(message, { title, kind: "warning", okLabel, cancelLabel: "Cancel" });
  }
  return window.confirm(`${title}\n\n${message}`);
}
