// Version history: snapshots taken on save and every few minutes while editing.

import { useEffect, useState } from "react";
import { History, X } from "lucide-react";
import { call } from "./api";

interface Version { id: string; at: number; reason: string; size: number }

function stamp(ms: number): string {
  const d = new Date(ms);
  const today = new Date();
  const same = d.toDateString() === today.toDateString();
  const yest = new Date(today.getTime() - 86_400_000).toDateString() === d.toDateString();
  const time = d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  if (same) return `Today, ${time}`;
  if (yest) return `Yesterday, ${time}`;
  return `${d.toLocaleDateString(undefined, { month: "short", day: "numeric", year: d.getFullYear() === today.getFullYear() ? undefined : "numeric" })}, ${time}`;
}

export function VersionsDialog({ onRestore, onClose }: { onRestore: (id: string) => void; onClose: () => void }) {
  const [list, setList] = useState<Version[] | null>(null);
  useEffect(() => { call<Version[]>("versions").then(setList).catch(() => setList([])); }, []);
  return (
    <div className="palette-backdrop" onMouseDown={onClose} onKeyDown={(e) => { if (e.key === "Escape") onClose(); }}>
      <div className="prompt versions" role="dialog" aria-label="Version history" onMouseDown={(e) => e.stopPropagation()}>
        <div className="versions-head">
          <History size={17} />
          <h2>Version history</h2>
          <button type="button" className="find-btn" aria-label="Close" onClick={onClose} autoFocus><X size={15} /></button>
        </div>
        <p className="versions-note">Quire keeps a copy each time you save and every few minutes while you edit. Restoring replaces what's open now; you can undo it.</p>
        {list === null ? <p className="pane-empty">Loading…</p> : list.length === 0 ? (
          <p className="pane-empty">No versions yet. They appear after you save or edit for a few minutes.</p>
        ) : (
          <ul className="versions-list">
            {list.map((v, i) => (
              <li key={v.id}>
                <div><b>{stamp(v.at)}</b><span>{v.reason || "Snapshot"}{i === 0 ? " · latest" : ""}</span></div>
                <button type="button" className="pane-btn" onClick={() => { onRestore(v.id); onClose(); }}>Restore</button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
