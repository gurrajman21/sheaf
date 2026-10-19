// Navigation pane: the document's headings; click one to go there.

import { X } from "lucide-react";

export interface Heading { p: number[]; level: number; text: string }

export function NavPane(p: { headings: Heading[]; current: number | null; onGo: (h: Heading) => void; onClose: () => void }) {
  return (
    <aside className="nav-pane" aria-label="Navigation">
      <div className="pane-head">
        <h2>Headings</h2>
        <button type="button" className="find-btn" aria-label="Close navigation" onClick={p.onClose}><X size={15} /></button>
      </div>
      {p.headings.length === 0 ? (
        <p className="pane-empty">Headings you add with the Heading styles appear here.</p>
      ) : (
        <ul className="nav-list">
          {p.headings.map((h) => (
            <li key={h.p.join(",")}>
              <button type="button" className={`nav-item lvl-${Math.min(h.level, 3)}${p.current === h.p[0] ? " is-current" : ""}`} onClick={() => p.onGo(h)} title={h.text}>
                {h.text}
              </button>
            </li>
          ))}
        </ul>
      )}
    </aside>
  );
}
