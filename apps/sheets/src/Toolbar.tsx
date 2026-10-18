import { useEffect, useRef, useState } from "react";
import {
  AlignCenter, AlignLeft, AlignRight, BetweenHorizontalStart, BetweenVerticalStart, Bold, FilePlus, FolderOpen,
  Italic, PaintBucket, Redo2, Save, Snowflake, Strikethrough, Underline, Undo2, WrapText, Baseline,
} from "lucide-react";
import type { CellStyle } from "./api";

export const NUMBER_FORMATS: { label: string; code: string }[] = [
  { label: "General", code: "general" },
  { label: "Number", code: "#,##0.00" },
  { label: "Currency", code: "$#,##0.00" },
  { label: "Percent", code: "0.00%" },
  { label: "Date", code: "yyyy-mm-dd" },
  { label: "Time", code: "h:mm AM/PM" },
  { label: "Scientific", code: "0.00E+00" },
  { label: "Text", code: "@" },
];

const SWATCHES = [
  "#1F2328", "#57606A", "#B42318", "#C4320A", "#B54708", "#027A48", "#0E7C6B", "#175CD3", "#5925DC", "#C11574",
  "#FFFFFF", "#F2F4F7", "#FEE4E2", "#FFE6D5", "#FEF0C7", "#D1FADF", "#CCF1EA", "#D1E9FF", "#EBE9FE", "#FCE7F6",
];

interface Props {
  style: CellStyle | null;
  canUndo: boolean;
  canRedo: boolean;
  frozen: boolean;
  onFile: (action: "new" | "open" | "save") => void;
  onUndo: () => void;
  onRedo: () => void;
  onStyle: (path: string, value: string) => void;
  onInsert: (what: "row" | "col") => void;
  onFreeze: () => void;
}

function Btn(props: { label: string; shortcut?: string; active?: boolean; disabled?: boolean; onClick: () => void; children: React.ReactNode }) {
  const title = props.shortcut ? `${props.label} (${props.shortcut})` : props.label;
  return (
    <button
      type="button"
      className={`tb-btn${props.active ? " is-active" : ""}`}
      aria-label={props.label}
      aria-pressed={props.active}
      title={title}
      disabled={props.disabled}
      onMouseDown={(e) => e.preventDefault()} // keep focus in the grid
      onClick={props.onClick}
    >
      {props.children}
    </button>
  );
}

function ColorButton(props: { label: string; icon: React.ReactNode; clearLabel: string; onPick: (color: string) => void }) {
  const [open, setOpen] = useState(false);
  const [last, setLast] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => { if (!ref.current?.contains(e.target as Node)) setOpen(false); };
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [open]);
  const pick = (c: string) => { setLast(c || null); setOpen(false); props.onPick(c); };
  return (
    <div className="color-btn" ref={ref}>
      <button
        type="button" className="tb-btn" aria-label={props.label} title={props.label}
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => (last ? props.onPick(last) : setOpen(true))}
      >
        {props.icon}
        <span className="color-bar" style={{ background: last ?? "transparent" }} />
      </button>
      <button
        type="button" className="tb-btn tb-caret" aria-label={`${props.label} options`}
        onMouseDown={(e) => e.preventDefault()} onClick={() => setOpen((o) => !o)}
      >
        <svg width="8" height="8" viewBox="0 0 8 8" aria-hidden><path d="M1 2.5 4 5.5 7 2.5" fill="none" stroke="currentColor" strokeWidth="1.4" /></svg>
      </button>
      {open && (
        <div className="swatches" role="dialog" aria-label={props.label}>
          <div className="swatch-grid">
            {SWATCHES.map((c) => (
              <button key={c} type="button" className="swatch" style={{ background: c }} aria-label={c} title={c}
                onMouseDown={(e) => e.preventDefault()} onClick={() => pick(c)} />
            ))}
          </div>
          <button type="button" className="swatch-clear" onMouseDown={(e) => e.preventDefault()} onClick={() => pick("")}>
            {props.clearLabel}
          </button>
        </div>
      )}
    </div>
  );
}

export function Toolbar(p: Props) {
  const s = p.style;
  const align = s?.align ?? "general";
  const fmt = NUMBER_FORMATS.find((f) => f.code.toLowerCase() === (s?.num_fmt ?? "general").toLowerCase());
  return (
    <div className="toolbar" role="toolbar" aria-label="Formatting">
      <div className="tb-group">
        <Btn label="New workbook" shortcut="⌘N" onClick={() => p.onFile("new")}><FilePlus size={17} /></Btn>
        <Btn label="Open" shortcut="⌘O" onClick={() => p.onFile("open")}><FolderOpen size={17} /></Btn>
        <Btn label="Save" shortcut="⌘S" onClick={() => p.onFile("save")}><Save size={17} /></Btn>
      </div>
      <div className="tb-group">
        <Btn label="Undo" shortcut="⌘Z" disabled={!p.canUndo} onClick={p.onUndo}><Undo2 size={17} /></Btn>
        <Btn label="Redo" shortcut="⇧⌘Z" disabled={!p.canRedo} onClick={p.onRedo}><Redo2 size={17} /></Btn>
      </div>
      <div className="tb-group">
        <select
          className="tb-select" aria-label="Number format" title="Number format"
          value={fmt?.code ?? "custom"}
          onChange={(e) => p.onStyle("num_fmt", e.target.value)}
        >
          {!fmt && <option value="custom">{s?.num_fmt ?? "Custom"}</option>}
          {NUMBER_FORMATS.map((f) => <option key={f.code} value={f.code}>{f.label}</option>)}
        </select>
        <div className="tb-size" role="group" aria-label="Font size">
          <button type="button" className="tb-btn tb-small" aria-label="Smaller text" title="Smaller text"
            onMouseDown={(e) => e.preventDefault()} onClick={() => p.onStyle("font.size_delta", "-1")}>A−</button>
          <span className="tb-size-value" aria-live="polite">{s?.size ?? 11}</span>
          <button type="button" className="tb-btn tb-small" aria-label="Larger text" title="Larger text"
            onMouseDown={(e) => e.preventDefault()} onClick={() => p.onStyle("font.size_delta", "1")}>A+</button>
        </div>
      </div>
      <div className="tb-group">
        <Btn label="Bold" shortcut="⌘B" active={s?.b} onClick={() => p.onStyle("font.b", String(!s?.b))}><Bold size={17} /></Btn>
        <Btn label="Italic" shortcut="⌘I" active={s?.i} onClick={() => p.onStyle("font.i", String(!s?.i))}><Italic size={17} /></Btn>
        <Btn label="Underline" shortcut="⌘U" active={s?.u} onClick={() => p.onStyle("font.u", String(!s?.u))}><Underline size={17} /></Btn>
        <Btn label="Strikethrough" active={s?.s} onClick={() => p.onStyle("font.strike", String(!s?.s))}><Strikethrough size={17} /></Btn>
        <ColorButton label="Text color" clearLabel="Automatic" icon={<Baseline size={17} />} onPick={(c) => p.onStyle("font.color", c)} />
        <ColorButton label="Fill color" clearLabel="No fill" icon={<PaintBucket size={17} />} onPick={(c) => p.onStyle("fill.color", c)} />
      </div>
      <div className="tb-group">
        <Btn label="Align left" active={align === "left"} onClick={() => p.onStyle("alignment.horizontal", align === "left" ? "general" : "left")}><AlignLeft size={17} /></Btn>
        <Btn label="Center" active={align === "center"} onClick={() => p.onStyle("alignment.horizontal", align === "center" ? "general" : "center")}><AlignCenter size={17} /></Btn>
        <Btn label="Align right" active={align === "right"} onClick={() => p.onStyle("alignment.horizontal", align === "right" ? "general" : "right")}><AlignRight size={17} /></Btn>
        <Btn label="Wrap text" active={s?.wrap} onClick={() => p.onStyle("alignment.wrap_text", String(!s?.wrap))}><WrapText size={17} /></Btn>
      </div>
      <div className="tb-group">
        <Btn label="Insert row above" onClick={() => p.onInsert("row")}><BetweenHorizontalStart size={17} /></Btn>
        <Btn label="Insert column to the left" onClick={() => p.onInsert("col")}><BetweenVerticalStart size={17} /></Btn>
        <Btn label={p.frozen ? "Unfreeze panes" : "Freeze rows and columns above and left of the selected cell"} active={p.frozen} onClick={p.onFreeze}><Snowflake size={17} /></Btn>
      </div>
    </div>
  );
}
