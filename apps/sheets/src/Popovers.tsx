// Small popovers anchored to grid cells: the filter menu on a header and the
// dropdown list of a validated cell.

import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowDownAZ, ArrowDownZA, Check, Search } from "lucide-react";

function useOutside(onClose: () => void) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const down = (e: MouseEvent) => { if (!ref.current?.contains(e.target as Node)) onClose(); };
    const key = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    const t = window.setTimeout(() => window.addEventListener("mousedown", down), 0);
    window.addEventListener("keydown", key);
    return () => { window.clearTimeout(t); window.removeEventListener("mousedown", down); window.removeEventListener("keydown", key); };
  }, [onClose]);
  return ref;
}

export function FilterPopover(p: {
  x: number; y: number;
  title: string;
  values: string[];
  selected: Set<string> | null; // null = everything shown
  onApply: (keep: Set<string> | null) => void;
  onSort: (desc: boolean) => void;
  onClose: () => void;
}) {
  const ref = useOutside(p.onClose);
  const [q, setQ] = useState("");
  const [sel, setSel] = useState<Set<string>>(() => new Set(p.selected ?? p.values));
  const shown = useMemo(() => p.values.filter((v) => v.toLowerCase().includes(q.toLowerCase())), [p.values, q]);
  const all = shown.every((v) => sel.has(v));
  const label = (v: string) => (v === "" ? "(Blanks)" : v);
  const toggleAll = () => setSel((s) => {
    const n = new Set(s);
    shown.forEach((v) => (all ? n.delete(v) : n.add(v)));
    return n;
  });
  return (
    <div className="popover filter-pop" ref={ref} style={{ left: p.x, top: p.y }} role="dialog" aria-label={`Filter ${p.title}`}>
      <button type="button" className="pop-row" onClick={() => { p.onSort(false); p.onClose(); }}><ArrowDownAZ size={15} /> Sort A to Z</button>
      <button type="button" className="pop-row" onClick={() => { p.onSort(true); p.onClose(); }}><ArrowDownZA size={15} /> Sort Z to A</button>
      <div className="pop-sep" />
      <div className="pop-search">
        <Search size={14} aria-hidden />
        <input autoFocus placeholder="Search" value={q} onChange={(e) => setQ(e.target.value)} />
      </div>
      <div className="pop-list">
        <label className="pop-check"><input type="checkbox" checked={all} onChange={toggleAll} /> <b>Select all</b></label>
        {shown.map((v) => (
          <label key={v} className="pop-check">
            <input type="checkbox" checked={sel.has(v)} onChange={() => setSel((s) => { const n = new Set(s); if (n.has(v)) n.delete(v); else n.add(v); return n; })} />
            <span className={v === "" ? "muted" : ""}>{label(v)}</span>
          </label>
        ))}
        {!shown.length && <div className="pop-empty">No values match.</div>}
      </div>
      <div className="pop-actions">
        <button type="button" className="pop-btn" onClick={() => { p.onApply(null); p.onClose(); }}>Clear filter</button>
        <button type="button" className="pop-btn primary" disabled={sel.size === 0}
          onClick={() => { p.onApply(sel.size === p.values.length ? null : sel); p.onClose(); }}>Apply</button>
      </div>
    </div>
  );
}

export function ListPopover(p: { x: number; y: number; width: number; items: string[]; current: string; onPick: (v: string) => void; onClose: () => void }) {
  const ref = useOutside(p.onClose);
  const [index, setIndex] = useState(Math.max(0, p.items.indexOf(p.current)));
  return (
    <div className="popover list-pop" ref={ref} style={{ left: p.x, top: p.y, minWidth: p.width }} role="listbox" tabIndex={-1}
      onKeyDown={(e) => {
        if (e.key === "ArrowDown") { e.preventDefault(); setIndex((i) => Math.min(i + 1, p.items.length - 1)); }
        else if (e.key === "ArrowUp") { e.preventDefault(); setIndex((i) => Math.max(i - 1, 0)); }
        else if (e.key === "Enter") { e.preventDefault(); p.onPick(p.items[index]); }
      }}
      >
      {p.items.map((v, i) => (
        <button key={v + i} type="button" role="option" aria-selected={v === p.current} autoFocus={i === index}
          className={`pop-row${i === index ? " is-active" : ""}`} onMouseMove={() => setIndex(i)} onClick={() => p.onPick(v)}>
          <span className="pop-tick">{v === p.current && <Check size={13} />}</span>{v}
        </button>
      ))}
    </div>
  );
}
