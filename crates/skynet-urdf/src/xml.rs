//! A small, strict XML reader.
//!
//! Written rather than depended on for the workspace's stated reason: no third-party dependencies,
//! so a vendor can build a driver toolchain in 2036 with no network. The cost is a few hundred
//! lines; the alternative is a transitive dependency tree under a tool whose entire job is to be
//! trustworthy about a robot's joint limits.
//!
//! It reads the subset URDF actually uses — elements, attributes, text, comments, CDATA, an XML
//! declaration and a DOCTYPE it skips — and refuses everything else with a position. It is not a
//! general XML processor: no namespace resolution, no entity declarations, no XInclude. A file
//! needing any of those is a file that has not been through `xacro` yet, and [`super::model`] says
//! so in those words.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// One element, with its attributes, children and direct text.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    /// Tag name, verbatim — a prefixed name like `xacro:property` keeps its prefix, because the
    /// prefix is exactly the thing [`super::model`] needs to see in order to refuse the file.
    pub name: String,
    /// Attributes in document order. A duplicate attribute is an error, so order is the only
    /// property that matters and a map would lose it for nothing.
    pub attributes: Vec<(String, String)>,
    /// Child elements, in document order.
    pub children: Vec<Element>,
    /// Concatenated text directly inside this element, entities already resolved.
    pub text: String,
}

impl Element {
    /// The value of an attribute, or `None`.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The first child with this tag name.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|child| child.name == name)
    }

    /// Every child with this tag name.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |child| child.name == name)
    }
}

/// A parse failure, with enough position to open the file at the right place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlError {
    /// What went wrong, phrased for whoever has to fix the file.
    pub message: String,
    /// 1-based line.
    pub line: usize,
    /// 1-based column.
    pub column: usize,
}

impl fmt::Display for XmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "line {}, column {}: {}",
            self.line, self.column, self.message
        )
    }
}

/// Parse a document and return its root element.
pub fn parse(input: &str) -> Result<Element, XmlError> {
    Parser::new(input).document()
}

struct Parser<'a> {
    bytes: &'a [u8],
    input: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            bytes: input.as_bytes(),
            input,
            pos: 0,
        }
    }

    fn error<T>(&self, at: usize, message: impl Into<String>) -> Result<T, XmlError> {
        let (line, column) = self.line_column(at);
        Err(XmlError {
            message: message.into(),
            line,
            column,
        })
    }

    fn line_column(&self, at: usize) -> (usize, usize) {
        let upto = &self.input[..at.min(self.input.len())];
        let line = upto.matches('\n').count() + 1;
        let column = upto
            .rsplit('\n')
            .next()
            .map(str::chars)
            .map_or(1, |c| c.count() + 1);
        (line, column)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn starts_with(&self, prefix: &str) -> bool {
        self.input[self.pos..].starts_with(prefix)
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.pos += 1;
        }
    }

    /// Skip everything that can legally sit between elements at the top level: whitespace, an XML
    /// declaration, comments, processing instructions and a DOCTYPE.
    fn skip_prolog_noise(&mut self) -> Result<(), XmlError> {
        loop {
            self.skip_whitespace();
            if self.starts_with("<!--") {
                self.skip_comment()?;
            } else if self.starts_with("<?") {
                self.skip_until("?>", "an unterminated `<?…?>`")?;
            } else if self.starts_with("<!DOCTYPE") {
                self.skip_doctype()?;
            } else {
                return Ok(());
            }
        }
    }

    fn skip_comment(&mut self) -> Result<(), XmlError> {
        self.skip_until("-->", "an unterminated comment; `<!--` with no `-->`")
    }

    fn skip_until(&mut self, terminator: &str, complaint: &str) -> Result<(), XmlError> {
        let start = self.pos;
        match self.input[self.pos..].find(terminator) {
            Some(offset) => {
                self.pos += offset + terminator.len();
                Ok(())
            }
            None => self.error(start, complaint.to_string()),
        }
    }

    /// A DOCTYPE may carry a bracketed internal subset, so it cannot be skipped by finding the
    /// next `>`. URDF never needs one; this exists so a file carrying one is skipped rather than
    /// misreported as a syntax error somewhere far away.
    fn skip_doctype(&mut self) -> Result<(), XmlError> {
        let start = self.pos;
        self.pos += "<!DOCTYPE".len();
        let mut depth = 0usize;
        while let Some(byte) = self.peek() {
            self.pos += 1;
            match byte {
                b'[' => depth += 1,
                b']' => depth = depth.saturating_sub(1),
                b'>' if depth == 0 => return Ok(()),
                _ => {}
            }
        }
        self.error(start, "an unterminated `<!DOCTYPE`")
    }

    fn document(mut self) -> Result<Element, XmlError> {
        self.skip_prolog_noise()?;
        if self.peek().is_none() {
            return self.error(self.pos, "the file contains no XML element");
        }
        let root = self.element()?;
        self.skip_prolog_noise()?;
        if self.pos < self.bytes.len() {
            return self.error(
                self.pos,
                "content after the root element closed; an XML document has exactly one root",
            );
        }
        Ok(root)
    }

    fn element(&mut self) -> Result<Element, XmlError> {
        let open_at = self.pos;
        if self.peek() != Some(b'<') {
            return self.error(self.pos, "expected an element to start here");
        }
        self.pos += 1;
        let name = self.name()?;
        if name.is_empty() {
            return self.error(open_at, "an element with no tag name");
        }

        let mut attributes: Vec<(String, String)> = Vec::new();
        loop {
            self.skip_whitespace();
            match self.peek() {
                Some(b'/') => {
                    self.pos += 1;
                    if self.peek() != Some(b'>') {
                        return self.error(self.pos, "expected `>` after `/`");
                    }
                    self.pos += 1;
                    return Ok(Element {
                        name,
                        attributes,
                        children: Vec::new(),
                        text: String::new(),
                    });
                }
                Some(b'>') => {
                    self.pos += 1;
                    break;
                }
                Some(_) => {
                    let at = self.pos;
                    let key = self.name()?;
                    if key.is_empty() {
                        return self.error(at, "expected an attribute name, `>` or `/>`");
                    }
                    self.skip_whitespace();
                    if self.peek() != Some(b'=') {
                        return self.error(
                            self.pos,
                            format!("attribute `{key}` has no value; XML requires `{key}=\"…\"`"),
                        );
                    }
                    self.pos += 1;
                    self.skip_whitespace();
                    let value = self.attribute_value(&key)?;
                    if attributes.iter().any(|(existing, _)| *existing == key) {
                        return self.error(at, format!("attribute `{key}` appears twice"));
                    }
                    attributes.push((key, value));
                }
                None => return self.error(open_at, format!("`<{name}` is never closed")),
            }
        }

        let mut children = Vec::new();
        let mut text = String::new();
        loop {
            if self.pos >= self.bytes.len() {
                return self.error(open_at, format!("`<{name}>` is never closed"));
            }
            if self.starts_with("</") {
                let at = self.pos;
                self.pos += 2;
                let close = self.name()?;
                self.skip_whitespace();
                if self.peek() != Some(b'>') {
                    return self.error(self.pos, format!("expected `>` closing `</{close}`"));
                }
                self.pos += 1;
                if close != name {
                    return self.error(
                        at,
                        format!("`</{close}>` closes `<{name}>`; tags must nest"),
                    );
                }
                return Ok(Element {
                    name,
                    attributes,
                    children,
                    text,
                });
            }
            if self.starts_with("<!--") {
                self.skip_comment()?;
            } else if self.starts_with("<![CDATA[") {
                let start = self.pos + "<![CDATA[".len();
                self.skip_until("]]>", "an unterminated `<![CDATA[`")?;
                text.push_str(&self.input[start..self.pos - "]]>".len()]);
            } else if self.starts_with("<?") {
                self.skip_until("?>", "an unterminated `<?…?>`")?;
            } else if self.peek() == Some(b'<') {
                children.push(self.element()?);
            } else {
                let start = self.pos;
                while let Some(byte) = self.peek() {
                    if byte == b'<' {
                        break;
                    }
                    self.pos += 1;
                }
                text.push_str(&self.unescape(&self.input[start..self.pos], start)?);
            }
        }
    }

    fn name(&mut self) -> Result<String, XmlError> {
        let start = self.pos;
        while let Some(byte) = self.peek() {
            let ok = byte.is_ascii_alphanumeric()
                || matches!(byte, b'_' | b'-' | b'.' | b':')
                || byte >= 0x80;
            if !ok {
                break;
            }
            self.pos += 1;
        }
        Ok(self.input[start..self.pos].to_string())
    }

    fn attribute_value(&mut self, key: &str) -> Result<String, XmlError> {
        let quote = match self.peek() {
            Some(q @ (b'"' | b'\'')) => q,
            _ => return self.error(self.pos, format!("the value of `{key}` must be quoted")),
        };
        self.pos += 1;
        let start = self.pos;
        while let Some(byte) = self.peek() {
            if byte == quote {
                let raw = &self.input[start..self.pos];
                self.pos += 1;
                return self.unescape(raw, start);
            }
            self.pos += 1;
        }
        self.error(start, format!("the value of `{key}` is never closed"))
    }

    /// Resolve the five predefined entities and numeric character references.
    ///
    /// Anything else — a `&foo;` from a DTD — is refused rather than passed through, because a
    /// joint named `left&shoulder;` silently becoming the literal text `left&shoulder;` is a name
    /// that will not match anything and will not explain why.
    fn unescape(&self, raw: &str, offset: usize) -> Result<String, XmlError> {
        if !raw.contains('&') {
            return Ok(raw.to_string());
        }
        let mut out = String::with_capacity(raw.len());
        let mut rest = raw;
        let mut consumed = 0usize;
        while let Some(amp) = rest.find('&') {
            out.push_str(&rest[..amp]);
            let tail = &rest[amp..];
            let Some(semi) = tail.find(';') else {
                return self.error(offset + consumed + amp, "an `&` with no `;`; write `&amp;`");
            };
            let entity = &tail[1..semi];
            match entity {
                "lt" => out.push('<'),
                "gt" => out.push('>'),
                "amp" => out.push('&'),
                "quot" => out.push('"'),
                "apos" => out.push('\''),
                _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                    let code = u32::from_str_radix(&entity[2..], 16).ok();
                    match code.and_then(char::from_u32) {
                        Some(ch) => out.push(ch),
                        None => {
                            return self.error(
                                offset + consumed + amp,
                                format!("`&{entity};` is not a character"),
                            )
                        }
                    }
                }
                _ if entity.starts_with('#') => {
                    let code = entity[1..].parse::<u32>().ok();
                    match code.and_then(char::from_u32) {
                        Some(ch) => out.push(ch),
                        None => {
                            return self.error(
                                offset + consumed + amp,
                                format!("`&{entity};` is not a character"),
                            )
                        }
                    }
                }
                _ => {
                    return self.error(
                        offset + consumed + amp,
                        format!(
                            "unknown entity `&{entity};`. Only the five predefined entities and \
                             numeric references are resolved; a file relying on DTD entities has \
                             not been through a preprocessor yet."
                        ),
                    )
                }
            }
            consumed += amp + semi + 1;
            rest = &tail[semi + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn elements_attributes_children_and_text() {
        let doc =
            parse(r#"<robot name="arm"><link name="base"/><joint>text</joint></robot>"#).unwrap();
        assert_eq!(doc.name, "robot");
        assert_eq!(doc.attr("name"), Some("arm"));
        assert_eq!(doc.children.len(), 2);
        assert_eq!(doc.child("link").unwrap().attr("name"), Some("base"));
        assert_eq!(doc.child("joint").unwrap().text, "text");
    }

    #[test]
    fn the_prolog_and_comments_are_skipped() {
        let doc = parse(
            "<?xml version=\"1.0\"?>\n<!-- a comment -->\n<!DOCTYPE robot [ <!ELEMENT x> ]>\n\
             <robot><!-- inside --><link/></robot>\n<!-- trailing -->",
        )
        .unwrap();
        assert_eq!(doc.name, "robot");
        assert_eq!(doc.children.len(), 1);
    }

    #[test]
    fn cdata_is_text() {
        let doc = parse("<a><![CDATA[<not markup>]]></a>").unwrap();
        assert_eq!(doc.text, "<not markup>");
    }

    #[test]
    fn entities_resolve_and_unknown_ones_are_refused() {
        let doc = parse(r#"<a name="x &amp; y &#65;&#x42;"/>"#).unwrap();
        assert_eq!(doc.attr("name"), Some("x & y AB"));

        let err = parse(r#"<a name="&nbsp;"/>"#).unwrap_err();
        assert!(
            err.message.contains("unknown entity `&nbsp;`"),
            "{}",
            err.message
        );
    }

    /// Every refusal names the fix (§64), and a position is part of naming it — "line 3, column 12"
    /// is actionable in a way "invalid XML" is not.
    #[test]
    fn failures_carry_a_position_and_say_what_is_wrong() {
        let err = parse("<robot>\n  <link>\n</robot>").unwrap_err();
        assert_eq!(err.line, 3);
        assert!(err.message.contains("tags must nest"), "{}", err.message);

        let err = parse("<a><b/>").unwrap_err();
        assert!(err.message.contains("never closed"), "{}", err.message);

        let err = parse("<a x/>").unwrap_err();
        assert!(err.message.contains("has no value"), "{}", err.message);

        let err = parse("<a/><b/>").unwrap_err();
        assert!(err.message.contains("exactly one root"), "{}", err.message);

        let err = parse("   \n  ").unwrap_err();
        assert!(err.message.contains("no XML element"), "{}", err.message);
    }

    /// A duplicated attribute is the sort of thing a generator emits and a lenient parser hides.
    /// Two `lower=` values on one limit is a joint limit nobody can predict.
    #[test]
    fn a_duplicated_attribute_is_an_error() {
        let err = parse(r#"<limit lower="-1" lower="-2"/>"#).unwrap_err();
        assert!(err.message.contains("appears twice"), "{}", err.message);
    }

    #[test]
    fn prefixed_names_survive_so_the_model_can_refuse_them() {
        let doc =
            parse("<robot xmlns:xacro=\"http://ros.org/wiki/xacro\"><xacro:property/></robot>")
                .unwrap();
        assert_eq!(doc.children[0].name, "xacro:property");
        assert!(doc.attr("xmlns:xacro").is_some());
    }

    #[test]
    fn children_named_finds_all_of_them() {
        let doc = parse("<robot><joint name=\"a\"/><link/><joint name=\"b\"/></robot>").unwrap();
        let names: Vec<&str> = doc
            .children_named("joint")
            .filter_map(|joint| joint.attr("name"))
            .collect();
        assert_eq!(names, vec!["a", "b"]);
    }
}
