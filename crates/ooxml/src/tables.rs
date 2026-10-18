//! Append-only views of the shared-string table and the style table.
//!
//! Existing entries are never rewritten (rich text runs, named styles, and
//! extension data in the original survive). New strings and styles introduced by
//! edits are appended, and their indices are handed back to the cell splicer.

use std::collections::{HashMap, HashSet};

use crate::package::append_to_root;
use crate::xml::{self, attr, set_attr, tokens, Kind, Tok};

const SST_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";

// --------------------------------------------------------------------------- strings

pub struct StringTable {
    src: Option<String>,
    prefix: String,
    count: usize,
    index: HashMap<String, u32>,
    appended: Vec<String>,
    refs_added: usize,
}

impl StringTable {
    pub fn parse(src: Option<String>) -> Self {
        let mut index = HashMap::new();
        let mut count = 0usize;
        let mut prefix = String::new();
        if let Some(s) = &src {
            let toks: Vec<Tok> = tokens(s).collect();
            if let Some(root) = toks.iter().find(|t| t.local() == "sst") {
                prefix = xml::prefix(root.name).map(|p| format!("{p}:")).unwrap_or_default();
            }
            let mut i = 0;
            while i < toks.len() {
                if toks[i].local() == "si" && matches!(toks[i].kind, Kind::Start | Kind::Empty) {
                    let end = xml::element_end(&toks, i);
                    let text = si_text(s, &toks, i, end);
                    index.entry(text).or_insert(count as u32);
                    count += 1;
                    while i < toks.len() && toks[i].start < end {
                        i += 1;
                    }
                    continue;
                }
                i += 1;
            }
        }
        StringTable { src, prefix, count, index, appended: Vec::new(), refs_added: 0 }
    }

    pub fn get_or_add(&mut self, text: &str) -> u32 {
        self.refs_added += 1;
        if let Some(i) = self.index.get(text) {
            return *i;
        }
        let i = (self.count + self.appended.len()) as u32;
        self.appended.push(text.to_string());
        self.index.insert(text.to_string(), i);
        i
    }

    pub fn exists(&self) -> bool {
        self.src.is_some()
    }

    pub fn changed(&self) -> bool {
        !self.appended.is_empty() || (self.src.is_none() && self.refs_added > 0)
    }

    pub fn added(&self) -> usize {
        self.appended.len()
    }

    pub fn render(&self) -> String {
        let p = &self.prefix;
        let new_si: String = self
            .appended
            .iter()
            .map(|t| format!("<{p}si><{p}t xml:space=\"preserve\">{}</{p}t></{p}si>", xml::escape(t)))
            .collect();
        let unique = self.count + self.appended.len();
        match &self.src {
            None => format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<sst xmlns=\"{SST_NS}\" count=\"{}\" uniqueCount=\"{unique}\">{new_si}</sst>",
                self.refs_added
            ),
            Some(src) => {
                let body = append_to_root(src, &new_si);
                // fix the root's counts
                let root = tokens(&body).find(|t| t.local() == "sst" && t.kind == Kind::Start);
                match root {
                    Some(t) => {
                        let tag = &body[t.start..t.end];
                        let old: usize = attr(tag, "count").and_then(|c| c.parse().ok()).unwrap_or(unique);
                        let tag = set_attr(tag, "uniqueCount", Some(&unique.to_string()));
                        let tag = set_attr(&tag, "count", Some(&(old + self.refs_added).to_string()));
                        format!("{}{tag}{}", &body[..t.start], &body[t.end..])
                    }
                    None => body,
                }
            }
        }
    }
}

fn si_text(s: &str, toks: &[Tok], i: usize, end: usize) -> String {
    let mut out = String::new();
    let mut in_phonetic = 0;
    let mut j = i + 1;
    while j < toks.len() && toks[j].start < end {
        let t = toks[j];
        match (t.kind, t.local()) {
            (Kind::Start, "rPh") => in_phonetic += 1,
            (Kind::End, "rPh") => in_phonetic -= 1,
            (Kind::Start, "t") if in_phonetic == 0 => out.push_str(&xml::unescape(xml::inner(s, toks, j))),
            _ => {}
        }
        j += 1;
    }
    out
}

// --------------------------------------------------------------------------- styles

/// A section of styles.xml (`<fonts>`, `<cellXfs>`, ...): its direct children.
struct Section {
    children: Vec<String>,
}

fn section(src: &str, name: &str) -> Option<Section> {
    let toks: Vec<Tok> = tokens(src).collect();
    let i = toks.iter().position(|t| t.local() == name && matches!(t.kind, Kind::Start | Kind::Empty))?;
    if toks[i].kind == Kind::Empty {
        return Some(Section { children: vec![] });
    }
    let end = xml::element_end(&toks, i);
    let mut children = Vec::new();
    let mut j = i + 1;
    while j < toks.len() && toks[j].start < end {
        let t = toks[j];
        if matches!(t.kind, Kind::Start | Kind::Empty) {
            let ce = xml::element_end(&toks, j);
            children.push(src[t.start..ce].to_string());
            while j < toks.len() && toks[j].start < ce {
                j += 1;
            }
            continue;
        }
        j += 1;
    }
    Some(Section { children })
}

pub struct StyleTable {
    orig: String,
    n_xf: usize,
    n_font: usize,
    n_fill: usize,
    n_border: usize,
    numfmt_ids: HashSet<i32>,
    t_xfs: Vec<String>,
    t_fonts: Vec<String>,
    t_fills: Vec<String>,
    t_borders: Vec<String>,
    t_numfmts: HashMap<i32, String>,
    new: HashMap<&'static str, Vec<String>>,
    xf_map: HashMap<i32, usize>,
    comp_map: HashMap<(&'static str, i32), usize>,
}

impl StyleTable {
    /// `expected` = (cellXfs, fonts, fills, borders) counts IronCalc loaded; they must
    /// match the original file or index-preserving splicing isn't safe.
    pub fn new(orig: String, exported: &str, expected: (usize, usize, usize, usize)) -> Result<Self, String> {
        if orig.contains("<x:styleSheet") || tokens(&orig).any(|t| t.local() == "styleSheet" && t.name.contains(':')) {
            return Err("prefixed styles.xml not supported".into());
        }
        let count = |name| section(&orig, name).map(|s| s.children.len()).unwrap_or(0);
        let got = (count("cellXfs"), count("fonts"), count("fills"), count("borders"));
        if got != expected {
            return Err(format!("style table mismatch: file has {got:?}, IronCalc loaded {expected:?}"));
        }
        let kids = |name| section(exported, name).map(|s| s.children).unwrap_or_default();
        let ids = |src: &str| -> HashMap<i32, String> {
            section(src, "numFmts")
                .map(|s| {
                    s.children
                        .into_iter()
                        .filter_map(|c| attr(&c, "numFmtId").and_then(|v| v.parse().ok()).map(|id| (id, c.clone())))
                        .collect()
                })
                .unwrap_or_default()
        };
        Ok(StyleTable {
            numfmt_ids: ids(&orig).into_keys().collect(),
            t_numfmts: ids(exported),
            n_xf: got.0,
            n_font: got.1,
            n_fill: got.2,
            n_border: got.3,
            t_xfs: kids("cellXfs"),
            t_fonts: kids("fonts"),
            t_fills: kids("fills"),
            t_borders: kids("borders"),
            orig,
            new: HashMap::new(),
            xf_map: HashMap::new(),
            comp_map: HashMap::new(),
        })
    }

    pub fn added(&self) -> usize {
        self.new.get("cellXfs").map(Vec::len).unwrap_or(0)
    }

    /// Map an xf index from IronCalc's working model to an index in the patched file.
    pub fn map_xf(&mut self, s: i32) -> Result<usize, String> {
        if s >= 0 && (s as usize) < self.n_xf {
            return Ok(s as usize);
        }
        if let Some(i) = self.xf_map.get(&s) {
            return Ok(*i);
        }
        let xf = self.t_xfs.get(s as usize).ok_or(format!("exported style {s} missing"))?.clone();
        let tag_end = xf.find('>').map(|i| i + 1).unwrap_or(xf.len());
        let mut tag = xf[..tag_end].to_string();
        for (key, sect) in [("fontId", "fonts"), ("fillId", "fills"), ("borderId", "borders")] {
            if let Some(id) = attr(&tag, key).and_then(|v| v.parse::<i32>().ok()) {
                let mapped = self.map_component(sect, id)?;
                tag = set_attr(&tag, key, Some(&mapped.to_string()));
            }
        }
        if let Some(id) = attr(&tag, "numFmtId").and_then(|v| v.parse::<i32>().ok()) {
            if id >= 164 && !self.numfmt_ids.contains(&id) {
                let nf = self.t_numfmts.get(&id).ok_or(format!("exported numFmt {id} missing"))?.clone();
                self.new.entry("numFmts").or_default().push(nf);
                self.numfmt_ids.insert(id);
            }
        }
        let list = self.new.entry("cellXfs").or_default();
        list.push(format!("{tag}{}", &xf[tag_end..]));
        let idx = self.n_xf + list.len() - 1;
        self.xf_map.insert(s, idx);
        Ok(idx)
    }

    fn map_component(&mut self, sect: &'static str, id: i32) -> Result<usize, String> {
        let (n, src) = match sect {
            "fonts" => (self.n_font, &self.t_fonts),
            "fills" => (self.n_fill, &self.t_fills),
            _ => (self.n_border, &self.t_borders),
        };
        if id >= 0 && (id as usize) < n {
            return Ok(id as usize);
        }
        if let Some(i) = self.comp_map.get(&(sect, id)) {
            return Ok(*i);
        }
        let xml = src.get(id as usize).ok_or(format!("exported {sect} {id} missing"))?.clone();
        let list = self.new.entry(sect).or_default();
        list.push(xml);
        let idx = n + list.len() - 1;
        self.comp_map.insert((sect, id), idx);
        Ok(idx)
    }

    pub fn changed(&self) -> bool {
        !self.new.is_empty()
    }

    pub fn render(&self) -> String {
        let mut out = self.orig.clone();
        for sect in ["numFmts", "fonts", "fills", "borders", "cellXfs"] {
            let Some(items) = self.new.get(sect) else { continue };
            out = append_children(&out, sect, items);
        }
        out
    }
}

/// Append children to a styles.xml section and fix its `count`; creates `<numFmts>` if absent.
fn append_children(src: &str, name: &str, items: &[String]) -> String {
    let toks: Vec<Tok> = tokens(src).collect();
    let joined: String = items.concat();
    match toks.iter().position(|t| t.local() == name && matches!(t.kind, Kind::Start | Kind::Empty)) {
        Some(i) => {
            let t = toks[i];
            let tag = &src[t.start..t.end];
            let old: usize = attr(tag, "count").and_then(|c| c.parse().ok()).unwrap_or(0);
            let existing = section(src, name).map(|s| s.children.len()).unwrap_or(old);
            let new_tag = set_attr(tag, "count", Some(&(existing + items.len()).to_string()));
            if t.kind == Kind::Empty {
                let open = new_tag.replacen("/>", ">", 1);
                format!("{}{open}{joined}</{}>{}", &src[..t.start], t.name, &src[t.end..])
            } else {
                let end = xml::element_end(&toks, i);
                let close = src[..end].rfind("</").unwrap_or(end);
                format!("{}{new_tag}{}{joined}{}", &src[..t.start], &src[t.end..close], &src[close..])
            }
        }
        None => {
            // numFmts must be the first child of styleSheet
            let root = toks.iter().find(|t| t.local() == "styleSheet" && t.kind == Kind::Start);
            match root {
                Some(r) => format!(
                    "{}<{name} count=\"{}\">{joined}</{name}>{}",
                    &src[..r.end],
                    items.len(),
                    &src[r.end..]
                ),
                None => src.to_string(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_keep_rich_entries_and_append() {
        let src = r#"<sst count="3" uniqueCount="2"><si><r><rPr><b/></rPr><t>Bold</t></r><r><t> plain</t></r></si><si><t>x</t><rPh><t>ignored</t></rPh></si></sst>"#;
        let mut t = StringTable::parse(Some(src.to_string()));
        assert_eq!(t.get_or_add("Bold plain"), 0);
        assert_eq!(t.get_or_add("x"), 1);
        assert_eq!(t.get_or_add("new & <odd>"), 2);
        let out = t.render();
        assert!(out.contains("<r><rPr><b/></rPr><t>Bold</t></r>"), "rich run kept");
        assert!(out.contains(r#"<si><t xml:space="preserve">new &amp; &lt;odd&gt;</t></si></sst>"#));
        assert!(out.contains(r#"uniqueCount="3""#));
    }

    #[test]
    fn styles_append_new_xf_with_components() {
        let orig = r#"<styleSheet><fonts count="1"><font><sz val="11"/></font></fonts><fills count="2"><fill/><fill/></fills><borders count="1"><border/></borders><cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellXfs></styleSheet>"#;
        let exported = r#"<styleSheet><numFmts count="1"><numFmt numFmtId="164" formatCode="0.0"/></numFmts><fonts count="2"><font><sz val="11"/></font><font><b/><sz val="11"/></font></fonts><fills count="2"><fill/><fill/></fills><borders count="1"><border/></borders><cellXfs count="2"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/><xf numFmtId="164" fontId="1" fillId="0" borderId="0"/></cellXfs></styleSheet>"#;
        let mut st = StyleTable::new(orig.into(), exported, (1, 1, 2, 1)).unwrap();
        assert_eq!(st.map_xf(0).unwrap(), 0);
        assert_eq!(st.map_xf(1).unwrap(), 1);
        let out = st.render();
        assert!(out.starts_with(r#"<styleSheet><numFmts count="1"><numFmt numFmtId="164""#));
        assert!(out.contains(r#"<fonts count="2"><font><sz val="11"/></font><font><b/><sz val="11"/></font></fonts>"#));
        assert!(out.contains(r#"<cellXfs count="2">"#));
    }
}
