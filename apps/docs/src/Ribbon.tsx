// Word-style ribbon: Home (clipboard, font, paragraph, styles), Insert, Review
// and View. Every control emits an `Action`; App decides what each one does.

import { useLayoutEffect, useRef, useState } from "react";
import {
  AlignCenter, AlignJustify, AlignLeft, AlignRight, Baseline, Bold, Check, CheckCheck, ClipboardPaste, Command, Copy,
  FilePlus, FileText, FolderOpen, Highlighter, ImagePlus, IndentDecrease, IndentIncrease, Italic, LayoutTemplate, List,
  ListOrdered, MessageSquareDiff, MessageSquarePlus, PanelRight, Printer, Redo2, RemoveFormatting, Save, SaveAll, Scissors, Search,
  SeparatorHorizontal, Strikethrough, Table, Underline, Undo2, X, ZoomIn, ZoomOut, RectangleVertical, RectangleHorizontal,
  Scaling, Columns2, BetweenHorizontalStart, BetweenHorizontalEnd, BetweenVerticalStart, BetweenVerticalEnd, Trash2,
  TableCellsMerge, PaintBucket, Columns3, ListTree, Hash, Link2, Footprints, TextQuote, Paintbrush, SpellCheck, ArrowUpDown, Replace,
} from "lucide-react";
import { Btn, Col, ColorBtn, Group, keep, MenuBtn, Row } from "./ribbonKit";

export type Action =
  | { t: "file"; a: "new" | "open" | "save" | "save_as" }
  | { t: "start" } | { t: "print" } | { t: "palette" } | { t: "find" }
  | { t: "undo" } | { t: "redo" }
  | { t: "cut" } | { t: "copy" } | { t: "paste" }
  | { t: "toggle"; what: "bold" | "italic" | "underline" | "strike" }
  | { t: "size"; value: number } | { t: "size_delta"; delta: number }
  | { t: "font"; value: string } | { t: "color"; value: string } | { t: "highlight"; value: string }
  | { t: "clear_format" }
  | { t: "style"; id: string } | { t: "align"; value: string } | { t: "list"; kind: "bullet" | "number" | "none" }
  | { t: "indent"; delta: number }
  | { t: "page_break" } | { t: "table"; rows: number; cols: number } | { t: "picture" }
  | { t: "track" } | { t: "accept_all" } | { t: "reject_all" } | { t: "pane" } | { t: "comment" }
  | { t: "zoom"; value: number }
  | { t: "table_row"; where: "above" | "below" } | { t: "table_col"; where: "left" | "right" }
  | { t: "table_delete"; what: "row" | "col" | "table" } | { t: "table_merge" } | { t: "table_shade"; color: string }
  | { t: "page_setup"; size?: string; orientation?: string; margins?: string; cols?: number }
  | { t: "toc"; update?: boolean } | { t: "footnote" } | { t: "page_numbers"; where: "header" | "footer"; align: string; total: boolean }
  | { t: "link" } | { t: "nav" } | { t: "versions" } | { t: "painter" } | { t: "spacing"; line?: number; before?: number; after?: number } | { t: "spell" } | { t: "replace" }

export interface PageInfo { size: string; orientation: string; margins: string; cols: number }

export interface RibbonState {
  font: string; size: number; bold: boolean; italic: boolean; underline: boolean; strike: boolean;
  style: string; align: string; list: boolean;
  track: boolean; pane: boolean; zoom: number; canUndo: boolean; canRedo: boolean;
  inTable: boolean; page: PageInfo | null; hasToc: boolean; nav: boolean; painter: boolean; spell: boolean;
}

const TABS = ["Home", "Insert", "Layout", "Review", "View"] as const;
type Tab = (typeof TABS)[number] | "Table";
const SIZES_PAGE: [string, string][] = [["letter", "Letter (8.5 × 11 in)"], ["legal", "Legal (8.5 × 14 in)"], ["a4", "A4 (210 × 297 mm)"], ["a5", "A5 (148 × 210 mm)"], ["tabloid", "Tabloid (11 × 17 in)"]];
const MARGINS: [string, string][] = [["normal", "Normal (1 in)"], ["narrow", "Narrow (0.5 in)"], ["moderate", "Moderate (1 in × 0.75 in)"], ["wide", "Wide (1 in × 2 in)"]];
const SIZES = [8, 9, 10, 10.5, 11, 12, 14, 16, 18, 20, 24, 28, 36, 48, 72];
const FONTS = ["Calibri", "Aptos", "Arial", "Cambria", "Georgia", "Helvetica Neue", "Times New Roman", "Garamond", "Verdana", "Courier New"];
const HIGHLIGHTS: Record<string, string> = {
  yellow: "#FFFF00", green: "#00FF00", cyan: "#00FFFF", magenta: "#FF00FF", red: "#FF0000", blue: "#0000FF", lightGray: "#C0C0C0",
};

/** The style gallery: each tile previews its style. */
export const STYLES: { id: string; name: string; css: React.CSSProperties }[] = [
  { id: "Normal", name: "Normal", css: { fontSize: 12 } },
  { id: "Title", name: "Title", css: { fontSize: 19, fontWeight: 300, letterSpacing: "-0.01em" } },
  { id: "Subtitle", name: "Subtitle", css: { fontSize: 12, color: "#5a5a5a", letterSpacing: "0.03em" } },
  { id: "Heading1", name: "Heading 1", css: { fontSize: 15, color: "#2F5496" } },
  { id: "Heading2", name: "Heading 2", css: { fontSize: 13, color: "#2F5496" } },
  { id: "Heading3", name: "Heading 3", css: { fontSize: 12, color: "#1F3763" } },
  { id: "Quote", name: "Quote", css: { fontSize: 12, fontStyle: "italic", color: "#404040" } },
];

function TablePicker({ act }: { act: (a: Action) => void }) {
  const [hover, setHover] = useState({ r: 2, c: 3 });
  const items: { r: number; c: number }[] = [];
  for (let r = 1; r <= 8; r++) for (let c = 1; c <= 10; c++) items.push({ r, c });
  return (
    <MenuBtn big label="Table" tint="#7A5AF8" icon={<Table size={20} />} items={[]}
      title="Insert a table" panel={(close) => (
        <div className="table-picker" onMouseLeave={() => setHover({ r: 2, c: 3 })}>
          <div className="table-grid">
            {items.map(({ r, c }) => (
              <button key={`${r}-${c}`} type="button" aria-label={`${r} by ${c} table`}
                className={`table-cell${r <= hover.r && c <= hover.c ? " is-on" : ""}`}
                onMouseDown={keep} onMouseEnter={() => setHover({ r, c })}
                onClick={() => { close(); act({ t: "table", rows: r, cols: c }); }} />
            ))}
          </div>
          <div className="table-size">{hover.c} × {hover.r} table</div>
        </div>
      )} />
  );
}

export function Ribbon({ s, act }: { s: RibbonState; act: (a: Action) => void }) {
  const [tab, setTabRaw] = useState<Tab>("Home");
  const setTab = setTabRaw;
  // The Table tab exists only while the caret is in a table.
  const shown: Tab = tab === "Table" && !s.inTable ? "Home" : tab;
  const bodyRef = useRef<HTMLDivElement>(null);
  const [edges, setEdges] = useState({ left: false, right: false });
  useLayoutEffect(() => {
    const el = bodyRef.current;
    if (!el) return;
    const update = () => setEdges({ left: el.scrollLeft > 2, right: el.scrollLeft + el.clientWidth < el.scrollWidth - 2 });
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    el.addEventListener("scroll", update);
    return () => { ro.disconnect(); el.removeEventListener("scroll", update); };
  }, [tab]);

  return (
    <div className="ribbon">
      <div className="rb-tabs" role="tablist" aria-label="Ribbon">
        <MenuBtn label="File" icon={null} showLabel items={[
          { label: "New document", icon: <FilePlus size={15} />, shortcut: "⌘N", onClick: () => act({ t: "file", a: "new" }) },
          { label: "New from template…", icon: <LayoutTemplate size={15} />, shortcut: "⇧⌘N", onClick: () => act({ t: "start" }) },
          { label: "Open…", icon: <FolderOpen size={15} />, shortcut: "⌘O", onClick: () => act({ t: "file", a: "open" }) },
          "-",
          { label: "Save", icon: <Save size={15} />, shortcut: "⌘S", onClick: () => act({ t: "file", a: "save" }) },
          { label: "Save as…", icon: <SaveAll size={15} />, shortcut: "⇧⌘S", onClick: () => act({ t: "file", a: "save_as" }) },
          { label: "Version history…", icon: <LayoutTemplate size={15} />, onClick: () => act({ t: "versions" }) },
          "-",
          { label: "Print or save as PDF…", icon: <Printer size={15} />, shortcut: "⌘P", onClick: () => act({ t: "print" }) },
        ]} />
        {TABS.map((t) => (
          <button key={t} type="button" role="tab" aria-selected={shown === t} className={`rb-tab${shown === t ? " is-active" : ""}`}
            onMouseDown={keep} onClick={() => setTab(t)}>{t}</button>
        ))}
        {s.inTable && (
          <button type="button" role="tab" aria-selected={shown === "Table"} className={`rb-tab rb-tab-context${shown === "Table" ? " is-active" : ""}`}
            onMouseDown={keep} onClick={() => setTab("Table")}>Table</button>
        )}
        <div className="rb-quick">
          <Btn label="Undo" shortcut="⌘Z" icon={<Undo2 size={16} />} disabled={!s.canUndo} onClick={() => act({ t: "undo" })} />
          <Btn label="Redo" shortcut="⇧⌘Z" icon={<Redo2 size={16} />} disabled={!s.canRedo} onClick={() => act({ t: "redo" })} />
          <Btn label="Save" shortcut="⌘S" icon={<Save size={16} />} onClick={() => act({ t: "file", a: "save" })} />
        </div>
      </div>

      <div className={`rb-body${edges.left ? " fade-left" : ""}${edges.right ? " fade-right" : ""}`} role="tabpanel" aria-label={shown} ref={bodyRef}
        onWheel={(e) => { const el = bodyRef.current; if (el && el.scrollWidth > el.clientWidth && Math.abs(e.deltaY) > Math.abs(e.deltaX)) el.scrollLeft += e.deltaY; }}>
        {shown === "Home" && (
          <>
            <Group label="Clipboard">
              <Btn big label="Paste" tint="#2F5BD3" icon={<ClipboardPaste size={20} />} shortcut="⌘V" onClick={() => act({ t: "paste" })} />
              <Col>
                <Btn showLabel label="Cut" shortcut="⌘X" icon={<Scissors size={15} />} onClick={() => act({ t: "cut" })} />
                <Btn showLabel label="Copy" shortcut="⌘C" icon={<Copy size={15} />} onClick={() => act({ t: "copy" })} />
                <Btn showLabel label="Format painter" active={s.painter} icon={<Paintbrush size={15} />} onClick={() => act({ t: "painter" })} />
              </Col>
            </Group>
            <Group label="Font">
              <Col>
                <Row>
                  <select className="rb-select rb-font" aria-label="Font" title="Font" value={s.font}
                    onChange={(e) => act({ t: "font", value: e.target.value })}>
                    {!FONTS.includes(s.font) && <option value={s.font}>{s.font}</option>}
                    {FONTS.map((f) => <option key={f} value={f} style={{ fontFamily: f }}>{f}</option>)}
                  </select>
                  <select className="rb-select rb-size" aria-label="Font size" title="Font size" value={s.size}
                    onChange={(e) => act({ t: "size", value: Number(e.target.value) })}>
                    {!SIZES.includes(s.size) && <option value={s.size}>{s.size}</option>}
                    {SIZES.map((n) => <option key={n} value={n}>{n}</option>)}
                  </select>
                  <Btn label="Increase font size" icon={<span className="glyph">A<sup>+</sup></span>} onClick={() => act({ t: "size_delta", delta: 1 })} />
                  <Btn label="Decrease font size" icon={<span className="glyph">A<sup>−</sup></span>} onClick={() => act({ t: "size_delta", delta: -1 })} />
                </Row>
                <Row>
                  <Btn label="Bold" shortcut="⌘B" active={s.bold} icon={<Bold size={16} />} onClick={() => act({ t: "toggle", what: "bold" })} />
                  <Btn label="Italic" shortcut="⌘I" active={s.italic} icon={<Italic size={16} />} onClick={() => act({ t: "toggle", what: "italic" })} />
                  <Btn label="Underline" shortcut="⌘U" active={s.underline} icon={<Underline size={16} />} onClick={() => act({ t: "toggle", what: "underline" })} />
                  <Btn label="Strikethrough" active={s.strike} icon={<Strikethrough size={16} />} onClick={() => act({ t: "toggle", what: "strike" })} />
                  <ColorBtn label="Font color" icon={<Baseline size={16} />} clearLabel="Automatic" initial="#B42318" onPick={(c) => act({ t: "color", value: c })} />
                  <MenuBtn label="Highlight" icon={<Highlighter size={16} />} items={[
                    ...Object.entries(HIGHLIGHTS).map(([name, hex]) => ({ label: name.replace(/([A-Z])/g, " $1").replace(/^./, (c) => c.toUpperCase()), icon: <span className="hl-swatch" style={{ background: hex }} />, onClick: () => act({ t: "highlight", value: name }) })),
                    "-" as const,
                    { label: "No highlight", icon: <X size={14} />, onClick: () => act({ t: "highlight", value: "" }) },
                  ]} />
                  <Btn label="Clear formatting" icon={<RemoveFormatting size={16} />} onClick={() => act({ t: "clear_format" })} />
                </Row>
              </Col>
            </Group>
            <Group label="Paragraph">
              <Col>
                <Row>
                  <Btn label="Bullets" active={s.list && s.style === "ListParagraph"} icon={<List size={16} />} onClick={() => act({ t: "list", kind: s.list ? "none" : "bullet" })} />
                  <Btn label="Numbering" icon={<ListOrdered size={16} />} onClick={() => act({ t: "list", kind: "number" })} />
                  <Btn label="Decrease indent" icon={<IndentDecrease size={16} />} onClick={() => act({ t: "indent", delta: -1 })} />
                  <Btn label="Increase indent" icon={<IndentIncrease size={16} />} onClick={() => act({ t: "indent", delta: 1 })} />
                  <MenuBtn label="Line and paragraph spacing" icon={<ArrowUpDown size={16} />} items={[
                    ...[1, 1.15, 1.5, 2].map((m) => ({ label: `${m.toFixed(m === 1.15 ? 2 : 1)} lines`, onClick: () => act({ t: "spacing", line: m }) })),
                    "-" as const,
                    { label: "Add space before paragraph", onClick: () => act({ t: "spacing", before: 12 }) },
                    { label: "Remove space before paragraph", onClick: () => act({ t: "spacing", before: 0 }) },
                    { label: "Add space after paragraph", onClick: () => act({ t: "spacing", after: 8 }) },
                    { label: "Remove space after paragraph", onClick: () => act({ t: "spacing", after: 0 }) },
                  ]} />
                </Row>
                <Row>
                  <Btn label="Align left" shortcut="⌘L" active={s.align === "left" || s.align === "start"} icon={<AlignLeft size={16} />} onClick={() => act({ t: "align", value: "left" })} />
                  <Btn label="Center" shortcut="⌘E" active={s.align === "center"} icon={<AlignCenter size={16} />} onClick={() => act({ t: "align", value: "center" })} />
                  <Btn label="Align right" shortcut="⌘R" active={s.align === "right" || s.align === "end"} icon={<AlignRight size={16} />} onClick={() => act({ t: "align", value: "right" })} />
                  <Btn label="Justify" shortcut="⌘J" active={s.align === "both"} icon={<AlignJustify size={16} />} onClick={() => act({ t: "align", value: "both" })} />
                </Row>
              </Col>
            </Group>
            <Group label="Styles">
              <div className="style-gallery" role="listbox" aria-label="Paragraph styles">
                {STYLES.map((st) => (
                  <button key={st.id} type="button" role="option" aria-selected={s.style === st.id}
                    className={`style-tile${s.style === st.id ? " is-active" : ""}`}
                    onMouseDown={keep} onClick={() => act({ t: "style", id: st.id })} title={st.name}>
                    <span className="style-sample" style={st.css}>Aa</span>
                    <span className="style-name">{st.name}</span>
                  </button>
                ))}
              </div>
            </Group>
          </>
        )}
        {shown === "Insert" && (
          <>
            <Group label="Pages">
              <Btn big label="Page break" tint="#2F5BD3" icon={<SeparatorHorizontal size={20} />} shortcut="⌘↩" onClick={() => act({ t: "page_break" })} />
              <MenuBtn big label="Page numbers" tint="#0E7C6B" icon={<Hash size={20} />} items={[
                { label: "Bottom center: Page 1 of 9", onClick: () => act({ t: "page_numbers", where: "footer", align: "center", total: true }) },
                { label: "Bottom right: 1", onClick: () => act({ t: "page_numbers", where: "footer", align: "right", total: false }) },
                { label: "Top right: 1", onClick: () => act({ t: "page_numbers", where: "header", align: "right", total: false }) },
              ]} />
            </Group>
            <Group label="References">
              <MenuBtn big label="Contents" tint="#7A5AF8" icon={<ListTree size={20} />} items={[
                { label: "Insert table of contents", onClick: () => act({ t: "toc" }) },
                { label: "Update table of contents", onClick: () => act({ t: "toc", update: true }) },
              ]} />
              <Btn big label="Footnote" tint="#B54708" icon={<Footprints size={20} />} onClick={() => act({ t: "footnote" })} />
            </Group>
            <Group label="Links">
              <Btn big label="Link" tint="#175CD3" icon={<Link2 size={20} />} shortcut="⌥⌘K" onClick={() => act({ t: "link" })} />
            </Group>
            <Group label="Tables">
              <TablePicker act={act} />
            </Group>
            <Group label="Illustrations">
              <Btn big label="Picture" tint="#0E7C6B" icon={<ImagePlus size={20} />} onClick={() => act({ t: "picture" })} />
            </Group>
          </>
        )}
        {shown === "Layout" && (
          <>
            <Group label="Page setup">
              <MenuBtn big label="Size" tint="#2F5BD3" icon={<Scaling size={20} />} items={SIZES_PAGE.map(([id, label]) => ({
                label, icon: s.page?.size === id ? <Check size={14} /> : undefined, onClick: () => act({ t: "page_setup", size: id }),
              }))} />
              <MenuBtn big label="Orientation" tint="#0E7C6B" icon={s.page?.orientation === "landscape" ? <RectangleHorizontal size={20} /> : <RectangleVertical size={20} />} items={[
                { label: "Portrait", icon: s.page?.orientation !== "landscape" ? <Check size={14} /> : <RectangleVertical size={15} />, onClick: () => act({ t: "page_setup", orientation: "portrait" }) },
                { label: "Landscape", icon: s.page?.orientation === "landscape" ? <Check size={14} /> : <RectangleHorizontal size={15} />, onClick: () => act({ t: "page_setup", orientation: "landscape" }) },
              ]} />
              <MenuBtn big label="Margins" tint="#7A5AF8" icon={<FileText size={20} />} items={MARGINS.map(([id, label]) => ({
                label, icon: s.page?.margins === id ? <Check size={14} /> : undefined, onClick: () => act({ t: "page_setup", margins: id }),
              }))} />
              <MenuBtn big label="Columns" tint="#C4320A" icon={<Columns2 size={20} />} items={[1, 2, 3].map((n) => ({
                label: n === 1 ? "One column" : `${n === 2 ? "Two" : "Three"} columns`, icon: s.page?.cols === n ? <Check size={14} /> : n === 3 ? <Columns3 size={15} /> : undefined,
                onClick: () => act({ t: "page_setup", cols: n }),
              }))} />
            </Group>
            <Group label="Breaks">
              <Btn big label="Page break" icon={<SeparatorHorizontal size={20} />} shortcut="⌘↩" onClick={() => act({ t: "page_break" })} />
            </Group>
          </>
        )}
        {shown === "Table" && (
          <>
            <Group label="Rows & columns">
              <Col>
                <Btn showLabel label="Insert above" icon={<BetweenHorizontalStart size={15} />} onClick={() => act({ t: "table_row", where: "above" })} />
                <Btn showLabel label="Insert below" icon={<BetweenHorizontalEnd size={15} />} onClick={() => act({ t: "table_row", where: "below" })} />
              </Col>
              <Col>
                <Btn showLabel label="Insert left" icon={<BetweenVerticalStart size={15} />} onClick={() => act({ t: "table_col", where: "left" })} />
                <Btn showLabel label="Insert right" icon={<BetweenVerticalEnd size={15} />} onClick={() => act({ t: "table_col", where: "right" })} />
              </Col>
              <MenuBtn big label="Delete" tint="#B42318" icon={<Trash2 size={20} />} items={[
                { label: "Delete row", onClick: () => act({ t: "table_delete", what: "row" }) },
                { label: "Delete column", onClick: () => act({ t: "table_delete", what: "col" }) },
                "-",
                { label: "Delete table", danger: true, onClick: () => act({ t: "table_delete", what: "table" }) },
              ]} />
            </Group>
            <Group label="Cells">
              <Btn big label="Merge right" tint="#2F5BD3" icon={<TableCellsMerge size={20} />} onClick={() => act({ t: "table_merge" })} />
              <ColorBtn label="Shading" icon={<PaintBucket size={16} />} clearLabel="No color" initial="#FFF2CC" onPick={(c) => act({ t: "table_shade", color: c })} />
            </Group>
          </>
        )}
        {shown === "Review" && (
          <>
            <Group label="Proofing">
              <Btn big label="Spelling" tint="#B42318" active={s.spell} icon={<SpellCheck size={20} />} onClick={() => act({ t: "spell" })} />
            </Group>
            <Group label="Comments">
              <Btn big label="New comment" tint="#B54708" icon={<MessageSquarePlus size={20} />} shortcut="⌥⌘M" onClick={() => act({ t: "comment" })} />
            </Group>
            <Group label="Tracking">
              <Btn big label={s.track ? "Tracking on" : "Track changes"} tint="#C4320A" active={s.track} icon={<MessageSquareDiff size={20} />} shortcut="⇧⌘E" onClick={() => act({ t: "track" })} />
              <Btn big label="Review panel" tint="#7A5AF8" active={s.pane} icon={<PanelRight size={20} />} onClick={() => act({ t: "pane" })} />
            </Group>
            <Group label="Changes">
              <Btn big label="Accept all" tint="#027A48" icon={<CheckCheck size={20} />} onClick={() => act({ t: "accept_all" })} />
              <Btn big label="Reject all" tint="#B42318" icon={<X size={20} />} onClick={() => act({ t: "reject_all" })} />
            </Group>
          </>
        )}
        {shown === "View" && (
          <>
            <Group label="Zoom">
              <Btn big label="Zoom in" icon={<ZoomIn size={20} />} shortcut="⌘=" onClick={() => act({ t: "zoom", value: Math.min(3, s.zoom + 0.1) })} />
              <Btn big label="Zoom out" icon={<ZoomOut size={20} />} shortcut="⌘−" onClick={() => act({ t: "zoom", value: Math.max(0.4, s.zoom - 0.1) })} />
              <Btn big label="100%" icon={<Check size={20} />} shortcut="⌘0" onClick={() => act({ t: "zoom", value: 1 })} />
              <Btn big label="Page width" icon={<FileText size={20} />} onClick={() => act({ t: "zoom", value: -1 })} />
            </Group>
            <Group label="Navigate">
              <Btn big label="Headings" active={s.nav} icon={<TextQuote size={20} />} onClick={() => act({ t: "nav" })} />
              <Btn big label="Find" icon={<Search size={20} />} shortcut="⌘F" onClick={() => act({ t: "find" })} />
              <Btn big label="Replace" icon={<Replace size={20} />} shortcut="⌥⌘F" onClick={() => act({ t: "replace" })} />
              <Btn big label="Commands" icon={<Command size={20} />} shortcut="⌘K" onClick={() => act({ t: "palette" })} />
            </Group>
          </>
        )}
      </div>
    </div>
  );
}
