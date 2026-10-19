// Start screen: new document, templates and recent files.

import { FileText, FolderOpen, X } from "lucide-react";
import { AppMark } from "./icons";

export interface Recent { path: string; name: string; at: number }

const KEY = "quire.recent";

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

/** Template contents: (paragraph style, text) pairs. */
export const TEMPLATES: { id: string; name: string; hint: string; paragraphs: [string, string][] }[] = [
  { id: "blank", name: "Blank document", hint: "Start from an empty page", paragraphs: [] },
  { id: "letter", name: "Letter", hint: "Address, date and a short letter", paragraphs: [
    ["", "Your Name"], ["", "123 Main Street, Springfield"], ["", ""], ["", "October 18, 2026"], ["", ""],
    ["", "Dear Ms. Rivera,"], ["", "Thank you for meeting with me last week. I'm writing to follow up on the points we discussed and to confirm the next steps."],
    ["", "Please let me know if anything here doesn't match your notes."], ["", "Kind regards,"], ["", ""], ["", "Your Name"],
  ] },
  { id: "report", name: "Report", hint: "Title, summary and sections", paragraphs: [
    ["Title", "Quarterly Report"], ["Subtitle", "Third quarter, 2026"], ["Heading1", "Summary"],
    ["", "Revenue grew while costs held steady. This report covers results, risks and the plan for next quarter."],
    ["Heading1", "Results"], ["", "Describe what happened, with the numbers that matter most."],
    ["Heading1", "Next steps"], ["", "List the decisions needed and who owns each one."],
  ] },
  { id: "notes", name: "Meeting notes", hint: "Attendees, agenda and actions", paragraphs: [
    ["Title", "Meeting Notes"], ["", "Date: October 18, 2026"], ["", "Attendees: "],
    ["Heading2", "Agenda"], ["", "1. "], ["Heading2", "Decisions"], ["", ""], ["Heading2", "Action items"], ["", ""],
  ] },
  { id: "resume", name: "Résumé", hint: "Experience, education and skills", paragraphs: [
    ["Title", "Your Name"], ["", "you@example.com · (555) 010-0100 · City, State"],
    ["Heading1", "Experience"], ["Heading3", "Job Title, Company"], ["", "2023 to present. What you did and what changed because of it."],
    ["Heading1", "Education"], ["", "Degree, School, Year"], ["Heading1", "Skills"], ["", "Skill, skill, skill"],
  ] },
];

function Thumb({ id }: { id: string }) {
  if (id === "blank") return <div className="thumb thumb-blank" aria-hidden><div className="thumb-plus">+</div></div>;
  const head = id === "letter" ? 0 : 1;
  return (
    <div className={`thumb thumb-${id}`} aria-hidden>
      {head ? <div className="thumb-title" /> : null}
      {Array.from({ length: 7 }, (_, i) => (
        <div key={i} className={i % 3 === 0 && head ? "thumb-head" : "thumb-row"} style={{ width: `${55 + ((i * 17) % 40)}%` }} />
      ))}
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
            <h1>Quire</h1>
            <p>Write and edit Word documents without breaking what's in them.</p>
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
            <p className="recent-empty">Documents you open or save appear here. You can also drop a .docx file onto this window.</p>
          ) : (
            <ul className="recents">
              {p.recents.map((r) => (
                <li key={r.path}>
                  <button type="button" className="recent" onClick={() => p.onOpenRecent(r)} title={r.path}>
                    <FileText size={18} className="recent-icon" />
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
