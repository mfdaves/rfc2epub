//! Source B: legacy HTML (§6).

use roxmltree::{Node, ParsingOptions};
use serde_json::Value;

use crate::model::{
    Author, Block, BlockKind, Date, Document, Inline, Metadata, Section, Target, collapse, squeeze,
    title_case, trim_inlines, xml_char,
};
use crate::{Error, Warning};

const BASE: &str = "https://www.rfc-editor.org/rfc/";

/// Parses a legacy HTML fragment together with its `rfc<N>.json` record.
pub(crate) fn parse(html: &str, info_json: &str) -> Result<Document, Error> {
    let info: Value = serde_json::from_str(info_json)
        .map_err(|e| Error::InvalidSource(format!("invalid metadata record: {e}")))?;
    let meta = metadata(&info)?;

    let html = html.trim_start_matches('\u{feff}').trim_start();
    if !html.starts_with("<pre") {
        return Err(Error::InvalidSource(
            "the HTML does not begin with <pre".into(),
        ));
    }
    // §6.2: drop characters XML forbids, name the one entity XML lacks, and
    // add a root element.
    let cleaned: String = html.chars().filter(|c| xml_char(*c)).collect();
    let wrapped = format!("<root>{}</root>", cleaned.replace("&nbsp;", "&#160;"));
    let options = ParsingOptions {
        allow_dtd: false,
        ..ParsingOptions::default()
    };
    let xml = roxmltree::Document::parse_with_options(&wrapped, options)
        .map_err(|e| Error::InvalidSource(format!("the HTML is not well-formed: {e}")))?;

    let mut warnings = Vec::new();
    let entries = linearize(xml.root_element(), &mut warnings);
    let lines = remove_furniture(entries);
    let (front, sections) = build(lines, &mut warnings);

    let mut doc = Document {
        meta,
        front_label: "Front Matter",
        front: if front.blocks.is_empty() {
            Vec::new()
        } else {
            vec![front]
        },
        sections,
        toc_depth: 3,
        warnings,
    };
    doc.finish();
    Ok(doc)
}

/// The metadata of §6.6, taken from the RFC Editor's record.
fn metadata(info: &Value) -> Result<Metadata, Error> {
    let text = |key: &str| collapse(info[key].as_str().unwrap_or(""));
    let list = |key: &str| -> Vec<String> {
        info[key]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(collapse)
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    let number = rfc_number(&text("doc_id"))
        .ok_or_else(|| Error::InvalidSource("the record has no RFC doc_id".into()))?;
    let title = text("title");
    if title.is_empty() {
        return Err(Error::InvalidSource("the record has no title".into()));
    }
    let date = Date::parse(&text("pub_date"))
        .ok_or_else(|| Error::InvalidSource("the record has no valid pub_date".into()))?;
    let status = text("pub_status");
    Ok(Metadata {
        number,
        title,
        authors: list("authors")
            .iter()
            .filter_map(|a| Author::from_record(a))
            .collect(),
        language: "en".into(),
        date,
        description: text("abstract"),
        keywords: list("keywords"),
        rights: None,
        source: format!("{BASE}rfc{number}.html"),
        category: (!status.is_empty()).then(|| title_case(&status)),
        stream: None,
        obsoletes: list("obsoletes")
            .iter()
            .filter_map(|s| rfc_number(s))
            .collect(),
        updates: list("updates")
            .iter()
            .filter_map(|s| rfc_number(s))
            .collect(),
    })
}

/// Parses `RFC2119`.
fn rfc_number(id: &str) -> Option<u32> {
    let digits = id
        .get(..3)
        .filter(|p| p.eq_ignore_ascii_case("rfc"))
        .map(|_| &id[3..])?;
    digits.parse().ok().filter(|n| (1..=99999).contains(n))
}

/// The span a piece of text sits in.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Span {
    Plain,
    Grey,
    Heading(u8),
}

#[derive(Debug, Clone)]
enum Piece {
    Text(String, Span),
    Link {
        href: String,
        text: String,
        selflink: bool,
        span: Span,
    },
    Anchor {
        id: String,
        selflink: bool,
        span: Span,
    },
}

impl Piece {
    fn span(&self) -> Span {
        match self {
            Piece::Text(_, span) | Piece::Link { span, .. } | Piece::Anchor { span, .. } => *span,
        }
    }

    fn is_blank(&self) -> bool {
        match self {
            Piece::Text(text, _) | Piece::Link { text, .. } => text.trim().is_empty(),
            Piece::Anchor { .. } => true,
        }
    }
}

type Line = Vec<Piece>;

enum Entry {
    Line(Line),
    PageBreak,
}

/// §6.3 step 1: the contents of all `<pre>` blocks as one sequence of lines.
fn linearize(root: Node, warnings: &mut Vec<Warning>) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut pages = 0;
    for node in root.children() {
        let tag = if node.is_element() {
            node.tag_name().name()
        } else {
            ""
        };
        match tag {
            "pre" => {
                if pages > 0 {
                    entries.push(Entry::PageBreak);
                }
                pages += 1;
                entries.push(Entry::Line(Vec::new()));
                walk(node, Span::Plain, None, &mut entries, warnings);
            }
            "hr" => {}
            _ if node.is_text() && node.text().unwrap_or("").trim().is_empty() => {}
            _ if node.is_text() || node.is_element() => {
                entries.push(Entry::Line(Vec::new()));
                walk(node, Span::Plain, None, &mut entries, warnings);
            }
            _ => {}
        }
    }
    entries
}

fn walk(
    node: Node,
    span: Span,
    link: Option<(&str, bool)>,
    entries: &mut Vec<Entry>,
    warnings: &mut Vec<Warning>,
) {
    if node.is_text() {
        for (i, part) in node.text().unwrap_or("").split('\n').enumerate() {
            if i > 0 {
                entries.push(Entry::Line(Vec::new()));
            }
            if part.is_empty() {
                continue;
            }
            let piece = match link {
                Some((href, selflink)) => Piece::Link {
                    href: href.to_string(),
                    text: part.to_string(),
                    selflink,
                    span,
                },
                None => Piece::Text(part.to_string(), span),
            };
            push(entries, piece);
        }
        return;
    }
    if !node.is_element() {
        return;
    }
    let class = node.attribute("class").unwrap_or("");
    let selflink = class.split_whitespace().any(|c| c == "selflink");
    if let Some(id) = node
        .attribute("id")
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        push(
            entries,
            Piece::Anchor {
                id: id.to_string(),
                selflink,
                span,
            },
        );
    }
    let (span, link) = match node.tag_name().name() {
        "span" => {
            let span = match class {
                "grey" => Span::Grey,
                "h2" => Span::Heading(2),
                "h3" => Span::Heading(3),
                "h4" => Span::Heading(4),
                "h5" => Span::Heading(5),
                "h6" => Span::Heading(6),
                _ => span,
            };
            (span, link)
        }
        "a" => match node.attribute("href") {
            Some(href) => (span, Some((href, selflink))),
            None => (span, link),
        },
        "pre" => (span, link),
        other => {
            warnings.push(Warning::UnknownElement(other.to_string()));
            (span, link)
        }
    };
    for child in node.children() {
        walk(child, span, link, entries, warnings);
    }
}

fn push(entries: &mut Vec<Entry>, piece: Piece) {
    match entries.last_mut() {
        Some(Entry::Line(line)) => line.push(piece),
        _ => entries.push(Entry::Line(vec![piece])),
    }
}

fn is_blank(line: &Line) -> bool {
    line.iter().all(Piece::is_blank)
}

/// §6.3 steps 2 and 3: drops running headers and footers, keeps their anchors
/// on the next line that survives, and leaves one blank line at each page
/// break.
fn remove_furniture(entries: Vec<Entry>) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    let mut pending: Vec<Piece> = Vec::new();
    let mut after_break = false;
    for entry in entries {
        let line = match entry {
            Entry::PageBreak => {
                while out.last().is_some_and(is_blank) {
                    out.pop();
                }
                if !out.is_empty() {
                    out.push(Vec::new());
                }
                after_break = true;
                continue;
            }
            Entry::Line(line) => line,
        };
        let grey = line.iter().any(|p| p.span() == Span::Grey);
        if grey || is_blank(&line) {
            pending.extend(
                line.into_iter()
                    .filter(|p| matches!(p, Piece::Anchor { .. })),
            );
            if !grey && !after_break {
                out.push(Vec::new());
            }
            continue;
        }
        after_break = false;
        let mut line = line;
        if !pending.is_empty() {
            line.splice(0..0, pending.drain(..));
        }
        out.push(line);
    }
    while out.last().is_some_and(is_blank) {
        out.pop();
    }
    while out.first().is_some_and(is_blank) {
        out.remove(0);
    }
    if !pending.is_empty() {
        out.push(pending);
    }
    out
}

/// The heading level of a line, if it is a heading.
fn heading_level(line: &Line) -> Option<u8> {
    line.iter().find_map(|p| match p.span() {
        Span::Heading(level) => Some(level),
        _ => None,
    })
}

/// §6.3 steps 4 to 6: the front matter and the tree of sections.
fn build(lines: Vec<Line>, warnings: &mut Vec<Warning>) -> (Section, Vec<Section>) {
    let shallowest = lines.iter().filter_map(heading_level).min().unwrap_or(2);
    let mut front = Section {
        toc: true,
        ..Section::default()
    };
    let mut top: Vec<Section> = Vec::new();
    let mut stack: Vec<(u8, Section)> = Vec::new();
    let mut text: Vec<Line> = Vec::new();
    let mut generated = 0;

    for (i, line) in lines.iter().enumerate() {
        let Some(level) = heading_level(line) else {
            text.push(line.clone());
            continue;
        };
        let target = stack.last_mut().map_or(&mut front, |(_, s)| s);
        flush(&mut text, &mut target.blocks, warnings);
        let depth = level - shallowest + 1;
        while stack.last().is_some_and(|(d, _)| *d >= depth) {
            close(&mut stack, &mut top);
        }
        let (section, rest) = heading(line, &mut generated, warnings);
        stack.push((depth, section));
        if let Some(mut rest) = rest {
            let indent = lines[i + 1..]
                .iter()
                .find(|l| !is_blank(l))
                .and_then(|l| {
                    l.iter().find_map(|p| match p {
                        Piece::Text(t, _) => Some(t.len() - t.trim_start().len()),
                        _ => None,
                    })
                })
                .filter(|n| *n > 0)
                .unwrap_or(3);
            rest.insert(0, Piece::Text(" ".repeat(indent), Span::Plain));
            text.push(rest);
        }
    }
    let target = stack.last_mut().map_or(&mut front, |(_, s)| s);
    flush(&mut text, &mut target.blocks, warnings);
    while !stack.is_empty() {
        close(&mut stack, &mut top);
    }
    (front, top)
}

/// Pops the innermost open section into its parent, or into the top level.
fn close(stack: &mut Vec<(u8, Section)>, top: &mut Vec<Section>) {
    if let Some((_, section)) = stack.pop() {
        match stack.last_mut() {
            Some((_, parent)) => parent.children.push(section),
            None => top.push(section),
        }
    }
}

/// Turns a heading line into a section, and returns the text that follows
/// the heading on the same line, if any.
fn heading(
    line: &Line,
    generated: &mut usize,
    warnings: &mut Vec<Warning>,
) -> (Section, Option<Line>) {
    let mut id = None;
    let mut title: Vec<Inline> = Vec::new();
    let mut number: Option<String> = None;
    let mut before: Line = Vec::new();
    let mut rest: Line = Vec::new();
    let mut started = false;
    for piece in line {
        let in_heading = matches!(piece.span(), Span::Heading(_));
        match piece {
            Piece::Anchor {
                id: anchor,
                selflink: true,
                ..
            } if id.is_none() => id = Some(anchor.clone()),
            Piece::Anchor { id: anchor, .. } => title.push(Inline::Anchor(anchor.clone())),
            Piece::Link {
                text,
                selflink: true,
                ..
            } if in_heading && !started && number.is_none() => {
                number = Some(collapse(text));
                started = true;
            }
            _ if in_heading => {
                started = true;
                title.extend(inline(piece, warnings));
            }
            _ if started => rest.push(piece.clone()),
            _ => before.push(piece.clone()),
        }
    }
    // Text on the line outside the heading span is kept as body text.
    if !is_blank(&before) {
        rest.splice(0..0, before);
    }
    for inline in &mut title {
        if let Inline::Text(text) = inline {
            *text = squeeze(text);
        }
    }
    // The full stop after the number belongs to the number.
    let first_text = title.iter_mut().find(|i| matches!(i, Inline::Text(_)));
    if let (Some(n), Some(Inline::Text(first))) = (&mut number, first_text)
        && let Some(stripped) = first.strip_prefix('.')
    {
        n.push('.');
        *first = stripped.trim_start().to_string();
    }
    trim_inlines(&mut title);
    let id = id.unwrap_or_else(|| {
        *generated += 1;
        format!("h-{generated}")
    });
    if let Some(Piece::Text(first, span)) = rest.first_mut() {
        *first = first.trim_start().to_string();
        *span = Span::Plain;
    }
    let rest = (!is_blank(&rest)).then_some(rest);
    let section = Section {
        id: Some(id),
        number,
        title,
        toc: true,
        ..Section::default()
    };
    (section, rest)
}

/// Moves the collected text into one preformatted block.
fn flush(text: &mut Vec<Line>, blocks: &mut Vec<Block>, warnings: &mut Vec<Warning>) {
    while text.last().is_some_and(is_blank) {
        text.pop();
    }
    let start = text.iter().position(|l| !is_blank(l)).unwrap_or(text.len());
    let lines: Vec<Line> = text.drain(..).skip(start).collect();
    if lines.is_empty() {
        return;
    }
    let mut content = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            content.push(Inline::Text("\n".into()));
        }
        for piece in line {
            content.extend(inline(piece, warnings));
        }
    }
    // Merge adjacent texts to keep the output compact.
    let mut merged: Vec<Inline> = Vec::new();
    for inline in content {
        match (merged.last_mut(), inline) {
            (Some(Inline::Text(last)), Inline::Text(next)) => last.push_str(&next),
            (_, inline) => merged.push(inline),
        }
    }
    blocks.push(Block::new(BlockKind::Pre {
        class: "legacy",
        content: merged,
    }));
}

/// One piece as inline content. Links follow §6.4.
fn inline(piece: &Piece, warnings: &mut Vec<Warning>) -> Vec<Inline> {
    match piece {
        Piece::Text(text, _) => vec![Inline::Text(text.clone())],
        Piece::Anchor { id, .. } => vec![Inline::Anchor(id.clone())],
        Piece::Link { href, text, .. } => {
            let content = vec![Inline::Text(text.clone())];
            match link_target(href) {
                Some(target) => vec![Inline::Link(target, content)],
                None => {
                    warnings.push(Warning::BrokenLink(href.clone()));
                    content
                }
            }
        }
    }
}

/// Where a link of the legacy HTML points (§6.4).
fn link_target(href: &str) -> Option<Target> {
    let href = href.trim();
    if let Some(fragment) = href.strip_prefix('#') {
        return (!fragment.is_empty()).then(|| Target::Internal(fragment.to_string()));
    }
    let lower = href.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || lower.starts_with("ftp://")
    {
        return Some(Target::External(href.to_string()));
    }
    if let Some(colon) = href.find(':')
        && !href[..colon].contains('/')
    {
        // Another scheme, which a book has no use for.
        return None;
    }
    if let Some(path) = href.strip_prefix('/') {
        return Some(Target::External(format!(
            "https://www.rfc-editor.org/{path}"
        )));
    }
    let mut base = BASE;
    let mut path = href;
    loop {
        if let Some(rest) = path.strip_prefix("./") {
            path = rest;
        } else if let Some(rest) = path.strip_prefix("../") {
            path = rest;
            base = "https://www.rfc-editor.org/";
        } else {
            break;
        }
    }
    Some(Target::External(format!("{base}{path}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: &str = r#"{"doc_id":"RFC42","title":"Test","authors":["A. Author, Ed."],
        "pub_date":"1 April 2023","abstract":"","keywords":[],"pub_status":"INFORMATIONAL",
        "obsoletes":["RFC41"],"updates":[],"format":["TEXT","HTML"]}"#;

    #[test]
    fn links() {
        let ext = |h: &str| match link_target(h) {
            Some(Target::External(url)) => url,
            other => format!("{other:?}"),
        };
        assert_eq!(ext("./rfc2119"), "https://www.rfc-editor.org/rfc/rfc2119");
        assert_eq!(
            ext("./rfc7841#section-2"),
            "https://www.rfc-editor.org/rfc/rfc7841#section-2"
        );
        assert_eq!(ext("https://example.com/x"), "https://example.com/x");
        assert_eq!(ext("../bcp/bcp14"), "https://www.rfc-editor.org/bcp/bcp14");
        assert!(matches!(link_target("#page-3"), Some(Target::Internal(id)) if id == "page-3"));
        assert!(link_target("javascript:alert(1)").is_none());
    }

    #[test]
    fn pages_headings_and_furniture() {
        let html = concat!(
            "<pre>Title block&nbsp;line\n\n",
            "<span class=\"h2\"><a class=\"selflink\" id=\"section-1\" href=\"#section-1\">1</a>.  MUST  </span> This word\n",
            "   means it, see <a href=\"#section-2\">Section 2</a>.\n\n\n",
            "<span class=\"grey\">Author   [Page 1]</span></pre>\n",
            "<hr class='noprint'/><!--NewPage--><pre class='newpage'><span id=\"page-2\" ></span>\n",
            "<span class=\"grey\"><a href=\"./rfc42\">RFC 42</a>   Test   April 2023</span>\n\n\n",
            "<span class=\"h3\"><a class=\"selflink\" id=\"section-1.1\" href=\"#section-1.1\">1.1</a>.  Sub</span>\n\n",
            "   Text on page 2.\n",
            "<span class=\"h2\">Unnumbered</span>\n   Last <a href=\"#nowhere\">x</a>.</pre>"
        );
        let doc = parse(html, INFO).unwrap();
        assert_eq!(doc.meta.authors[0].file_as, "Author, A.");
        assert_eq!(doc.meta.category.as_deref(), Some("Informational"));
        assert_eq!(doc.meta.obsoletes, vec![41]);
        assert_eq!(doc.sections.len(), 2);
        let s1 = &doc.sections[0];
        assert_eq!(s1.label(), "1. MUST");
        assert_eq!(s1.id.as_deref(), Some("section-1"));
        let BlockKind::Pre { content, .. } = &s1.blocks[0].kind else {
            panic!("pre")
        };
        assert_eq!(
            crate::model::plain_text(content),
            "This word means it, see Section 2."
        );
        assert_eq!(s1.children[0].label(), "1.1. Sub");
        // The page anchor moved onto the heading of page 2.
        assert!(matches!(&s1.children[0].title[0], Inline::Anchor(id) if id == "page-2"));
        assert_eq!(doc.sections[1].id.as_deref(), Some("h-1"));
        assert!(
            doc.warnings
                .contains(&Warning::BrokenLink("section-2".into()))
        );
        assert!(
            doc.warnings
                .contains(&Warning::BrokenLink("nowhere".into()))
        );
        let BlockKind::Pre { content, .. } = &doc.front[0].blocks[0].kind else {
            panic!("pre")
        };
        assert_eq!(content.len(), 1);
        assert!(matches!(&content[0], Inline::Text(t) if t == "Title block\u{a0}line"));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(matches!(
            parse("<p>x</p>", INFO),
            Err(Error::InvalidSource(_))
        ));
        assert!(matches!(
            parse("<pre>x<b></pre>", INFO),
            Err(Error::InvalidSource(_))
        ));
        assert!(matches!(
            parse("<pre>x</pre>", "{}"),
            Err(Error::InvalidSource(_))
        ));
    }
}
