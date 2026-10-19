// Quire: the word processor shell. Owns the document state (info, layout, caret,
// selection), turns keys, ribbon actions and menu items into backend commands,
// and keeps typing responsive by batching keystrokes while a layout is in flight.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronUp, Command, Search, X } from "lucide-react";
import { at, call, isTauri, isTop, keyOf, order, sameContainer, samePos, type DocInfo, type LayoutData, type ParaInfo, type Pos, type Revision, type RunFmt, type TextItem } from "./api";
import { readClipboard, writeClipboard, writeRich } from "./clipboard";
import { htmlToParas } from "./richtext";
import { confirmAction, pickOpenPath, pickSavePath } from "./files";
import { anchoredItems, caretRect, lineEdge, lineNav, PAD, paraLength, readingOrder, type Placed } from "./geometry";
import { AppMark } from "./icons";
import { nativeMenu, onMenuAction, openUrl, printWindow } from "./native";
import { Palette, type PaletteItem } from "./Palette";
import { PageView, type Mark, type PageViewHandle } from "./PageView";
import { Ribbon, STYLES, type Action } from "./Ribbon";
import { SidePanel, type Comment, type PanelTab } from "./SidePanel";
import { NavPane, type Heading } from "./NavPane";
import { Prompt, type PromptSpec } from "./Prompt";
import { SlashMenu, type SlashHandle, type SlashItem } from "./SlashMenu";
import { VersionsDialog } from "./Versions";
import { addRecent, loadRecents, removeRecent, StartScreen, TEMPLATES, type Recent } from "./StartScreen";

type Toast = { text: string; kind?: "error" } | null;

export default function App() {
  const [info, setInfo] = useState<DocInfo | null>(null);
  const [layout, setLayout] = useState<LayoutData | null>(null);
  const [caret, setCaret] = useState<Pos | null>({ p: [0], o: 0 });
  const [anchor, setAnchor] = useState<Pos | null>(null);
  const [ghost, setGhost] = useState("");
  const [pendingFmt, setPendingFmt] = useState<RunFmt | null>(null);
  const [zoom, setZoom] = useState(1);
  const [start, setStart] = useState(false);
  const [palette, setPalette] = useState(false);
  const [find, setFind] = useState<{ q: string; marks: Mark[]; current: number; replace?: boolean; with?: string } | null>(null);
  const [painter, setPainter] = useState<{ fmt: RunFmt; style: string } | null>(null);
  const [spellOn, setSpellOn] = useState(() => { try { return localStorage.getItem("quire.spell") !== "off"; } catch { return true; } });
  const [squiggles, setSquiggles] = useState<(Mark & { word: string })[]>([]);
  const [pane, setPane] = useState(false);
  const [revisions, setRevisions] = useState<Revision[]>([]);
  const [panelTab, setPanelTab] = useState<PanelTab>("comments");
  const [comments, setComments] = useState<Comment[]>([]);
  const [draft, setDraft] = useState<[Pos, Pos] | null>(null);
  const [activeComment, setActiveComment] = useState<number | null>(null);
  const [nav, setNav] = useState(false);
  const [headings, setHeadings] = useState<Heading[]>([]);
  const [prompt, setPrompt] = useState<PromptSpec | null>(null);
  const [versions, setVersions] = useState(false);
  const [recovery, setRecovery] = useState<{ key: string; name: string; path: string | null; at: number }[]>([]);
  const [autosavedAt, setAutosavedAt] = useState<number | null>(null);
  /** Slash menu: where the "/" was typed and the text typed after it. */
  const [slash, setSlash] = useState<{ at: Pos; query: string; x: number; y: number } | null>(null);
  const slashRef = useRef<SlashHandle>(null);
  const slashState = useRef(slash);
  slashState.current = slash;
  const [para, setPara] = useState<ParaInfo | null>(null);
  const [page, setPage] = useState(0);
  const [toast, setToast] = useState<Toast>(null);
  const [recents, setRecents] = useState<Recent[]>(loadRecents);
  const viewRef = useRef<PageViewHandle>(null);
  const goalX = useRef<number | null>(null);

  const say = useCallback((text: string, kind?: "error") => {
    setToast({ text, kind });
    window.setTimeout(() => setToast((t) => (t?.text === text ? null : t)), kind === "error" ? 5000 : 2200);
  }, []);

  // ---- backend calls, one at a time
  const chain = useRef<Promise<unknown>>(Promise.resolve());
  const queue = useCallback(<T,>(fn: () => Promise<T>): Promise<T> => {
    const next = chain.current.then(fn, fn);
    chain.current = next.catch(() => {});
    return next;
  }, []);

  const layoutRef = useRef<LayoutData | null>(null);
  const itemsCache = useRef<{ layout: LayoutData | null; items: Placed[][] }>({ layout: null, items: [] });
  const fullTimer = useRef<number | null>(null);
  /** Info + layout. Edits ask for a quick (windowed) layout; a full one follows when idle. */
  const refresh = useCallback(async (quick = false) => {
    const [i, l] = await Promise.all([call<DocInfo>("info"), call<LayoutData & { stale?: boolean }>("layout", { quick })]);
    layoutRef.current = l;
    setInfo(i);
    setLayout(l);
    if (quick) {
      if (fullTimer.current) window.clearTimeout(fullTimer.current);
      fullTimer.current = window.setTimeout(() => {
        fullTimer.current = null;
        void chain.current.then(async () => {
          const full = await call<LayoutData>("layout", { quick: false });
          layoutRef.current = full;
          setLayout(full);
        }).catch(() => {});
      }, 700);
    }
    return l;
  }, []);

  const run = useCallback(async <T,>(cmd: string, args: object = {}, opts: { quiet?: boolean; full?: boolean } = {}): Promise<T | undefined> => {
    try {
      const out = await queue(async () => {
        const r = await call<T>(cmd, args);
        await refresh(!opts.full && !["open", "new", "template", "save"].includes(cmd));
        return r;
      });
      return out;
    } catch (e) {
      if (!opts.quiet) say(e instanceof Error ? e.message : String(e), "error");
      return undefined;
    }
  }, [queue, refresh, say]);

  // First load; the backend may still be starting (dev bridge), so retry for a while.
  useEffect(() => {
    let tries = 0, alive = true;
    const attempt = () => refresh().catch((e) => {
      if (!alive) return;
      if (++tries < 20) window.setTimeout(attempt, 500);
      else say(`Couldn't reach the document backend: ${e instanceof Error ? e.message : e}`, "error");
    });
    attempt();
    return () => { alive = false; };
  }, [refresh, say]);

  /** Run an edit whose result is the new caret. `args` sees the caret as it is when the edit runs. */
  const edit = useCallback(async (cmd: string, args: (caret: Pos, anchor: Pos | null) => object | null): Promise<Pos | null> => {
    try {
      return await queue(async () => {
        const c = caretRef.current;
        if (!c) return null;
        const a = args(c, anchorRef.current);
        if (!a) return null;
        const p = await call<Pos>(cmd, a);
        caretRef.current = p;
        anchorRef.current = null;
        setCaret(p);
        setAnchor(null);
        await refresh(true);
        return p;
      });
    } catch (e) {
      say(e instanceof Error ? e.message : String(e), "error");
      return null;
    }
  }, [queue, refresh, say]);

  // ---- typing: keystrokes queue up while the previous insert is being laid out
  const pending = useRef("");
  const sending = useRef(false);
  const caretRef = useRef(caret);
  caretRef.current = caret;
  const anchorRef = useRef(anchor);
  anchorRef.current = anchor;
  const fmtRef = useRef(pendingFmt);
  fmtRef.current = pendingFmt;

  const flushText = useCallback(async (): Promise<void> => {
    if (sending.current || !pending.current) return;
    sending.current = true;
    try {
      while (pending.current) {
        const text = pending.current;
        pending.current = "";
        const fmt = fmtRef.current;
        await queue(async () => {
          const at = caretRef.current;
          if (!at) return;
          const c = await call<Pos>("insert", { at, text, fmt });
          caretRef.current = c;
          setCaret(c);
          await refresh(true);
        });
        setGhost(pending.current);
      }
    } catch (e) {
      say(e instanceof Error ? e.message : String(e), "error");
      pending.current = "";
      setGhost("");
    } finally {
      sending.current = false;
    }
  }, [queue, refresh, say]);

  /** Wait until every queued keystroke is in the document. */
  const settle = useCallback(async () => {
    while (pending.current || sending.current) {
      await flushText();
      await chain.current;
      if (sending.current) await new Promise((r) => setTimeout(r, 10));
    }
  }, [flushText]);

  const deleteSelection = useCallback(async (): Promise<Pos | null> => {
    const a = anchorRef.current, c = caretRef.current;
    if (!a || !c || samePos(a, c)) return null;
    return edit("delete", (cur, anc) => {
      if (!anc || samePos(anc, cur)) return null;
      const [from, to] = order(anc, cur);
      return { from, to };
    });
  }, [edit]);

  const typeText = useCallback(async (text: string) => {
    // "/" at the start of an empty paragraph opens the slash menu; later keys filter it.
    const c0 = caretRef.current;
    const sl = slashState.current;
    if (sl) {
      if (/\s/.test(text)) setSlash(null);
      else setSlash({ ...sl, query: sl.query + text });
    } else if (text === "/" && c0 && (!anchorRef.current || samePos(anchorRef.current, c0)) && c0.o === 0 && paraLength(items(), c0) === 0) {
      requestAnimationFrame(() => {
        const box = viewRef.current?.caretBox();
        if (box) setSlash({ at: c0, query: "", x: box.left, y: box.bottom + 6 });
      });
    }
    if (anchorRef.current && caretRef.current && !samePos(anchorRef.current, caretRef.current)) {
      await settle();
      await deleteSelection();
    }
    anchorRef.current = null;
    setAnchor(null);
    pending.current += text;
    setGhost((g) => g + text);
    goalX.current = null;
    void flushText();
  }, [deleteSelection, flushText, settle]);

  // ---- geometry helpers
  /** Anchored text of the newest layout (key handlers can run before React re-renders). */
  const items = (): Placed[][] => {
    const l = layoutRef.current;
    if (itemsCache.current.layout !== l) itemsCache.current = { layout: l, items: l ? anchoredItems(l) : [] };
    return itemsCache.current.items;
  };
  const caretItem = useMemo((): TextItem | null => {
    if (!layout || !caret) return null;
    return caretRect(anchoredItems(layout), caret)?.it ?? null;
  }, [layout, caret]);

  // Paragraph properties for the ribbon.
  useEffect(() => {
    if (!caret || !layout) return;
    let alive = true;
    call<ParaInfo>("para_info", { at: caret }).then((p) => { if (alive) setPara(p); }).catch(() => {});
    return () => { alive = false; };
  }, [caret ? keyOf(caret) : "", layout]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (nav) call<Heading[]>("outline").then(setHeadings).catch(() => {});
  }, [nav, layout]);

  // Autosave a recovery copy every 20 s while there are unsaved changes.
  const infoNow = useRef(info);
  infoNow.current = info;
  useEffect(() => {
    const t = window.setInterval(() => {
      if (!infoNow.current?.dirty) return;
      void queue(() => call<{ saved: boolean; at?: number }>("autosave")).then((r) => { if (r?.saved && r.at) setAutosavedAt(r.at); }).catch(() => {});
    }, 20_000);
    return () => window.clearInterval(t);
  }, [queue]);
  // Unsaved work left behind by a crash or force-quit.
  useEffect(() => {
    call<{ key: string; name: string; path: string | null; at: number }[]>("recovery_list").then((r) => setRecovery(r)).catch(() => {});
  }, []);

  // Spelling: custom words live in this browser profile; misspellings refresh after edits settle.
  const userWords = useCallback((): string[] => { try { return JSON.parse(localStorage.getItem("quire.words") ?? "[]"); } catch { return []; } }, []);
  useEffect(() => { void call("spell_words", { words: userWords() }).catch(() => {}); }, [userWords]);
  useEffect(() => {
    try { localStorage.setItem("quire.spell", spellOn ? "on" : "off"); } catch { /* ignore */ }
    if (!spellOn) { setSquiggles([]); return; }
    const t = window.setTimeout(() => { call<(Mark & { word: string })[]>("spell").then(setSquiggles).catch(() => {}); }, 700);
    return () => window.clearTimeout(t);
  }, [layout, spellOn]);

  // Comments are always loaded (their highlights show on the page); changes only with the panel.
  useEffect(() => {
    call<Comment[]>("comments").then(setComments).catch(() => {});
    if (pane) call<Revision[]>("revisions").then(setRevisions).catch(() => {});
  }, [pane, layout]);

  useEffect(() => { requestAnimationFrame(() => viewRef.current?.scrollToCaret()); }, [caret, layout]);

  // Window title and recents.
  useEffect(() => {
    if (!info) return;
    const title = `${info.name.replace(/\.docx$/i, "")}${info.dirty ? " — Edited" : ""}`;
    document.title = title;
    if (isTauri) import("@tauri-apps/api/window").then(({ getCurrentWindow }) => getCurrentWindow().setTitle(title)).catch(() => {});
  }, [info]);

  // ---- caret movement
  const moveTo = useCallback((p: Pos, extend: boolean) => {
    setPendingFmt(null); // formatting toggled at the caret applies only to text typed right there
    // Refs first: the next queued key may run before React re-renders.
    const a = extend ? (anchorRef.current ?? caretRef.current) : null;
    // A selection stays inside one cell, header or note (top-level body may span tables).
    if (a && !sameContainer(a, p) && !(isTop(a) && isTop(p))) return;
    anchorRef.current = a;
    caretRef.current = p;
    setAnchor(a);
    setCaret(p);
  }, []);

  const stepChar = useCallback((p: Pos, dir: 1 | -1): Pos => {
    const its = items();
    const paras = readingOrder(its);
    const len = paraLength(its, p);
    if (dir < 0 && p.o > 0) return { ...p, o: p.o - 1 };
    if (dir > 0 && p.o < len) return { ...p, o: p.o + 1 };
    const i = paras.findIndex((q) => keyOf(q) === keyOf(p));
    const nb = paras[i + dir];
    if (nb === undefined) return p;
    return at(nb, dir < 0 ? paraLength(its, nb) : 0);
  }, []);

  /** The word around `p`, from the paragraph's real text. */
  const wordAt = useCallback(async (p: Pos): Promise<[Pos, Pos]> => {
    const text = await call<string>("text", { from: at(p, 0), to: at(p, 1 << 30) }).catch(() => "");
    const chars = [...text];
    const isWord = (ch: string | undefined) => ch !== undefined && /[\p{L}\p{N}_'’-]/u.test(ch);
    let s = Math.min(p.o, chars.length), e = s;
    while (s > 0 && isWord(chars[s - 1])) s--;
    while (e < chars.length && isWord(chars[e])) e++;
    if (s === e && e < chars.length) e++;
    return [at(p, s), at(p, e)];
  }, []);

  // ---- commands
  const guardUnsaved = useCallback(async () => {
    if (!info?.dirty) return true;
    const ok = await confirmAction(`Your changes to ${info.name} haven't been saved.`, "Discard changes?", "Discard changes");
    if (ok) await call("discard").catch(() => {});
    return ok;
  }, [info]);

  const resetCaret = useCallback(() => {
    caretRef.current = { p: [0], o: 0 };
    anchorRef.current = null;
    setCaret(caretRef.current);
    setAnchor(null);
    setFind(null);
  }, []);

  const openPath = useCallback(async (path: string) => {
    await settle();
    const i = await run<DocInfo>("open", { path });
    if (i) { addRecent(path, i.name); setRecents(loadRecents()); resetCaret(); setStart(false); }
  }, [run, settle, resetCaret]);

  const openFile = useCallback(async () => {
    if (!(await guardUnsaved())) return;
    const p = await pickOpenPath();
    if (p) await openPath(p);
  }, [guardUnsaved, openPath]);

  const save = useCallback(async (as: boolean) => {
    await settle();
    let path: string | undefined;
    if (as || !info?.path) {
      const p = await pickSavePath(info?.name ?? "Untitled.docx");
      if (!p) return;
      path = p;
    }
    const i = await run<DocInfo>("save", path ? { path } : {});
    if (i?.path) { addRecent(i.path, i.name); setRecents(loadRecents()); say(`Saved ${i.name}`); }
  }, [info, run, say, settle]);

  const newDoc = useCallback(async (template?: string) => {
    if (!(await guardUnsaved())) return;
    await settle();
    const t = TEMPLATES.find((x) => x.id === template);
    if (t && t.paragraphs.length) await run("template", { name: `${t.name}.docx`, paragraphs: t.paragraphs });
    else await run("new");
    resetCaret();
    setStart(false);
  }, [guardUnsaved, run, settle, resetCaret]);

  const selection = (): [Pos, Pos] | null => {
    const a = anchorRef.current, c = caretRef.current;
    if (!c) return null;
    return a ? order(a, c) : [c, c];
  };

  const format = useCallback(async (fmt: RunFmt) => {
    await settle();
    const sel = selection();
    if (!sel) return;
    if (samePos(sel[0], sel[1])) {
      // Collapsed caret: remember it for the next text typed here.
      setPendingFmt((f) => ({ ...(f ?? {}), ...fmt }));
      return;
    }
    await run("format", { from: sel[0], to: sel[1], fmt });
  }, [run, settle]);

  const paraFormat = useCallback(async (fmt: object) => {
    await settle();
    const sel = selection();
    if (!sel) return;
    if (!sameContainer(sel[0], sel[1]) && !(isTop(sel[0]) && isTop(sel[1]))) return;
    await run("para_format", { from: sel[0], to: sel[1], fmt });
  }, [run, settle]);

  const copy = useCallback(async (cut: boolean) => {
    await settle();
    const sel = selection();
    if (!sel || samePos(sel[0], sel[1])) return;
    try {
      const text = await call<string>("text", { from: sel[0], to: sel[1] });
      const html = await call<string>("html", { from: sel[0], to: sel[1] }).catch(() => "");
      if (html) await writeRich(text, html);
      else await writeClipboard(text);
      if (cut) await deleteSelection();
      else say("Copied");
    } catch (e) {
      say(`Couldn't copy: ${e instanceof Error ? e.message : e}`, "error");
    }
  }, [deleteSelection, say, settle]);

  const paste = useCallback(async (text?: string, html?: string) => {
    const t = text ?? (await readClipboard().catch(() => ""));
    const paras = html ? htmlToParas(html) : [];
    if (!t && !paras.length) return;
    await settle();
    await deleteSelection();
    if (paras.length) await edit("paste_rich", (at) => ({ at, paras }));
    else await edit("insert", (at) => ({ at, text: t.replace(/\r\n?/g, "\n") }));
  }, [deleteSelection, edit, settle]);

  const doFind = useCallback(async (q: string, current = 0) => {
    if (!q) { setFind({ q, marks: [], current: 0 }); return; }
    const marks = await call<Mark[]>("find", { q }).catch(() => [] as Mark[]);
    const cur = marks.length ? ((current % marks.length) + marks.length) % marks.length : 0;
    setFind({ q, marks, current: cur });
    const m = marks[cur];
    if (m) {
      anchorRef.current = at(m, m.o);
      caretRef.current = at(m, m.o + m.len);
      setAnchor(anchorRef.current);
      setCaret(caretRef.current);
    }
  }, []);

  const print = useCallback(async () => {
    await settle();
    const urls = viewRef.current?.snapshot(2) ?? [];
    const root = document.getElementById("print-root")!;
    root.innerHTML = "";
    urls.forEach((u) => { const img = document.createElement("img"); img.src = u; root.appendChild(img); });
    await new Promise((r) => setTimeout(r, 50));
    await printWindow();
  }, [settle]);

  const insertPicture = useCallback(async () => {
    let path: string | null;
    if (isTauri) {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const p = await open({ multiple: false, filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "gif"] }] });
      path = typeof p === "string" ? p : null;
    } else path = window.prompt("Picture file (absolute path)");
    if (!path) return;
    await settle();
    const at = caretRef.current;
    if (at) await run("picture", { at, path });
  }, [run, settle]);

  const fitWidth = useCallback(() => {
    const el = document.querySelector(".page-scroll") as HTMLElement | null;
    const w = layout?.pages[0]?.w;
    if (el && w) setZoom(Math.max(0.4, Math.min(3, (el.clientWidth - 2 * PAD - 16) / w)));
  }, [layout]);

  const act = useCallback(async (a: Action) => {
    const it = caretItem;
    switch (a.t) {
      case "file":
        if (a.a === "new") await newDoc();
        else if (a.a === "open") await openFile();
        else await save(a.a === "save_as");
        break;
      case "start": setStart(true); break;
      case "print": await print(); break;
      case "palette": setPalette(true); break;
      case "find": setFind((f) => f ?? { q: "", marks: [], current: 0 }); break;
      case "undo": await settle(); await run("undo"); anchorRef.current = null; setAnchor(null); break;
      case "redo": await settle(); await run("redo"); anchorRef.current = null; setAnchor(null); break;
      case "cut": await copy(true); break;
      case "copy": await copy(false); break;
      case "paste": await paste(); break;
      case "toggle": {
        const cur = { bold: it?.b, italic: it?.i, underline: it?.u, strike: it?.s }[a.what] ?? false;
        const pf = pendingFmt?.[a.what];
        await format({ [a.what]: !(pf ?? cur) });
        break;
      }
      case "size": await format({ size: a.value }); break;
      case "size_delta": await format({ size: Math.max(1, Math.round((it?.size ?? 11) + a.delta)) }); break;
      case "font": await format({ font: a.value }); break;
      case "color": await format({ color: a.value || null }); break;
      case "highlight": await format({ highlight: a.value || null }); break;
      case "clear_format": await format({ bold: false, italic: false, underline: false, strike: false, size: null, font: null, color: null, highlight: null }); break;
      case "style": await paraFormat({ style: a.id }); break;
      case "align": await paraFormat({ align: a.value }); break;
      case "list": await paraFormat({ list: a.kind }); break;
      case "indent": await paraFormat({ indent: a.delta }); break;
      case "page_break": await settle(); await edit("page_break", (at) => ({ at })); break;
      case "table": await settle(); await edit("table", (at) => ({ at, rows: a.rows, cols: a.cols })); break;
      case "picture": await insertPicture(); break;
      case "track": { await settle(); const i = await run<DocInfo>("track", { on: !info?.track }); if (i) say(i.track ? "Tracking changes" : "Stopped tracking changes"); break; }
      case "accept_all": case "reject_all": {
        await settle();
        const r = await run<{ resolved: number }>(a.t);
        if (r) { say(r.resolved ? `${a.t === "accept_all" ? "Accepted" : "Rejected"} ${r.resolved} change${r.resolved === 1 ? "" : "s"}` : "No tracked changes"); resetCaret(); }
        break;
      }
      case "pane": setPane((x) => !x); if (!pane) setPanelTab("changes"); break;
      case "nav": setNav((x) => !x); break;
      case "versions": setVersions(true); break;
      case "painter": {
        if (painter) { setPainter(null); break; }
        setPainter({
          fmt: { bold: !!it?.b, italic: !!it?.i, underline: !!it?.u, strike: !!it?.s, size: it?.size ?? null, font: it?.font ?? null, color: it?.color?.replace("#", "") ?? null },
          style: para?.style ?? "Normal",
        });
        say("Select text to apply this formatting");
        break;
      }
      case "spacing": { const { t: _t, ...sp } = a; await paraFormat({ spacing: sp }); break; }
      case "spell": setSpellOn((x) => !x); break;
      case "replace": setFind((f) => ({ q: f?.q ?? "", marks: f?.marks ?? [], current: f?.current ?? 0, replace: true, with: f?.with ?? "" })); break;
      case "toc": {
        await settle();
        if (a.update) await run("toc_update", {}, { full: true });
        else await edit("toc_insert", (at) => ({ at }));
        break;
      }
      case "footnote": {
        await settle();
        const c = caretRef.current;
        if (!c || c.s) { say("Footnotes go in the body text.", "error"); break; }
        await edit("footnote_insert", (at) => ({ at, text: "" }));
        break;
      }
      case "page_numbers": { await settle(); await run("page_numbers", { where: a.where, align: a.align, total: a.total }, { full: true }); say("Page numbers added"); break; }
      case "link": {
        await settle();
        const sel = selection();
        if (!sel) break;
        const existing = await call<{ url: string | null; from: Pos; to: Pos } | null>("link_at", { at: sel[0] }).catch(() => null);
        let range: [Pos, Pos] = existing && samePos(sel[0], sel[1]) ? [existing.from, existing.to] : sel;
        if (samePos(range[0], range[1])) range = await wordAt(range[0]);
        if (keyOf(range[0]) !== keyOf(range[1])) { say("Select text within one paragraph to link it.", "error"); break; }
        setPrompt({
          title: existing ? "Edit link" : "Add link", label: "Web address", initial: existing?.url ?? "", placeholder: "example.com", submit: existing ? "Update" : "Add link",
          onSubmit: (url) => void (async () => {
            if (existing) await run("link_remove", { at: existing.from });
            await run("link_insert", { from: range[0], to: range[1], url });
          })(),
          extra: existing ? { label: "Remove link", danger: true, run: () => void run("link_remove", { at: existing.from }) } : undefined,
        });
        break;
      }
      case "comment": {
        await settle();
        const sel = selection();
        if (!sel) break;
        let range: [Pos, Pos] = sel;
        if (samePos(sel[0], sel[1])) range = await wordAt(sel[0]);
        if (!sameContainer(range[0], range[1])) { say("Comment on text within one paragraph, cell or note.", "error"); break; }
        setDraft(range);
        setPanelTab("comments");
        setPane(true);
        break;
      }
      case "zoom": if (a.value < 0) fitWidth(); else setZoom(a.value); break;
      case "table_row": case "table_col": case "table_delete": case "table_merge": case "table_shade": {
        await settle();
        const op = a.t === "table_row" ? `row_${a.where}` : a.t === "table_col" ? `col_${a.where}` : a.t === "table_delete" ? (a.what === "table" ? "delete_table" : `delete_${a.what}`) : a.t === "table_merge" ? "merge" : "shade";
        await edit("table_op", (at) => ({ at, op, color: a.t === "table_shade" ? a.color.replace("#", "") : undefined }));
        break;
      }
      case "page_setup": { await settle(); const { t: _t, ...opts } = a; await run("page_setup", opts, { full: true }); break; }
    }
    viewRef.current?.focus();
  }, [caretItem, copy, fitWidth, format, info, insertPicture, moveTo, newDoc, openFile, paraFormat, paste, pendingFmt, print, resetCaret, run, save, say, settle]);

  const actRef = useRef(act);
  actRef.current = act;

  // ---- keyboard: every key and every typed string is handled strictly in order
  const inputChain = useRef<Promise<void>>(Promise.resolve());
  const serial = useCallback((fn: () => Promise<void> | void) => {
    inputChain.current = inputChain.current.then(fn).catch((e) => say(e instanceof Error ? e.message : String(e), "error"));
  }, [say]);

  const handleKey = useCallback(async (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    const mod = e.metaKey || e.ctrlKey;
    const k = e.key;
    const c = caretRef.current;
    if (!c) return;
    // Shortcuts the desktop menu bar owns arrive as menu events there.
    if (mod && !isTauri) {
      const map: Record<string, Action> = {
        b: { t: "toggle", what: "bold" }, i: { t: "toggle", what: "italic" }, u: { t: "toggle", what: "underline" },
        z: e.shiftKey ? { t: "redo" } : { t: "undo" }, s: { t: "file", a: e.shiftKey ? "save_as" : "save" },
        o: { t: "file", a: "open" }, p: { t: "print" }, k: { t: "palette" }, f: { t: "find" },
        e: { t: "align", value: "center" }, l: { t: "align", value: "left" }, r: { t: "align", value: "right" }, j: { t: "align", value: "both" },
        "=": { t: "zoom", value: Math.min(3, zoom + 0.1) }, "-": { t: "zoom", value: Math.max(0.4, zoom - 0.1) }, "0": { t: "zoom", value: 1 },
      };
      const a = map[k.toLowerCase()];
      if (a) { e.preventDefault(); await act(a); return; }
      if (k === "Enter") { e.preventDefault(); await act({ t: "page_break" }); return; }
      if (k.toLowerCase() === "a") { e.preventDefault(); selectAll(); return; }
    }
    if (mod && isTauri && k === "Enter") { e.preventDefault(); await act({ t: "page_break" }); return; }
    if (mod && e.altKey && e.code === "KeyM") { e.preventDefault(); await act({ t: "comment" }); return; }
    if (mod && e.altKey && e.code === "KeyK") { e.preventDefault(); await act({ t: "link" }); return; }
    if (mod && e.altKey && e.code === "KeyF") { e.preventDefault(); await act({ t: "replace" }); return; }
    const nav = ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(k);
    if (nav) {
      e.preventDefault();
      await settle();
      const its = items();
      const cur = caretRef.current!;
      const a = anchorRef.current;
      let next: Pos | null = cur;
      if (k === "ArrowLeft" || k === "ArrowRight") {
        const dir = k === "ArrowLeft" ? -1 : 1;
        if (a && !e.shiftKey && !samePos(a, cur)) next = order(a, cur)[dir < 0 ? 0 : 1];
        else if (e.altKey || (mod && !isTauri)) {
          next = cur;
          if (e.altKey) {
            // word jump
            const [s, en] = await wordAt(dir < 0 ? stepChar(cur, -1) : cur);
            next = dir < 0 ? (samePos(s, cur) ? stepChar(cur, -1) : s) : (samePos(en, cur) ? stepChar(cur, 1) : en);
          } else next = lineEdge(its, cur, dir > 0) ?? cur;
        } else next = stepChar(cur, dir);
        goalX.current = null;
      } else if (k === "ArrowUp" || k === "ArrowDown") {
        if (mod) {
          const paras = readingOrder(its).filter((q) => !q.s && q.p.length === 1);
          const first = paras[0], last = paras[paras.length - 1];
          next = k === "ArrowUp" ? (first ?? cur) : last ? at(last, paraLength(its, last)) : cur;
        } else {
          const r = caretRect(its, cur);
          if (goalX.current === null && r) goalX.current = r.x;
          next = lineNav(its, cur, k === "ArrowUp" ? -1 : 1, goalX.current ?? 0) ?? cur;
        }
      } else {
        next = lineEdge(its, cur, k === "End") ?? cur;
        goalX.current = null;
      }
      if (next) moveTo(next, e.shiftKey);
      return;
    }
    if (k === "Backspace" || k === "Delete") {
      e.preventDefault();
      await settle();
      const cur = caretRef.current!;
      if (await deleteSelection()) return;
      const other = stepChar(cur, k === "Backspace" ? -1 : 1);
      if (e.altKey) {
        const [s, en] = await wordAt(k === "Backspace" ? other : cur);
        await edit("delete", () => ({ from: k === "Backspace" ? s : cur, to: k === "Backspace" ? cur : en }));
        return;
      }
      if (samePos(other, cur)) return;
      await edit("delete", () => ({ from: cur, to: other }));
      return;
    }
    if (k === "Enter") {
      e.preventDefault();
      await settle();
      await deleteSelection();
      await edit("split", (at) => ({ at }));
      return;
    }
    if (k === "Tab") {
      e.preventDefault();
      const cur = caretRef.current!;
      if (cur.p.length >= 4 && !cur.s?.startsWith("fn:")) {
        // In a table: Tab / Shift+Tab move to the next / previous cell.
        await settle();
        const paras = readingOrder(items());
        const i = paras.findIndex((q) => keyOf(q) === keyOf(cur));
        const cellOf = (q: Pos) => `${q.s ?? ""}|${q.p.slice(0, -1).join(",")}`;
        const step = e.shiftKey ? -1 : 1;
        for (let j = i + step; j >= 0 && j < paras.length; j += step) {
          if (cellOf(paras[j]) !== cellOf(cur) && paras[j].p.length === cur.p.length && paras[j].p[0] === cur.p[0]) {
            const target = paras[j];
            const len = paraLength(items(), target);
            anchorRef.current = at(target, 0);
            caretRef.current = at(target, len);
            setAnchor(anchorRef.current);
            setCaret(caretRef.current);
            return;
          }
        }
        if (!e.shiftKey) await act({ t: "table_row", where: "below" });
        return;
      }
      if (para?.list) await act({ t: "indent", delta: e.shiftKey ? -1 : 1 });
      else await typeText("\t");
      return;
    }
    if (k === "Escape") { anchorRef.current = null; setAnchor(null); setFind(null); }
  }, [act, deleteSelection, edit, moveTo, para, settle, stepChar, typeText, wordAt, zoom]); // eslint-disable-line react-hooks/exhaustive-deps

  const onKeyDown = useCallback((e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    const mod = e.metaKey || e.ctrlKey;
    const k = e.key;
    if (slashState.current && ["ArrowUp", "ArrowDown", "Enter", "Tab", "Escape"].includes(k)) {
      if (slashRef.current?.key(k)) { e.preventDefault(); return; }
    }
    if (slashState.current && k === "Backspace" && !slashState.current.query) setSlash(null);
    else if (slashState.current && k === "Backspace") setSlash({ ...slashState.current, query: slashState.current.query.slice(0, -1) });
    if (slashState.current && (k.startsWith("Arrow") || k === "Home" || k === "End")) setSlash(null);
    const ours = ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "Backspace", "Delete", "Enter", "Tab", "Escape"].includes(k)
      || (mod && !isTauri && "biuzsopkfelrj=-0a".includes(k.toLowerCase()))
      || (mod && e.altKey && (e.code === "KeyM" || e.code === "KeyK" || e.code === "KeyF"));
    if (!ours) return; // plain typing arrives as input events; ⌘C/⌘X/⌘V as clipboard events
    e.preventDefault();
    serial(() => handleKey(e));
  }, [handleKey, serial]);

  const selectAll = useCallback(() => {
    const its = items();
    // Select all: the whole body, or the whole container when in a cell, header or note.
    const cur = caretRef.current;
    const scope = readingOrder(its).filter((q) => (cur && !isTop(cur) ? sameContainer(q, cur) : isTop(q)));
    if (!scope.length) return;
    const last = scope[scope.length - 1];
    const a = scope[0], c = at(last, paraLength(its, last));
    anchorRef.current = a;
    caretRef.current = c;
    setAnchor(a);
    setCaret(c);
  }, []);

  // ---- menu bar (desktop)
  useEffect(() => onMenuAction((id) => {
    const a: Record<string, Action> = {
      "file:new": { t: "file", a: "new" }, "file:start": { t: "start" }, "file:open": { t: "file", a: "open" },
      "file:save": { t: "file", a: "save" }, "file:save_as": { t: "file", a: "save_as" }, "file:print": { t: "print" },
      "file:versions": { t: "versions" },
      "edit:undo": { t: "undo" }, "edit:redo": { t: "redo" }, "edit:find": { t: "find" },
      "view:palette": { t: "palette" }, "view:zoom_reset": { t: "zoom", value: 1 },
      "insert:page_break": { t: "page_break" }, "insert:table": { t: "table", rows: 3, cols: 3 }, "insert:picture": { t: "picture" },
      "format:bold": { t: "toggle", what: "bold" }, "format:italic": { t: "toggle", what: "italic" }, "format:underline": { t: "toggle", what: "underline" },
      "format:bullets": { t: "list", kind: "bullet" }, "format:numbering": { t: "list", kind: "number" },
      "format:left": { t: "align", value: "left" }, "format:center": { t: "align", value: "center" },
      "format:right": { t: "align", value: "right" }, "format:justify": { t: "align", value: "both" },
      "review:track": { t: "track" }, "review:accept_all": { t: "accept_all" }, "review:reject_all": { t: "reject_all" },
      "review:comment": { t: "comment" }, "review:panel": { t: "pane" },
      "insert:link": { t: "link" }, "insert:footnote": { t: "footnote" }, "insert:toc": { t: "toc" }, "view:nav": { t: "nav" },
    };
    if (id === "edit:select_all") { selectAll(); return; }
    if (id === "view:zoom_in") { setZoom((z) => Math.min(3, z + 0.1)); return; }
    if (id === "view:zoom_out") { setZoom((z) => Math.max(0.4, z - 0.1)); return; }
    const action = a[id];
    if (action) void actRef.current(action);
  }), [selectAll]);

  // ---- window: unsaved changes on close, drop a .docx to open it
  const infoRef = useRef(info);
  infoRef.current = info;
  const dropRef = useRef<(f: string) => void>(() => {});
  dropRef.current = (f) => { guardUnsaved().then((ok) => { if (ok) openPath(f); }); };
  useEffect(() => {
    if (!isTauri) return;
    let cancelled = false;
    const unlisten: (() => void)[] = [];
    import("@tauri-apps/api/window").then(async ({ getCurrentWindow }) => {
      const win = getCurrentWindow();
      const subs = [
        await win.onCloseRequested(async (e) => {
          const i = infoRef.current;
          if (!i?.dirty) return;
          if (!(await confirmAction(`Your changes to ${i.name} haven't been saved.`, "Close without saving?", "Close without saving"))) e.preventDefault();
          else await call("discard").catch(() => {});
        }),
        await win.onDragDropEvent((ev) => {
          if (ev.payload.type !== "drop") return;
          const file = ev.payload.paths.find((p) => /\.doc[xm]$/i.test(p));
          if (file) dropRef.current(file);
        }),
      ];
      if (cancelled) subs.forEach((u) => u());
      else unlisten.push(...subs);
    });
    return () => { cancelled = true; unlisten.forEach((u) => u()); };
  }, []);

  // ⌘K palette works everywhere, even with focus outside the document.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k" && !isTauri) { e.preventDefault(); setPalette(true); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const commentRanges = useMemo(() => comments.filter((c) => c.from && c.to && !c.done && c.parent === null)
    .map((c) => ({ from: c.from!, to: c.to!, active: c.id === activeComment })), [comments, activeComment]);

  const paletteItems = useMemo((): PaletteItem[] => {
    const A = (id: string, label: string, group: string, a: Action, shortcut?: string): PaletteItem => ({ id, label, group, shortcut, run: () => void act(a) });
    return [
      A("new", "New document", "File", { t: "file", a: "new" }, "⌘N"),
      A("open", "Open…", "File", { t: "file", a: "open" }, "⌘O"),
      A("save", "Save", "File", { t: "file", a: "save" }, "⌘S"),
      A("save_as", "Save as…", "File", { t: "file", a: "save_as" }, "⇧⌘S"),
      A("print", "Print or save as PDF", "File", { t: "print" }, "⌘P"),
      A("templates", "New from template", "File", { t: "start" }),
      A("versions", "Version history…", "File", { t: "versions" }),
      A("bold", "Bold", "Format", { t: "toggle", what: "bold" }, "⌘B"),
      A("italic", "Italic", "Format", { t: "toggle", what: "italic" }, "⌘I"),
      A("underline", "Underline", "Format", { t: "toggle", what: "underline" }, "⌘U"),
      A("strike", "Strikethrough", "Format", { t: "toggle", what: "strike" }),
      A("clear", "Clear formatting", "Format", { t: "clear_format" }),
      A("bullets", "Bulleted list", "Paragraph", { t: "list", kind: "bullet" }),
      A("numbers", "Numbered list", "Paragraph", { t: "list", kind: "number" }),
      A("center", "Center", "Paragraph", { t: "align", value: "center" }, "⌘E"),
      A("justify", "Justify", "Paragraph", { t: "align", value: "both" }, "⌘J"),
      ...STYLES.map((s) => A(`style-${s.id}`, s.name, "Style", { t: "style", id: s.id })),
      A("page_break", "Page break", "Insert", { t: "page_break" }, "⌘↩"),
      A("table", "Table (3 × 3)", "Insert", { t: "table", rows: 3, cols: 3 }),
      A("picture", "Picture…", "Insert", { t: "picture" }),
      A("track", info?.track ? "Stop tracking changes" : "Track changes", "Review", { t: "track" }, "⇧⌘E"),
      A("accept", "Accept all changes", "Review", { t: "accept_all" }),
      A("reject", "Reject all changes", "Review", { t: "reject_all" }),
      A("pane", "Show or hide the changes pane", "Review", { t: "pane" }),
      A("find", "Find", "Edit", { t: "find" }, "⌘F"),
      A("nav", "Show or hide headings", "View", { t: "nav" }),
      A("toc", "Insert table of contents", "Insert", { t: "toc" }),
      A("toc_update", "Update table of contents", "Insert", { t: "toc", update: true }),
      A("footnote", "Footnote", "Insert", { t: "footnote" }),
      A("pagenum", "Page numbers (bottom center)", "Insert", { t: "page_numbers", where: "footer", align: "center", total: true }),
      A("link", "Link…", "Insert", { t: "link" }, "⌥⌘K"),
      A("comment", "New comment", "Review", { t: "comment" }, "⌥⌘M"),
      A("landscape", "Landscape orientation", "Layout", { t: "page_setup", orientation: "landscape" }),
      A("portrait", "Portrait orientation", "Layout", { t: "page_setup", orientation: "portrait" }),
      A("narrow", "Narrow margins", "Layout", { t: "page_setup", margins: "narrow" }),
      A("a4", "A4 paper", "Layout", { t: "page_setup", size: "a4" }),
      A("letter", "Letter paper", "Layout", { t: "page_setup", size: "letter" }),
      A("fit", "Zoom to page width", "View", { t: "zoom", value: -1 }),
      A("actual", "Actual size", "View", { t: "zoom", value: 1 }, "⌘0"),
    ];
  }, [act, info?.track]);

  const paletteExtra = useCallback((q: string): PaletteItem[] => {
    const out: PaletteItem[] = [];
    const m = /^(?:page\s*|p)?(\d+)$/i.exec(q.trim());
    if (m && layout) {
      const n = Number(m[1]);
      if (n >= 1 && n <= layout.pages.length) {
        out.push({ id: "goto", label: `Go to page ${n}`, group: "Go to", run: () => {
          const el = document.querySelector(".page-scroll") as HTMLElement | null;
          const target = document.querySelectorAll(".page")[n - 1] as HTMLElement | undefined;
          if (el && target) el.scrollTop = target.offsetTop - 20;
        } });
      }
    }
    if (q.length >= 2) out.push({ id: "find-q", label: `Find “${q}”`, group: "Find", run: () => { setFind({ q, marks: [], current: 0 }); void doFind(q); } });
    return [...out, ...recents.slice(0, 5).filter((r) => !q || r.name.toLowerCase().includes(q.toLowerCase())).map((r) => ({ id: `recent-${r.path}`, label: r.name, group: "Recent", hint: r.path, run: () => void openPath(r.path) }))];
  }, [doFind, layout, openPath, recents]);

  if (!info || !layout) return <div className="app loading">Opening…</div>;

  const words = info.words;
  const style = para?.style ?? "Normal";
  const ribbonState = {
    font: pendingFmt?.font ?? caretItem?.font ?? "Calibri",
    size: pendingFmt?.size ?? caretItem?.size ?? 11,
    bold: pendingFmt?.bold ?? !!caretItem?.b, italic: pendingFmt?.italic ?? !!caretItem?.i,
    underline: pendingFmt?.underline ?? !!caretItem?.u, strike: pendingFmt?.strike ?? !!caretItem?.s,
    style, align: para?.align ?? "left", list: !!para?.list,
    track: info.track, pane, zoom, canUndo: info.can_undo, canRedo: info.can_redo,
    inTable: !!caret && caret.p.length >= 4 && caret.p.some((v) => v >= 0), page: info.page ?? null,
    hasToc: !!info.toc, nav, painter: !!painter, spell: spellOn,
  };

  return (
    <div className={`app${pane ? " with-pane" : ""}`}>
      <header className="titlebar" data-tauri-drag-region>
        <AppMark />
        <span className="doc-name" data-tauri-drag-region>{info.name.replace(/\.docx$/i, "")}</span>
        <span className={`doc-state${info.dirty ? " is-dirty" : ""}`} data-tauri-drag-region title={autosavedAt && info.dirty ? `A recovery copy was saved at ${new Date(autosavedAt).toLocaleTimeString()}` : undefined}>
          {info.dirty ? (autosavedAt ? "Edited · recovery saved" : "Edited") : info.path ? "Saved" : "Not saved yet"}
        </span>
        {info.track && <span className="doc-state is-tracking" data-tauri-drag-region>Tracking changes</span>}
        <span className="title-spacer" data-tauri-drag-region />
        <button type="button" className="title-search" onClick={() => setPalette(true)} title="Search commands (⌘K)">
          <Command size={13} /> <span>Search commands</span> <kbd>⌘K</kbd>
        </button>
      </header>
      <Ribbon s={ribbonState} act={(a) => void act(a)} />
      <main className={`workspace${nav ? " with-nav" : ""}`}>
        {nav && (
          <NavPane headings={headings} current={caret && !caret.s ? (headings.filter((h) => h.p[0] <= caret.p[0]).pop()?.p[0] ?? null) : null}
            onGo={(h) => { moveTo({ p: h.p, o: 0 }, false); viewRef.current?.focus(); }} onClose={() => setNav(false)} />
        )}
        <div className="doc-area">
          <PageView
            ref={viewRef}
            layout={layout}
            zoom={zoom}
            caret={caret}
            anchor={anchor}
            ghost={ghost}
            marks={find?.marks ?? []}
            current={find?.current ?? -1}
            comments={commentRanges}
            onPoint={(p, extend, meta) => {
              goalX.current = null;
              if (meta) {
                void call<{ url: string | null } | null>("link_at", { at: p }).then((l) => { if (l?.url && !l.url.startsWith("#")) void openUrl(l.url); }).catch(() => {});
                return;
              }
              void settle().then(() => moveTo(p, extend));
            }}
            onDrag={(p) => {
              const a = anchorRef.current ?? caretRef.current;
              if (a && !sameContainer(a, p) && !(isTop(a) && isTop(p))) return; // stay in one cell/header/note
              anchorRef.current = a; caretRef.current = p; setAnchor(a); setCaret(p);
            }}
            onWord={(p) => serial(async () => { await settle(); const [s, e] = await wordAt(p); anchorRef.current = s; caretRef.current = e; setAnchor(s); setCaret(e); })}
            onKeyDown={onKeyDown}
            onText={(t) => serial(() => typeText(t))}
            onPaste={(t, h) => serial(() => paste(t, h))}
            squiggles={squiggles}
            onSelectEnd={() => {
              const pf = painter;
              const a = anchorRef.current, c = caretRef.current;
              if (!pf || !a || !c || samePos(a, c)) return;
              setPainter(null);
              void (async () => {
                await format(pf.fmt);
                if (pf.style) await paraFormat({ style: pf.style });
              })();
            }}
            onCopy={(cut) => void copy(cut)}
            onContext={(e, p) => {
              if (p && !(anchor && caret && !samePos(anchor, caret))) moveTo(p, false);
              void (async () => {
              const link = p ? await call<{ url: string | null } | null>("link_at", { at: p }).catch(() => null) : null;
              const miss = p ? squiggles.find((m) => keyOf(m) === keyOf(p) && m.o <= p.o && p.o <= m.o + m.len) : undefined;
              const sugg = miss ? await call<string[]>("suggest", { word: miss.word }).catch(() => []) : [];
              const spellItems = miss ? [
                ...(sugg.length ? sugg.slice(0, 5).map((w) => ({ label: w, action: () => void run("replace", { q: miss.word, with: w, case: true, at: { s: miss.s, p: miss.p, o: miss.o } }) })) : [{ label: "No suggestions", disabled: true }]),
                { label: "Add to dictionary", action: () => {
                  const words = Array.from(new Set([...userWords(), miss.word]));
                  try { localStorage.setItem("quire.words", JSON.stringify(words)); } catch { /* ignore */ }
                  void call("spell_words", { words }).then(() => call<(Mark & { word: string })[]>("spell")).then(setSquiggles).catch(() => {});
                } },
                { label: "" },
              ] : [];
              const linkItems = link?.url ? [
                { label: "Open link", action: () => void openUrl(link.url!) },
                { label: "Edit link…", action: () => void act({ t: "link" }) },
                { label: "Remove link", action: () => void run("link_remove", { at: p }) },
                { label: "" },
              ] : [];
              await nativeMenu([
                ...spellItems,
                ...linkItems,
                { label: "Cut", shortcut: "⌘X", action: () => void copy(true) },
                { label: "Copy", shortcut: "⌘C", action: () => void copy(false) },
                { label: "Paste", shortcut: "⌘V", action: () => void paste() },
                { label: "" },
                { label: "Bold", action: () => void act({ t: "toggle", what: "bold" }) },
                { label: "Bulleted list", action: () => void act({ t: "list", kind: "bullet" }) },
                { label: "" },
                { label: "New comment", shortcut: "⌥⌘M", action: () => void act({ t: "comment" }) },
                { label: "Link…", shortcut: "⌥⌘K", action: () => void act({ t: "link" }) },
                { label: "" },
                { label: "Select all", shortcut: "⌘A", action: selectAll },
              ]);
              })();
              void e;
            }}
            onScroll={setPage}
          />
          {find && (
            <div className="find-panel" role="search" onKeyDown={(e) => { if (e.key === "Escape") { setFind(null); viewRef.current?.focus(); } }}>
              <div className="find-row">
                <Search size={14} className="find-icon" aria-hidden />
                <input className="find-input" autoFocus placeholder="Find in document" value={find.q}
                  onChange={(e) => void doFind(e.target.value)}
                  onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); void doFind(find.q, find.current + (e.shiftKey ? -1 : 1)); } }} />
                <span className="find-status">{find.q ? (find.marks.length ? `${find.current + 1} of ${find.marks.length}` : "No results") : ""}</span>
                <button type="button" className="find-btn" aria-label="Previous match" disabled={!find.marks.length} onClick={() => void doFind(find.q, find.current - 1)}><ChevronUp size={15} /></button>
                <button type="button" className="find-btn" aria-label="Next match" disabled={!find.marks.length} onClick={() => void doFind(find.q, find.current + 1)}><ChevronDown size={15} /></button>
                <button type="button" className="find-btn" aria-label="Close find" onClick={() => { setFind(null); viewRef.current?.focus(); }}><X size={15} /></button>
              </div>
              {find.replace ? (
                <div className="find-row">
                  <span className="find-icon find-spacer" />
                  <input className="find-input" placeholder="Replace with" value={find.with ?? ""} onChange={(e) => setFind({ ...find, with: e.target.value })}
                    onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); document.getElementById("replace-one")?.click(); } }} />
                  <button id="replace-one" type="button" className="find-action" disabled={!find.marks.length} onClick={() => void (async () => {
                    const m = find.marks[find.current];
                    if (!m) return;
                    await run("replace", { q: find.q, with: find.with ?? "", at: { s: m.s, p: m.p, o: m.o } });
                    await doFind(find.q, find.current);
                  })()}>Replace</button>
                  <button type="button" className="find-action" disabled={!find.marks.length} onClick={() => void (async () => {
                    const r = await run<{ replaced: number }>("replace", { q: find.q, with: find.with ?? "" });
                    if (r) say(`Replaced ${r.replaced} ${r.replaced === 1 ? "match" : "matches"}`);
                    await doFind(find.q);
                  })()}>All</button>
                </div>
              ) : (
                <button type="button" className="find-more" onClick={() => setFind({ ...find, replace: true, with: "" })}>Replace…</button>
              )}
            </div>
          )}
        </div>
        {pane && (
          <SidePanel
            tab={panelTab} setTab={setPanelTab} comments={comments} revisions={revisions} track={info.track}
            draft={!!draft} active={activeComment}
            onSelectComment={(c) => {
              setActiveComment(c.id);
              if (c.from && c.to) { anchorRef.current = c.from; caretRef.current = c.to; setAnchor(c.from); setCaret(c.to); }
            }}
            onAdd={(text) => {
              const d = draft;
              setDraft(null);
              if (d) void run<{ id: number }>("comment_add", { from: d[0], to: d[1], text }).then((r) => r && setActiveComment(r.id));
            }}
            onCancelDraft={() => setDraft(null)}
            onReply={(parent, text) => void run("comment_add", { from: caretRef.current, text, parent })}
            onEdit={(id, text) => void run("comment_edit", { id, text })}
            onResolve={(id, done) => void run("comment_resolve", { id, done })}
            onDelete={(id) => void run("comment_delete", { id })}
            onNewComment={() => void act({ t: "comment" })}
            onRevision={(r, action) => {
              if (action === "show") {
                if (r.at) moveTo(r.at, false);
                return;
              }
              void run(action, { ids: [r.id] });
            }}
            onAll={(acceptAll) => void act({ t: acceptAll ? "accept_all" : "reject_all" })}
            onClose={() => setPane(false)}
          />
        )}
      </main>
      <footer className="statusbar">
        <span className="stat">Page <b>{Math.min(page + 1, layout.pages.length)}</b> of {layout.pages.length}</span>
        <span className="stat">{words.toLocaleString()} words</span>
        {info.track && <span className="stat accent">Track changes on</span>}
        <span className="status-spacer" />
        <div className="zoom">
          <button type="button" className="zoom-btn" aria-label="Zoom out" onClick={() => setZoom((z) => Math.max(0.4, +(z - 0.1).toFixed(2)))}>−</button>
          <input type="range" min={40} max={300} step={5} value={Math.round(zoom * 100)} aria-label="Zoom" onChange={(e) => setZoom(Number(e.target.value) / 100)} />
          <button type="button" className="zoom-btn" aria-label="Zoom in" onClick={() => setZoom((z) => Math.min(3, +(z + 0.1).toFixed(2)))}>+</button>
          <button type="button" className="zoom-value" onClick={() => setZoom(1)} title="Actual size">{Math.round(zoom * 100)}%</button>
        </div>
      </footer>
      {start && (
        <StartScreen recents={recents} onTemplate={(id) => void newDoc(id)} onOpen={() => void openFile()}
          onOpenRecent={(r) => void guardUnsaved().then((ok) => { if (ok) void openPath(r.path); })}
          onForget={(r) => { removeRecent(r.path); setRecents(loadRecents()); }} onClose={() => setStart(false)} />
      )}
      {palette && <Palette items={paletteItems} extra={paletteExtra} onClose={() => { setPalette(false); viewRef.current?.focus(); }} />}
      {recovery.length > 0 && (
        <div className="recovery" role="alert">
          <b>Quire recovered unsaved changes</b>
          {recovery.slice(0, 3).map((r) => (
            <div key={r.key} className="recovery-row">
              <span>{r.name}{r.path ? "" : " (never saved)"} · {new Date(r.at).toLocaleString(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" })}</span>
              <button type="button" className="pane-btn primary" onClick={() => void (async () => {
                if (!(await guardUnsaved())) return;
                await run("recovery_restore", { key: r.key }, { full: true });
                setRecovery((list) => list.filter((x) => x.key !== r.key));
                resetCaret();
              })()}>Restore</button>
              <button type="button" className="pane-btn" onClick={() => { void call("recovery_discard", { key: r.key }); setRecovery((list) => list.filter((x) => x.key !== r.key)); }}>Discard</button>
            </div>
          ))}
        </div>
      )}
      {versions && <VersionsDialog onClose={() => { setVersions(false); viewRef.current?.focus(); }} onRestore={(id) => void run("version_restore", { id }, { full: true }).then(() => say("Restored. Undo to go back."))} />}
      {slash && (
        <SlashMenu ref={slashRef} query={slash.query} x={slash.x} y={slash.y} onClose={() => setSlash(null)}
          onPick={(item: SlashItem) => {
            const sl = slash;
            setSlash(null);
            serial(async () => {
              await settle();
              // Remove the "/query" text, then run the item.
              await edit("delete", () => ({ from: sl.at, to: { ...sl.at, o: sl.at.o + 1 + sl.query.length } }));
              await act(item.action);
            });
          }} />
      )}
      {prompt && <Prompt spec={prompt} onClose={() => { setPrompt(null); viewRef.current?.focus(); }} />}
      {toast && <div className={`toast${toast.kind === "error" ? " is-error" : ""}`} role="status">{toast.text}</div>}
    </div>
  );
}
