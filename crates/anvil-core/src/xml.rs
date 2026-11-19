//! A lossless view of an XML document: elements with the byte ranges of
//! their tags, attribute values and content. Nothing is ever re-serialised;
//! changes are [`TextEdit`]s on the original text, so comments, whitespace,
//! attribute order and quoting stay exactly as Visual Studio wrote them.

use std::ops::Range;

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::edit::TextEdit;

#[derive(Clone, Debug)]
pub struct Attr {
    pub name: String,
    /// Unescaped value.
    pub value: String,
    /// Range of the raw value between the quotes.
    pub value_span: Range<usize>,
}

#[derive(Clone, Debug)]
pub struct Element {
    pub name: String,
    /// From `<` to after the `>` of the start tag (the whole tag when empty).
    pub tag: Range<usize>,
    /// From `<` of the start tag to after the `>` of the end tag.
    pub span: Range<usize>,
    /// Between the start and the end tag; empty at `tag.end` when self-closing.
    pub content: Range<usize>,
    pub self_closing: bool,
    pub attrs: Vec<Attr>,
    pub children: Vec<usize>,
    pub parent: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct Doc {
    pub text: String,
    pub elements: Vec<Element>,
}

#[derive(Debug)]
pub struct XmlError(pub String);

impl std::fmt::Display for XmlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "XML: {}", self.0)
    }
}

impl std::error::Error for XmlError {}

pub type Id = usize;

impl Doc {
    pub fn parse(text: &str) -> Result<Doc, XmlError> {
        let mut reader = Reader::from_str(text);
        reader.config_mut().check_end_names = true;
        let mut elements: Vec<Element> = Vec::new();
        let mut stack: Vec<Id> = Vec::new();
        loop {
            let before = usize::try_from(reader.buffer_position()).unwrap_or(0);
            let event = reader.read_event().map_err(|e| XmlError(format!("{e} at byte {before}")))?;
            let after = usize::try_from(reader.buffer_position()).unwrap_or(0);
            match event {
                Event::Start(ref e) | Event::Empty(ref e) => {
                    let self_closing = matches!(event, Event::Empty(_));
                    // The event's range may start with whitespace the reader
                    // skipped; the tag starts at its '<'.
                    let lt = text[before..after].find('<').map_or(before, |i| before + i);
                    let name = e.name().as_ref().to_owned();
                    let attrs = parse_attrs(text, lt, after);
                    let id = elements.len();
                    elements.push(Element {
                        name,
                        tag: lt..after,
                        span: lt..after,
                        content: after..after,
                        self_closing,
                        attrs,
                        children: Vec::new(),
                        parent: stack.last().copied(),
                    });
                    if let Some(&p) = stack.last() {
                        elements[p].children.push(id);
                    }
                    if !self_closing {
                        stack.push(id);
                    }
                }
                Event::End(_) => {
                    let id = stack.pop().ok_or_else(|| XmlError("unbalanced end tag".into()))?;
                    let lt = text[before..after].find('<').map_or(before, |i| before + i);
                    let el = &mut elements[id];
                    el.content = el.tag.end..lt;
                    el.span = el.tag.start..after;
                }
                Event::Eof => break,
                _ => {}
            }
        }
        if elements.is_empty() {
            return Err(XmlError("no root element".into()));
        }
        if !stack.is_empty() {
            return Err(XmlError("unclosed element".into()));
        }
        Ok(Doc { text: text.to_owned(), elements })
    }

    pub fn root(&self) -> Id {
        0
    }

    pub fn el(&self, id: Id) -> &Element {
        &self.elements[id]
    }

    pub fn children<'a>(&'a self, id: Id) -> impl Iterator<Item = Id> + 'a {
        self.elements[id].children.iter().copied()
    }

    pub fn child(&self, id: Id, name: &str) -> Option<Id> {
        self.children(id).find(|&c| self.elements[c].name == name)
    }

    pub fn children_named<'a>(&'a self, id: Id, name: &'a str) -> impl Iterator<Item = Id> + 'a {
        self.children(id).filter(move |&c| self.elements[c].name == name)
    }

    /// Every element named `name`, in document order.
    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = Id> + 'a {
        (0..self.elements.len()).filter(move |&i| self.elements[i].name == name)
    }

    pub fn attr(&self, id: Id, name: &str) -> Option<&str> {
        self.elements[id].attrs.iter().find(|a| a.name == name).map(|a| a.value.as_str())
    }

    /// The unescaped text inside an element (child markup removed).
    pub fn inner_text(&self, id: Id) -> String {
        let el = &self.elements[id];
        let raw = &self.text[el.content.clone()];
        let mut out = String::new();
        let mut rest = raw;
        while let Some(i) = rest.find('<') {
            out.push_str(&rest[..i]);
            rest = rest[i..].find('>').map_or("", |j| &rest[i + j + 1..]);
        }
        out.push_str(rest);
        unescape(&out)
    }

    /// The whitespace before the element on its line.
    pub fn indent_of(&self, id: Id) -> &str {
        let start = self.elements[id].span.start;
        let line_start = self.text[..start].rfind('\n').map_or(0, |i| i + 1);
        let ws = &self.text[line_start..start];
        if ws.chars().all(|c| c == ' ' || c == '\t') { ws } else { "" }
    }

    /// The indentation step of the document (difference between the root's
    /// first child and the root), two spaces when it cannot be told.
    pub fn indent_step(&self) -> String {
        if let Some(&c) = self.elements[0].children.first() {
            let inner = self.indent_of(c);
            let outer = self.indent_of(0);
            if inner.len() > outer.len() {
                return inner[outer.len()..].to_owned();
            }
        }
        "  ".to_owned()
    }

    /// Changes or adds an attribute. A new one goes at the end of the tag.
    pub fn set_attr(&self, id: Id, name: &str, value: &str) -> TextEdit {
        let el = &self.elements[id];
        if let Some(a) = el.attrs.iter().find(|a| a.name == name) {
            return TextEdit::new(a.value_span.start, a.value_span.end, escape_attr(value));
        }
        let close = if el.self_closing {
            // Before " />" or "/>", keeping VS' space before the slash.
            let tag = &self.text[el.tag.clone()];
            let slash = tag.rfind('/').unwrap_or(tag.len() - 1);
            let trimmed = tag[..slash].trim_end().len();
            el.tag.start + trimmed
        } else {
            el.tag.end - 1
        };
        TextEdit::insert(close, format!(" {name}=\"{}\"", escape_attr(value)))
    }

    /// Replaces the content of an element with escaped text. A self-closing
    /// element becomes `<Name>text</Name>`.
    pub fn set_text(&self, id: Id, value: &str) -> TextEdit {
        let el = &self.elements[id];
        if el.self_closing {
            let tag = &self.text[el.tag.clone()];
            let slash = tag.rfind('/').unwrap_or(tag.len() - 1);
            let open = tag[..slash].trim_end();
            return TextEdit::new(el.tag.start, el.tag.end, format!("{open}>{}</{}>", escape_text(value), el.name));
        }
        TextEdit::new(el.content.start, el.content.end, escape_text(value))
    }

    /// Inserts `fragment` (one or more lines without the outer indentation,
    /// separated by `\n`) as the last child of `parent`, or right after
    /// `after` when given. Indentation follows the document.
    pub fn insert_child(&self, parent: Id, after: Option<Id>, fragment: &str) -> TextEdit {
        let step = self.indent_step();
        let child_indent = format!("{}{step}", self.indent_of(parent));
        // Empty lines stay empty: VS leaves no spaces on blank lines.
        let body: String = fragment.lines().map(|l| if l.is_empty() { "\n".to_owned() } else { format!("{child_indent}{l}\n") }).collect();
        let el = &self.elements[parent];
        if let Some(prev) = after {
            // After the previous sibling's line.
            let end = self.elements[prev].span.end;
            let line_end = self.text[end..].find('\n').map_or(self.text.len(), |i| end + i + 1);
            return TextEdit::insert(line_end, body);
        }
        if el.self_closing {
            let tag = &self.text[el.tag.clone()];
            let slash = tag.rfind('/').unwrap_or(tag.len() - 1);
            let open = tag[..slash].trim_end();
            let outer = self.indent_of(parent);
            return TextEdit::new(el.tag.start, el.tag.end, format!("{open}>\n{body}{outer}</{}>", el.name));
        }
        // Before the end tag's line.
        let end_tag = el.content.end;
        let line_start = self.text[..end_tag].rfind('\n').map_or(0, |i| i + 1);
        if self.text[line_start..end_tag].trim().is_empty() {
            TextEdit::insert(line_start, body)
        } else {
            TextEdit::insert(end_tag, format!("\n{body}"))
        }
    }

    /// Removes an element with its own line (indentation and line break)
    /// when it stands alone on that line.
    pub fn remove(&self, id: Id) -> TextEdit {
        let span = self.elements[id].span.clone();
        let line_start = self.text[..span.start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = self.text[span.end..].find('\n').map_or(self.text.len(), |i| span.end + i + 1);
        let alone = self.text[line_start..span.start].trim().is_empty() && self.text[span.end..line_end].trim().is_empty();
        if alone { TextEdit::new(line_start, line_end, "") } else { TextEdit::new(span.start, span.end, "") }
    }
}

fn parse_attrs(text: &str, start: usize, end: usize) -> Vec<Attr> {
    let tag = &text[start..end];
    let bytes = tag.as_bytes();
    // Skip '<' and the element name.
    let mut i = 1;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' && bytes[i] != b'/' {
        i += 1;
    }
    let mut attrs = Vec::new();
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] == b'>' || bytes[i] == b'/' || bytes[i] == b'?' {
            break;
        }
        let name_start = i;
        while i < bytes.len() && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let name = tag[name_start..i].to_owned();
        while i < bytes.len() && (bytes[i] == b'=' || bytes[i].is_ascii_whitespace()) {
            i += 1;
        }
        let Some(&quote) = bytes.get(i) else { break };
        if quote != b'"' && quote != b'\'' {
            break;
        }
        let vstart = i + 1;
        let vend = tag[vstart..].find(char::from(quote)).map_or(tag.len(), |j| vstart + j);
        attrs.push(Attr { name, value: unescape(&tag[vstart..vend]), value_span: start + vstart..start + vend });
        i = vend + 1;
    }
    attrs
}

pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
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
        let entity = &tail[1..semi];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
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

pub fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::apply;

    const SLNX: &str = "<Solution>\n  <Project Path=\"a/a.csproj\" />\n</Solution>\n";

    #[test]
    fn spans_and_attributes() {
        let d = Doc::parse(SLNX).unwrap();
        let p = d.child(0, "Project").unwrap();
        assert_eq!(d.attr(p, "Path"), Some("a/a.csproj"));
        assert!(d.el(p).self_closing);
        assert_eq!(&d.text[d.el(p).span.clone()], "<Project Path=\"a/a.csproj\" />");
        assert_eq!(d.indent_of(p), "  ");
    }

    #[test]
    fn set_attr_changes_only_the_value() {
        let d = Doc::parse(SLNX).unwrap();
        let p = d.child(0, "Project").unwrap();
        let out = apply(SLNX, &[d.set_attr(p, "Path", "b & c.csproj")]);
        assert_eq!(out, "<Solution>\n  <Project Path=\"b &amp; c.csproj\" />\n</Solution>\n");
        let out = apply(SLNX, &[d.set_attr(p, "Id", "x")]);
        assert_eq!(out, "<Solution>\n  <Project Path=\"a/a.csproj\" Id=\"x\" />\n</Solution>\n");
    }

    #[test]
    fn insert_and_remove_children_keep_indentation() {
        let d = Doc::parse(SLNX).unwrap();
        let out = apply(SLNX, &[d.insert_child(0, None, "<Project Path=\"b.csproj\" />")]);
        assert_eq!(out, "<Solution>\n  <Project Path=\"a/a.csproj\" />\n  <Project Path=\"b.csproj\" />\n</Solution>\n");
        let d2 = Doc::parse(&out).unwrap();
        let first = d2.child(0, "Project").unwrap();
        assert_eq!(apply(&out, &[d2.remove(first)]), "<Solution>\n  <Project Path=\"b.csproj\" />\n</Solution>\n");
    }

    #[test]
    fn set_text_and_inner_text() {
        let src = "<A>\n  <B>x &amp; y</B>\n  <C />\n</A>";
        let d = Doc::parse(src).unwrap();
        let b = d.child(0, "B").unwrap();
        assert_eq!(d.inner_text(b), "x & y");
        let c = d.child(0, "C").unwrap();
        assert_eq!(apply(src, &[d.set_text(c, "1<2")]), "<A>\n  <B>x &amp; y</B>\n  <C>1&lt;2</C>\n</A>");
    }

    #[test]
    fn self_closing_parent_opens() {
        let src = "<A>\n  <B />\n</A>";
        let d = Doc::parse(src).unwrap();
        let b = d.child(0, "B").unwrap();
        assert_eq!(apply(src, &[d.insert_child(b, None, "<C />")]), "<A>\n  <B>\n    <C />\n  </B>\n</A>");
    }

    #[test]
    fn declaration_and_bom_free_text() {
        let src = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<Root a='1'><X/></Root>";
        let d = Doc::parse(src).unwrap();
        assert_eq!(d.el(0).name, "Root");
        assert_eq!(d.attr(0, "a"), Some("1"));
    }
}
