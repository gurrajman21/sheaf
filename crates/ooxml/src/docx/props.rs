//! Run and paragraph properties (`w:rPr`, `w:pPr`) as ordered child elements.
//!
//! Children we don't touch are kept verbatim. Children we set are inserted at the
//! position the schema requires (Word rejects some out-of-order properties).

use crate::xml::{self, element_end, tokens, Kind, Tok};

/// CT_RPr child order (ECMA-376 Part 1, 17.3.2.28), `rPrChange` last.
pub const RPR_ORDER: &[&str] = &[
    "rStyle", "rFonts", "b", "bCs", "i", "iCs", "caps", "smallCaps", "strike", "dstrike", "outline", "shadow",
    "emboss", "imprint", "noProof", "snapToGrid", "vanish", "webHidden", "color", "spacing", "w", "kern",
    "position", "sz", "szCs", "highlight", "u", "effect", "bdr", "shd", "fitText", "vertAlign", "rtl", "cs", "em",
    "lang", "eastAsianLayout", "specVanish", "oMath", "ins", "del", "moveFrom", "moveTo", "rPrChange",
];

/// CT_PPr child order (17.3.1.26), `pPrChange` last.
pub const PPR_ORDER: &[&str] = &[
    "pStyle", "keepNext", "keepLines", "pageBreakBefore", "framePr", "widowControl", "numPr", "suppressLineNumbers",
    "pBdr", "shd", "tabs", "suppressAutoHyphens", "kinsoku", "wordWrap", "overflowPunct", "topLinePunct",
    "autoSpaceDE", "autoSpaceDN", "bidi", "adjustRightInd", "snapToGrid", "spacing", "ind", "contextualSpacing",
    "mirrorIndents", "suppressOverlap", "jc", "textDirection", "textAlignment", "textboxTightWrap", "outlineLvl",
    "divId", "cnfStyle", "rPr", "sectPr", "pPrChange",
];

/// CT_TcPr child order (17.4.70).
pub const TCPR_ORDER: &[&str] = &[
    "cnfStyle", "tcW", "gridSpan", "hMerge", "vMerge", "tcBorders", "shd", "noWrap", "tcMar", "textDirection",
    "tcFitText", "vAlign", "hideMark", "headers", "cellIns", "cellDel", "cellMerge", "tcPrChange",
];

/// CT_SectPr child order (17.6.17).
pub const SECTPR_ORDER: &[&str] = &[
    "headerReference", "footerReference", "footnotePr", "endnotePr", "type", "pgSz", "pgMar", "paperSrc",
    "pgBorders", "lnNumType", "pgNumType", "cols", "formProt", "vAlign", "noEndnote", "titlePg", "textDirection",
    "bidi", "rtlGutter", "docGrid", "printerSettings", "sectPrChange",
];

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Props {
    /// `w:rPr` or `w:pPr` (qualified, as written).
    pub tag: String,
    /// Attributes of the opening tag, raw (rarely present; kept).
    pub open: String,
    pub children: Vec<(String, String)>, // (local name, raw xml)
}

impl Props {
    pub fn new(tag: &str) -> Self {
        Props { tag: tag.to_string(), open: format!("<{tag}>"), children: vec![] }
    }

    /// Parse `<w:rPr>…</w:rPr>` (or a self-closing one).
    pub fn parse(src: &str) -> Self {
        let toks: Vec<Tok> = tokens(src).collect();
        let Some(first) = toks.iter().position(|t| matches!(t.kind, Kind::Start | Kind::Empty)) else { return Props::default() };
        let root = toks[first];
        let mut p = Props {
            tag: root.name.to_string(),
            open: src[root.start..root.end].trim_end_matches("/>").trim_end_matches('>').to_string() + ">",
            children: vec![],
        };
        if root.kind == Kind::Empty {
            return p;
        }
        let end = element_end(&toks, first);
        let mut i = first + 1;
        while i < toks.len() && toks[i].start < end {
            let t = toks[i];
            if matches!(t.kind, Kind::Start | Kind::Empty) {
                let e = element_end(&toks, i);
                p.children.push((t.local().to_string(), src[t.start..e].to_string()));
                while i < toks.len() && toks[i].start < e {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        p
    }

    fn order(&self) -> &'static [&'static str] {
        match xml::local(&self.tag) {
            "pPr" => PPR_ORDER,
            "tcPr" => TCPR_ORDER,
            "sectPr" => SECTPR_ORDER,
            _ => RPR_ORDER,
        }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.children.iter().find(|(n, _)| n == name).map(|(_, x)| x.as_str())
    }

    /// Set (or with `None`, remove) the child `name` to `raw`, keeping schema order.
    pub fn set(&mut self, name: &str, raw: Option<String>) {
        let pos = self.children.iter().position(|(n, _)| n == name);
        match (pos, raw) {
            (Some(i), Some(x)) => self.children[i].1 = x,
            (Some(i), None) => {
                self.children.remove(i);
            }
            (None, Some(x)) => {
                let order = self.order();
                let rank = |n: &str| order.iter().position(|o| *o == n).unwrap_or(order.len() - 1);
                let r = rank(name);
                let at = self.children.iter().position(|(n, _)| rank(n) > r).unwrap_or(self.children.len());
                self.children.insert(at, (name.to_string(), x));
            }
            (None, None) => {}
        }
    }

    /// Toggle property like `b`, `i`, `strike`: `true` writes `<w:b/>`, `false` writes
    /// `<w:b w:val="0"/>` (which overrides a style that turns it on), `None` removes it.
    pub fn set_toggle(&mut self, name: &str, on: Option<bool>) {
        self.set(name, on.map(|v| if v { format!("<w:{name}/>") } else { format!(r#"<w:{name} w:val="0"/>"#) }));
    }

    pub fn set_val(&mut self, name: &str, val: Option<&str>) {
        self.set(name, val.map(|v| format!(r#"<w:{name} w:val="{}"/>"#, xml::escape(v))));
    }

    /// Whether a toggle property is on in this element (not considering styles).
    pub fn toggle(&self, name: &str) -> Option<bool> {
        self.get(name).map(|x| !matches!(xml::attr(x, "w:val"), Some("0" | "false" | "off")))
    }

    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    /// Children as XML, optionally leaving out some (used to snapshot old properties).
    pub fn inner(&self, skip: &[&str]) -> String {
        self.children.iter().filter(|(n, _)| !skip.contains(&n.as_str())).map(|(_, x)| x.as_str()).collect()
    }

    pub fn to_xml(&self) -> String {
        if self.children.is_empty() {
            return String::new();
        }
        let tag = if self.tag.is_empty() { "w:rPr" } else { &self.tag };
        let open = if self.open.is_empty() { format!("<{tag}>") } else { self.open.clone() };
        format!("{open}{}</{tag}>", self.inner(&[]))
    }
}

/// Run formatting the editor can apply. `None` fields are left alone.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunFormat {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    /// Font size in half-points (`w:sz`), e.g. 24 = 12pt. `Some(None)` removes it.
    pub size_half_pt: Option<Option<u32>>,
    pub font: Option<Option<String>>,
    /// Hex color without '#'. `Some(None)` removes it.
    pub color: Option<Option<String>>,
    pub highlight: Option<Option<String>>,
    pub vert_align: Option<Option<String>>,
    pub style: Option<Option<String>>,
}

impl RunFormat {
    pub fn apply(&self, p: &mut Props) {
        if let Some(b) = self.bold {
            p.set_toggle("b", Some(b));
            p.set_toggle("bCs", Some(b));
        }
        if let Some(v) = self.italic {
            p.set_toggle("i", Some(v));
            p.set_toggle("iCs", Some(v));
        }
        if let Some(v) = self.underline {
            p.set_val("u", Some(if v { "single" } else { "none" }));
        }
        if let Some(v) = self.strike {
            p.set_toggle("strike", Some(v));
        }
        if let Some(v) = &self.size_half_pt {
            let s = v.map(|n| n.to_string());
            p.set_val("sz", s.as_deref());
            p.set_val("szCs", s.as_deref());
        }
        if let Some(v) = &self.font {
            p.set("rFonts", v.as_ref().map(|f| {
                let f = xml::escape(f);
                format!(r#"<w:rFonts w:ascii="{f}" w:hAnsi="{f}" w:eastAsia="{f}" w:cs="{f}"/>"#)
            }));
        }
        if let Some(v) = &self.color {
            p.set_val("color", v.as_deref());
        }
        if let Some(v) = &self.highlight {
            p.set_val("highlight", v.as_deref());
        }
        if let Some(v) = &self.vert_align {
            p.set_val("vertAlign", v.as_deref());
        }
        if let Some(v) = &self.style {
            p.set_val("rStyle", v.as_deref());
        }
    }
}

/// Paragraph formatting the editor can apply.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParaFormat {
    pub style: Option<Option<String>>,
    /// `left`/`start`, `center`, `right`/`end`, `both`. `Some(None)` removes it.
    pub align: Option<Option<String>>,
    /// Raw `<w:spacing …/>` / `<w:ind …/>` / `<w:numPr>…</w:numPr>` replacements.
    pub spacing: Option<Option<String>>,
    pub indent: Option<Option<String>>,
    pub numbering: Option<Option<String>>,
    pub keep_next: Option<bool>,
    pub page_break_before: Option<bool>,
}

impl ParaFormat {
    pub fn apply(&self, p: &mut Props) {
        if let Some(v) = &self.style {
            p.set_val("pStyle", v.as_deref());
        }
        if let Some(v) = &self.align {
            p.set_val("jc", v.as_deref());
        }
        if let Some(v) = &self.spacing {
            p.set("spacing", v.clone());
        }
        if let Some(v) = &self.indent {
            p.set("ind", v.clone());
        }
        if let Some(v) = &self.numbering {
            p.set("numPr", v.clone());
        }
        if let Some(v) = self.keep_next {
            p.set_toggle("keepNext", if v { Some(true) } else { None });
        }
        if let Some(v) = self.page_break_before {
            p.set_toggle("pageBreakBefore", if v { Some(true) } else { None });
        }
    }
}
