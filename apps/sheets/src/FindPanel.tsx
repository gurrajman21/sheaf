// Find & replace across every sheet (⌘F / ⌘H).

import { useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronUp, X } from "lucide-react";
import { call, type FindHit } from "./api";
import { cellName } from "./geometry";

interface Props {
  replace: boolean;
  sheets: string[];
  version: number;
  onClose: () => void;
  onGo: (hit: FindHit) => void;
  onReplaced: (count: number) => void;
}

export function FindPanel(p: Props) {
  const [query, setQuery] = useState("");
  const [replacement, setReplacement] = useState("");
  const [matchCase, setMatchCase] = useState(false);
  const [hits, setHits] = useState<FindHit[]>([]);
  const [index, setIndex] = useState(0);
  const [replaceMode, setReplaceMode] = useState(p.replace);
  const started = useRef(false); // first Enter goes to the current match, later ones move on
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => { setReplaceMode(p.replace); inputRef.current?.select(); }, [p.replace]);

  useEffect(() => {
    const t = window.setTimeout(() => {
      started.current = false;
      if (!query) { setHits([]); return; }
      call<FindHit[]>("find", { query, match_case: matchCase }).then((h) => { setHits(h); setIndex((i) => Math.min(i, Math.max(h.length - 1, 0))); });
    }, 120);
    return () => window.clearTimeout(t);
  }, [query, matchCase, p.version]);

  const go = (i: number) => {
    if (!hits.length) return;
    started.current = true;
    const n = (i + hits.length) % hits.length;
    setIndex(n);
    p.onGo(hits[n]);
  };

  const replaceOne = async () => {
    const h = hits[index];
    if (!h) return;
    const n = await call<number>("replace", { query, with: replacement, match_case: matchCase, cells: [[h.sheet, h.row, h.col]] });
    p.onReplaced(n);
  };

  const replaceAll = async () => {
    if (!hits.length) return;
    const n = await call<number>("replace", { query, with: replacement, match_case: matchCase, cells: hits.map((h) => [h.sheet, h.row, h.col]) });
    p.onReplaced(n);
  };

  const status = !query ? "" : hits.length ? `${index + 1} of ${hits.length}${hits.length >= 5000 ? "+" : ""}` : "No matches";
  const cur = hits[index];

  return (
    <div className="find-panel" role="search" onKeyDown={(e) => { if (e.key === "Escape") { e.preventDefault(); p.onClose(); } }}>
      <div className="find-row">
        <button type="button" className="find-toggle" aria-label={replaceMode ? "Hide replace" : "Show replace"} aria-expanded={replaceMode}
          onClick={() => setReplaceMode((r) => !r)}>
          <ChevronDown size={14} style={{ transform: replaceMode ? "none" : "rotate(-90deg)" }} />
        </button>
        <input ref={inputRef} autoFocus className="find-input" placeholder="Find in workbook" aria-label="Find" value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); go(started.current ? index + (e.shiftKey ? -1 : 1) : index); } }} />
        <span className="find-status" aria-live="polite">{status}</span>
        <button type="button" className="find-btn" aria-label="Previous match" title="Previous (⇧↩)" disabled={!hits.length} onClick={() => go(index - 1)}><ChevronUp size={15} /></button>
        <button type="button" className="find-btn" aria-label="Next match" title="Next (↩)" disabled={!hits.length} onClick={() => go(index + 1)}><ChevronDown size={15} /></button>
        <button type="button" className="find-btn" aria-label="Close find" title="Close (Esc)" onClick={p.onClose}><X size={15} /></button>
      </div>
      {replaceMode && (
        <div className="find-row">
          <span className="find-toggle-space" />
          <input className="find-input" placeholder="Replace with" aria-label="Replace with" value={replacement}
            onChange={(e) => setReplacement(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); replaceOne(); } }} />
          <button type="button" className="find-action" disabled={!cur} onClick={replaceOne}>Replace</button>
          <button type="button" className="find-action" disabled={!hits.length} onClick={replaceAll}>Replace all</button>
        </div>
      )}
      <div className="find-row find-options">
        <label><input type="checkbox" checked={matchCase} onChange={(e) => setMatchCase(e.target.checked)} /> Match case</label>
        {cur && <span className="find-where">{p.sheets[cur.sheet]}!{cellName({ r: cur.row, c: cur.col })}</span>}
      </div>
    </div>
  );
}
