// Excel-style ribbon: tabs of labeled groups. Every control emits an `Action`;
// App decides what each one does.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  AlignCenter, AlignLeft, AlignRight, AlignVerticalJustifyCenter, AlignVerticalJustifyEnd, AlignVerticalJustifyStart,
  ArrowDownAZ, ArrowDownToLine, ArrowDownZA, ArrowRightToLine, Baseline, BetweenHorizontalStart, BetweenVerticalStart,
  Bold, CalendarPlus, ChevronDown, ClipboardPaste, Clock, Copy, DollarSign, Eraser, Eye, EyeOff, FilePlus, FolderOpen,
  Grid3x3, Italic, PaintBucket, PanelLeft, PanelTop, Percent, Redo2, Replace, Rows3, Save, SaveAll, Scissors, Search,
  Sheet, Sigma, Snowflake, SquareFunction, Strikethrough, TableCellsMerge, TableCellsSplit, Trash2, Underline, Undo2,
  WrapText, ZoomIn, ZoomOut, Columns3, Paintbrush, Funnel, ChartColumnBig, ChartLine, ChartPie, ChartBarBig, ChartArea,
  ChartScatter, CopyMinus, SplitSquareHorizontal, Printer, LayoutTemplate, Command, Crosshair, ClipboardType, MoveHorizontal,
} from "lucide-react";
import type { CellStyle } from "./api";
import { FUNCTIONS } from "./functions";
import { BorderIcon, type BorderKind } from "./icons";

export type Action =
  | { t: "file"; a: "new" | "open" | "save" | "save_as" }
  | { t: "undo" } | { t: "redo" }
  | { t: "cut" } | { t: "copy" } | { t: "paste" }
  | { t: "style"; path: string; value: string }
  | { t: "border"; kind: BorderKind }
  | { t: "merge"; mode: "center" | "merge" | "unmerge" }
  | { t: "decimals"; delta: number }
  | { t: "insert"; what: "rows" | "cols" | "sheet" }
  | { t: "delete"; what: "rows" | "cols" | "sheet" }
  | { t: "autosum"; fn: string }
  | { t: "fill"; dir: "down" | "right" }
  | { t: "clear"; what: "contents" | "formats" | "all" }
  | { t: "sort"; desc: boolean }
  | { t: "find"; replace: boolean }
  | { t: "freeze"; mode: "selection" | "row" | "col" | "none" }
  | { t: "zoom"; value: number }
  | { t: "toggle"; what: "grid" | "formulas" }
  | { t: "hide"; kind: "rows" | "cols"; hidden: boolean }
  | { t: "function"; name: string }
  | { t: "stamp"; what: "date" | "time" }
  | { t: "chart"; kind: string }
  | { t: "filter" }
  | { t: "painter" }
  | { t: "paste_values" }
  | { t: "autofit" }
  | { t: "goto" }
  | { t: "dedupe" }
  | { t: "split" }
  | { t: "print" }
  | { t: "palette" }
  | { t: "start" };

export const NUMBER_FORMATS: { label: string; code: string; sample: string }[] = [
  { label: "General", code: "general", sample: "1234.5" },
  { label: "Number", code: "#,##0.00", sample: "1,234.50" },
  { label: "Currency", code: "$#,##0.00", sample: "$1,234.50" },
  { label: "Accounting", code: '_($* #,##0.00_);_($* (#,##0.00);_($* "-"??_);_(@_)', sample: "$ 1,234.50" },
  { label: "Short date", code: "yyyy-mm-dd", sample: "2026-10-18" },
  { label: "Long date", code: "dddd, mmmm d, yyyy", sample: "Sunday, October 18, 2026" },
  { label: "Time", code: "h:mm:ss AM/PM", sample: "3:45:00 PM" },
  { label: "Percentage", code: "0.00%", sample: "12.50%" },
  { label: "Fraction", code: "# ?/?", sample: "1 1/4" },
  { label: "Scientific", code: "0.00E+00", sample: "1.23E+03" },
  { label: "Text", code: "@", sample: "1234.5" },
];

const SIZES = [8, 9, 10, 11, 12, 14, 16, 18, 20, 24, 28, 36, 48, 72];
const TABS = ["Home", "Insert", "Formulas", "Data", "View"] as const;
type Tab = (typeof TABS)[number];

const SWATCHES = [
  "#1F2328", "#57606A", "#B42318", "#C4320A", "#B54708", "#027A48", "#0E7C6B", "#175CD3", "#5925DC", "#C11574",
  "#FFFFFF", "#F2F4F7", "#FEE4E2", "#FFE6D5", "#FEF0C7", "#D1FADF", "#CCF1EA", "#D1E9FF", "#EBE9FE", "#FCE7F6",
  "#D0D5DD", "#98A2B3", "#F97066", "#FD853A", "#FDB022", "#32D583", "#2FB39C", "#53B1FD", "#9B8AFB", "#F670C7",
];

export interface RibbonState {
  style: CellStyle | null;
  canUndo: boolean;
  canRedo: boolean;
  frozen: boolean;
  showGrid: boolean;
  showFormulas: boolean;
  zoom: number;
  painter: boolean;
  filterOn: boolean;
}

const keep = (e: React.MouseEvent) => e.preventDefault(); // keep focus in the grid

function usePopover() {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);      // the button wrapper (anchor)
  const floatRef = useRef<HTMLDivElement>(null); // the floating panel, rendered outside the ribbon
  useEffect(() => {
    if (!open) return;
    const inside = (n: Node) => ref.current?.contains(n) || floatRef.current?.contains(n);
    const close = (e: MouseEvent) => { if (!inside(e.target as Node)) setOpen(false); };
    const esc = (e: KeyboardEvent) => { if (e.key === "Escape") setOpen(false); };
    const away = () => setOpen(false);
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", esc);
    window.addEventListener("resize", away);
    return () => { window.removeEventListener("mousedown", close); window.removeEventListener("keydown", esc); window.removeEventListener("resize", away); };
  }, [open]);
  return { open, setOpen, ref, floatRef };
}

/** A dropdown panel drawn at the document root (the ribbon clips its overflow),
 *  placed under its anchor and kept inside the window. */
function Floating(p: { anchor: React.RefObject<HTMLDivElement | null>; floatRef: React.RefObject<HTMLDivElement | null>; className: string; role: string; label?: string; children: React.ReactNode }) {
  const [pos, setPos] = useState<{ left: number; top: number; maxH: number } | null>(null);
  useLayoutEffect(() => {
    const a = p.anchor.current?.getBoundingClientRect();
    const el = p.floatRef.current;
    if (!a || !el) return;
    const w = el.offsetWidth, h = el.offsetHeight;
    const left = Math.max(8, Math.min(a.left, window.innerWidth - w - 8));
    let top = a.bottom + 4;
    const below = window.innerHeight - top - 8;
    if (h > below && a.top - 4 - h > 8) top = a.top - 4 - h; // flip above if it doesn't fit below
    setPos({ left, top, maxH: Math.max(160, window.innerHeight - top - 8) });
  }, [p.anchor, p.floatRef]);
  return createPortal(
    <div ref={p.floatRef} className={`rb-dropdown ${p.className}`} role={p.role} aria-label={p.label}
      style={{ position: "fixed", left: pos?.left ?? -9999, top: pos?.top ?? -9999, maxHeight: pos?.maxH, overflowY: "auto" }}>
      {p.children}
    </div>,
    document.body,
  );
}

function Btn(p: {
  label: string; icon?: React.ReactNode; big?: boolean; showLabel?: boolean; shortcut?: string;
  active?: boolean; disabled?: boolean; onClick: () => void; tint?: string;
}) {
  return (
    <button
      type="button"
      className={`rb-btn${p.big ? " is-big" : ""}${p.showLabel ? " has-label" : ""}${p.active ? " is-active" : ""}`}
      aria-label={p.label}
      aria-pressed={p.active}
      title={p.shortcut ? `${p.label} (${p.shortcut})` : p.label}
      disabled={p.disabled}
      onMouseDown={keep}
      onClick={p.onClick}
    >
      {p.tint ? <span className="rb-tile" style={{ ["--tint" as string]: p.tint }}>{p.icon}</span> : p.icon}
      {(p.big || p.showLabel) && <span className="rb-label">{p.label}</span>}
    </button>
  );
}

interface MenuItem { label: string; icon?: React.ReactNode; shortcut?: string; onClick: () => void; danger?: boolean }

function MenuBtn(p: { label: string; icon: React.ReactNode; big?: boolean; showLabel?: boolean; items: (MenuItem | "-")[]; title?: string; tint?: string; active?: boolean }) {
  const { open, setOpen, ref, floatRef } = usePopover();
  return (
    <div className="rb-menu-wrap" ref={ref}>
      <button
        type="button"
        className={`rb-btn rb-menu-btn${p.big ? " is-big" : ""}${p.showLabel ? " has-label" : ""}${open ? " is-open" : ""}${p.active ? " is-active" : ""}`}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={p.label}
        title={p.title ?? p.label}
        onMouseDown={keep}
        onClick={() => setOpen((o) => !o)}
      >
        {p.tint ? <span className="rb-tile" style={{ ["--tint" as string]: p.tint }}>{p.icon}</span> : p.icon}
        {(p.big || p.showLabel) && <span className="rb-label">{p.label}</span>}
        <ChevronDown size={12} className="rb-caret" aria-hidden />
      </button>
      {open && (
        <Floating anchor={ref} floatRef={floatRef} className="" role="menu">
          {p.items.map((it, i) => it === "-" ? <div key={i} className="menu-sep" role="separator" /> : (
            <button key={i} type="button" role="menuitem" className={`menu-item${it.danger ? " is-danger" : ""}`}
              onMouseDown={keep} onClick={() => { setOpen(false); it.onClick(); }}>
              <span className="menu-icon" aria-hidden>{it.icon}</span>
              <span className="menu-text">{it.label}</span>
              {it.shortcut && <kbd>{it.shortcut}</kbd>}
            </button>
          ))}
        </Floating>
      )}
    </div>
  );
}

function ColorBtn(p: { label: string; icon: React.ReactNode; clearLabel: string; onPick: (c: string) => void; initial: string }) {
  const { open, setOpen, ref, floatRef } = usePopover();
  const [last, setLast] = useState(p.initial);
  const pick = (c: string) => { if (c) setLast(c); setOpen(false); p.onPick(c); };
  return (
    <div className="rb-split" ref={ref}>
      <button type="button" className="rb-btn" aria-label={`${p.label}: ${last}`} title={p.label} onMouseDown={keep} onClick={() => p.onPick(last)}>
        {p.icon}
        <span className="color-bar" style={{ background: last }} />
      </button>
      <button type="button" className="rb-btn rb-split-caret" aria-label={`${p.label} options`} aria-expanded={open}
        onMouseDown={keep} onClick={() => setOpen((o) => !o)}>
        <ChevronDown size={12} aria-hidden />
      </button>
      {open && (
        <Floating anchor={ref} floatRef={floatRef} className="swatches" role="dialog" label={p.label}>
          <div className="swatch-grid">
            {SWATCHES.map((c) => (
              <button key={c} type="button" className="swatch" style={{ background: c }} aria-label={c} title={c}
                onMouseDown={keep} onClick={() => pick(c)} />
            ))}
          </div>
          <button type="button" className="swatch-clear" onMouseDown={keep} onClick={() => pick("")}>{p.clearLabel}</button>
        </Floating>
      )}
    </div>
  );
}

function FunctionPicker({ act }: { act: (a: Action) => void }) {
  const { open, setOpen, ref, floatRef } = usePopover();
  const [q, setQ] = useState("");
  const list = FUNCTIONS.filter((f) => !q || f.name.includes(q.toUpperCase()) || f.desc.toLowerCase().includes(q.toLowerCase()));
  const groups = [...new Set(list.map((f) => f.category))];
  return (
    <div className="rb-menu-wrap" ref={ref}>
      <Btn big label="Insert function" icon={<SquareFunction size={21} />} onClick={() => setOpen((o) => !o)} />
      {open && (
        <Floating anchor={ref} floatRef={floatRef} className="fn-picker" role="dialog" label="Insert function">
          <input autoFocus className="fn-search" placeholder="Search functions, like lookup or average" value={q} onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter" && list[0]) { setOpen(false); act({ t: "function", name: list[0].name }); } }} />
          <div className="fn-list">
            {groups.map((g) => (
              <div key={g}>
                <div className="fn-group">{g}</div>
                {list.filter((f) => f.category === g).map((f) => (
                  <button key={f.name} type="button" className="fn-item" onClick={() => { setOpen(false); act({ t: "function", name: f.name }); }}>
                    <b>{f.name}</b><span>{f.desc}</span>
                  </button>
                ))}
              </div>
            ))}
            {!list.length && <div className="fn-empty">No function matches “{q}”.</div>}
          </div>
        </Floating>
      )}
    </div>
  );
}

function Group({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="rb-group" role="group" aria-label={label}>
      <div className="rb-group-body">{children}</div>
      <div className="rb-group-label">{label}</div>
    </div>
  );
}

const Col = ({ children }: { children: React.ReactNode }) => <div className="rb-col">{children}</div>;
const Row = ({ children }: { children: React.ReactNode }) => <div className="rb-row">{children}</div>;

export function Ribbon({ s, act }: { s: RibbonState; act: (a: Action) => void }) {
  const [tab, setTab] = useState<Tab>("Home");
  // when the window is too narrow for a tab, the wheel scrolls it sideways and the edges fade
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
  const st = s.style;
  const align = st?.align ?? "general";
  const valign = st?.valign ?? "bottom";
  const fmt = NUMBER_FORMATS.find((f) => f.code.toLowerCase() === (st?.num_fmt ?? "general").toLowerCase());
  const toggle = (path: string, on: boolean | undefined) => act({ t: "style", path, value: String(!on) });
  const sum = (fn: string) => () => act({ t: "autosum", fn });

  return (
    <div className="ribbon">
      <div className="rb-tabs" role="tablist" aria-label="Ribbon">
        <MenuBtn label="File" icon={null} showLabel items={[
          { label: "New workbook", icon: <FilePlus size={15} />, shortcut: "⌘N", onClick: () => act({ t: "file", a: "new" }) },
          { label: "New from template…", icon: <LayoutTemplate size={15} />, shortcut: "⇧⌘N", onClick: () => act({ t: "start" }) },
          { label: "Open…", icon: <FolderOpen size={15} />, shortcut: "⌘O", onClick: () => act({ t: "file", a: "open" }) },
          "-",
          { label: "Save", icon: <Save size={15} />, shortcut: "⌘S", onClick: () => act({ t: "file", a: "save" }) },
          { label: "Save as…", icon: <SaveAll size={15} />, shortcut: "⇧⌘S", onClick: () => act({ t: "file", a: "save_as" }) },
          "-",
          { label: "Print or save as PDF…", icon: <Printer size={15} />, shortcut: "⌘P", onClick: () => act({ t: "print" }) },
        ]} />
        {TABS.map((t) => (
          <button key={t} type="button" role="tab" aria-selected={tab === t} className={`rb-tab${tab === t ? " is-active" : ""}`}
            onMouseDown={keep} onClick={() => setTab(t)}>{t}</button>
        ))}
        <div className="rb-quick">
          <Btn label="Undo" shortcut="⌘Z" icon={<Undo2 size={16} />} disabled={!s.canUndo} onClick={() => act({ t: "undo" })} />
          <Btn label="Redo" shortcut="⇧⌘Z" icon={<Redo2 size={16} />} disabled={!s.canRedo} onClick={() => act({ t: "redo" })} />
          <Btn label="Save" shortcut="⌘S" icon={<Save size={16} />} onClick={() => act({ t: "file", a: "save" })} />
        </div>
      </div>

      <div className={`rb-body${edges.left ? " fade-left" : ""}${edges.right ? " fade-right" : ""}`} role="tabpanel" aria-label={tab} ref={bodyRef}
        onWheel={(e) => { const el = bodyRef.current; if (el && el.scrollWidth > el.clientWidth && Math.abs(e.deltaY) > Math.abs(e.deltaX)) el.scrollLeft += e.deltaY; }}>
        {tab === "Home" && (
          <>
            <Group label="Clipboard">
              <MenuBtn big label="Paste" tint="#0E7C6B" icon={<ClipboardPaste size={20} />} items={[
                { label: "Paste", icon: <ClipboardPaste size={15} />, shortcut: "⌘V", onClick: () => act({ t: "paste" }) },
                { label: "Paste values only", icon: <ClipboardType size={15} />, shortcut: "⇧⌘V", onClick: () => act({ t: "paste_values" }) },
              ]} />
              <Col>
                <Btn showLabel label="Cut" shortcut="⌘X" icon={<Scissors size={15} />} onClick={() => act({ t: "cut" })} />
                <Btn showLabel label="Copy" shortcut="⌘C" icon={<Copy size={15} />} onClick={() => act({ t: "copy" })} />
                <Btn showLabel label="Format painter" active={s.painter} icon={<Paintbrush size={15} />} onClick={() => act({ t: "painter" })} />
              </Col>
            </Group>
            <Group label="Font">
              <Col>
                <Row>
                  <select className="rb-select rb-size" aria-label="Font size" title="Font size" value={st?.size ?? 11}
                    onChange={(e) => act({ t: "style", path: "font.size", value: e.target.value })}>
                    {!SIZES.includes(st?.size ?? 11) && <option value={st?.size}>{st?.size}</option>}
                    {SIZES.map((n) => <option key={n} value={n}>{n}</option>)}
                  </select>
                  <Btn label="Increase font size" icon={<span className="glyph">A<sup>+</sup></span>} onClick={() => act({ t: "style", path: "font.size_delta", value: "1" })} />
                  <Btn label="Decrease font size" icon={<span className="glyph">A<sup>−</sup></span>} onClick={() => act({ t: "style", path: "font.size_delta", value: "-1" })} />
                </Row>
                <Row>
                  <Btn label="Bold" shortcut="⌘B" active={st?.b} icon={<Bold size={16} />} onClick={() => toggle("font.b", st?.b)} />
                  <Btn label="Italic" shortcut="⌘I" active={st?.i} icon={<Italic size={16} />} onClick={() => toggle("font.i", st?.i)} />
                  <Btn label="Underline" shortcut="⌘U" active={st?.u} icon={<Underline size={16} />} onClick={() => toggle("font.u", st?.u)} />
                  <Btn label="Strikethrough" active={st?.s} icon={<Strikethrough size={16} />} onClick={() => toggle("font.strike", st?.s)} />
                  <MenuBtn label="Borders" icon={<BorderIcon kind="Bottom" />} items={[
                    { label: "Bottom border", icon: <BorderIcon kind="Bottom" />, onClick: () => act({ t: "border", kind: "Bottom" }) },
                    { label: "Top border", icon: <BorderIcon kind="Top" />, onClick: () => act({ t: "border", kind: "Top" }) },
                    { label: "Left border", icon: <BorderIcon kind="Left" />, onClick: () => act({ t: "border", kind: "Left" }) },
                    { label: "Right border", icon: <BorderIcon kind="Right" />, onClick: () => act({ t: "border", kind: "Right" }) },
                    "-",
                    { label: "All borders", icon: <BorderIcon kind="All" />, onClick: () => act({ t: "border", kind: "All" }) },
                    { label: "Outside borders", icon: <BorderIcon kind="Outer" />, onClick: () => act({ t: "border", kind: "Outer" }) },
                    { label: "Inside borders", icon: <BorderIcon kind="Inner" />, onClick: () => act({ t: "border", kind: "Inner" }) },
                    "-",
                    { label: "No border", icon: <BorderIcon kind="None" />, onClick: () => act({ t: "border", kind: "None" }) },
                  ]} />
                  <ColorBtn label="Fill color" clearLabel="No fill" initial="#FEF0C7" icon={<PaintBucket size={16} />} onPick={(c) => act({ t: "style", path: "fill.color", value: c })} />
                  <ColorBtn label="Font color" clearLabel="Automatic" initial="#B42318" icon={<Baseline size={16} />} onPick={(c) => act({ t: "style", path: "font.color", value: c })} />
                </Row>
              </Col>
            </Group>
            <Group label="Alignment">
              <Col>
                <Row>
                  <Btn label="Top align" active={valign === "top"} icon={<AlignVerticalJustifyStart size={16} />} onClick={() => act({ t: "style", path: "alignment.vertical", value: "top" })} />
                  <Btn label="Middle align" active={valign === "center"} icon={<AlignVerticalJustifyCenter size={16} />} onClick={() => act({ t: "style", path: "alignment.vertical", value: "center" })} />
                  <Btn label="Bottom align" active={valign === "bottom"} icon={<AlignVerticalJustifyEnd size={16} />} onClick={() => act({ t: "style", path: "alignment.vertical", value: "bottom" })} />
                </Row>
                <Row>
                  <Btn label="Align left" active={align === "left"} icon={<AlignLeft size={16} />} onClick={() => act({ t: "style", path: "alignment.horizontal", value: align === "left" ? "general" : "left" })} />
                  <Btn label="Center" active={align === "center"} icon={<AlignCenter size={16} />} onClick={() => act({ t: "style", path: "alignment.horizontal", value: align === "center" ? "general" : "center" })} />
                  <Btn label="Align right" active={align === "right"} icon={<AlignRight size={16} />} onClick={() => act({ t: "style", path: "alignment.horizontal", value: align === "right" ? "general" : "right" })} />
                </Row>
              </Col>
              <Col>
                <Btn showLabel label="Wrap text" active={st?.wrap} icon={<WrapText size={15} />} onClick={() => toggle("alignment.wrap_text", st?.wrap)} />
                <MenuBtn showLabel label="Merge & center" icon={<TableCellsMerge size={15} />} items={[
                  { label: "Merge & center", icon: <TableCellsMerge size={15} />, onClick: () => act({ t: "merge", mode: "center" }) },
                  { label: "Merge cells", icon: <TableCellsMerge size={15} />, onClick: () => act({ t: "merge", mode: "merge" }) },
                  { label: "Unmerge cells", icon: <TableCellsSplit size={15} />, onClick: () => act({ t: "merge", mode: "unmerge" }) },
                ]} />
              </Col>
            </Group>
            <Group label="Number">
              <Col>
                <select className="rb-select rb-format" aria-label="Number format" title="Number format" value={fmt?.code ?? "custom"}
                  onChange={(e) => act({ t: "style", path: "num_fmt", value: e.target.value })}>
                  {!fmt && <option value="custom">Custom: {st?.num_fmt}</option>}
                  {NUMBER_FORMATS.map((f) => <option key={f.code} value={f.code}>{f.label}</option>)}
                </select>
                <Row>
                  <Btn label="Currency" icon={<DollarSign size={15} />} onClick={() => act({ t: "style", path: "num_fmt", value: "$#,##0.00" })} />
                  <Btn label="Percent" icon={<Percent size={15} />} onClick={() => act({ t: "style", path: "num_fmt", value: "0%" })} />
                  <Btn label="Thousands separator" icon={<span className="glyph">,000</span>} onClick={() => act({ t: "style", path: "num_fmt", value: "#,##0.00" })} />
                  <Btn label="Increase decimals" icon={<span className="glyph">.0<sub>→</sub>.00</span>} onClick={() => act({ t: "decimals", delta: 1 })} />
                  <Btn label="Decrease decimals" icon={<span className="glyph">.00<sub>→</sub>.0</span>} onClick={() => act({ t: "decimals", delta: -1 })} />
                </Row>
              </Col>
            </Group>
            <Group label="Cells">
              <Col>
              <MenuBtn showLabel label="Insert" icon={<BetweenHorizontalStart size={15} />} items={[
                { label: "Insert rows above", icon: <Rows3 size={15} />, onClick: () => act({ t: "insert", what: "rows" }) },
                { label: "Insert columns left", icon: <Columns3 size={15} />, onClick: () => act({ t: "insert", what: "cols" }) },
                { label: "Insert sheet", icon: <Sheet size={15} />, onClick: () => act({ t: "insert", what: "sheet" }) },
              ]} />
              <MenuBtn showLabel label="Format" icon={<MoveHorizontal size={15} />} items={[
                { label: "Auto-fit column width", icon: <MoveHorizontal size={15} />, onClick: () => act({ t: "autofit" }) },
                "-",
                { label: "Hide rows", icon: <EyeOff size={15} />, onClick: () => act({ t: "hide", kind: "rows", hidden: true }) },
                { label: "Unhide rows", icon: <Eye size={15} />, onClick: () => act({ t: "hide", kind: "rows", hidden: false }) },
                { label: "Hide columns", icon: <EyeOff size={15} />, onClick: () => act({ t: "hide", kind: "cols", hidden: true }) },
                { label: "Unhide columns", icon: <Eye size={15} />, onClick: () => act({ t: "hide", kind: "cols", hidden: false }) },
              ]} />
              <MenuBtn showLabel label="Delete" icon={<Trash2 size={15} />} items={[
                { label: "Delete rows", icon: <Rows3 size={15} />, danger: true, onClick: () => act({ t: "delete", what: "rows" }) },
                { label: "Delete columns", icon: <Columns3 size={15} />, danger: true, onClick: () => act({ t: "delete", what: "cols" }) },
                { label: "Delete sheet", icon: <Sheet size={15} />, danger: true, onClick: () => act({ t: "delete", what: "sheet" }) },
              ]} />
              </Col>
            </Group>
            <Group label="Editing">
              <Col>
                <MenuBtn showLabel label="AutoSum" icon={<Sigma size={15} />} items={[
                  { label: "Sum", icon: <Sigma size={15} />, onClick: sum("SUM") },
                  { label: "Average", onClick: sum("AVERAGE") },
                  { label: "Count numbers", onClick: sum("COUNT") },
                  { label: "Max", onClick: sum("MAX") },
                  { label: "Min", onClick: sum("MIN") },
                ]} />
                <MenuBtn showLabel label="Fill" icon={<ArrowDownToLine size={15} />} items={[
                  { label: "Down", icon: <ArrowDownToLine size={15} />, shortcut: "⌘D", onClick: () => act({ t: "fill", dir: "down" }) },
                  { label: "Right", icon: <ArrowRightToLine size={15} />, shortcut: "⌘R", onClick: () => act({ t: "fill", dir: "right" }) },
                ]} />
                <MenuBtn showLabel label="Clear" icon={<Eraser size={15} />} items={[
                  { label: "Clear contents", shortcut: "⌫", onClick: () => act({ t: "clear", what: "contents" }) },
                  { label: "Clear formats", onClick: () => act({ t: "clear", what: "formats" }) },
                  { label: "Clear all", onClick: () => act({ t: "clear", what: "all" }) },
                ]} />
              </Col>
              <MenuBtn big label="Sort & Filter" tint="#E8A33D" active={s.filterOn} icon={<Funnel size={20} />} items={[
                { label: "Sort A to Z", icon: <ArrowDownAZ size={15} />, onClick: () => act({ t: "sort", desc: false }) },
                { label: "Sort Z to A", icon: <ArrowDownZA size={15} />, onClick: () => act({ t: "sort", desc: true }) },
                "-",
                { label: s.filterOn ? "Remove filter" : "Filter", icon: <Funnel size={15} />, shortcut: "⇧⌘L", onClick: () => act({ t: "filter" }) },
              ]} />
              <MenuBtn big label="Find & Select" tint="#4F5BD5" icon={<Search size={20} />} items={[
                { label: "Find…", icon: <Search size={15} />, shortcut: "⌘F", onClick: () => act({ t: "find", replace: false }) },
                { label: "Replace…", icon: <Replace size={15} />, shortcut: "⌘H", onClick: () => act({ t: "find", replace: true }) },
                { label: "Go to…", icon: <Crosshair size={15} />, shortcut: "⌘G", onClick: () => act({ t: "goto" }) },
                "-",
                { label: "Command palette…", icon: <Command size={15} />, shortcut: "⌘K", onClick: () => act({ t: "palette" }) },
              ]} />
            </Group>
          </>
        )}

        {tab === "Insert" && (
          <>
            <Group label="Cells">
              <Btn big label="Rows above" icon={<BetweenHorizontalStart size={21} />} onClick={() => act({ t: "insert", what: "rows" })} />
              <Btn big label="Columns left" icon={<BetweenVerticalStart size={21} />} onClick={() => act({ t: "insert", what: "cols" })} />
              <Btn big label="Sheet" icon={<Sheet size={21} />} onClick={() => act({ t: "insert", what: "sheet" })} />
            </Group>
            <Group label="Charts">
              <Btn big label="Column" tint="#0E7C6B" icon={<ChartColumnBig size={20} />} onClick={() => act({ t: "chart", kind: "column" })} />
              <Btn big label="Line" tint="#4F5BD5" icon={<ChartLine size={20} />} onClick={() => act({ t: "chart", kind: "line" })} />
              <Btn big label="Pie" tint="#E8A33D" icon={<ChartPie size={20} />} onClick={() => act({ t: "chart", kind: "pie" })} />
              <Col>
                <Btn showLabel label="Bar" icon={<ChartBarBig size={15} />} onClick={() => act({ t: "chart", kind: "bar" })} />
                <Btn showLabel label="Area" icon={<ChartArea size={15} />} onClick={() => act({ t: "chart", kind: "area" })} />
                <Btn showLabel label="Scatter" icon={<ChartScatter size={15} />} onClick={() => act({ t: "chart", kind: "scatter" })} />
              </Col>
            </Group>
            <Group label="Functions">
              <FunctionPicker act={act} />
            </Group>
            <Group label="Date & time">
              <Btn big label="Today's date" icon={<CalendarPlus size={21} />} shortcut="⌘;" onClick={() => act({ t: "stamp", what: "date" })} />
              <Btn big label="Current time" icon={<Clock size={21} />} onClick={() => act({ t: "stamp", what: "time" })} />
            </Group>
          </>
        )}

        {tab === "Formulas" && (
          <>
            <Group label="Function library">
              <FunctionPicker act={act} />
              <Btn big label="Sum" icon={<Sigma size={21} />} onClick={sum("SUM")} />
              <Col>
                <Btn showLabel label="Average" icon={<span className="glyph">x̄</span>} onClick={sum("AVERAGE")} />
                <Btn showLabel label="Count" icon={<span className="glyph">#</span>} onClick={sum("COUNT")} />
              </Col>
              <Col>
                <Btn showLabel label="Max" icon={<span className="glyph">↑</span>} onClick={sum("MAX")} />
                <Btn showLabel label="Min" icon={<span className="glyph">↓</span>} onClick={sum("MIN")} />
              </Col>
            </Group>
            <Group label="Formula auditing">
              <Btn big label="Show formulas" active={s.showFormulas} icon={<SquareFunction size={21} />} onClick={() => act({ t: "toggle", what: "formulas" })} />
            </Group>
          </>
        )}

        {tab === "Data" && (
          <>
            <Group label="Sort & filter">
              <Btn big label="Sort A to Z" icon={<ArrowDownAZ size={21} />} onClick={() => act({ t: "sort", desc: false })} />
              <Btn big label="Sort Z to A" icon={<ArrowDownZA size={21} />} onClick={() => act({ t: "sort", desc: true })} />
              <Btn big label="Filter" tint="#E8A33D" active={s.filterOn} shortcut="⇧⌘L" icon={<Funnel size={20} />} onClick={() => act({ t: "filter" })} />
            </Group>
            <Group label="Data tools">
              <Btn big label="Remove duplicates" icon={<CopyMinus size={21} />} onClick={() => act({ t: "dedupe" })} />
              <Btn big label="Text to columns" icon={<SplitSquareHorizontal size={21} />} onClick={() => act({ t: "split" })} />
            </Group>
            <Group label="Find">
              <Btn big label="Find" icon={<Search size={21} />} shortcut="⌘F" onClick={() => act({ t: "find", replace: false })} />
              <Btn big label="Replace" icon={<Replace size={21} />} shortcut="⌘H" onClick={() => act({ t: "find", replace: true })} />
            </Group>
            <Group label="Rows & columns">
              <Col>
                <Btn showLabel label="Hide rows" icon={<EyeOff size={15} />} onClick={() => act({ t: "hide", kind: "rows", hidden: true })} />
                <Btn showLabel label="Unhide rows" icon={<Eye size={15} />} onClick={() => act({ t: "hide", kind: "rows", hidden: false })} />
              </Col>
              <Col>
                <Btn showLabel label="Hide columns" icon={<EyeOff size={15} />} onClick={() => act({ t: "hide", kind: "cols", hidden: true })} />
                <Btn showLabel label="Unhide columns" icon={<Eye size={15} />} onClick={() => act({ t: "hide", kind: "cols", hidden: false })} />
              </Col>
            </Group>
          </>
        )}

        {tab === "View" && (
          <>
            <Group label="Show">
              <Btn big label="Gridlines" active={s.showGrid} icon={<Grid3x3 size={21} />} onClick={() => act({ t: "toggle", what: "grid" })} />
              <Btn big label="Formulas" active={s.showFormulas} icon={<SquareFunction size={21} />} onClick={() => act({ t: "toggle", what: "formulas" })} />
            </Group>
            <Group label="Window">
              <MenuBtn big label="Freeze panes" icon={<Snowflake size={21} />} items={[
                { label: "Freeze at selection", icon: <Snowflake size={15} />, onClick: () => act({ t: "freeze", mode: "selection" }) },
                { label: "Freeze top row", icon: <PanelTop size={15} />, onClick: () => act({ t: "freeze", mode: "row" }) },
                { label: "Freeze first column", icon: <PanelLeft size={15} />, onClick: () => act({ t: "freeze", mode: "col" }) },
                ...(s.frozen ? ["-" as const, { label: "Unfreeze panes", onClick: () => act({ t: "freeze", mode: "none" }) }] : []),
              ]} />
            </Group>
            <Group label="Zoom">
              <Btn big label="Zoom out" icon={<ZoomOut size={21} />} onClick={() => act({ t: "zoom", value: Math.max(0.5, +(s.zoom - 0.1).toFixed(2)) })} />
              <Btn big label={`${Math.round(s.zoom * 100)}%`} icon={<span className="glyph big">100</span>} onClick={() => act({ t: "zoom", value: 1 })} />
              <Btn big label="Zoom in" icon={<ZoomIn size={21} />} onClick={() => act({ t: "zoom", value: Math.min(2, +(s.zoom + 0.1).toFixed(2)) })} />
            </Group>
          </>
        )}
      </div>
    </div>
  );
}
