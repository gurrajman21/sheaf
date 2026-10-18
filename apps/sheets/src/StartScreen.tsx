// Start screen: new workbook, templates and recent files.

import { FileSpreadsheet, FolderOpen, X } from "lucide-react";
import { AppMark } from "./icons";

export interface Recent { path: string; name: string; at: number }

const KEY = "sheaf.recent";

export function loadRecents(): Recent[] {
  try { return JSON.parse(localStorage.getItem(KEY) ?? "[]"); } catch { return []; }
}

export function addRecent(path: string, name: string) {
  try {
    const list = loadRecents().filter((r) => r.path !== path);
    list.unshift({ path, name, at: Date.now() });
    localStorage.setItem(KEY, JSON.stringify(list.slice(0, 12)));
  } catch { /* storage unavailable: recents are a convenience */ }
}

export function removeRecent(path: string) {
  try { localStorage.setItem(KEY, JSON.stringify(loadRecents().filter((r) => r.path !== path))); } catch { /* ignore */ }
}

const TEMPLATES = [
  { id: "blank", name: "Blank workbook", hint: "Start from an empty sheet" },
  { id: "budget", name: "Monthly budget", hint: "Planned vs. actual spending" },
  { id: "invoice", name: "Invoice", hint: "Line items, tax and total" },
  { id: "sales", name: "Sales tracker", hint: "Units, price and revenue by month" },
  { id: "todo", name: "To-do list", hint: "Tasks, owners and due dates" },
];

function Thumb({ id }: { id: string }) {
  // a tiny sketch of each template's layout
  const rows = id === "blank" ? 0 : id === "invoice" ? 4 : 6;
  return (
    <div className={`thumb thumb-${id}`} aria-hidden>
      {id !== "blank" && <div className="thumb-title" />}
      {id !== "blank" && <div className="thumb-head" />}
      {Array.from({ length: rows }, (_, i) => <div key={i} className="thumb-row" style={{ width: `${70 + ((i * 13) % 30)}%` }} />)}
      {id === "blank" && <div className="thumb-plus">+</div>}
    </div>
  );
}

export function StartScreen(p: {
  recents: Recent[];
  onTemplate: (id: string) => void;
  onOpen: () => void;
  onOpenRecent: (r: Recent) => void;
  onForget: (r: Recent) => void;
  onClose: () => void;
}) {
  const when = (t: number) => {
    const d = Date.now() - t;
    if (d < 60_000) return "just now";
    if (d < 3_600_000) return `${Math.round(d / 60_000)} min ago`;
    if (d < 86_400_000) return `${Math.round(d / 3_600_000)} h ago`;
    return new Date(t).toLocaleDateString(undefined, { month: "short", day: "numeric" });
  };
  return (
    <div className="start" role="dialog" aria-label="Start" onKeyDown={(e) => { if (e.key === "Escape") p.onClose(); }}>
      <div className="start-inner">
        <header className="start-head">
          <AppMark size={30} />
          <div>
            <h1>Sheaf</h1>
            <p>Edit Excel workbooks without breaking what's in them.</p>
          </div>
        </header>
        <section aria-label="New">
          <h2>New</h2>
          <div className="templates">
            {TEMPLATES.map((t) => (
              <button key={t.id} type="button" className="template" onClick={() => p.onTemplate(t.id)} autoFocus={t.id === "blank"}>
                <Thumb id={t.id} />
                <span className="template-name">{t.name}</span>
                <span className="template-hint">{t.hint}</span>
              </button>
            ))}
          </div>
        </section>
        <section aria-label="Recent">
          <div className="recent-head">
            <h2>Recent</h2>
            <button type="button" className="start-open" onClick={p.onOpen}><FolderOpen size={15} /> Open…</button>
          </div>
          {p.recents.length === 0 ? (
            <p className="recent-empty">Workbooks you open or save appear here. You can also drop an .xlsx file onto this window.</p>
          ) : (
            <ul className="recents">
              {p.recents.map((r) => (
                <li key={r.path}>
                  <button type="button" className="recent" onClick={() => p.onOpenRecent(r)} title={r.path}>
                    <FileSpreadsheet size={18} className="recent-icon" />
                    <span className="recent-name">{r.name}</span>
                    <span className="recent-path">{r.path.replace(/^\/Users\/[^/]+/, "~").replace(/\/[^/]+$/, "")}</span>
                    <span className="recent-when">{when(r.at)}</span>
                  </button>
                  <button type="button" className="recent-forget" aria-label={`Remove ${r.name} from recent`} title="Remove from list" onClick={() => p.onForget(r)}><X size={13} /></button>
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>
    </div>
  );
}
