//! A small Markdown renderer for release notes and agent replies: headings, bullet and
//! numbered lists, paragraphs, fenced code, and inline **bold**, `code` and [links](url)
//! (shown as their text, accent-coloured).

use std::ops::Range;

use gpui::{AnyElement, App, FontWeight, HighlightStyle, SharedString, StyledText, div, prelude::*, px};

use crate::theme::{ActiveTheme, MONO, size as sz};

/// One block of the document.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading(u8, String),
    Bullet(String),
    Numbered(String, String),
    Paragraph(String),
    Code(String),
}

pub fn parse(md: &str) -> Vec<Block> {
    let mut out = vec![];
    let mut para: Vec<&str> = vec![];
    let mut code: Option<Vec<&str>> = None;
    let flush = |para: &mut Vec<&str>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Paragraph(para.join(" ")));
            para.clear();
        }
    };
    for line in md.lines() {
        if let Some(lines) = code.as_mut() {
            if line.trim_start().starts_with("```") {
                out.push(Block::Code(lines.join("\n")));
                code = None;
            } else {
                lines.push(line);
            }
            continue;
        }
        let t = line.trim();
        if t.starts_with("```") {
            flush(&mut para, &mut out);
            code = Some(vec![]);
        } else if t.is_empty() {
            flush(&mut para, &mut out);
        } else if let Some((level, text)) = heading(t) {
            flush(&mut para, &mut out);
            out.push(Block::Heading(level, text.to_string()));
        } else if let Some(text) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).or_else(|| t.strip_prefix("• ")) {
            flush(&mut para, &mut out);
            out.push(Block::Bullet(text.trim().to_string()));
        } else if let Some((n, text)) = numbered(t) {
            flush(&mut para, &mut out);
            out.push(Block::Numbered(n.to_string(), text.trim().to_string()));
        } else if line.starts_with("  ")
            && para.is_empty()
            && let Some(Block::Bullet(prev) | Block::Numbered(_, prev)) = out.last_mut()
        {
            // A wrapped list item.
            prev.push(' ');
            prev.push_str(t);
        } else {
            para.push(t);
        }
    }
    if let Some(lines) = code {
        out.push(Block::Code(lines.join("\n")));
    }
    flush(&mut para, &mut out);
    out
}

fn heading(t: &str) -> Option<(u8, &str)> {
    let hashes = t.chars().take_while(|c| *c == '#').count();
    (1..=6).contains(&hashes).then(|| t[hashes..].strip_prefix(' ')).flatten().map(|rest| (hashes as u8, rest.trim()))
}

fn numbered(t: &str) -> Option<(&str, &str)> {
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    let rest = t[digits..].strip_prefix(". ").or_else(|| t[digits..].strip_prefix(") "))?;
    Some((&t[..digits], rest))
}

/// Inline spans: the text with markers removed, and where bold, code and links are.
#[derive(Debug, Default, PartialEq)]
pub struct Inline {
    pub text: String,
    pub bold: Vec<Range<usize>>,
    pub code: Vec<Range<usize>>,
    pub links: Vec<(Range<usize>, String)>,
}

pub fn inline(src: &str) -> Inline {
    let mut out = Inline::default();
    let mut rest = src;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("**")
            && let Some(end) = after.find("**").filter(|&e| e > 0)
        {
            let start = out.text.len();
            out.text.push_str(&after[..end]);
            out.bold.push(start..out.text.len());
            rest = &after[end + 2..];
        } else if let Some(after) = rest.strip_prefix('`')
            && let Some(end) = after.find('`').filter(|&e| e > 0)
        {
            let start = out.text.len();
            out.text.push_str(&after[..end]);
            out.code.push(start..out.text.len());
            rest = &after[end + 1..];
        } else if let Some(after) = rest.strip_prefix('[')
            && let Some(close) = after.find("](")
            && let Some(end) = after[close + 2..].find(')')
        {
            let start = out.text.len();
            out.text.push_str(&after[..close]);
            out.links.push((start..out.text.len(), after[close + 2..close + 2 + end].to_string()));
            rest = &after[close + 2 + end + 1..];
        } else {
            let ch = rest.chars().next().unwrap_or(' ');
            out.text.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    out
}

fn styled(src: &str, cx: &App) -> StyledText {
    let t = cx.theme();
    let i = inline(src);
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = vec![];
    highlights.extend(i.bold.iter().map(|r| (r.clone(), HighlightStyle { font_weight: Some(FontWeight::SEMIBOLD), color: Some(t.text), ..Default::default() })));
    highlights.extend(i.code.iter().map(|r| (r.clone(), HighlightStyle { background_color: Some(t.bg_sunken), color: Some(t.text), ..Default::default() })));
    highlights.extend(i.links.iter().map(|(r, _)| (r.clone(), HighlightStyle { color: Some(t.accent_text), ..Default::default() })));
    highlights.sort_by_key(|(r, _)| r.start);
    // Overlapping ranges aren't allowed: keep the first of any overlap.
    let mut clean: Vec<(Range<usize>, HighlightStyle)> = vec![];
    for (r, h) in highlights {
        if clean.last().is_none_or(|(last, _)| last.end <= r.start) {
            clean.push((r, h));
        }
    }
    StyledText::new(SharedString::from(i.text)).with_highlights(clean)
}

/// Renders `md` as a column of blocks in the current text colour.
pub fn render(md: &str, cx: &App) -> AnyElement {
    let t = cx.theme().clone();
    let blocks = parse(md);
    let mut children: Vec<AnyElement> = vec![];
    for (i, b) in blocks.iter().enumerate() {
        let first = i == 0;
        children.push(match b {
            Block::Heading(level, text) => div()
                .when(!first, |d| d.mt(px(if *level <= 2 { 10. } else { 6. })))
                .text_size(px(match level {
                    1 => sz::XL,
                    2 => sz::LG,
                    _ => sz::BASE,
                }))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text)
                .child(styled(text, cx))
                .into_any_element(),
            Block::Bullet(text) => div()
                .flex()
                .gap(px(8.))
                .pl(px(2.))
                .child(div().flex_none().mt(px(7.)).size(px(4.)).bg(t.text_3))
                .child(div().flex_1().min_w_0().child(styled(text, cx)))
                .into_any_element(),
            Block::Numbered(n, text) => div()
                .flex()
                .gap(px(6.))
                .child(div().flex_none().min_w(px(16.)).text_color(t.text_3).child(format!("{n}.")))
                .child(div().flex_1().min_w_0().child(styled(text, cx)))
                .into_any_element(),
            Block::Paragraph(text) => div().when(!first, |d| d.mt(px(2.))).child(styled(text, cx)).into_any_element(),
            Block::Code(code) => div()
                .px(px(10.))
                .py(px(8.))
                .rounded(px(sz::R_SM))
                .bg(t.bg_sunken)
                .border_1()
                .border_color(t.line)
                .font_family(MONO)
                .text_size(px(sz::XS))
                .text_color(t.text)
                .child(code.clone())
                .into_any_element(),
        });
    }
    div().flex().flex_col().gap(px(5.)).children(children).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks() {
        let b = parse("### New\n- **What's new**: a\n  wrapped\n- b\n\nSome text\ncontinued.\n\n1. one\n```\ncode\n```");
        assert_eq!(
            b,
            vec![
                Block::Heading(3, "New".into()),
                Block::Bullet("**What's new**: a wrapped".into()),
                Block::Bullet("b".into()),
                Block::Paragraph("Some text continued.".into()),
                Block::Numbered("1".into(), "one".into()),
                Block::Code("code".into()),
            ]
        );
    }

    #[test]
    fn inline_markers() {
        let i = inline("**Bold** and `code` and [a link](https://x.y) ✓ **unclosed");
        assert_eq!(i.text, "Bold and code and a link ✓ **unclosed");
        assert_eq!(&i.text[i.bold[0].clone()], "Bold");
        assert_eq!(&i.text[i.code[0].clone()], "code");
        assert_eq!(i.links[0].1, "https://x.y");
        assert_eq!(&i.text[i.links[0].0.clone()], "a link");
    }
}
