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
    let mut lines = remove_furniture(entries, &meta);
    remove_toc(&mut lines);
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

/// The visible text of a line.
fn line_text(line: &Line) -> String {
    line.iter()
        .map(|p| match p {
            Piece::Text(text, _) | Piece::Link { text, .. } => text.as_str(),
            Piece::Anchor { .. } => "",
        })
        .collect()
}

/// The indentation of a line, and whether its text starts in lowercase.
fn shape(line: &Line) -> (usize, bool) {
    let text = line_text(line);
    let body = text.trim_start();
    let lower = body.chars().next().is_some_and(char::is_lowercase);
    (text.len() - body.len(), lower)
}

/// The width of an original RFC page, in columns.
const PAGE_WIDTH: usize = 72;

/// Splits the linearized lines into the original pages.
fn paginate(entries: Vec<Entry>) -> Vec<Vec<Line>> {
    let mut pages = vec![Vec::new()];
    for entry in entries {
        match entry {
            Entry::PageBreak => pages.push(Vec::new()),
            Entry::Line(line) => {
                if let Some(page) = pages.last_mut() {
                    page.push(line);
                }
            }
        }
    }
    pages
}

/// §6.3 step 2: marks the page furniture of every page. A line is furniture
/// because of where it sits on its page: a `span.grey` line, the page footer
/// at the bottom, and an unmarked running header at the top.
fn furniture(pages: &[Vec<Line>], meta: &Metadata) -> Vec<Vec<bool>> {
    let mut marks: Vec<Vec<bool>> = pages
        .iter()
        .map(|page| {
            page.iter()
                .map(|line| line.iter().any(|p| p.span() == Span::Grey))
                .collect()
        })
        .collect();
    for (page, mark) in pages.iter().zip(&mut marks) {
        if let Some(last) = page.iter().rposition(|l| !is_blank(l))
            && is_footer(&page[last])
        {
            mark[last] = true;
        }
    }
    // The document's own title block is on the first page, so headers are
    // looked for on the later pages only.
    let headers: Vec<Option<Header>> = pages
        .iter()
        .enumerate()
        .map(|(i, page)| (i > 0).then(|| running_header(page, meta)).flatten())
        .collect();
    let section_text =
        |page: &[Line], header: &Header| header.section.map(|i| collapse(&line_text(&page[i])));
    for ((page, mark), header) in pages.iter().zip(&mut marks).zip(&headers) {
        let Some(header) = header else { continue };
        mark[header.date] = true;
        mark[header.title] = true;
        // A running section title ends the header block or recurs on other
        // pages; otherwise it may be content and is kept.
        if let Some(section) = header.section {
            let ends_block = page.get(section + 1).is_none_or(is_blank);
            let text = section_text(page, header);
            let recurs = pages
                .iter()
                .zip(&headers)
                .filter(|(p, h)| h.as_ref().is_some_and(|h| section_text(p, h) == text))
                .count()
                > 1;
            if ends_block || recurs {
                mark[section] = true;
            }
        }
    }
    marks
}

/// The lines of an unmarked running header, as in RFC 791: a date line, a
/// line with the document title, and possibly a section title.
struct Header {
    date: usize,
    title: usize,
    /// A candidate section title: the line right after the title line, on the
    /// opposite side of the page from the date.
    section: Option<usize>,
}

fn running_header(page: &[Line], meta: &Metadata) -> Option<Header> {
    let date = page.iter().position(|l| !is_blank(l))?;
    let title = date + 1;
    let plain = |i: usize| {
        page.get(i)
            .is_some_and(|l| !is_blank(l) && heading_level(l).is_none())
    };
    // The date line counts even when the RFC Editor marked it as a heading,
    // as in RFC 768; the title line that must follow it shows what it is.
    let is_date = Date::parse(&line_text(&page[date])).is_some();
    if !(plain(title) && is_date && is_title_line(&page[title], meta)) {
        return None;
    }
    let section = date + 2;
    let section =
        (plain(section) && left_side(&page[section]) != left_side(&page[date])).then_some(section);
    Some(Header {
        date,
        title,
        section,
    })
}

/// Whether a line holds the document title, `RFC <N>`, or both.
fn is_title_line(line: &Line, meta: &Metadata) -> bool {
    let text = collapse(&line_text(line));
    let rest = collapse(&text.replace(&format!("RFC {}", meta.number), " "));
    rest.is_empty() || rest.eq_ignore_ascii_case(&meta.title)
}

/// Whether a line sits on the left half of the page: its left margin is no
/// wider than its right margin.
fn left_side(line: &Line) -> bool {
    let text = line_text(line);
    let indent = text.len() - text.trim_start().len();
    let width = text.trim().chars().count();
    indent <= PAGE_WIDTH.saturating_sub(indent + width)
}

/// Whether a line is a page footer: it starts or ends with a page marker
/// such as `[Page 4]`, `[Page iii]` or `[page 2]`.
fn is_footer(line: &Line) -> bool {
    let text = line_text(line);
    let text = text.trim();
    let start = text.find(']').map(|i| &text[..=i]);
    let end = text.rfind('[').map(|i| &text[i..]);
    [start, end].into_iter().flatten().any(is_page_marker)
}

fn is_page_marker(text: &str) -> bool {
    let Some(inner) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) else {
        return false;
    };
    let mut words = inner.split_whitespace();
    let (Some(word), Some(number), None) = (words.next(), words.next(), words.next()) else {
        return false;
    };
    word.eq_ignore_ascii_case("page") && is_page_number(number)
}

/// A page number: digits, or a lowercase or uppercase roman numeral.
fn is_page_number(text: &str) -> bool {
    !text.is_empty()
        && (text.bytes().all(|b| b.is_ascii_digit())
            || (text.len() <= 6
                && (text.bytes().all(|b| b"ivxlc".contains(&b))
                    || text.bytes().all(|b| b"IVXLC".contains(&b)))))
}

/// §6.3 steps 2 and 3: drops page furniture, keeps its anchors on the next line
/// that survives, and leaves one blank line at each page break, or none when a
/// paragraph continues across it (§6.5).
fn remove_furniture(entries: Vec<Entry>, meta: &Metadata) -> Vec<Line> {
    let pages = paginate(entries);
    let marks = furniture(&pages, meta);
    let mut out: Vec<Line> = Vec::new();
    let mut pending: Vec<Piece> = Vec::new();
    let mut after_break = false;
    // The indentation of the last line before the latest page break.
    let mut break_indent = None;
    for (number, (page, mark)) in pages.into_iter().zip(marks).enumerate() {
        if number > 0 {
            while out.last().is_some_and(is_blank) {
                out.pop();
            }
            if let Some(last) = out.last() {
                break_indent = Some(shape(last).0);
                out.push(Vec::new());
            }
            after_break = true;
        }
        for (line, furniture) in page.into_iter().zip(mark) {
            if furniture || is_blank(&line) {
                // Carried anchors are plain anchors, so that one from a
                // dropped line never makes a later line a heading or gives
                // a heading its id.
                pending.extend(line.into_iter().filter_map(|p| match p {
                    Piece::Anchor { id, .. } => Some(Piece::Anchor {
                        id,
                        selflink: false,
                        span: Span::Plain,
                    }),
                    _ => None,
                }));
                if !furniture && !after_break {
                    out.push(Vec::new());
                }
                continue;
            }
            after_break = false;
            if let Some(indent) = break_indent.take()
                && shape(&line) == (indent, true)
                && heading_level(&line).is_none()
                && out.last().is_some_and(is_blank)
            {
                out.pop();
            }
            let mut line = line;
            if !pending.is_empty() {
                line.splice(0..0, pending.drain(..));
            }
            out.push(line);
        }
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

/// How many lines after the end of a contents list are checked for entries
/// that would show the list goes on.
const TOC_LOOKAHEAD: usize = 10;

/// Removes the original table of contents from the front matter, since the
/// book has its own (§6.5). The list is removed completely or not at all.
/// Its anchors move to the next line kept.
fn remove_toc(lines: &mut Vec<Line>) {
    let front = lines
        .iter()
        .position(|l| heading_level(l).is_some())
        .unwrap_or(lines.len());
    let Some(title) = lines[..front].iter().position(|l| {
        line_text(l)
            .trim()
            .eq_ignore_ascii_case("table of contents")
    }) else {
        return;
    };
    let mut end = title + 1;
    let mut entries = 0;
    while end < front {
        if is_blank(&lines[end]) {
            end += 1;
        } else if is_toc_entry(&lines[end]) {
            entries += 1;
            end += 1;
        } else if let Some(next) = wrapped_entry(&lines[..front], end) {
            entries += 1;
            end = next;
        } else {
            break;
        }
    }
    // An entry soon after the stop means the list goes on in a form not
    // recognized here; then it is kept whole rather than cut.
    let continues = lines[end..front]
        .iter()
        .filter(|l| !is_blank(l))
        .take(TOC_LOOKAHEAD)
        .any(is_toc_entry);
    if entries == 0 || continues {
        return;
    }
    // Drop the title, the entries and the blank lines after them.
    let removed: Vec<Line> = lines.drain(title..end).collect();
    let anchors: Vec<Piece> = removed
        .into_iter()
        .flatten()
        .filter(|p| matches!(p, Piece::Anchor { .. }))
        .collect();
    if let Some(next) = lines.get_mut(title) {
        next.splice(0..0, anchors);
    } else if !anchors.is_empty() {
        lines.push(anchors);
    }
}

/// A line of an in-text table of contents: it links to a section or a page,
/// ends with a dot leader and a page number, or starts with a section number
/// and ends with a page number.
fn is_toc_entry(line: &Line) -> bool {
    let links = line.iter().any(|p| match p {
        Piece::Link { href, .. } => ["#section-", "#appendix-", "#page-"]
            .iter()
            .any(|h| href.starts_with(h)),
        _ => false,
    });
    let text = line_text(line);
    let text = text.trim_end();
    let Some((before, page)) = text.rsplit_once([' ', '.']) else {
        return links;
    };
    let numbered = is_page_number(page) && !before.trim().is_empty();
    let leader = text[..text.len() - page.len()].trim_end().ends_with("..")
        || text[..text.len() - page.len()].trim_end().ends_with(". .");
    links || (numbered && (leader || starts_with_section_number(line)))
}

/// An entry wrapped over two or three lines, as in RFC 3958: one or two head
/// lines, then a more indented tail that is an entry without a section number
/// of its own. Returns the index after the tail.
fn wrapped_entry(lines: &[Line], start: usize) -> Option<usize> {
    let indent = shape(&lines[start]).0;
    for (tail, line) in lines.iter().enumerate().skip(start + 1).take(2) {
        if is_blank(line) {
            return None;
        }
        if is_toc_entry(line) {
            let completes = shape(line).0 > indent && !starts_with_section_number(line);
            return completes.then_some(tail + 1);
        }
    }
    None
}

/// Whether a line starts with a section number such as `13.3.4`, `A.1.` or
/// `Appendix A`, followed by more text.
fn starts_with_section_number(line: &Line) -> bool {
    let text = line_text(line);
    let mut words = text.split_whitespace();
    let (Some(first), Some(_)) = (words.next(), words.next()) else {
        return false;
    };
    if first.eq_ignore_ascii_case("appendix") {
        return true;
    }
    let mut parts = first.trim_end_matches('.').split('.');
    let head = parts.next().unwrap_or("");
    let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    let letter =
        head.len() == 1 && head.bytes().all(|b| b.is_ascii_uppercase()) && first.contains('.');
    (digits(head) || letter) && parts.all(digits)
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
    // Anchors carried onto the heading may precede its text.
    if let Some(Inline::Text(first)) = title.iter_mut().find(|i| !matches!(i, Inline::Anchor(_))) {
        *first = first.trim_start().to_string();
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
    fn refinements() {
        let html = concat!(
            "<pre>Front\n\nTable of Contents\n\n",
            "   <a href=\"#section-1\">1</a>.  One . . . . . <a href=\"#page-2\">2</a>\n",
            "   Appendix A.  Two  . . . . . . . 12\n\n",
            "<span class=\"h2\"><a class=\"selflink\" id=\"section-1\" href=\"#section-1\">1</a>.  One</span>\n\n",
            "   A paragraph split\n\n<span class=\"grey\">[Page 1]</span></pre>\n",
            "<hr class='noprint'/><!--NewPage--><pre class='newpage'><span id=\"page-2\" ></span>\n",
            "<span class=\"grey\">RFC 42</span>\n\n   across pages.\n\n<span class=\"grey\">[Page 2]</span></pre>\n",
            "<hr class='noprint'/><!--NewPage--><pre class='newpage'>\n",
            "   New paragraph.</pre>"
        );
        let doc = parse(html, INFO).unwrap();
        let text = |s: &Section| match &s.blocks[0].kind {
            BlockKind::Pre { content, .. } => content
                .iter()
                .map(|i| match i {
                    Inline::Text(t) => t.as_str(),
                    _ => "",
                })
                .collect::<String>(),
            _ => String::new(),
        };
        assert_eq!(text(&doc.front[0]), "Front");
        assert_eq!(
            text(&doc.sections[0]),
            "   A paragraph split\n   across pages.\n\n   New paragraph."
        );
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
    }

    /// A record for an RFC titled "Test Protocol", as in the headers below.
    const PROTOCOL: &str = r#"{"doc_id":"RFC999","title":"Test Protocol","authors":["A. Author"],
        "pub_date":"September 1981","abstract":"","keywords":[],"pub_status":"UNKNOWN",
        "obsoletes":[],"updates":[],"format":["TEXT","HTML"]}"#;

    const BREAK: &str = "</pre>\n<hr class='noprint'/><!--NewPage--><pre class='newpage'>";

    /// Every line of the book's text, in order.
    fn book_lines(doc: &Document) -> Vec<String> {
        fn section(s: &Section, out: &mut Vec<String>) {
            out.push(s.label());
            for block in &s.blocks {
                if let BlockKind::Pre { content, .. } = &block.kind {
                    let text: String = content
                        .iter()
                        .map(|i| match i {
                            Inline::Text(t) => t.clone(),
                            Inline::Link(_, inner) => crate::model::plain_text(inner),
                            _ => String::new(),
                        })
                        .collect();
                    out.extend(text.lines().map(str::to_string));
                }
            }
            for child in &s.children {
                section(child, out);
            }
        }
        let mut out = Vec::new();
        for s in doc.front.iter().chain(&doc.sections) {
            section(s, &mut out);
        }
        out.retain(|l| !l.trim().is_empty());
        out
    }

    #[test]
    fn unmarked_running_headers_and_footers() {
        let html = [
            // The title block on the first page stays, date included.
            "<pre>RFC:  999\n                           September 1981\n\n                         TEST PROTOCOL\n\n   Body one.\n\n                                                          [Page 1]",
            // Date left, title and section right; a content line follows on the
            // date's side and stays, as `SEGMENT ARRIVES` does in RFC 793.
            "<span id=\"page-2\" ></span>\nSeptember 1981\n                                                       Test Protocol\n                                                       Specification\nSEGMENT ARRIVES\n\n   Body two.\n\n\nAuthor                                                    [Page 2]",
            // A grey date right, then title and section left.
            "<span class=\"grey\">                                                    September 1981</span>\nTest Protocol\nSpecification\n\n   Body three.\n\n[Page 3]",
            // A two-line header, then content directly under it: kept.
            "                                                    September 1981\nTest Protocol\nThis paragraph starts right away\n   and goes on.\n\n[page 4]                                                   Author",
            // RFC 768: a date wrongly marked as a heading, the number with the
            // title, and a running section title that ends the block.
            "<span class=\"h2\"><a class=\"selflink\" id=\"section-28\" href=\"#section-28\">28</a> Sep 1981</span>\nRFC 999                                      Test Protocol\n                                                       Fields\n\n   Body five.\n\n[Page 5]",
            // The last page: its unmarked footer and the padding before it go.
            "September 1981\n                                                       Test Protocol\n\n   Last words.\n\n\n\n\nAuthor                                                    [Page 6]</pre>",
        ]
        .join(BREAK);
        let doc = parse(&html, PROTOCOL).unwrap();
        let lines = book_lines(&doc);
        let trimmed: Vec<&str> = lines.iter().map(|l| l.trim()).collect();
        for gone in [
            "Test Protocol",
            "Specification",
            "Fields",
            "September 1981",
            "28 Sep 1981",
        ] {
            assert_eq!(
                trimmed.iter().filter(|l| **l == gone).count(),
                usize::from(gone == "September 1981"),
                "{gone}: {lines:#?}"
            );
        }
        assert!(
            lines
                .iter()
                .all(|l| !is_footer(&vec![Piece::Text(l.clone(), Span::Plain)])),
            "{lines:#?}"
        );
        for kept in [
            "RFC:  999",
            "TEST PROTOCOL",
            "SEGMENT ARRIVES",
            "This paragraph starts right away",
            "Body five.",
        ] {
            assert!(trimmed.contains(&kept), "{kept}: {lines:#?}");
        }
        assert_eq!(trimmed.last(), Some(&"Last words."));
        assert!(doc.sections.is_empty(), "the date heading made a section");
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
    }

    #[test]
    fn contents_are_removed_completely() {
        let html = concat!(
            "<pre>Front\n\nTable of Contents\n\n",
            "   <a href=\"#section-1\">1</a>.  One . . . . . . . . . . . . . <a href=\"#page-2\">2</a>\n",
            // RFC 2616: no link and a one-dot leader.
            "   13.3.4   Rules for When to Use Entity Tags and Last-Modified Dates.89\n",
            // RFC 3958: an entry wrapped over two lines, the first unlinked.
            "             3.1.1.  Registration of Application Service and\n",
            "                     Protocol Tags. . . . . . . . . . . . . . . . . .  7\n",
            "   Appendix A.  Collected ABNF . . . . . . . . . . . . . . . . . . 12\n\n",
            // RFC 791: a preface follows the list before the first heading.
            "                              PREFACE\n\n   This preface stays.\n\n",
            "<span class=\"h2\"><a class=\"selflink\" id=\"section-1\" href=\"#section-1\">1</a>.  One</span>\n\n   Text.</pre>"
        );
        let doc = parse(html, INFO).unwrap();
        let lines = book_lines(&doc);
        let trimmed: Vec<&str> = lines.iter().map(|l| l.trim()).collect();
        assert_eq!(
            trimmed,
            ["Front", "PREFACE", "This preface stays.", "1. One", "Text."],
            "{lines:#?}"
        );
    }

    #[test]
    fn contents_are_kept_whole_when_unsure() {
        let html = concat!(
            "<pre>Front\n\nTable of Contents\n\n",
            "   <a href=\"#section-1\">1</a>.  One . . . . . . . . . . . . . <a href=\"#page-2\">2</a>\n",
            "   An entry in a shape this converter does not know\n",
            "   <a href=\"#section-2\">2</a>.  Two . . . . . . . . . . . . . <a href=\"#page-3\">3</a>\n\n",
            "<span class=\"h2\"><a class=\"selflink\" id=\"section-1\" href=\"#section-1\">1</a>.  One</span>\n",
            "<span id=\"page-2\" ></span>   Text.\n",
            "<span class=\"h2\"><a class=\"selflink\" id=\"section-2\" href=\"#section-2\">2</a>.  Two</span>\n",
            "<span id=\"page-3\" ></span>   More.</pre>"
        );
        let doc = parse(html, INFO).unwrap();
        let lines = book_lines(&doc);
        assert_eq!(lines.len(), 9, "{lines:#?}");
        assert!(
            lines.iter().any(|l| l.contains("An entry in a shape")),
            "{lines:#?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("Table of Contents")),
            "{lines:#?}"
        );
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
