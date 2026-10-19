// The slash menu: type "/" at the start of an empty paragraph for a quick insert menu.
// Keeps filtering as you type; Enter inserts, Esc closes.

import { forwardRef, useImperativeHandle, useMemo, useState } from "react";
import { Footprints, Hash, Heading1, Heading2, Heading3, ImagePlus, List, ListOrdered, ListTree, MessageSquarePlus, SeparatorHorizontal, Table, TextQuote, Type } from "lucide-react";
import type { Action } from "./Ribbon";

export interface SlashItem { id: string; label: string; hint: string; icon: React.ReactNode; action: Action; keywords: string }

export const SLASH_ITEMS: SlashItem[] = [
  { id: "text", label: "Text", hint: "Plain paragraph", icon: <Type size={16} />, action: { t: "style", id: "Normal" }, keywords: "normal body paragraph" },
  { id: "h1", label: "Heading 1", hint: "Big section heading", icon: <Heading1 size={16} />, action: { t: "style", id: "Heading1" }, keywords: "title h1" },
  { id: "h2", label: "Heading 2", hint: "Medium heading", icon: <Heading2 size={16} />, action: { t: "style", id: "Heading2" }, keywords: "subtitle h2" },
  { id: "h3", label: "Heading 3", hint: "Small heading", icon: <Heading3 size={16} />, action: { t: "style", id: "Heading3" }, keywords: "h3" },
  { id: "bullets", label: "Bulleted list", hint: "A simple list", icon: <List size={16} />, action: { t: "list", kind: "bullet" }, keywords: "ul unordered points" },
  { id: "numbers", label: "Numbered list", hint: "1, 2, 3", icon: <ListOrdered size={16} />, action: { t: "list", kind: "number" }, keywords: "ol ordered steps" },
  { id: "quote", label: "Quote", hint: "Pull out a quotation", icon: <TextQuote size={16} />, action: { t: "style", id: "Quote" }, keywords: "blockquote citation" },
  { id: "table", label: "Table", hint: "3 × 3 table", icon: <Table size={16} />, action: { t: "table", rows: 3, cols: 3 }, keywords: "grid rows columns" },
  { id: "picture", label: "Picture", hint: "From a file", icon: <ImagePlus size={16} />, action: { t: "picture" }, keywords: "image photo" },
  { id: "pagebreak", label: "Page break", hint: "Start a new page", icon: <SeparatorHorizontal size={16} />, action: { t: "page_break" }, keywords: "new page" },
  { id: "toc", label: "Table of contents", hint: "From your headings", icon: <ListTree size={16} />, action: { t: "toc" }, keywords: "contents toc index" },
  { id: "footnote", label: "Footnote", hint: "Note at the page bottom", icon: <Footprints size={16} />, action: { t: "footnote" }, keywords: "reference citation note" },
  { id: "pagenum", label: "Page numbers", hint: "In the footer", icon: <Hash size={16} />, action: { t: "page_numbers", where: "footer", align: "center", total: true }, keywords: "numbering footer" },
  { id: "comment", label: "Comment", hint: "Note for reviewers", icon: <MessageSquarePlus size={16} />, action: { t: "comment" }, keywords: "review note" },
];

export interface SlashHandle { key: (k: string) => boolean }

export const SlashMenu = forwardRef<SlashHandle, { query: string; x: number; y: number; onPick: (i: SlashItem) => void; onClose: () => void }>(function SlashMenu(p, ref) {
  const items = useMemo(() => {
    const q = p.query.toLowerCase().trim();
    return SLASH_ITEMS.filter((i) => !q || i.label.toLowerCase().includes(q) || i.keywords.includes(q));
  }, [p.query]);
  const [index, setIndex] = useState(0);
  const cur = Math.min(index, Math.max(0, items.length - 1));
  // The document keeps focus; App forwards navigation keys here.
  useImperativeHandle(ref, () => ({
    key: (k: string) => {
      if (k === "ArrowDown") { setIndex((i) => Math.min(i + 1, items.length - 1)); return true; }
      if (k === "ArrowUp") { setIndex((i) => Math.max(i - 1, 0)); return true; }
      if (k === "Enter" || k === "Tab") { if (items[cur]) p.onPick(items[cur]); else p.onClose(); return true; }
      if (k === "Escape") { p.onClose(); return true; }
      return false;
    },
  }), [items, cur, p]);
  if (!items.length) return null;
  return (
    <div className="slash-menu" role="listbox" aria-label="Insert" style={{ left: p.x, top: p.y }}>
      {items.map((it, i) => (
        <button key={it.id} type="button" role="option" aria-selected={i === cur} className={`slash-item${i === cur ? " is-active" : ""}`}
          onMouseDown={(e) => e.preventDefault()} onMouseMove={() => setIndex(i)} onClick={() => p.onPick(it)}>
          <span className="slash-icon">{it.icon}</span>
          <span className="slash-text"><b>{it.label}</b><span>{it.hint}</span></span>
        </button>
      ))}
    </div>
  );
});
