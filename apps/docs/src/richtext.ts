// Clipboard HTML (from browsers, Word, Google Docs, Pages...) to the structured
// paragraphs `paste_rich` takes: style, list kind and formatted runs with links.

export interface RichRun { text: string; bold?: boolean; italic?: boolean; underline?: boolean; strike?: boolean; color?: string; size?: number; link?: string }
export interface RichPara { style?: string; list?: "bullet" | "number"; runs: RichRun[] }

const BLOCK = new Set(["P", "DIV", "H1", "H2", "H3", "H4", "H5", "H6", "LI", "BLOCKQUOTE", "PRE", "TR", "SECTION", "ARTICLE", "HEADER", "FOOTER", "TABLE", "UL", "OL", "BR"]);

function rgbToHex(c: string): string | undefined {
  const m = /rgba?\((\d+),\s*(\d+),\s*(\d+)/.exec(c);
  if (m) return [m[1], m[2], m[3]].map((n) => Number(n).toString(16).padStart(2, "0")).join("").toUpperCase();
  const h = /^#([0-9a-f]{6}|[0-9a-f]{3})$/i.exec(c.trim());
  if (h) return (h[1].length === 3 ? h[1].split("").map((x) => x + x).join("") : h[1]).toUpperCase();
  return undefined;
}

export function htmlToParas(html: string): RichPara[] {
  const doc = new DOMParser().parseFromString(html, "text/html");
  const out: RichPara[] = [];
  let cur: RichPara | null = null;
  const newPara = (style?: string, list?: "bullet" | "number") => { cur = { style, list, runs: [] }; out.push(cur); return cur; };

  const walk = (node: Node, fmt: Omit<RichRun, "text">, block: { style?: string; list?: "bullet" | "number" }) => {
    if (node.nodeType === Node.TEXT_NODE) {
      const text = (node.textContent ?? "").replace(/\s+/g, " ");
      if (!text.trim() && !(cur && cur.runs.length)) return;
      const para: RichPara = cur ?? newPara(block.style, block.list);
      para.runs.push({ text, ...fmt });
      return;
    }
    if (!(node instanceof HTMLElement)) return;
    const tag = node.tagName;
    if (tag === "STYLE" || tag === "SCRIPT" || tag === "META" || tag === "TITLE") return;
    if (tag === "BR") { newPara(block.style, block.list); return; }
    const f = { ...fmt };
    const css = node.style;
    if (tag === "B" || tag === "STRONG" || /^(bold|[6-9]00)$/.test(css.fontWeight)) f.bold = true;
    if (css.fontWeight === "normal" || css.fontWeight === "400") f.bold = undefined;
    if (tag === "I" || tag === "EM" || css.fontStyle === "italic") f.italic = true;
    if (tag === "U" || css.textDecoration.includes("underline")) f.underline = true;
    if (tag === "S" || tag === "STRIKE" || tag === "DEL" || css.textDecoration.includes("line-through")) f.strike = true;
    if (css.color) { const hex = rgbToHex(css.color); if (hex && hex !== "000000") f.color = hex; }
    if (css.fontSize.endsWith("pt")) f.size = parseFloat(css.fontSize);
    else if (css.fontSize.endsWith("px")) f.size = Math.round(parseFloat(css.fontSize) * 0.75 * 2) / 2;
    if (tag === "A" && node.getAttribute("href") && !node.getAttribute("href")!.startsWith("#")) { f.link = node.getAttribute("href")!; f.underline = true; }
    let b = block;
    if (BLOCK.has(tag)) {
      const heading = /^H([1-6])$/.exec(tag);
      const list = tag === "LI" ? (node.parentElement?.tagName === "OL" ? "number" : "bullet") : block.list;
      b = { style: heading ? `Heading${Math.min(3, Number(heading[1]))}` : tag === "BLOCKQUOTE" ? "Quote" : tag === "LI" ? undefined : block.style, list: tag === "UL" || tag === "OL" ? block.list : list };
      if (!["UL", "OL", "TABLE", "TR"].includes(tag)) newPara(b.style, b.list);
    }
    node.childNodes.forEach((c) => walk(c, f, b));
    if (BLOCK.has(tag) && !["UL", "OL", "TABLE"].includes(tag)) cur = null;
  };
  walk(doc.body, {}, {});
  // Trim: drop empty paragraphs at the ends, collapse leading spaces.
  const paras = out.map((p) => ({ ...p, runs: p.runs.filter((r) => r.text.length) })).filter((p, i, all) => p.runs.some((r) => r.text.trim()) || (i > 0 && i < all.length - 1));
  for (const p of paras) if (p.runs[0]) p.runs[0].text = p.runs[0].text.replace(/^\s+/, "");
  return paras;
}
