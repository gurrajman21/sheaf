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

export async function readClipboard(): Promise<string> {
  if (isTauri) {
    const { readText } = await import("@tauri-apps/plugin-clipboard-manager");
    return readText();
  }
  return navigator.clipboard.readText();
}

/** Write text plus HTML (so formatting survives pasting into other apps). */
export async function writeRich(text: string, html: string): Promise<void> {
  if (isTauri) {
    const { writeHtml } = await import("@tauri-apps/plugin-clipboard-manager");
    return writeHtml(html, text);
  }
  try {
    await navigator.clipboard.write([new ClipboardItem({ "text/plain": new Blob([text], { type: "text/plain" }), "text/html": new Blob([html], { type: "text/html" }) })]);
  } catch {
    await navigator.clipboard.writeText(text);
  }
}
