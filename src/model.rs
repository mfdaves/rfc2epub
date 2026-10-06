//! The private document model (§2), the public metadata types (§7.2) and the
//! pure helpers both parsers share: dates, author names, slugs and file names.

use std::collections::HashSet;
use std::fmt::Write as _;

use crate::Warning;

/// The values of §7.2, as plain fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    /// The RFC number.
    pub number: u32,
    /// The RFC title, without the `RFC <N>: ` prefix.
    pub title: String,
    /// The authors, in order.
    pub authors: Vec<Author>,
    /// The language tag.
    pub language: String,
    /// The publication date.
    pub date: Date,
    /// The abstract as plain text. Empty when the RFC has none.
    pub description: String,
    /// The keywords.
    pub keywords: Vec<String>,
    /// The first paragraph of the copyright notice, when there is one.
    pub rights: Option<String>,
    /// The URL of the source file.
    pub source: String,
    /// The category or status, such as `Standards Track`.
    pub category: Option<String>,
    /// The stream, such as `IETF`.
    pub stream: Option<String>,
    /// The RFCs this one obsoletes.
    pub obsoletes: Vec<u32>,
    /// The RFCs this one updates.
    pub updates: Vec<u32>,
}

impl Metadata {
    /// The package title: `RFC <N>: <title>`.
    pub fn full_title(&self) -> String {
        format!("RFC {}: {}", self.number, self.title)
    }

    /// The unique identifier: `urn:ietf:rfc:<N>`.
    pub fn urn(&self) -> String {
        format!("urn:ietf:rfc:{}", self.number)
    }

    /// The DOI link, with the number padded to at least four digits.
    pub fn doi(&self) -> String {
        format!("https://doi.org/10.17487/RFC{:04}", self.number)
    }
}

/// One author of an RFC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    /// The full name, or the organization for an organizational author.
    pub name: String,
    /// The sort name, `Surname, Given`.
    pub file_as: String,
    /// Whether the author is an editor.
    pub editor: bool,
    /// The author's organization, when known.
    pub organization: Option<String>,
}

impl Author {
    /// Builds an author from an RFC Editor record entry such as `R. Fielding`
    /// or `M. Bishop, Ed.`.
    pub(crate) fn from_record(entry: &str) -> Option<Author> {
        let entry = collapse(entry);
        let (name, editor) = match entry.strip_suffix(", Ed.") {
            Some(name) => (name.trim().to_string(), true),
            None => (entry, false),
        };
        if name.is_empty() {
            return None;
        }
        let words: Vec<&str> = name.split(' ').collect();
        let initials = words.iter().take_while(|w| is_initial(w)).count();
        let file_as = if initials > 0 && initials < words.len() {
            format!(
                "{}, {}",
                words[initials..].join(" "),
                words[..initials].join(" ")
            )
        } else {
            name.clone()
        };
        Some(Author {
            name,
            file_as,
            editor,
            organization: None,
        })
    }
}

/// An initial is a short word ending in a full stop with no lowercase letter,
/// such as `R.` or `J.-P.`.
fn is_initial(word: &str) -> bool {
    word.len() <= 6
        && word.ends_with('.')
        && word.chars().next().is_some_and(char::is_uppercase)
        && !word.chars().any(char::is_lowercase)
}

/// A publication date. The day is optional, as in the RFC Editor's records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Date {
    /// The year.
    pub year: u16,
    /// The month, 1 to 12.
    pub month: u8,
    /// The day of the month, when known.
    pub day: Option<u8>,
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

impl Date {
    /// Builds a date from its parts. The month may be a number or an English
    /// month name.
    pub(crate) fn from_parts(year: &str, month: &str, day: Option<&str>) -> Option<Date> {
        let year: u16 = year.trim().parse().ok()?;
        let month = parse_month(month)?;
        let day = match day.map(str::trim).filter(|d| !d.is_empty()) {
            Some(d) => Some(d.parse::<u8>().ok().filter(|d| (1..=31).contains(d))?),
            None => None,
        };
        (1000..=9999)
            .contains(&year)
            .then_some(Date { year, month, day })
    }

    /// Parses `June 1999` or `1 April 2023`.
    pub(crate) fn parse(text: &str) -> Option<Date> {
        let words: Vec<&str> = text.split_whitespace().collect();
        match words.as_slice() {
            [month, year] => Date::from_parts(year, month, None),
            [day, month, year] => Date::from_parts(year, month, Some(day)),
            _ => None,
        }
    }

    /// `YYYY-MM-DD` when the day is known, else `YYYY-MM`.
    pub fn iso(&self) -> String {
        match self.day {
            Some(day) => format!("{:04}-{:02}-{:02}", self.year, self.month, day),
            None => format!("{:04}-{:02}", self.year, self.month),
        }
    }

    /// The `dcterms:modified` value: the date at midnight UTC, day 1 when the
    /// day is unknown.
    pub fn modified(&self) -> String {
        let day = self.day.unwrap_or(1);
        format!("{:04}-{:02}-{:02}T00:00:00Z", self.year, self.month, day)
    }

    /// `Month YYYY` or `D Month YYYY`.
    pub fn display(&self) -> String {
        let month = month_name(self.month);
        match self.day {
            Some(day) => format!("{day} {month} {}", self.year),
            None => format!("{month} {}", self.year),
        }
    }
}

fn month_name(month: u8) -> &'static str {
    MONTHS
        .get(usize::from(month).wrapping_sub(1))
        .copied()
        .unwrap_or("")
}

/// Parses a month number or an English month name, full or abbreviated.
pub(crate) fn parse_month(text: &str) -> Option<u8> {
    let text = text.trim();
    if let Ok(n) = text.parse::<u8>() {
        return (1..=12).contains(&n).then_some(n);
    }
    if text.len() < 3 {
        return None;
    }
    let lower = text.to_lowercase();
    let position = MONTHS
        .iter()
        .position(|m| m.to_lowercase().starts_with(&lower))?;
    u8::try_from(position + 1).ok()
}

/// Formats a possibly partial date of a cited document: `YYYY`,
/// `Month YYYY` or `D Month YYYY`. An unrecognized month is kept as written.
pub(crate) fn citation_date(year: &str, month: &str, day: &str) -> String {
    let month = match parse_month(month) {
        Some(n) => month_name(n).to_string(),
        None => month.trim().to_string(),
    };
    [day.trim(), month.as_str(), year.trim()]
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Turns an all-caps status such as `BEST CURRENT PRACTICE` into title case.
pub(crate) fn title_case(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first
                    .to_uppercase()
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// Collapses every run of whitespace into one space and trims both ends.
pub(crate) fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Replaces every run of XML whitespace with one space, keeping the ends.
pub(crate) fn squeeze(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if matches!(c, ' ' | '\t' | '\n' | '\r') {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(c);
            space = false;
        }
    }
    out
}

/// The file-name slug of §8.
pub(crate) fn slug(title: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            if gap && !out.is_empty() {
                out.push('-');
            }
            gap = false;
            out.push(c.to_ascii_lowercase());
        } else {
            gap = true;
        }
    }
    if out.len() > 60 {
        let cut = out.as_bytes()[..=60]
            .iter()
            .rposition(|&b| b == b'-')
            .filter(|&i| i > 0)
            .unwrap_or(60);
        out.truncate(cut);
    }
    out
}

/// The output file name of §8.
pub(crate) fn file_name(number: u32, title: &str) -> String {
    let slug = slug(title);
    if slug.is_empty() {
        format!("rfc{number}.epub")
    } else {
        format!("rfc{number}-{slug}.epub")
    }
}

/// A parsed RFC: metadata, front matter, sections and warnings.
#[derive(Debug)]
pub(crate) struct Document {
    pub meta: Metadata,
    /// The label of the first navigation entry: `Abstract` or `Front Matter`.
    pub front_label: &'static str,
    /// The sections of the front-matter document.
    pub front: Vec<Section>,
    /// The top-level sections; each one is a content document.
    pub sections: Vec<Section>,
    /// How deep the table of contents goes.
    pub toc_depth: usize,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Default)]
pub(crate) struct Section {
    pub id: Option<String>,
    /// The heading prefix, such as `3.1.` or `Appendix A.`.
    pub number: Option<String>,
    pub title: Vec<Inline>,
    /// Whether the section may appear in the table of contents.
    pub toc: bool,
    pub blocks: Vec<Block>,
    pub children: Vec<Section>,
}

impl Section {
    /// The heading as plain text: number, then title.
    pub fn label(&self) -> String {
        let title = plain_text(&self.title);
        match &self.number {
            Some(number) if title.is_empty() => number.clone(),
            Some(number) => format!("{number} {title}"),
            None => title,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Block {
    pub id: Option<String>,
    pub kind: BlockKind,
}

impl Block {
    pub fn new(kind: BlockKind) -> Block {
        Block { id: None, kind }
    }
}

#[derive(Debug)]
pub(crate) enum BlockKind {
    Para(Vec<Inline>),
    List {
        ordered: bool,
        /// The `type` and `start` attributes of an ordered list.
        attrs: Vec<(&'static str, String)>,
        classes: Vec<&'static str>,
        items: Vec<Item>,
    },
    /// A definition list; `true` marks a term.
    Defs {
        classes: Vec<&'static str>,
        items: Vec<(bool, Item)>,
    },
    /// Preformatted text with the class `artwork`, `sourcecode` or `legacy`.
    Pre {
        class: &'static str,
        content: Vec<Inline>,
    },
    Figure {
        blocks: Vec<Block>,
        caption: Option<Vec<Inline>>,
    },
    Table(Table),
    /// A sanitized inline SVG drawing, already serialized.
    Svg(String),
    Quote(Vec<Block>),
    Aside(Vec<Block>),
    References(Vec<Reference>),
    /// An address block, one entry per line.
    Address(Vec<Vec<Inline>>),
}

/// A list item, table cell or definition part.
#[derive(Debug)]
pub(crate) struct Item {
    pub id: Option<String>,
    pub content: Flow,
}

#[derive(Debug)]
pub(crate) enum Flow {
    Inline(Vec<Inline>),
    Blocks(Vec<Block>),
}

#[derive(Debug, Default)]
pub(crate) struct Table {
    pub caption: Option<Vec<Inline>>,
    pub head: Vec<Vec<Cell>>,
    pub body: Vec<Vec<Cell>>,
    pub foot: Vec<Vec<Cell>>,
}

#[derive(Debug)]
pub(crate) struct Cell {
    pub header: bool,
    pub align: Option<&'static str>,
    pub colspan: u32,
    pub rowspan: u32,
    pub item: Item,
}

/// One bibliography entry.
#[derive(Debug)]
pub(crate) struct Reference {
    pub id: Option<String>,
    pub label: String,
    pub citation: Vec<Inline>,
}

#[derive(Debug, Clone)]
pub(crate) enum Inline {
    Text(String),
    /// `em`, `strong`, `sub`, `sup` or `code`.
    Tag(&'static str, Vec<Inline>),
    /// A `span` with a class.
    Span(&'static str, Vec<Inline>),
    Link(Target, Vec<Inline>),
    /// An empty element that carries an id.
    Anchor(String),
    Break,
}

#[derive(Debug, Clone)]
pub(crate) enum Target {
    /// An id somewhere in this book.
    Internal(String),
    /// An absolute URL.
    External(String),
}

/// The text of inline content, with markup removed and whitespace collapsed.
pub(crate) fn plain_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    push_text(inlines, &mut out);
    collapse(&out)
}

fn push_text(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(text) => out.push_str(text),
            Inline::Tag(_, inner) | Inline::Span(_, inner) | Inline::Link(_, inner) => {
                push_text(inner, out);
            }
            Inline::Break => out.push(' '),
            Inline::Anchor(_) => {}
        }
    }
}

/// The text of blocks, paragraphs separated by one space.
pub(crate) fn blocks_text(blocks: &[Block]) -> String {
    let mut out = String::new();
    for block in blocks {
        match &block.kind {
            BlockKind::Para(inlines)
            | BlockKind::Pre {
                content: inlines, ..
            } => {
                push_text(inlines, &mut out);
            }
            BlockKind::List { items, .. } => {
                for item in items {
                    flow_text(&item.content, &mut out);
                }
            }
            BlockKind::Defs { items, .. } => {
                for (_, item) in items {
                    flow_text(&item.content, &mut out);
                }
            }
            BlockKind::Quote(inner) | BlockKind::Aside(inner) => out.push_str(&blocks_text(inner)),
            _ => {}
        }
        out.push(' ');
    }
    collapse(&out)
}

fn flow_text(flow: &Flow, out: &mut String) {
    match flow {
        Flow::Inline(inlines) => push_text(inlines, out),
        Flow::Blocks(blocks) => out.push_str(&blocks_text(blocks)),
    }
    out.push(' ');
}

/// Trims leading whitespace from the first text and trailing whitespace from
/// the last text, and drops empty texts.
pub(crate) fn trim_inlines(inlines: &mut Vec<Inline>) {
    inlines.retain(|i| !matches!(i, Inline::Text(t) if t.is_empty()));
    while let Some(Inline::Text(first)) = inlines.first_mut() {
        let trimmed = first.trim_start();
        if trimmed.is_empty() {
            inlines.remove(0);
        } else {
            *first = trimmed.to_string();
            break;
        }
    }
    while let Some(Inline::Text(last)) = inlines.last_mut() {
        let trimmed = last.trim_end();
        if trimmed.is_empty() {
            inlines.pop();
        } else {
            *last = trimmed.to_string();
            break;
        }
    }
}

impl Document {
    /// Finishes a parsed document: drops duplicate ids, turns links whose
    /// target does not exist into plain text with a warning, and removes
    /// repeated warnings.
    pub fn finish(&mut self) {
        let mut ids = HashSet::new();
        let mut ids_pass = Pass::Ids(&mut ids);
        for section in self.front.iter_mut().chain(self.sections.iter_mut()) {
            walk_section(section, &mut ids_pass);
        }
        let mut broken = Vec::new();
        let mut links_pass = Pass::Links(&ids, &mut broken);
        for section in self.front.iter_mut().chain(self.sections.iter_mut()) {
            walk_section(section, &mut links_pass);
        }
        self.warnings
            .extend(broken.into_iter().map(Warning::BrokenLink));
        let mut seen = Vec::new();
        self.warnings.retain(|w| {
            let new = !seen.contains(w);
            if new {
                seen.push(w.clone());
            }
            new
        });
    }
}

enum Pass<'a> {
    /// Collects ids in document order and drops repeated ones.
    Ids(&'a mut HashSet<String>),
    /// Unlinks internal links whose id was not collected.
    Links(&'a HashSet<String>, &'a mut Vec<String>),
}

impl Pass<'_> {
    fn id(&mut self, id: &mut Option<String>) {
        if let (Pass::Ids(ids), Some(value)) = (self, id.as_ref())
            && !ids.insert(value.clone())
        {
            *id = None;
        }
    }
}

fn walk_section(section: &mut Section, pass: &mut Pass) {
    pass.id(&mut section.id);
    walk_inlines(&mut section.title, pass);
    walk_blocks(&mut section.blocks, pass);
    for child in &mut section.children {
        walk_section(child, pass);
    }
}

fn walk_blocks(blocks: &mut [Block], pass: &mut Pass) {
    for block in blocks {
        pass.id(&mut block.id);
        match &mut block.kind {
            BlockKind::Para(inlines)
            | BlockKind::Pre {
                content: inlines, ..
            } => {
                walk_inlines(inlines, pass);
            }
            BlockKind::List { items, .. } => items.iter_mut().for_each(|i| walk_item(i, pass)),
            BlockKind::Defs { items, .. } => items.iter_mut().for_each(|(_, i)| walk_item(i, pass)),
            BlockKind::Figure { blocks, caption } => {
                walk_blocks(blocks, pass);
                if let Some(caption) = caption {
                    walk_inlines(caption, pass);
                }
            }
            BlockKind::Table(table) => {
                if let Some(caption) = &mut table.caption {
                    walk_inlines(caption, pass);
                }
                for row in table
                    .head
                    .iter_mut()
                    .chain(&mut table.body)
                    .chain(&mut table.foot)
                {
                    row.iter_mut()
                        .for_each(|cell| walk_item(&mut cell.item, pass));
                }
            }
            BlockKind::Quote(inner) | BlockKind::Aside(inner) => walk_blocks(inner, pass),
            BlockKind::References(entries) => {
                for entry in entries {
                    pass.id(&mut entry.id);
                    walk_inlines(&mut entry.citation, pass);
                }
            }
            BlockKind::Address(lines) => lines.iter_mut().for_each(|l| walk_inlines(l, pass)),
            BlockKind::Svg(_) => {}
        }
    }
}

fn walk_item(item: &mut Item, pass: &mut Pass) {
    pass.id(&mut item.id);
    match &mut item.content {
        Flow::Inline(inlines) => walk_inlines(inlines, pass),
        Flow::Blocks(blocks) => walk_blocks(blocks, pass),
    }
}

fn walk_inlines(inlines: &mut Vec<Inline>, pass: &mut Pass) {
    let old = std::mem::take(inlines);
    for mut inline in old {
        match &mut inline {
            Inline::Tag(_, inner) | Inline::Span(_, inner) => walk_inlines(inner, pass),
            Inline::Link(target, inner) => {
                walk_inlines(inner, pass);
                if let (Pass::Links(ids, broken), Target::Internal(id)) = (&mut *pass, &*target)
                    && !ids.contains(id)
                {
                    broken.push(id.clone());
                    inlines.append(inner);
                    continue;
                }
            }
            Inline::Anchor(id) => {
                if let Pass::Ids(ids) = pass
                    && !ids.insert(id.clone())
                {
                    continue;
                }
            }
            Inline::Text(_) | Inline::Break => {}
        }
        inlines.push(inline);
    }
}

/// Appends `text` to a string, escaped for XML text and attribute values.
/// Characters that XML 1.0 does not allow are dropped.
pub(crate) fn escape_into(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if xml_char(c) => out.push(c),
            _ => {}
        }
    }
}

/// Escapes a string for XML.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    escape_into(&mut out, text);
    out
}

/// Whether XML 1.0 allows the character.
pub(crate) fn xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

/// Formats `RFC 1, RFC 2` for lists of RFC numbers.
pub(crate) fn rfc_list(numbers: &[u32]) -> String {
    let mut out = String::new();
    for (i, n) in numbers.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        let _ = write!(out, "RFC {n}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_from_spec() {
        let rows = [
            (9114, "HTTP/3", "rfc9114-http-3.epub"),
            (791, "Internet Protocol", "rfc791-internet-protocol.epub"),
            (
                2616,
                "Hypertext Transfer Protocol -- HTTP/1.1",
                "rfc2616-hypertext-transfer-protocol-http-1-1.epub",
            ),
            (
                8650,
                "Dynamic Subscription to YANG Events and Datastores over RESTCONF",
                "rfc8650-dynamic-subscription-to-yang-events-and-datastores-over.epub",
            ),
        ];
        for (n, title, name) in rows {
            assert_eq!(file_name(n, title), name);
        }
        assert_eq!(file_name(7, "?!"), "rfc7.epub");
        assert_eq!(slug(&"a".repeat(70)).len(), 60);
        assert_eq!(slug("--Ünïcode  Title--"), "n-code-title");
    }

    #[test]
    fn dates() {
        let d = Date::parse("June 1999").unwrap_or_else(|| panic!("date"));
        assert_eq!(
            (d.iso(), d.modified(), d.display()),
            (
                "1999-06".into(),
                "1999-06-01T00:00:00Z".into(),
                "June 1999".into()
            )
        );
        let d = Date::parse("1 April 2023").unwrap_or_else(|| panic!("date"));
        assert_eq!(
            (d.iso(), d.display()),
            ("2023-04-01".into(), "1 April 2023".into())
        );
        assert_eq!(
            Date::from_parts("2022", "06", None).map(|d| d.iso()),
            Some("2022-06".into())
        );
        assert_eq!(
            Date::from_parts("2022", "Sept", Some("3")).map(|d| d.iso()),
            Some("2022-09-03".into())
        );
        assert!(Date::parse("Smarch 2020").is_none());
        assert!(Date::from_parts("2020", "13", None).is_none());
        assert_eq!(citation_date("2013", "", ""), "2013");
        assert_eq!(citation_date("2013", "7", "4"), "4 July 2013");
        assert_eq!(citation_date("2013", "Spring", ""), "Spring 2013");
    }

    #[test]
    fn record_authors() {
        let a = Author::from_record("M. Bishop, Ed.").unwrap_or_else(|| panic!("author"));
        assert_eq!(
            (a.name.as_str(), a.file_as.as_str(), a.editor),
            ("M. Bishop", "Bishop, M.", true)
        );
        let a = Author::from_record("J.-P. Vasseur").unwrap_or_else(|| panic!("author"));
        assert_eq!(a.file_as, "Vasseur, J.-P.");
        let a = Author::from_record("M. St. Johns").unwrap_or_else(|| panic!("author"));
        assert_eq!(a.file_as, "St. Johns, M.");
        let a =
            Author::from_record("Internet Architecture Board").unwrap_or_else(|| panic!("author"));
        assert_eq!(a.file_as, "Internet Architecture Board");
        assert!(Author::from_record("  ").is_none());
    }

    #[test]
    fn text_helpers() {
        assert_eq!(title_case("BEST CURRENT PRACTICE"), "Best Current Practice");
        assert_eq!(escape("a<b & \"c\"\u{1}"), "a&lt;b &amp; &quot;c&quot;");
        assert_eq!(rfc_list(&[760, 1]), "RFC 760, RFC 1");
    }
}
