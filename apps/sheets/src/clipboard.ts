// Copy has to wait for the backend to produce the text, so it can't use the
// synchronous copy event: write through the Tauri clipboard plugin, or the
// async Clipboard API in a browser.

import { isTauri } from "./api";

export async function writeClipboard(text: string): Promise<void> {
  if (isTauri) {
    const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
    return writeText(text);
  }
  return navigator.clipboard.writeText(text);
}
