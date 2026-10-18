//! Minimal, byte-exact XML tokenizer for splicing machine-written OOXML parts.
//!
//! The patch writer never re-serializes a part it doesn't need to change; it
//! copies byte ranges and swaps out individual elements. That needs exact
//! offsets for tags, which this tokenizer provides. It assumes well-formed XML
//! (which every part we accept has already passed through IronCalc's parser).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Start,
    End,
    Empty,
    Other, // comments, PIs, CDATA, doctype
}

#[derive(Debug, Clone, Copy)]
pub struct Tok<'a> {
    pub kind: Kind,
    /// Qualified name (`x:row` or `row`); empty for `Other`.
    pub name: &'a str,
    pub start: usize,
    /// Exclusive end offset (just past `>`).
    pub end: usize,
}

impl<'a> Tok<'a> {
    pub fn local(&self) -> &'a str {
        local(self.name)
    }
}

pub fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

pub fn prefix(name: &str) -> Option<&str> {
    name.split_once(':').map(|(p, _)| p)
}

pub struct Tokens<'a> {
    s: &'a str,
    pos: usize,
}

pub fn tokens(s: &str) -> Tokens<'_> {
    Tokens { s, pos: 0 }
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Tok<'a>;

    fn next(&mut self) -> Option<Tok<'a>> {
        let s = self.s;
        let i = self.pos + s[self.pos..].find('<')?;
        let rest = &s[i..];
        let other = |close: &str, skip: usize| -> Option<usize> { rest[skip..].find(close).map(|p| i + skip + p + close.len()) };
        let (kind, name, end) = if rest.starts_with("<!--") {
            (Kind::Other, "", other("-->", 4)?)
        } else if rest.starts_with("<![CDATA[") {
            (Kind::Other, "", other("]]>", 9)?)
        } else if rest.starts_with("<?") {
            (Kind::Other, "", other("?>", 2)?)
        } else if rest.starts_with("<!") {
            (Kind::Other, "", other(">", 2)?)
        } else {
            // scan to the closing '>' outside quotes
            let bytes = rest.as_bytes();
            let mut quote = 0u8;
            let mut j = 1;
            while j < bytes.len() {
                let b = bytes[j];
                if quote != 0 {
                    if b == quote {
                        quote = 0;
                    }
                } else if b == b'"' || b == b'\'' {
                    quote = b;
                } else if b == b'>' {
                    break;
                }
                j += 1;
            }
            if j >= bytes.len() {
                return None;
            }
            let end = i + j + 1;
            let tag = &s[i..end];
            if let Some(stripped) = tag.strip_prefix("</") {
                (Kind::End, stripped.trim_end_matches('>').trim(), end)
            } else {
                let body = &tag[1..];
                let n = body.find(|c: char| c.is_whitespace() || c == '/' || c == '>').unwrap_or(body.len());
                let kind = if tag.ends_with("/>") { Kind::Empty } else { Kind::Start };
                (kind, &body[..n], end)
            }
        };
        self.pos = end;
        Some(Tok { kind, name, start: i, end })
    }
}

/// Attributes of a start/empty tag, in order, with raw (still escaped) values.
pub fn attrs(tag: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let body = tag.trim_start_matches('<');
    let body = body.trim_end_matches('>').trim_end_matches('/');
    let mut rest = match body.find(char::is_whitespace) {
        Some(n) => &body[n..],
        None => return out,
    };
    loop {
        rest = rest.trim_start();
        let Some(eq) = rest.find('=') else { break };
        let key = rest[..eq].trim();
        let after = rest[eq + 1..].trim_start();
        let Some(q) = after.chars().next() else { break };
        if q != '"' && q != '\'' {
            break;
        }
        let Some(close) = after[1..].find(q) else { break };
        out.push((key, &after[1..1 + close]));
        rest = &after[close + 2..];
    }
    out
}

pub fn attr<'a>(tag: &'a str, key: &str) -> Option<&'a str> {
    attrs(tag).into_iter().find(|(k, _)| *k == key).map(|(_, v)| v)
}

/// Rebuild a start/empty tag with `key` set to `value` (already escaped), or removed if `None`.
pub fn set_attr(tag: &str, key: &str, value: Option<&str>) -> String {
    let empty = tag.ends_with("/>");
    let body = &tag[1..];
    let n = body.find(|c: char| c.is_whitespace() || c == '/' || c == '>').unwrap_or(body.len());
    let name = &body[..n];
    let mut pairs: Vec<(String, String)> = attrs(tag).into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    match (pairs.iter().position(|(k, _)| k == key), value) {
        (Some(i), Some(v)) => pairs[i].1 = v.to_string(),
        (Some(i), None) => {
            pairs.remove(i);
        }
        (None, Some(v)) => {
            // keep `r` first on cells/rows for readability; otherwise append
            if key == "r" {
                pairs.insert(0, (key.to_string(), v.to_string()))
            } else {
                pairs.push((key.to_string(), v.to_string()))
            }
        }
        (None, None) => {}
    }
    let mut out = format!("<{name}");
    for (k, v) in pairs {
        out.push_str(&format!(" {k}=\"{v}\""));
    }
    out.push_str(if empty { "/>" } else { ">" });
    out
}

/// Offset just past the element that starts at token index `i` (handles nesting).
pub fn element_end(toks: &[Tok], i: usize) -> usize {
    let t = toks[i];
    if t.kind != Kind::Start {
        return t.end;
    }
    let mut depth = 0usize;
    for tk in &toks[i..] {
        match tk.kind {
            Kind::Start if tk.name == t.name => depth += 1,
            Kind::End if tk.name == t.name => {
                depth -= 1;
                if depth == 0 {
                    return tk.end;
                }
            }
            _ => {}
        }
    }
    t.end
}

/// Text content between a start tag and its end tag (raw, still escaped).
pub fn inner<'a>(s: &'a str, toks: &[Tok], i: usize) -> &'a str {
    let end = element_end(toks, i);
    let close = s[..end].rfind("</").unwrap_or(end);
    &s[toks[i].end..close.max(toks[i].end)]
}

pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(semi) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let ent = &tail[1..semi];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if ent.starts_with("#x") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => out.push(c),
            None => out.push_str(&tail[..=semi]),
        }
        rest = &tail[semi + 1..];
    }
    out.push_str(rest);
    out
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_with_quoted_gt() {
        let s = r#"<?xml version="1.0"?><a x="1>2"><b/><!-- c --></a>"#;
        let t: Vec<_> = tokens(s).collect();
        assert_eq!(t[1].name, "a");
        assert_eq!(attr(&s[t[1].start..t[1].end], "x"), Some("1>2"));
        assert_eq!(t[2].kind, Kind::Empty);
        assert_eq!(t[3].kind, Kind::Other);
        assert_eq!(t[4].kind, Kind::End);
        assert_eq!(element_end(&t, 1), s.len());
    }

    #[test]
    fn set_attr_roundtrip() {
        assert_eq!(set_attr(r#"<c r="A1" s="3" t="s">"#, "t", None), r#"<c r="A1" s="3">"#);
        assert_eq!(set_attr(r#"<c s="3"/>"#, "r", Some("B2")), r#"<c r="B2" s="3"/>"#);
        assert_eq!(unescape("a&amp;b&#x41;&lt;"), "a&bA<");
    }
}
