//! Final Draft (`.fdx`) XML parser and writer.
//!
//! The FDX body is a flat list of `<Paragraph Type="...">` elements under
//! `<FinalDraft><Content>`. Scene numbers live in the paragraph `Number`
//! attribute (and, informationally, in `<SceneProperties>`); dual dialogue is
//! a wrapper paragraph containing `<DualDialogue>` with the two blocks
//! inside; page breaks are `StartsNewPage="Yes"` on the following paragraph;
//! notes are `<ScriptNote>` children. Elements Final Draft has no paragraph
//! type for (sections, synopses, lyrics, boneyards) are written as `General`
//! paragraphs carrying a `Fountain="..."` attribute so they survive a round
//! trip; other tools simply see plain general text.

use crate::error::{Error, Result};
use crate::model::{split_character_cue, Element, Screenplay};
use crate::stats::page_eighths;
use roxmltree::{Document, Node};

/// Parse an FDX document.
pub fn parse_fdx(xml: &str) -> Result<Screenplay> {
    let doc = Document::parse(xml).map_err(|e| Error::Xml(e.to_string()))?;
    let root = doc.root_element();
    if root.tag_name().name() != "FinalDraft" {
        return Err(Error::InvalidDocument(format!(
            "expected <FinalDraft> root, found <{}>",
            root.tag_name().name()
        )));
    }
    let mut sp = Screenplay::new();
    let content = root
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "Content")
        .ok_or_else(|| Error::InvalidDocument("missing <Content>".into()))?;
    for p in content
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "Paragraph")
    {
        parse_paragraph(p, &mut sp.elements, false);
    }
    if let Some(tp) = root
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "TitlePage")
    {
        sp.title_page = parse_title_page(tp);
    }
    Ok(sp)
}

fn attr<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    n.attribute(name)
}

/// Concatenate every `<Text>` run of a paragraph (skipping ScriptNote bodies).
fn paragraph_text(p: Node) -> String {
    let mut s = String::new();
    for c in p.children() {
        if c.is_element() && c.tag_name().name() == "Text" {
            for t in c.descendants() {
                if t.is_text() {
                    s.push_str(t.text().unwrap_or(""));
                }
            }
        }
    }
    s
}

fn parse_paragraph(p: Node, out: &mut Vec<Element>, dual_second: bool) {
    if attr(p, "StartsNewPage").is_some_and(|v| v.eq_ignore_ascii_case("yes")) {
        out.push(Element::PageBreak);
    }
    // Notes attached to this paragraph come first.
    for note in p
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "ScriptNote")
    {
        let mut text = String::new();
        for np in note
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "Paragraph")
        {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&paragraph_text(np));
        }
        out.push(Element::Note {
            text: text.trim().to_string(),
        });
    }
    if let Some(dd) = p
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "DualDialogue")
    {
        let mut cues = 0;
        for inner in dd
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "Paragraph")
        {
            let is_cue = attr(inner, "Type").is_some_and(|t| t == "Character");
            if is_cue {
                cues += 1;
            }
            parse_paragraph(inner, out, is_cue && cues >= 2);
        }
        return;
    }
    let ty = attr(p, "Type").unwrap_or("General");
    let text = paragraph_text(p);
    let fountain = attr(p, "Fountain");
    match ty {
        "Scene Heading" => {
            let number = attr(p, "Number")
                .map(str::to_string)
                .or_else(|| {
                    p.children()
                        .find(|n| n.is_element() && n.tag_name().name() == "SceneProperties")
                        .and_then(|sp| attr(sp, "Number").map(str::to_string))
                })
                .filter(|n| !n.trim().is_empty());
            let (text, inline_number) = crate::fountain::split_scene_number(text.trim());
            out.push(Element::scene_heading(text, number.or(inline_number)));
        }
        "Action" => out.push(Element::Action { text }),
        "Character" => {
            let cue = text.trim();
            let cue = cue.strip_suffix('^').unwrap_or(cue).trim();
            let (name, extension) = split_character_cue(cue);
            out.push(Element::Character {
                name,
                extension,
                dual: dual_second,
            });
        }
        "Parenthetical" => out.push(Element::Parenthetical {
            text: text.trim().to_string(),
        }),
        "Dialogue" => out.push(Element::Dialogue { text }),
        "Transition" => out.push(Element::Transition {
            text: text.trim().to_string(),
        }),
        "Centered" => out.push(Element::Centered {
            text: text.trim().to_string(),
        }),
        "General" => match fountain {
            Some("section") => {
                let level = attr(p, "Level")
                    .and_then(|l| l.parse::<u8>().ok())
                    .unwrap_or(1);
                out.push(Element::Section {
                    level,
                    text: text.trim().to_string(),
                });
            }
            Some("synopsis") => out.push(Element::Synopsis {
                text: text.trim().to_string(),
            }),
            Some("note") => out.push(Element::Note {
                text: text.trim().to_string(),
            }),
            Some("lyric") => out.push(Element::Lyric { text }),
            Some("boneyard") => out.push(Element::Boneyard { text }),
            Some("page_break") => {}
            _ => {
                if !(text.is_empty() && attr(p, "StartsNewPage").is_some()) {
                    out.push(Element::Action { text });
                }
            }
        },
        // Shot, Cast List, New Act, End of Act and anything unknown: keep the text as action.
        _ => out.push(Element::Action { text }),
    }
}

fn parse_title_page(tp: Node) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let content = tp
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "Content");
    let paragraphs: Vec<Node> = content
        .map(|c| {
            c.children()
                .filter(|n| n.is_element() && n.tag_name().name() == "Paragraph")
                .collect()
        })
        .unwrap_or_default();
    let mut prev_credit = false;
    for (i, p) in paragraphs.iter().enumerate() {
        let text = paragraph_text(*p);
        if let Some(key) = attr(*p, "Key") {
            pairs.push((key.to_string(), text));
            continue;
        }
        let t = text.trim();
        if t.is_empty() {
            continue;
        }
        let lower = t.to_lowercase();
        let key = if i == 0 {
            "Title"
        } else if lower == "by"
            || lower.starts_with("written by")
            || lower.starts_with("screenplay by")
        {
            prev_credit = true;
            "Credit"
        } else if prev_credit {
            prev_credit = false;
            "Author"
        } else {
            "Notes"
        };
        pairs.push((key.to_string(), t.to_string()));
    }
    pairs
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

/// Escape text for an XML text node or attribute value.
pub fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// One serialized paragraph, or a dual-dialogue group of paragraphs.
enum Para {
    Plain {
        ty: &'static str,
        text: String,
        extra: Vec<(String, String)>,
        new_page: bool,
        self_closing_props: Option<String>,
    },
    Dual(Vec<Para>),
}

impl Para {
    fn plain(ty: &'static str, text: impl Into<String>) -> Para {
        Para::Plain {
            ty,
            text: text.into(),
            extra: Vec::new(),
            new_page: false,
            self_closing_props: None,
        }
    }
    fn set_new_page(&mut self) {
        match self {
            Para::Plain { new_page, .. } => *new_page = true,
            Para::Dual(inner) => {
                if let Some(first) = inner.first_mut() {
                    first.set_new_page();
                }
            }
        }
    }
}

fn write_para(out: &mut String, p: &Para, indent: usize) {
    let pad = "  ".repeat(indent);
    match p {
        Para::Plain {
            ty,
            text,
            extra,
            new_page,
            self_closing_props,
        } => {
            out.push_str(&pad);
            out.push_str(&format!("<Paragraph Type=\"{}\"", escape_xml(ty)));
            for (k, v) in extra {
                out.push_str(&format!(" {}=\"{}\"", k, escape_xml(v)));
            }
            if *new_page {
                out.push_str(" StartsNewPage=\"Yes\"");
            }
            out.push('>');
            if let Some(props) = self_closing_props {
                out.push_str(props);
            }
            out.push_str("<Text>");
            out.push_str(&escape_xml(text));
            out.push_str("</Text></Paragraph>\n");
        }
        Para::Dual(inner) => {
            out.push_str(&pad);
            out.push_str("<Paragraph>\n");
            out.push_str(&pad);
            out.push_str("  <DualDialogue>\n");
            for q in inner {
                write_para(out, q, indent + 2);
            }
            out.push_str(&pad);
            out.push_str("  </DualDialogue>\n");
            out.push_str(&pad);
            out.push_str("</Paragraph>\n");
        }
    }
}

/// Serialize a [`Screenplay`] as Final Draft XML.
pub fn write_fdx(sp: &Screenplay) -> String {
    // Page-eighth lengths for SceneProperties (informational).
    let lengths: Vec<u32> = sp
        .scenes()
        .iter()
        .filter(|s| s.heading.is_some())
        .map(page_eighths)
        .collect();
    let mut heading_idx = 0usize;

    // Build blocks: a dialogue block is a cue plus its following lines.
    let mut paras: Vec<Para> = Vec::new();
    // Index into `paras` where the most recent dialogue block starts.
    let mut last_block_start: Option<usize> = None;
    let mut pending_page_break = false;
    let mut in_block = false;

    for el in &sp.elements {
        let mut para = match el {
            Element::SceneHeading {
                text, scene_number, ..
            } => {
                let mut p = Para::plain("Scene Heading", text.clone());
                let len = lengths.get(heading_idx).copied().unwrap_or(1);
                heading_idx += 1;
                if let Para::Plain {
                    extra,
                    self_closing_props,
                    ..
                } = &mut p
                {
                    if let Some(n) = scene_number {
                        extra.push(("Number".into(), n.clone()));
                    }
                    *self_closing_props = Some(format!("<SceneProperties Length=\"{len}/8\"/>"));
                }
                p
            }
            Element::Action { text } => Para::plain("Action", text.clone()),
            Element::Character {
                name, extension, ..
            } => {
                let mut cue = name.clone();
                if let Some(e) = extension {
                    cue.push(' ');
                    cue.push_str(e);
                }
                Para::plain("Character", cue)
            }
            Element::Parenthetical { text } => Para::plain("Parenthetical", text.clone()),
            Element::Dialogue { text } => Para::plain("Dialogue", text.clone()),
            Element::Transition { text } => Para::plain("Transition", text.clone()),
            Element::Centered { text } => Para::plain("Centered", text.clone()),
            Element::Section { level, text } => {
                let mut p = Para::plain("General", text.clone());
                if let Para::Plain { extra, .. } = &mut p {
                    extra.push(("Fountain".into(), "section".into()));
                    extra.push(("Level".into(), level.to_string()));
                }
                p
            }
            Element::Synopsis { text } => general("synopsis", text),
            Element::Note { text } => general("note", text),
            Element::Lyric { text } => general("lyric", text),
            Element::Boneyard { text } => general("boneyard", text),
            Element::PageBreak => {
                pending_page_break = true;
                continue;
            }
        };
        if pending_page_break {
            para.set_new_page();
            pending_page_break = false;
        }
        match el {
            Element::Character { dual: true, .. } => {
                // Merge the previous block and this new one into a dual group.
                let start = last_block_start.take();
                match start {
                    Some(s) if paras.len() > s => {
                        let mut prev: Vec<Para> = paras.drain(s..).collect();
                        prev.push(para);
                        paras.push(Para::Dual(prev));
                    }
                    _ => paras.push(Para::Dual(vec![para])),
                }
                in_block = true;
                last_block_start = None;
            }
            Element::Character { .. } => {
                last_block_start = Some(paras.len());
                paras.push(para);
                in_block = true;
            }
            Element::Parenthetical { .. } | Element::Dialogue { .. } | Element::Lyric { .. }
                if in_block =>
            {
                match paras.last_mut() {
                    Some(Para::Dual(inner)) => inner.push(para),
                    _ => paras.push(para),
                }
            }
            _ => {
                in_block = false;
                last_block_start = None;
                paras.push(para);
            }
        }
    }
    if pending_page_break {
        let mut p = Para::plain("General", "");
        if let Para::Plain { extra, .. } = &mut p {
            extra.push(("Fountain".into(), "page_break".into()));
        }
        p.set_new_page();
        paras.push(p);
    }

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n");
    out.push_str("<FinalDraft DocumentType=\"Script\" Template=\"No\" Version=\"5\">\n");
    out.push_str("  <Content>\n");
    for p in &paras {
        write_para(&mut out, p, 2);
    }
    out.push_str("  </Content>\n");
    if !sp.title_page.is_empty() {
        out.push_str("  <TitlePage>\n    <Content>\n");
        for (k, v) in &sp.title_page {
            out.push_str(&format!(
                "      <Paragraph Type=\"Action\" Key=\"{}\"><Text>{}</Text></Paragraph>\n",
                escape_xml(k),
                escape_xml(v)
            ));
        }
        out.push_str("    </Content>\n  </TitlePage>\n");
    }
    out.push_str("</FinalDraft>\n");
    out
}

fn general(kind: &str, text: &str) -> Para {
    let mut p = Para::plain("General", text.to_string());
    if let Para::Plain { extra, .. } = &mut p {
        extra.push(("Fountain".into(), kind.to_string()));
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes() {
        assert_eq!(escape_xml("a<b>&\"'"), "a&lt;b&gt;&amp;&quot;&apos;");
    }

    #[test]
    fn parses_minimal_document() {
        let xml = r#"<?xml version="1.0"?>
<FinalDraft DocumentType="Script" Template="No" Version="1">
  <Content>
    <Paragraph Type="Scene Heading" Number="3"><SceneProperties Length="1/8" Page="1"/><Text>INT. </Text><Text Style="Bold">SHED</Text><Text> - DAY</Text></Paragraph>
    <Paragraph Type="Action"><Text>A &amp; B.</Text></Paragraph>
    <Paragraph>
      <DualDialogue>
        <Paragraph Type="Character"><Text>A (V.O.)</Text></Paragraph>
        <Paragraph Type="Dialogue"><Text>Hi.</Text></Paragraph>
        <Paragraph Type="Character"><Text>B</Text></Paragraph>
        <Paragraph Type="Dialogue"><Text>Yo.</Text></Paragraph>
      </DualDialogue>
    </Paragraph>
    <Paragraph Type="Transition" StartsNewPage="Yes"><Text>CUT TO:</Text></Paragraph>
  </Content>
  <TitlePage><Content>
    <Paragraph Type="Action"><Text>THE FILM</Text></Paragraph>
    <Paragraph Type="Action"><Text>Written by</Text></Paragraph>
    <Paragraph Type="Action"><Text>Someone</Text></Paragraph>
  </Content></TitlePage>
</FinalDraft>"#;
        let sp = parse_fdx(xml).unwrap();
        assert_eq!(
            sp.title_page,
            vec![
                ("Title".into(), "THE FILM".into()),
                ("Credit".into(), "Written by".into()),
                ("Author".into(), "Someone".into())
            ]
        );
        let kinds: Vec<&str> = sp.elements.iter().map(|e| e.kind_name()).collect();
        assert_eq!(
            kinds,
            vec![
                "scene_heading",
                "action",
                "character",
                "dialogue",
                "character",
                "dialogue",
                "page_break",
                "transition"
            ]
        );
        assert_eq!(
            sp.elements[0],
            Element::scene_heading("INT. SHED - DAY", Some("3".into()))
        );
        assert_eq!(
            sp.elements[1],
            Element::Action {
                text: "A & B.".into()
            }
        );
        assert_eq!(
            sp.elements[2],
            Element::Character {
                name: "A".into(),
                extension: Some("(V.O.)".into()),
                dual: false
            }
        );
        assert_eq!(
            sp.elements[4],
            Element::Character {
                name: "B".into(),
                extension: None,
                dual: true
            }
        );
        let back = parse_fdx(&write_fdx(&sp)).unwrap();
        assert_eq!(back, sp);
    }

    #[test]
    fn rejects_non_fdx() {
        assert!(parse_fdx("<Other/>").is_err());
        assert!(parse_fdx("not xml").is_err());
    }
}
