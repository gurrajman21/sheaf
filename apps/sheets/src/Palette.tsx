// ⌘K command palette: every command, sheets, functions, recent files, go-to cell.

import { useEffect, useMemo, useRef, useState } from "react";
import { Search } from "lucide-react";

export interface PaletteItem { id: string; label: string; group: string; hint?: string; shortcut?: string; run: () => void }

/** Subsequence match with a bonus for word starts and contiguous runs. */
function score(q: string, text: string): number {
  if (!q) return 1;
  const t = text.toLowerCase();
  let ti = 0, s = 0, run = 0;
  for (const ch of q.toLowerCase()) {
    const i = t.indexOf(ch, ti);
    if (i < 0) return 0;
    run = i === ti ? run + 1 : 0;
    s += 1 + run * 2 + (i === 0 || t[i - 1] === " " ? 3 : 0);
    ti = i + 1;
  }
  return s - t.length * 0.01;
}

export function Palette({ items, onClose, extra }: { items: PaletteItem[]; onClose: () => void; extra: (q: string) => PaletteItem[] }) {
  const [q, setQ] = useState("");
  const [index, setIndex] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const results = useMemo(() => {
    const dynamic = extra(q);
    const ranked = items
      .map((it) => ({ it, s: Math.max(score(q, it.label), score(q, `${it.group} ${it.label}`) - 2) }))
      .filter((x) => x.s > 0)
      .sort((a, b) => b.s - a.s)
      .slice(0, 40)
      .map((x) => x.it);
    return [...dynamic, ...ranked];
  }, [q, items, extra]);

  useEffect(() => { setIndex(0); }, [q]);
  useEffect(() => {
    listRef.current?.querySelector(`[data-i="${index}"]`)?.scrollIntoView({ block: "nearest" });
  }, [index]);

  const run = (it?: PaletteItem) => { if (!it) return; onClose(); it.run(); };

  return (
    <div className="palette-backdrop" onMouseDown={onClose}>
      <div className="palette" role="dialog" aria-label="Command palette" onMouseDown={(e) => e.stopPropagation()}>
        <div className="palette-input-row">
          <Search size={17} aria-hidden />
          <input
            autoFocus
            className="palette-input"
            placeholder="Search commands, functions, sheets… or type a cell like B12"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") { e.preventDefault(); setIndex((i) => Math.min(i + 1, results.length - 1)); }
              else if (e.key === "ArrowUp") { e.preventDefault(); setIndex((i) => Math.max(i - 1, 0)); }
              else if (e.key === "Enter") { e.preventDefault(); run(results[index]); }
              else if (e.key === "Escape") { e.preventDefault(); onClose(); }
            }}
          />
          <kbd className="palette-esc">esc</kbd>
        </div>
        <div className="palette-list" ref={listRef} role="listbox">
          {results.map((it, i) => (
            <button key={it.id} type="button" role="option" aria-selected={i === index} data-i={i}
              className={`palette-item${i === index ? " is-active" : ""}`}
              onMouseMove={() => setIndex(i)} onClick={() => run(it)}>
              <span className="palette-group">{it.group}</span>
              <span className="palette-label">{it.label}</span>
              {it.hint && <span className="palette-hint">{it.hint}</span>}
              {it.shortcut && <kbd>{it.shortcut}</kbd>}
            </button>
          ))}
          {!results.length && <div className="palette-empty">Nothing matches “{q}”.</div>}
        </div>
      </div>
    </div>
  );
}
