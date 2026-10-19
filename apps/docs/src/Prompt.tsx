// A small modal text prompt (link addresses and the like).

import { useState } from "react";

export interface PromptSpec { title: string; label: string; initial?: string; placeholder?: string; submit: string; onSubmit: (v: string) => void; extra?: { label: string; danger?: boolean; run: () => void } }

export function Prompt({ spec, onClose }: { spec: PromptSpec; onClose: () => void }) {
  const [v, setV] = useState(spec.initial ?? "");
  const ok = () => { if (v.trim()) { spec.onSubmit(v.trim()); onClose(); } };
  return (
    <div className="palette-backdrop" onMouseDown={onClose}>
      <div className="prompt" role="dialog" aria-label={spec.title} onMouseDown={(e) => e.stopPropagation()}>
        <h2>{spec.title}</h2>
        <label className="prompt-label">{spec.label}
          <input autoFocus value={v} placeholder={spec.placeholder} onChange={(e) => setV(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter") ok(); if (e.key === "Escape") onClose(); }} />
        </label>
        <div className="prompt-actions">
          {spec.extra && <button type="button" className={`pane-btn${spec.extra.danger ? " danger" : ""}`} onClick={() => { spec.extra!.run(); onClose(); }}>{spec.extra.label}</button>}
          <span className="title-spacer" />
          <button type="button" className="pane-btn" onClick={onClose}>Cancel</button>
          <button type="button" className="pane-btn primary" disabled={!v.trim()} onClick={ok}>{spec.submit}</button>
        </div>
      </div>
    </div>
  );
}
