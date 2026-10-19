// Ribbon building blocks shared with Sheaf's ribbon: buttons, menu buttons,
// color pickers and popovers drawn outside the ribbon so they never get clipped.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { ChevronDown } from "lucide-react";

export const SWATCHES = [
  "#1F2328", "#57606A", "#B42318", "#C4320A", "#B54708", "#027A48", "#0E7C6B", "#175CD3", "#5925DC", "#C11574",
  "#FFFFFF", "#F2F4F7", "#FEE4E2", "#FFE6D5", "#FEF0C7", "#D1FADF", "#CCF1EA", "#D1E9FF", "#EBE9FE", "#FCE7F6",
  "#D0D5DD", "#98A2B3", "#F97066", "#FD853A", "#FDB022", "#32D583", "#2FB39C", "#53B1FD", "#9B8AFB", "#F670C7",
];


export const keep = (e: React.MouseEvent) => e.preventDefault(); // keep focus in the document

export function usePopover() {
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
export function Floating(p: { anchor: React.RefObject<HTMLDivElement | null>; floatRef: React.RefObject<HTMLDivElement | null>; className: string; role: string; label?: string; children: React.ReactNode }) {
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

export function Btn(p: {
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

export interface MenuItem { label: string; icon?: React.ReactNode; shortcut?: string; onClick: () => void; danger?: boolean }

export function MenuBtn(p: { label: string; icon: React.ReactNode; big?: boolean; showLabel?: boolean; items: (MenuItem | "-")[]; title?: string; tint?: string; active?: boolean; panel?: (close: () => void) => React.ReactNode }) {
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
        <Floating anchor={ref} floatRef={floatRef} className={p.panel ? "rb-panel" : ""} role={p.panel ? "dialog" : "menu"} label={p.label}>
          {p.panel?.(() => setOpen(false))}
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

export function ColorBtn(p: { label: string; icon: React.ReactNode; clearLabel: string; onPick: (c: string) => void; initial: string }) {
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

export function Group({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="rb-group" role="group" aria-label={label}>
      <div className="rb-group-body">{children}</div>
      <div className="rb-group-label">{label}</div>
    </div>
  );
}

export const Col = ({ children }: { children: React.ReactNode }) => <div className="rb-col">{children}</div>;
export const Row = ({ children }: { children: React.ReactNode }) => <div className="rb-row">{children}</div>;
