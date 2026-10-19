//! Comments: list, add (with replies), resolve, edit and delete, Word-compatible.
//!
//! Comment bodies live in `word/comments.xml`; threading and the resolved state live in
//! `word/commentsExtended.xml` (Word 2013+), keyed by the `w14:paraId` of each comment's
//! last paragraph. Anchors in the text (`commentRangeStart`/`End` and the reference run)
//! are placed by the caller with [`super::Para::insert_marker`] and friends.

use super::{append, para, Docx, DocxEdits, R_NS, W_NS};
use crate::package;
use crate::xml::{self, attr, element_end, tokens, Kind};

pub const PART: &str = "word/comments.xml";
pub const EXT: &str = "word/commentsExtended.xml";
const CT: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments";
const CT_EXT: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.commentsExtended+xml";
const REL_EXT: &str = "http://schemas.microsoft.com/office/2011/relationships/commentsExtended";
const W14: &str = "http://schemas.microsoft.com/office/word/2010/wordml";
const W15: &str = "http://schemas.microsoft.com/office/word/2012/wordml";

#[derive(Debug, Clone, PartialEq)]
pub struct CommentInfo {
    pub id: i64,
    pub author: String,
    pub initials: String,
    pub date: String,
    pub text: String,
    pub para_id: Option<String>,
    pub parent: Option<i64>,
    pub done: bool,
}

/// (id, start, end) of every `w:comment` element.
fn spans(src: &str) -> Vec<(i64, usize, usize)> {
    let toks: Vec<_> = tokens(src).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        if t.local() == "comment" && matches!(t.kind, Kind::Start | Kind::Empty) {
            let end = element_end(&toks, i);
            let id = attr(&src[t.start..t.end], "w:id").and_then(|v| v.parse().ok()).unwrap_or(-1);
            out.push((id, t.start, end));
            while i < toks.len() && toks[i].start < end {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// `w15:commentEx` entries: (paraId, paraIdParent, done).
fn ext_entries(src: &str) -> Vec<(String, Option<String>, bool)> {
    tokens(src).filter(|t| t.local() == "commentEx" && matches!(t.kind, Kind::Start | Kind::Empty)).map(|t| {
        let tag = &src[t.start..t.end];
        (attr(tag, "w15:paraId").unwrap_or("").to_string(), attr(tag, "w15:paraIdParent").map(str::to_string), attr(tag, "w15:done") == Some("1"))
    }).collect()
}

fn last_para_id(comment_xml: &str) -> Option<String> {
    tokens(comment_xml).filter(|t| t.local() == "p" && matches!(t.kind, Kind::Start | Kind::Empty))
        .filter_map(|t| attr(&comment_xml[t.start..t.end], "w14:paraId").map(str::to_string)).last()
}

/// A fresh paragraph id (8 hex digits below 0x80000000, as Word requires).
fn new_para_id(seed: i64, taken: &str) -> String {
    let mut x = (seed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    loop {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let id = format!("{:08X}", (x as u32) & 0x7FFF_FFFF);
        if !taken.contains(&id) {
            return id;
        }
    }
}

impl Docx {
    /// Every comment, replies linked to their parent.
    pub fn comments(&self, edits: &DocxEdits) -> Vec<CommentInfo> {
        let Some(src) = self.current(edits, PART) else { return vec![] };
        let ext = self.current(edits, EXT).map(|e| ext_entries(&e)).unwrap_or_default();
        let mut out: Vec<CommentInfo> = spans(&src).into_iter().map(|(id, s, e)| {
            let x = &src[s..e];
            let open = tokens(x).next().map(|t| &x[t.start..t.end]).unwrap_or("");
            let text = tokens(x).enumerate().filter(|(_, t)| t.local() == "p" && t.kind == Kind::Start).map(|(i, _)| {
                let toks: Vec<_> = tokens(x).collect();
                let end = element_end(&toks, i);
                let start = toks[i].start;
                para::Para::parse(&x[start..end]).map(|p| p.text()).unwrap_or_default()
            }).collect::<Vec<_>>().join("\n");
            CommentInfo {
                id,
                author: attr(open, "w:author").map(xml::unescape).unwrap_or_default(),
                initials: attr(open, "w:initials").map(xml::unescape).unwrap_or_default(),
                date: attr(open, "w:date").unwrap_or("").to_string(),
                text,
                para_id: last_para_id(x),
                parent: None,
                done: false,
            }
        }).collect();
        for c in out.clone() {
            let Some(pid) = &c.para_id else { continue };
            if let Some((_, parent, done)) = ext.iter().find(|(p, _, _)| p == pid) {
                let parent_id = parent.as_ref().and_then(|pp| out.iter().find(|o| o.para_id.as_deref() == Some(pp.as_str())).map(|o| o.id));
                if let Some(me) = out.iter_mut().find(|o| o.id == c.id) {
                    me.parent = parent_id;
                    me.done = *done;
                }
            }
        }
        out
    }

    fn comments_part(&self, edits: &mut DocxEdits) -> String {
        let initial = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>{}<w:comments xmlns:w="{W_NS}" xmlns:r="{R_NS}" xmlns:w14="{W14}"></w:comments>"#, "\r\n");
        let src = self.ensure_part(edits, PART, CT, REL, &initial);
        // Paragraph ids need the w14 namespace on the root.
        match tokens(&src).find(|t| t.local() == "comments" && matches!(t.kind, Kind::Start | Kind::Empty)) {
            Some(t) if attr(&src[t.start..t.end], "xmlns:w14").is_none() => {
                let tag = xml::set_attr(&src[t.start..t.end], "xmlns:w14", Some(W14));
                format!("{}{}{}", &src[..t.start], tag, &src[t.end..])
            }
            _ => src,
        }
    }

    fn ext_part(&self, edits: &mut DocxEdits) -> String {
        let initial = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>{}<w15:commentsEx xmlns:w15="{W15}" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" mc:Ignorable="w15"></w15:commentsEx>"#, "\r\n");
        self.ensure_part(edits, EXT, CT_EXT, REL_EXT, &initial)
    }

    /// Add a comment (or a reply to `parent`). The caller anchors it in the text.
    pub fn add_comment(&self, edits: &mut DocxEdits, id: i64, author: &str, initials: &str, date: &str, text: &str, parent: Option<i64>) -> Result<(), String> {
        let src = self.comments_part(edits);
        let pid = new_para_id(id, &src);
        let paras: String = text.split('\n').enumerate().map(|(i, line)| {
            let mark = if i == 0 { r#"<w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:annotationRef/></w:r>"# } else { "" };
            let pid_attr = if i == text.split('\n').count() - 1 { format!(r#" w14:paraId="{pid}" w14:textId="77777777""#) } else { String::new() };
            format!(r#"<w:p{pid_attr}><w:pPr><w:pStyle w:val="CommentText"/></w:pPr>{mark}{}</w:p>"#, para::text_run(line, None))
        }).collect();
        let comment = format!(r#"<w:comment w:id="{id}" w:author="{}" w:date="{}" w:initials="{}">{paras}</w:comment>"#, xml::escape(author), xml::escape(date), xml::escape(initials));
        edits.parts.insert(PART.into(), package::append_to_root(&src, &comment).into_bytes());
        for s in ["CommentText", "CommentReference"] {
            append::ensure_builtin_style(self, edits, s);
        }
        // Threading: Word links replies through commentsExtended.
        let parent_pid = match parent {
            Some(p) => Some(self.ensure_para_id(edits, p)?),
            None => None,
        };
        let ext = self.ext_part(edits);
        let entry = format!(r#"<w15:commentEx w15:paraId="{pid}"{} w15:done="0"/>"#, parent_pid.map(|p| format!(r#" w15:paraIdParent="{p}""#)).unwrap_or_default());
        edits.parts.insert(EXT.into(), package::append_to_root(&ext, &entry).into_bytes());
        Ok(())
    }

    /// The paraId of comment `id`'s last paragraph, adding one if it has none.
    fn ensure_para_id(&self, edits: &mut DocxEdits, id: i64) -> Result<String, String> {
        let src = self.comments_part(edits);
        let (_, s, e) = spans(&src).into_iter().find(|(i, _, _)| *i == id).ok_or("no such comment")?;
        if let Some(p) = last_para_id(&src[s..e]) {
            return Ok(p);
        }
        let x = &src[s..e];
        let toks: Vec<_> = tokens(x).collect();
        let t = toks.iter().filter(|t| t.local() == "p" && matches!(t.kind, Kind::Start | Kind::Empty)).last().ok_or("empty comment")?;
        let pid = new_para_id(id, &src);
        let tag = xml::set_attr(&x[t.start..t.end], "w14:paraId", Some(&pid));
        let new = format!("{}{}{}{}{}", &src[..s], &x[..t.start], tag, &x[t.end..], &src[e..]);
        edits.parts.insert(PART.into(), new.into_bytes());
        Ok(pid)
    }

    /// Mark a comment thread resolved (or open again).
    pub fn set_comment_done(&self, edits: &mut DocxEdits, id: i64, done: bool) -> Result<(), String> {
        let pid = self.ensure_para_id(edits, id)?;
        let ext = self.ext_part(edits);
        let val = if done { "1" } else { "0" };
        let new = match tokens(&ext).find(|t| t.local() == "commentEx" && attr(&ext[t.start..t.end], "w15:paraId") == Some(pid.as_str())) {
            Some(t) => {
                let tag = xml::set_attr(&ext[t.start..t.end], "w15:done", Some(val));
                format!("{}{}{}", &ext[..t.start], tag, &ext[t.end..])
            }
            None => package::append_to_root(&ext, &format!(r#"<w15:commentEx w15:paraId="{pid}" w15:done="{val}"/>"#)),
        };
        edits.parts.insert(EXT.into(), new.into_bytes());
        Ok(())
    }

    /// Replace a comment's text (keeps its author, date and thread).
    pub fn set_comment_text(&self, edits: &mut DocxEdits, id: i64, text: &str) -> Result<(), String> {
        let src = self.comments_part(edits);
        let (_, s, e) = spans(&src).into_iter().find(|(i, _, _)| *i == id).ok_or("no such comment")?;
        let x = &src[s..e];
        let pid = last_para_id(x).unwrap_or_else(|| new_para_id(id, &src));
        let open_end = x.find('>').map(|i| i + 1).unwrap_or(0);
        let lines: Vec<&str> = text.split('\n').collect();
        let paras: String = lines.iter().enumerate().map(|(i, line)| {
            let mark = if i == 0 { r#"<w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:annotationRef/></w:r>"# } else { "" };
            let pid_attr = if i == lines.len() - 1 { format!(r#" w14:paraId="{pid}" w14:textId="77777777""#) } else { String::new() };
            format!(r#"<w:p{pid_attr}><w:pPr><w:pStyle w:val="CommentText"/></w:pPr>{mark}{}</w:p>"#, para::text_run(line, None))
        }).collect();
        let new = format!("{}{}{}</w:comment>{}", &src[..s], &x[..open_end], paras, &src[e..]);
        edits.parts.insert(PART.into(), new.into_bytes());
        Ok(())
    }

    /// Remove comments (bodies and thread entries). Remove their anchors from the text separately.
    pub fn remove_comments(&self, edits: &mut DocxEdits, ids: &[i64]) -> Result<(), String> {
        let src = self.comments_part(edits);
        let mut out = String::with_capacity(src.len());
        let mut cursor = 0;
        let mut pids = Vec::new();
        for (id, s, e) in spans(&src) {
            if ids.contains(&id) {
                if let Some(p) = last_para_id(&src[s..e]) {
                    pids.push(p);
                }
                out.push_str(&src[cursor..s]);
                cursor = e;
            }
        }
        out.push_str(&src[cursor..]);
        edits.parts.insert(PART.into(), out.into_bytes());
        if let Some(ext) = self.current(edits, EXT) {
            let new = package::remove_elements(&ext, |tag| attr(tag, "w15:paraId").is_some_and(|p| pids.iter().any(|x| x == p)));
            edits.parts.insert(EXT.into(), new.into_bytes());
        }
        Ok(())
    }
}
