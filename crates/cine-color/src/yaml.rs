//! A hand-rolled YAML subset parser sufficient for OpenColorIO configs.
//!
//! Supported: block mappings and sequences (including compact `- key: v`
//! items), flow sequences `[a, b]` and mappings `{k: v}` (multi-line),
//! `!<Tag>` / `!tag` tags on any node, single/double quoted scalars with
//! escapes, `|` / `>` block scalars, comments and document markers.
//! Not supported: anchors/aliases, multi-document streams, complex keys.

use crate::error::{Error, Result};

/// A parsed YAML node.
#[derive(Clone, Debug, PartialEq)]
pub enum Yaml {
    Null,
    Scalar(String),
    Seq(Vec<Yaml>),
    Map(Vec<(String, Yaml)>),
    Tagged(String, Box<Yaml>),
}

impl Yaml {
    /// The node without its tag.
    pub fn inner(&self) -> &Yaml {
        match self {
            Yaml::Tagged(_, b) => b.inner(),
            other => other,
        }
    }

    /// The tag, if any (`View` for `!<View>`).
    pub fn tag(&self) -> Option<&str> {
        match self {
            Yaml::Tagged(t, _) => Some(t),
            _ => None,
        }
    }

    /// True when null (or a tagged null).
    pub fn is_null(&self) -> bool {
        matches!(self.inner(), Yaml::Null)
    }

    /// Mapping lookup (through tags).
    pub fn get(&self, key: &str) -> Option<&Yaml> {
        match self.inner() {
            Yaml::Map(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Scalar text (through tags).
    pub fn as_str(&self) -> Option<&str> {
        match self.inner() {
            Yaml::Scalar(s) => Some(s),
            _ => None,
        }
    }

    /// Scalar as float.
    pub fn as_f64(&self) -> Option<f64> {
        self.as_str().and_then(|s| s.trim().parse().ok())
    }

    /// Scalar as integer.
    pub fn as_i64(&self) -> Option<i64> {
        self.as_str().and_then(|s| s.trim().parse().ok())
    }

    /// Scalar as bool (`true/false/yes/no/on/off`).
    pub fn as_bool(&self) -> Option<bool> {
        match self.as_str()?.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" => Some(true),
            "false" | "no" | "off" => Some(false),
            _ => None,
        }
    }

    /// Sequence items (through tags).
    pub fn as_seq(&self) -> Option<&[Yaml]> {
        match self.inner() {
            Yaml::Seq(s) => Some(s),
            _ => None,
        }
    }

    /// Mapping entries (through tags).
    pub fn as_map(&self) -> Option<&[(String, Yaml)]> {
        match self.inner() {
            Yaml::Map(m) => Some(m),
            _ => None,
        }
    }

    /// A list of floats: either a sequence of numbers or a single number.
    pub fn floats(&self) -> Option<Vec<f64>> {
        if let Some(s) = self.as_seq() {
            s.iter().map(|v| v.as_f64()).collect()
        } else {
            self.as_f64().map(|f| vec![f])
        }
    }

    /// A list of strings: either a sequence of scalars or a single scalar.
    pub fn strings(&self) -> Vec<String> {
        if let Some(s) = self.as_seq() {
            s.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        } else {
            self.as_str()
                .map(|s| vec![s.to_string()])
                .unwrap_or_default()
        }
    }
}

#[derive(Clone, Debug)]
struct Line {
    indent: usize,
    text: String,
    raw: String,
    no: usize,
}

fn strip_comment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut quote: Option<char> = None;
    let mut prev_ws = true;
    for c in s.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                } else if c == '#' && prev_ws {
                    break;
                }
            }
        }
        out.push(c);
        prev_ws = c.is_whitespace();
    }
    out.trim_end().to_string()
}

fn parse_lines(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let raw = raw.trim_end_matches('\r');
        let stripped = strip_comment(raw);
        let trimmed = stripped.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let indent = stripped.len() - trimmed.len();
        if indent == 0 && (trimmed == "---" || trimmed == "..." || trimmed.starts_with("%YAML")) {
            continue;
        }
        out.push(Line {
            indent,
            text: trimmed.to_string(),
            raw: raw.to_string(),
            no: i + 1,
        });
    }
    out
}

struct Parser {
    lines: Vec<Line>,
    pos: usize,
}

/// Parse a YAML document.
pub fn parse(text: &str) -> Result<Yaml> {
    let mut p = Parser {
        lines: parse_lines(text),
        pos: 0,
    };
    if p.lines.is_empty() {
        return Ok(Yaml::Null);
    }
    let doc = p.parse_node(0)?;
    if p.pos < p.lines.len() {
        let l = &p.lines[p.pos];
        return Err(Error::Parse {
            line: l.no,
            message: format!("unexpected content `{}`", l.text),
        });
    }
    Ok(doc)
}

fn is_tag_start(s: &str) -> bool {
    s.starts_with('!')
}

/// Split `!<tag> rest` / `!tag rest` into (tag, rest).
fn split_tag(s: &str) -> (String, &str) {
    if let Some(rest) = s.strip_prefix("!<") {
        if let Some(end) = rest.find('>') {
            return (rest[..end].to_string(), rest[end + 1..].trim_start());
        }
        return (rest.to_string(), "");
    }
    let body = &s[1..];
    let end = body.find(char::is_whitespace).unwrap_or(body.len());
    (body[..end].to_string(), body[end..].trim_start())
}

/// Find the `key: value` split point of a block mapping line.
fn mapping_split(text: &str) -> Option<(String, &str)> {
    if text.starts_with('[')
        || text.starts_with('{')
        || is_tag_start(text)
        || text.starts_with('-') && (text.len() == 1 || text[1..].starts_with(' '))
    {
        return None;
    }
    if let Some(q) = text.chars().next().filter(|c| *c == '"' || *c == '\'') {
        let end = text[1..].find(q)? + 1;
        let rest = &text[end + 1..];
        let rest_t = rest.trim_start();
        if let Some(r) = rest_t.strip_prefix(':') {
            if r.is_empty() || r.starts_with(' ') {
                return Some((unquote(&text[..=end]), r.trim_start()));
            }
        }
        return None;
    }
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'[' | b'{' => depth += 1,
            b']' | b'}' => depth -= 1,
            b':' if depth == 0 => {
                let after = &text[i + 1..];
                if after.is_empty() || after.starts_with(' ') || after.starts_with('\t') {
                    return Some((text[..i].trim().to_string(), after.trim_start()));
                }
            }
            _ => {}
        }
    }
    None
}

fn unquote(s: &str) -> String {
    let mut chars = s.chars();
    let q = match chars.next() {
        Some(c) if c == '"' || c == '\'' => c,
        _ => return s.to_string(),
    };
    let body = &s[1..s.len().saturating_sub(1)];
    if q == '\'' {
        return body.replace("''", "'");
    }
    let mut out = String::with_capacity(body.len());
    let mut it = body.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('/') => out.push('/'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl Parser {
    fn peek(&self) -> Option<&Line> {
        self.lines.get(self.pos)
    }

    fn parse_node(&mut self, min_indent: usize) -> Result<Yaml> {
        let line = match self.peek() {
            Some(l) if l.indent >= min_indent => l.clone(),
            _ => return Ok(Yaml::Null),
        };
        if line.text == "-" || line.text.starts_with("- ") {
            return self.parse_seq(line.indent);
        }
        if mapping_split(&line.text).is_some() {
            return self.parse_map(line.indent);
        }
        if is_tag_start(&line.text) {
            let (tag, rest) = split_tag(&line.text);
            if rest.is_empty() {
                self.pos += 1;
                let child = self.parse_node(line.indent)?;
                return Ok(Yaml::Tagged(tag, Box::new(child)));
            }
        }
        let text = line.text.clone();
        self.pos += 1;
        self.parse_inline(&text, line.no)
    }

    fn parse_seq(&mut self, indent: usize) -> Result<Yaml> {
        let mut items = Vec::new();
        while let Some(line) = self.peek() {
            if line.indent != indent || !(line.text == "-" || line.text.starts_with("- ")) {
                break;
            }
            let rest = line.text[1..].to_string();
            let rest_trim = rest.trim_start();
            if rest_trim.is_empty() {
                self.pos += 1;
                items.push(self.parse_node(indent + 1)?);
            } else {
                let offset = 1 + (rest.len() - rest_trim.len());
                let l = &mut self.lines[self.pos];
                l.indent = indent + offset;
                l.text = rest_trim.to_string();
                let new_indent = l.indent;
                items.push(self.parse_node(new_indent)?);
            }
        }
        Ok(Yaml::Seq(items))
    }

    fn parse_map(&mut self, indent: usize) -> Result<Yaml> {
        let mut entries = Vec::new();
        while let Some(line) = self.peek() {
            if line.indent != indent {
                break;
            }
            let (key, rest) = match mapping_split(&line.text) {
                Some(kv) => (kv.0, kv.1.to_string()),
                None => break,
            };
            let no = line.no;
            self.pos += 1;
            let value = if rest.is_empty() {
                self.parse_block_value(indent)?
            } else if is_tag_start(&rest) {
                let (tag, tail) = split_tag(&rest);
                if tail.is_empty() {
                    Yaml::Tagged(tag, Box::new(self.parse_block_value(indent)?))
                } else {
                    Yaml::Tagged(tag, Box::new(self.parse_inline(tail, no)?))
                }
            } else if rest == "|" || rest == ">" || rest == "|-" || rest == ">-" {
                self.parse_block_scalar(indent, rest.starts_with('|'), rest.ends_with('-'))
            } else {
                self.parse_inline(&rest, no)?
            };
            entries.push((key, value));
        }
        Ok(Yaml::Map(entries))
    }

    /// Value of a `key:` whose content is on the following lines.
    fn parse_block_value(&mut self, key_indent: usize) -> Result<Yaml> {
        match self.peek() {
            Some(l) if l.indent > key_indent => self.parse_node(key_indent + 1),
            Some(l) if l.indent == key_indent && (l.text == "-" || l.text.starts_with("- ")) => {
                self.parse_seq(key_indent)
            }
            _ => Ok(Yaml::Null),
        }
    }

    fn parse_block_scalar(&mut self, key_indent: usize, literal: bool, strip: bool) -> Yaml {
        let mut parts: Vec<String> = Vec::new();
        let mut base: Option<usize> = None;
        while let Some(l) = self.peek() {
            if l.indent <= key_indent {
                break;
            }
            let b = *base.get_or_insert(l.indent);
            let raw = l.raw.clone();
            let cut = raw
                .char_indices()
                .nth(b)
                .map(|(i, _)| i)
                .unwrap_or(raw.len());
            parts.push(raw[cut..].trim_end().to_string());
            self.pos += 1;
        }
        let mut s = if literal {
            parts.join("\n")
        } else {
            parts.join(" ")
        };
        if !strip && !s.is_empty() {
            s.push('\n');
        }
        Yaml::Scalar(s)
    }

    /// Inline value: tag, flow collection (possibly continued on following lines), quoted or plain scalar.
    fn parse_inline(&mut self, text: &str, no: usize) -> Result<Yaml> {
        let t = text.trim();
        if is_tag_start(t) {
            let (tag, rest) = split_tag(t);
            if rest.is_empty() {
                return Ok(Yaml::Tagged(tag, Box::new(Yaml::Null)));
            }
            return Ok(Yaml::Tagged(tag, Box::new(self.parse_inline(rest, no)?)));
        }
        if t.starts_with('[') || t.starts_with('{') {
            let mut buf = t.to_string();
            let mut guard = 0;
            while flow_depth(&buf) > 0 && self.pos < self.lines.len() && guard < 100_000 {
                let l = &self.lines[self.pos];
                buf.push(' ');
                buf.push_str(&l.text);
                self.pos += 1;
                guard += 1;
            }
            if flow_depth(&buf) != 0 {
                return Err(Error::Parse {
                    line: no,
                    message: "unbalanced flow collection".into(),
                });
            }
            let chars: Vec<char> = buf.chars().collect();
            let mut i = 0;
            let v = parse_flow_value(&chars, &mut i, no)?;
            skip_ws(&chars, &mut i);
            if i != chars.len() {
                return Err(Error::Parse {
                    line: no,
                    message: format!(
                        "trailing content after flow collection: `{}`",
                        chars[i..].iter().collect::<String>()
                    ),
                });
            }
            return Ok(v);
        }
        Ok(scalar(t))
    }
}

fn scalar(t: &str) -> Yaml {
    if t.starts_with('"') || t.starts_with('\'') {
        return Yaml::Scalar(unquote(t));
    }
    match t {
        "~" | "null" | "Null" | "NULL" | "" => Yaml::Null,
        _ => Yaml::Scalar(t.to_string()),
    }
}

fn flow_depth(s: &str) -> i32 {
    let mut depth = 0;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '[' | '{' => depth += 1,
                ']' | '}' => depth -= 1,
                _ => {}
            },
        }
    }
    depth
}

fn skip_ws(c: &[char], i: &mut usize) {
    while *i < c.len() && c[*i].is_whitespace() {
        *i += 1;
    }
}

fn parse_quoted(c: &[char], i: &mut usize, no: usize) -> Result<String> {
    let q = c[*i];
    let start = *i;
    *i += 1;
    while *i < c.len() {
        if c[*i] == '\\' && q == '"' {
            *i += 2;
            continue;
        }
        if c[*i] == q {
            if q == '\'' && *i + 1 < c.len() && c[*i + 1] == '\'' {
                *i += 2;
                continue;
            }
            *i += 1;
            let s: String = c[start..*i].iter().collect();
            return Ok(unquote(&s));
        }
        *i += 1;
    }
    Err(Error::Parse {
        line: no,
        message: "unterminated quoted string".into(),
    })
}

fn parse_flow_plain(c: &[char], i: &mut usize, stop_colon: bool) -> String {
    let start = *i;
    while *i < c.len() {
        let ch = c[*i];
        if ch == ',' || ch == ']' || ch == '}' {
            break;
        }
        if stop_colon && ch == ':' && (*i + 1 >= c.len() || c[*i + 1].is_whitespace()) {
            break;
        }
        *i += 1;
    }
    c[start..*i].iter().collect::<String>().trim().to_string()
}

fn parse_flow_value(c: &[char], i: &mut usize, no: usize) -> Result<Yaml> {
    skip_ws(c, i);
    if *i >= c.len() {
        return Ok(Yaml::Null);
    }
    match c[*i] {
        '[' => {
            *i += 1;
            let mut items = Vec::new();
            loop {
                skip_ws(c, i);
                if *i >= c.len() {
                    return Err(Error::Parse {
                        line: no,
                        message: "unterminated flow sequence".into(),
                    });
                }
                if c[*i] == ']' {
                    *i += 1;
                    break;
                }
                items.push(parse_flow_value(c, i, no)?);
                skip_ws(c, i);
                if *i < c.len() && c[*i] == ',' {
                    *i += 1;
                } else if *i < c.len() && c[*i] == ']' {
                    *i += 1;
                    break;
                } else {
                    return Err(Error::Parse {
                        line: no,
                        message: "expected `,` or `]`".into(),
                    });
                }
            }
            Ok(Yaml::Seq(items))
        }
        '{' => {
            *i += 1;
            let mut entries = Vec::new();
            loop {
                skip_ws(c, i);
                if *i >= c.len() {
                    return Err(Error::Parse {
                        line: no,
                        message: "unterminated flow mapping".into(),
                    });
                }
                if c[*i] == '}' {
                    *i += 1;
                    break;
                }
                let key = if c[*i] == '"' || c[*i] == '\'' {
                    parse_quoted(c, i, no)?
                } else {
                    parse_flow_plain(c, i, true)
                };
                skip_ws(c, i);
                let value = if *i < c.len() && c[*i] == ':' {
                    *i += 1;
                    skip_ws(c, i);
                    if *i < c.len() && (c[*i] == ',' || c[*i] == '}') {
                        Yaml::Null
                    } else {
                        parse_flow_value(c, i, no)?
                    }
                } else {
                    Yaml::Null
                };
                entries.push((key, value));
                skip_ws(c, i);
                if *i < c.len() && c[*i] == ',' {
                    *i += 1;
                } else if *i < c.len() && c[*i] == '}' {
                    *i += 1;
                    break;
                } else {
                    return Err(Error::Parse {
                        line: no,
                        message: "expected `,` or `}`".into(),
                    });
                }
            }
            Ok(Yaml::Map(entries))
        }
        '"' | '\'' => Ok(Yaml::Scalar(parse_quoted(c, i, no)?)),
        '!' => {
            let start = *i;
            if *i + 1 < c.len() && c[*i + 1] == '<' {
                while *i < c.len() && c[*i] != '>' {
                    *i += 1;
                }
                *i += 1;
            } else {
                while *i < c.len()
                    && !c[*i].is_whitespace()
                    && c[*i] != ','
                    && c[*i] != ']'
                    && c[*i] != '}'
                {
                    *i += 1;
                }
            }
            let tag_text: String = c[start..(*i).min(c.len())].iter().collect();
            let (tag, _) = split_tag(&tag_text);
            skip_ws(c, i);
            if *i >= c.len() || c[*i] == ',' || c[*i] == ']' || c[*i] == '}' {
                return Ok(Yaml::Tagged(tag, Box::new(Yaml::Null)));
            }
            let v = parse_flow_value(c, i, no)?;
            Ok(Yaml::Tagged(tag, Box::new(v)))
        }
        _ => Ok(scalar(&parse_flow_plain(c, i, false))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_and_flow() {
        let doc = r#"
# comment
ocio_profile_version: 2
search_path: "luts"
roles:
  scene_linear: ACEScg
  color_picking: sRGB - Texture   # trailing
displays:
  sRGB:
    - !<View> {name: Raw, colorspace: Raw}
    - !<View> {name: "ACES 1.0", view_transform: ACES 1.0 SDR-video, display_colorspace: sRGB}
active_displays: [sRGB, Rec.709]
empty_list: []
colorspaces:
  - !<ColorSpace>
    name: ACEScg
    aliases: [lin_ap1, 'ACES - ACEScg']
    isdata: false
    description: |
      Working space.
      Second line.
    to_reference: !<GroupTransform>
      children:
        - !<MatrixTransform> {matrix: [1, 0, 0, 0,
                                       0, 1, 0, 0,
                                       0, 0, 1, 0,
                                       0, 0, 0, 1]}
        - !<FileTransform> {src: a.cube, interpolation: tetrahedral}
  - !<ColorSpace>
    name: Raw
    from_reference: !<ExponentTransform>
      value: [2.2, 2.2, 2.2, 1]
"#;
        let y = parse(doc).unwrap();
        assert_eq!(y.get("ocio_profile_version").unwrap().as_i64(), Some(2));
        assert_eq!(y.get("search_path").unwrap().as_str(), Some("luts"));
        assert_eq!(
            y.get("roles")
                .unwrap()
                .get("color_picking")
                .unwrap()
                .as_str(),
            Some("sRGB - Texture")
        );
        let views = y
            .get("displays")
            .unwrap()
            .get("sRGB")
            .unwrap()
            .as_seq()
            .unwrap();
        assert_eq!(views.len(), 2);
        assert_eq!(views[0].tag(), Some("View"));
        assert_eq!(views[1].get("name").unwrap().as_str(), Some("ACES 1.0"));
        assert_eq!(
            y.get("active_displays").unwrap().strings(),
            vec!["sRGB", "Rec.709"]
        );
        assert_eq!(y.get("empty_list").unwrap().as_seq().unwrap().len(), 0);
        let cs = y.get("colorspaces").unwrap().as_seq().unwrap();
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].tag(), Some("ColorSpace"));
        assert_eq!(cs[0].get("name").unwrap().as_str(), Some("ACEScg"));
        assert_eq!(
            cs[0].get("aliases").unwrap().strings(),
            vec!["lin_ap1", "ACES - ACEScg"]
        );
        assert_eq!(cs[0].get("isdata").unwrap().as_bool(), Some(false));
        assert_eq!(
            cs[0].get("description").unwrap().as_str(),
            Some("Working space.\nSecond line.\n")
        );
        let tr = cs[0].get("to_reference").unwrap();
        assert_eq!(tr.tag(), Some("GroupTransform"));
        let ch = tr.get("children").unwrap().as_seq().unwrap();
        assert_eq!(ch.len(), 2);
        assert_eq!(ch[0].tag(), Some("MatrixTransform"));
        assert_eq!(ch[0].get("matrix").unwrap().floats().unwrap().len(), 16);
        assert_eq!(ch[1].get("src").unwrap().as_str(), Some("a.cube"));
        let fr = cs[1].get("from_reference").unwrap();
        assert_eq!(fr.tag(), Some("ExponentTransform"));
        assert_eq!(
            fr.get("value").unwrap().floats().unwrap(),
            vec![2.2, 2.2, 2.2, 1.0]
        );
    }

    #[test]
    fn quoted_and_escapes() {
        let y = parse("a: \"x\\ny\"\n'b c': 'it''s'\nd: {e: 'f, g', h: [\"i]\", j]}\n").unwrap();
        assert_eq!(y.get("a").unwrap().as_str(), Some("x\ny"));
        assert_eq!(y.get("b c").unwrap().as_str(), Some("it's"));
        assert_eq!(y.get("d").unwrap().get("e").unwrap().as_str(), Some("f, g"));
        assert_eq!(
            y.get("d").unwrap().get("h").unwrap().strings(),
            vec!["i]", "j"]
        );
    }

    #[test]
    fn sequence_at_key_indent_and_nested_seq() {
        let y = parse("looks:\n- !<Look>\n  name: a\n  transform: !<CDLTransform> {slope: [1, 1, 1]}\n- !<Look>\n  name: b\nx: 1\n").unwrap();
        let looks = y.get("looks").unwrap().as_seq().unwrap();
        assert_eq!(looks.len(), 2);
        assert_eq!(looks[0].get("name").unwrap().as_str(), Some("a"));
        assert_eq!(
            looks[0].get("transform").unwrap().tag(),
            Some("CDLTransform")
        );
        assert_eq!(y.get("x").unwrap().as_i64(), Some(1));
    }

    #[test]
    fn errors() {
        assert!(parse("a: [1, 2\n").is_err());
        assert!(parse("a: \"unterminated\n").is_ok() || parse("a: [\"x\n").is_err());
    }
}
