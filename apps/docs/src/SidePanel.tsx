// Review side panel: comment threads (add, reply, resolve, edit, delete) and tracked
// changes (accept or reject one at a time, or all).

import { useEffect, useMemo, useRef, useState } from "react";
import { Check, CheckCheck, MessageSquarePlus, MoreHorizontal, RotateCcw, Trash2, X } from "lucide-react";
import { before, type Pos, type Revision } from "./api";

export interface Comment {
  id: number; author: string; initials: string; date: string; text: string;
  parent: number | null; done: boolean; from: Pos | null; to: Pos | null;
}

export type PanelTab = "comments" | "changes";

const AVATAR = ["#2F5BD3", "#0E7C6B", "#C4320A", "#7A5AF8", "#B42318", "#B54708", "#027A48", "#C11574"];
const colorFor = (name: string) => AVATAR[[...name].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7) % AVATAR.length];

function when(iso: string): string {
  const t = Date.parse(iso);
  if (!t) return "";
  const d = Date.now() - t;
  if (d < 60_000) return "just now";
  if (d < 3_600_000) return `${Math.round(d / 60_000)} min ago`;
  if (d < 86_400_000) return `${Math.round(d / 3_600_000)} h ago`;
  return new Date(t).toLocaleDateString(undefined, { month: "short", day: "numeric", year: new Date(t).getFullYear() === new Date().getFullYear() ? undefined : "numeric" });
}

function Avatar({ name, initials }: { name: string; initials: string }) {
  return <span className="avatar" style={{ background: colorFor(name) }} aria-hidden>{(initials || name.slice(0, 1)).slice(0, 2).toUpperCase()}</span>;
}

function Editor(p: { initial?: string; placeholder: string; submitLabel: string; onSubmit: (t: string) => void; onCancel: () => void; autoFocus?: boolean }) {
  const [text, setText] = useState(p.initial ?? "");
  const ref = useRef<HTMLTextAreaElement>(null);
  useEffect(() => { if (p.autoFocus) ref.current?.focus(); }, [p.autoFocus]);
  const submit = () => { if (text.trim()) p.onSubmit(text.trim()); };
  return (
    <div className="comment-editor">
      <textarea ref={ref} value={text} placeholder={p.placeholder} rows={2} onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) { e.preventDefault(); submit(); }
          if (e.key === "Escape") { e.preventDefault(); p.onCancel(); }
        }} />
      <div className="comment-editor-actions">
        <button type="button" className="pane-btn" onClick={p.onCancel}>Cancel</button>
        <button type="button" className="pane-btn primary" disabled={!text.trim()} onClick={submit}>{p.submitLabel}</button>
      </div>
    </div>
  );
}

export function SidePanel(p: {
  tab: PanelTab;
  setTab: (t: PanelTab) => void;
  comments: Comment[];
  revisions: Revision[];
  track: boolean;
  draft: boolean;
  active: number | null;
  onSelectComment: (c: Comment) => void;
  onAdd: (text: string) => void;
  onCancelDraft: () => void;
  onReply: (parent: number, text: string) => void;
  onEdit: (id: number, text: string) => void;
  onResolve: (id: number, done: boolean) => void;
  onDelete: (id: number) => void;
  onNewComment: () => void;
  onRevision: (r: Revision, action: "accept" | "reject" | "show") => void;
  onAll: (accept: boolean) => void;
  onClose: () => void;
}) {
  const [showResolved, setShowResolved] = useState(false);
  const [replying, setReplying] = useState<number | null>(null);
  const [editing, setEditing] = useState<number | null>(null);
  const threads = useMemo(() => {
    const roots = p.comments.filter((c) => c.parent === null || !p.comments.some((x) => x.id === c.parent));
    roots.sort((a, b) => (a.from && b.from ? (before(a.from, b.from) ? -1 : before(b.from, a.from) ? 1 : a.id - b.id) : a.id - b.id));
    return roots.map((r) => ({ root: r, replies: p.comments.filter((c) => c.parent === r.id).sort((a, b) => a.id - b.id) }));
  }, [p.comments]);
  const open = threads.filter((t) => !t.root.done);
  const resolved = threads.filter((t) => t.root.done);

  const Card = ({ c, reply }: { c: Comment; reply?: boolean }) => (
    <div className={`comment${reply ? " is-reply" : ""}`}>
      <div className="comment-head">
        <Avatar name={c.author} initials={c.initials} />
        <div className="comment-who"><b>{c.author || "Unknown"}</b><span>{when(c.date)}</span></div>
        {!reply && (
          <button type="button" className="icon-btn" title={c.done ? "Reopen" : "Resolve"} aria-label={c.done ? "Reopen thread" : "Resolve thread"}
            onClick={(e) => { e.stopPropagation(); p.onResolve(c.id, !c.done); }}>
            {c.done ? <RotateCcw size={14} /> : <Check size={15} />}
          </button>
        )}
        <details className="comment-more" onClick={(e) => e.stopPropagation()}>
          <summary className="icon-btn" aria-label="More actions"><MoreHorizontal size={15} /></summary>
          <div className="comment-menu">
            <button type="button" onClick={() => setEditing(c.id)}>Edit</button>
            <button type="button" className="is-danger" onClick={() => p.onDelete(c.id)}><Trash2 size={13} /> Delete{reply ? "" : " thread"}</button>
          </div>
        </details>
      </div>
      {editing === c.id ? (
        <Editor initial={c.text} placeholder="Edit comment" submitLabel="Save" autoFocus
          onSubmit={(t) => { p.onEdit(c.id, t); setEditing(null); }} onCancel={() => setEditing(null)} />
      ) : (
        <div className="comment-text">{c.text}</div>
      )}
    </div>
  );

  const Thread = ({ t }: { t: (typeof threads)[number] }) => (
    <li className={`thread${p.active === t.root.id ? " is-active" : ""}${t.root.done ? " is-done" : ""}`} onClick={() => p.onSelectComment(t.root)}>
      <Card c={t.root} />
      {t.replies.map((r) => <Card key={r.id} c={r} reply />)}
      {replying === t.root.id ? (
        <div onClick={(e) => e.stopPropagation()}>
          <Editor placeholder="Reply" submitLabel="Reply" autoFocus onSubmit={(text) => { p.onReply(t.root.id, text); setReplying(null); }} onCancel={() => setReplying(null)} />
        </div>
      ) : !t.root.done && (
        <button type="button" className="reply-btn" onClick={(e) => { e.stopPropagation(); setReplying(t.root.id); }}>Reply</button>
      )}
    </li>
  );

  return (
    <aside className="changes-pane" aria-label="Review">
      <div className="pane-head">
        <div className="pane-tabs" role="tablist">
          <button type="button" role="tab" aria-selected={p.tab === "comments"} className={`pane-tab${p.tab === "comments" ? " is-active" : ""}`} onClick={() => p.setTab("comments")}>
            Comments <span className="pane-count">{open.length}</span>
          </button>
          <button type="button" role="tab" aria-selected={p.tab === "changes"} className={`pane-tab${p.tab === "changes" ? " is-active" : ""}`} onClick={() => p.setTab("changes")}>
            Changes <span className="pane-count">{p.revisions.length}</span>
          </button>
        </div>
        <button type="button" className="find-btn" aria-label="Close review panel" onClick={p.onClose}><X size={15} /></button>
      </div>
      {p.tab === "comments" ? (
        <>
          <div className="pane-actions">
            <button type="button" className="pane-btn primary" onClick={p.onNewComment}><MessageSquarePlus size={14} /> New comment</button>
          </div>
          <ul className="rev-list">
            {p.draft && (
              <li className="thread is-active">
                <Editor placeholder="Add a comment" submitLabel="Comment" autoFocus onSubmit={p.onAdd} onCancel={p.onCancelDraft} />
              </li>
            )}
            {open.map((t) => <Thread key={t.root.id} t={t} />)}
            {!open.length && !p.draft && <p className="pane-empty">No open comments. Select text and choose New comment (⌥⌘M).</p>}
            {resolved.length > 0 && (
              <li className="resolved-toggle">
                <button type="button" onClick={() => setShowResolved((x) => !x)}>{showResolved ? "Hide" : "Show"} {resolved.length} resolved</button>
              </li>
            )}
            {showResolved && resolved.map((t) => <Thread key={t.root.id} t={t} />)}
          </ul>
        </>
      ) : (
        <>
          <div className="pane-actions">
            <button type="button" className="pane-btn primary" disabled={!p.revisions.length} onClick={() => p.onAll(true)}><CheckCheck size={14} /> Accept all</button>
            <button type="button" className="pane-btn" disabled={!p.revisions.length} onClick={() => p.onAll(false)}>Reject all</button>
          </div>
          {p.revisions.length === 0 ? (
            <p className="pane-empty">{p.track ? "Edits you make are tracked and will be listed here." : "No tracked changes. Turn on Track changes to record edits for review."}</p>
          ) : (
            <ul className="rev-list">
              {p.revisions.map((r, i) => (
                <li key={`${r.part}-${r.id}-${i}`} className={`rev rev-${r.kind}`} onClick={() => p.onRevision(r, "show")}>
                  <div className="rev-meta">
                    <Avatar name={r.author} initials="" />
                    <b>{r.author}</b><span>{revLabel(r.kind)}</span>
                    <span className="rev-actions">
                      <button type="button" className="icon-btn ok" title="Accept" aria-label="Accept change" onClick={(e) => { e.stopPropagation(); p.onRevision(r, "accept"); }}><Check size={15} /></button>
                      <button type="button" className="icon-btn no" title="Reject" aria-label="Reject change" onClick={(e) => { e.stopPropagation(); p.onRevision(r, "reject"); }}><X size={15} /></button>
                    </span>
                  </div>
                  {r.text && <div className="rev-text">{r.text}</div>}
                  {r.part !== "word/document.xml" && <div className="rev-where">{r.part.replace(/^word\//, "").replace(/\.xml$/, "").replace(/\d+$/, "")}</div>}
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </aside>
  );
}

export function revLabel(kind: string): string {
  return ({
    ins: "inserted", del: "deleted", moveFrom: "moved from", moveTo: "moved to", rPrChange: "formatted", pPrChange: "changed paragraph",
    paragraphMark: "paragraph mark", rowIns: "inserted row", rowDel: "deleted row", sectPrChange: "changed section",
  } as Record<string, string>)[kind] ?? kind;
}
