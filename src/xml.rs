//! The source of RFCs from 8650 onward: the published RFCXML v3.

use std::collections::{HashMap, HashSet};

use roxmltree::{Node, ParsingOptions};

use crate::model::{
    Author, Block, BlockKind, Cell, Date, Document, Flow, Inline, Item, Metadata, Reference,
    Section, Table, Target, blocks_text, citation_date, collapse, escape_into, plain_text, squeeze,
    trim_inlines,
};
use crate::{Error, Warning};

const SVG_NS: &str = "http://www.w3.org/2000/svg";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// Elements that are inline content.
const INLINE: &[&str] = &[
    "em", "strong", "sub", "sup", "br", "tt", "bcp14", "eref", "xref", "relref", "contact", "u",
    "iref", "cref",
];

/// Parses a published RFCXML v3 file.
pub(crate) fn parse(text: &str) -> Result<Document, Error> {
    let options = ParsingOptions {
        allow_dtd: false,
        ..ParsingOptions::default()
    };
    let xml = roxmltree::Document::parse_with_options(text, options)
        .map_err(|e| Error::InvalidSource(format!("not well-formed XML: {e}")))?;
    let root = xml.root_element();
    if name(root) != "rfc" || root.attribute("version") != Some("3") {
        return Err(Error::InvalidSource(
            "the root is not <rfc version=\"3\">".into(),
        ));
    }
    let number = root
        .attribute("number")
        .and_then(|n| n.trim().parse::<u32>().ok())
        .filter(|n| (1..=99999).contains(n))
        .ok_or_else(|| Error::InvalidSource("<rfc> has no numeric number attribute".into()))?;
    let front =
        child(root, "front").ok_or_else(|| Error::InvalidSource("missing <front>".into()))?;
    let toc_depth = root
        .attribute("tocDepth")
        .and_then(|d| d.trim().parse::<usize>().ok())
        .unwrap_or(3)
        .clamp(1, 6);

    let mut p = Parser::new(root);

    let mut front_sections = Vec::new();
    let mut description = String::new();
    if let Some(abstract_) = child(front, "abstract") {
        let blocks = p.blocks_of(abstract_);
        description = blocks_text(&blocks);
        front_sections.push(Section {
            id: p.id_for(abstract_, false),
            title: vec![Inline::Text("Abstract".into())],
            toc: true,
            blocks,
            ..Section::default()
        });
    }
    for note in children(front, "note") {
        front_sections.push(p.section(note, 1, false));
    }
    let mut rights = None;
    if let Some(boilerplate) = child(front, "boilerplate") {
        for section in children(boilerplate, "section") {
            if section.attribute("anchor") == Some("copyright") {
                rights = child(section, "t")
                    .map(|t| plain_text(&p.inlines(t)))
                    .filter(|r| !r.is_empty());
            }
            front_sections.push(p.section(section, 1, false));
        }
    }

    let mut sections = Vec::new();
    for part in ["middle", "back"] {
        let Some(part) = child(root, part) else {
            continue;
        };
        for node in part.children().filter(Node::is_element) {
            match name(node) {
                "section" => sections.push(p.section(node, 1, true)),
                "references" => sections.push(p.references(node, 1, true)),
                // Consumed by the prep tool into derivedAnchor.
                "displayreference" => {}
                _ => {
                    // Keep the text with whatever precedes it.
                    p.warn(Warning::UnknownElement(node.tag_name().name().to_string()));
                    let blocks = p.blocks_of(node);
                    match sections.last_mut().or(front_sections.last_mut()) {
                        Some(section) => section.blocks.extend(blocks),
                        None => front_sections.push(Section {
                            blocks,
                            ..Section::default()
                        }),
                    }
                }
            }
        }
    }

    let meta = Metadata {
        number,
        title: text_of(child(front, "title")),
        authors: children(front, "author").filter_map(author).collect(),
        language: root
            .attribute((XML_NS, "lang"))
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .unwrap_or("en")
            .to_string(),
        date: child(front, "date")
            .and_then(|d| {
                Date::from_parts(
                    d.attribute("year").unwrap_or(""),
                    d.attribute("month").unwrap_or(""),
                    d.attribute("day"),
                )
            })
            .ok_or_else(|| Error::InvalidSource("missing or invalid <date>".into()))?,
        description,
        keywords: children(front, "keyword")
            .map(|k| text_of(Some(k)))
            .filter(|k| !k.is_empty())
            .collect(),
        rights,
        source: format!("https://www.rfc-editor.org/rfc/rfc{number}.xml"),
        category: root.attribute("category").and_then(category),
        stream: root.attribute("submissionType").and_then(stream),
        obsoletes: rfc_numbers(root.attribute("obsoletes")),
        updates: rfc_numbers(root.attribute("updates")),
    };

    let mut doc = Document {
        meta,
        front_label: "Abstract",
        front: front_sections,
        sections,
        toc_depth,
        warnings: p.warnings,
    };
    doc.finish();
    Ok(doc)
}

/// The local name of an RFCXML element; empty for other nodes.
fn name<'a>(node: Node<'a, '_>) -> &'a str {
    if node.is_element() && node.tag_name().namespace().is_none() {
        node.tag_name().name()
    } else {
        ""
    }
}

fn child<'a, 'i>(node: Node<'a, 'i>, tag: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|c| name(*c) == tag)
}

fn children<'a, 'i>(node: Node<'a, 'i>, tag: &'static str) -> impl Iterator<Item = Node<'a, 'i>> {
    node.children().filter(move |c| name(*c) == tag)
}

/// All text inside a node, whitespace collapsed.
fn text_of(node: Option<Node>) -> String {
    node.map(|n| collapse(&raw_text(n))).unwrap_or_default()
}

/// All text inside a node, exactly as written.
fn raw_text(node: Node) -> String {
    node.descendants()
        .filter(Node::is_text)
        .filter_map(|n| n.text())
        .collect()
}

/// Removes leading and trailing blank lines; the rest is kept exactly.
fn trim_blank_lines(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    let first = lines.iter().position(|l| !l.trim().is_empty());
    let last = lines.iter().rposition(|l| !l.trim().is_empty());
    match (first, last) {
        (Some(first), Some(last)) => lines[first..=last].join("\n"),
        _ => String::new(),
    }
}

/// The heading prefix derived from a `pn` value.
fn heading_number(pn: &str) -> Option<String> {
    let rest = pn.strip_prefix("section-")?;
    if let Some(appendix) = rest.strip_prefix("appendix.") {
        let mut parts = appendix.split('.');
        let letter = parts.next().filter(|l| !l.is_empty())?.to_uppercase();
        let tail: Vec<&str> = parts.collect();
        if tail.is_empty() {
            Some(format!("Appendix {letter}."))
        } else {
            Some(format!("{letter}.{}.", tail.join(".")))
        }
    } else if !rest.is_empty()
        && rest
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        Some(format!("{rest}."))
    } else {
        None
    }
}

fn category(code: &str) -> Option<String> {
    let name = match code.trim() {
        "std" => "Standards Track",
        "bcp" => "Best Current Practice",
        "info" => "Informational",
        "exp" => "Experimental",
        "historic" => "Historic",
        _ => return None,
    };
    Some(name.to_string())
}

fn stream(code: &str) -> Option<String> {
    let name = match code.trim() {
        "" => return None,
        "independent" => "Independent Submission",
        "editorial" => "Editorial",
        other => other,
    };
    Some(name.to_string())
}

/// Parses `4007, 7622, 8089`.
fn rfc_numbers(list: Option<&str>) -> Vec<u32> {
    list.unwrap_or("")
        .split([',', ' '])
        .filter_map(|n| n.trim().parse::<u32>().ok())
        .filter(|n| *n > 0)
        .collect()
}

/// An author of the RFC itself, for the package metadata.
fn author(node: Node) -> Option<Author> {
    let fullname = collapse(node.attribute("fullname").unwrap_or(""));
    let surname = collapse(node.attribute("surname").unwrap_or(""));
    let initials = collapse(node.attribute("initials").unwrap_or(""));
    let organization = text_of(child(node, "organization"));
    let name = if !fullname.is_empty() {
        fullname.clone()
    } else if !surname.is_empty() {
        collapse(&format!("{initials} {surname}"))
    } else if !organization.is_empty() {
        organization.clone()
    } else {
        return None;
    };
    let file_as = if surname.is_empty() {
        name.clone()
    } else {
        let given = match fullname.rfind(&surname) {
            Some(at) => collapse(&format!(
                "{} {}",
                &fullname[..at],
                &fullname[at + surname.len()..]
            )),
            None => initials,
        };
        let given = given.trim_matches([',', ' ']);
        if given.is_empty() {
            surname
        } else {
            format!("{surname}, {given}")
        }
    };
    Some(Author {
        name,
        file_as,
        editor: node.attribute("role") == Some("editor"),
        organization: Some(organization).filter(|o| !o.is_empty()),
    })
}

/// The converter state: the target index of the first pass and the
/// warnings.
struct Parser<'a> {
    /// Every anchor, pn and slugifiedName, mapped to the id of the element
    /// that owns it and whether that element is a bibliography entry.
    keys: HashMap<&'a str, (&'a str, bool)>,
    /// The ids that some cross-reference points to.
    targeted: HashSet<&'a str>,
    warnings: Vec<Warning>,
    toc_depth: usize,
}

impl<'a> Parser<'a> {
    fn new(root: Node<'a, '_>) -> Parser<'a> {
        let toc_depth = root
            .attribute("tocDepth")
            .and_then(|d| d.trim().parse::<usize>().ok())
            .unwrap_or(3)
            .clamp(1, 6);
        let mut keys = HashMap::new();
        let elements: Vec<Node<'a, '_>> = root.descendants().filter(Node::is_element).collect();
        for node in &elements {
            if let Some(anchor) = node.attribute("anchor") {
                keys.entry(anchor).or_insert_with(|| owner(*node));
            }
        }
        for node in &elements {
            if let Some(pn) = node.attribute("pn") {
                keys.entry(pn).or_insert_with(|| owner(*node));
            }
            if name(*node) == "name"
                && let Some(slug) = node.attribute("slugifiedName")
                && let Some(id) = node.parent_element().and_then(canonical)
            {
                keys.entry(slug).or_insert((id, false));
            }
        }
        let mut targeted = HashSet::new();
        for node in &elements {
            if matches!(name(*node), "xref" | "relref")
                && !node.ancestors().any(|a| name(a) == "toc")
                && let Some((id, _)) = node.attribute("target").and_then(|t| keys.get(t))
            {
                targeted.insert(*id);
            }
        }
        Parser {
            keys,
            targeted,
            warnings: Vec::new(),
            toc_depth,
        }
    }

    fn warn(&mut self, warning: Warning) {
        self.warnings.push(warning);
    }

    /// The id to write on an element: only for link and navigation targets.
    fn id_for(&self, node: Node, nav: bool) -> Option<String> {
        let id = canonical(node)?;
        (nav || self.targeted.contains(id)).then(|| id.to_string())
    }

    fn section(&mut self, node: Node<'a, '_>, depth: usize, listed: bool) -> Section {
        let toc = node.attribute("toc") != Some("exclude");
        let nav = listed && toc && depth <= self.toc_depth;
        let number = if node.attribute("numbered") == Some("false") {
            None
        } else {
            node.attribute("pn").and_then(heading_number)
        };
        let mut title = child(node, "name")
            .map(|n| self.inlines(n))
            .unwrap_or_default();
        trim_inlines(&mut title);
        let mut section = Section {
            id: self.id_for(node, nav),
            number,
            title,
            toc,
            ..Section::default()
        };
        let mut run = Vec::new();
        for c in node.children() {
            match name(c) {
                "name" => {}
                "section" | "note" => {
                    flush(&mut run, &mut section.blocks);
                    section.children.push(self.section(c, depth + 1, nav));
                }
                "references" => {
                    flush(&mut run, &mut section.blocks);
                    section.children.push(self.references(c, depth + 1, nav));
                }
                _ => self.push_node(c, &mut run, &mut section.blocks),
            }
        }
        flush(&mut run, &mut section.blocks);
        section
    }

    /// A `<references>` element: a section holding a bibliography.
    fn references(&mut self, node: Node<'a, '_>, depth: usize, listed: bool) -> Section {
        let toc = node.attribute("toc") != Some("exclude");
        let nav = listed && toc && depth <= self.toc_depth;
        let mut title = child(node, "name")
            .map(|n| self.inlines(n))
            .unwrap_or_default();
        trim_inlines(&mut title);
        let mut section = Section {
            id: self.id_for(node, nav),
            number: node.attribute("pn").and_then(heading_number),
            title,
            toc,
            ..Section::default()
        };
        let mut entries = Vec::new();
        for c in node.children().filter(Node::is_element) {
            match name(c) {
                "name" => {}
                "reference" => entries.push(Reference {
                    id: self.id_for(c, false),
                    label: label(c),
                    citation: self.citation(c),
                }),
                "referencegroup" => {
                    let mut citation = Vec::new();
                    for (i, r) in children(c, "reference").enumerate() {
                        if i > 0 {
                            citation.push(Inline::Break);
                        }
                        citation.extend(self.citation(r));
                    }
                    if let Some(target) = c.attribute("target") {
                        citation.push(Inline::Break);
                        citation.extend(angle_link(target));
                    }
                    entries.push(Reference {
                        id: self.id_for(c, false),
                        label: label(c),
                        citation,
                    });
                }
                "references" => section.children.push(self.references(c, depth + 1, nav)),
                other => {
                    self.warn(Warning::UnknownElement(other.to_string()));
                    let blocks = self.blocks_of(c);
                    section.blocks.extend(blocks);
                }
            }
        }
        if !entries.is_empty() {
            section
                .blocks
                .push(Block::new(BlockKind::References(entries)));
        }
        section
    }

    /// The citation text of one `<reference>`.
    fn citation(&mut self, node: Node<'a, '_>) -> Vec<Inline> {
        let front = child(node, "front");
        let mut parts: Vec<Vec<Inline>> = Vec::new();
        if let Some(front) = front {
            let authors = citation_authors(front);
            if !authors.is_empty() {
                parts.push(vec![Inline::Text(authors)]);
            }
            let title = text_of(child(front, "title"));
            if !title.is_empty() {
                let title = if node.attribute("quoteTitle") == Some("false") {
                    title
                } else {
                    format!("\"{title}\"")
                };
                parts.push(vec![Inline::Text(title)]);
            }
        }
        let series = front
            .into_iter()
            .flat_map(|f| children(f, "seriesInfo"))
            .chain(children(node, "seriesInfo"));
        for info in series {
            let text = collapse(&format!(
                "{} {}",
                info.attribute("name").unwrap_or(""),
                info.attribute("value").unwrap_or("")
            ));
            if !text.is_empty() {
                parts.push(vec![Inline::Text(text)]);
            }
        }
        for content in children(node, "refcontent") {
            let mut inlines = self.inlines(content);
            trim_inlines(&mut inlines);
            if !inlines.is_empty() {
                parts.push(inlines);
            }
        }
        if let Some(date) = front.and_then(|f| child(f, "date")) {
            let text = citation_date(
                date.attribute("year").unwrap_or(""),
                date.attribute("month").unwrap_or(""),
                date.attribute("day").unwrap_or(""),
            );
            if !text.is_empty() {
                parts.push(vec![Inline::Text(text)]);
            }
        }
        if let Some(target) = node.attribute("target") {
            parts.push(angle_link(target));
        }
        let mut out = Vec::new();
        for (i, part) in parts.into_iter().enumerate() {
            if i > 0 {
                out.push(Inline::Text(", ".into()));
            }
            out.extend(part);
        }
        if !out.is_empty() {
            out.push(Inline::Text(".".into()));
        }
        for annotation in children(node, "annotation") {
            let mut inlines = self.inlines(annotation);
            trim_inlines(&mut inlines);
            out.push(Inline::Text(" ".into()));
            out.extend(inlines);
        }
        out
    }

    /// Adds a node in block context: inline nodes gather into a paragraph,
    /// block elements flush it.
    fn push_node(&mut self, node: Node<'a, '_>, run: &mut Vec<Inline>, out: &mut Vec<Block>) {
        if node.is_text() || INLINE.contains(&name(node)) {
            self.inline(node, run);
        } else if node.is_element() {
            flush(run, out);
            self.block(node, out);
        }
    }

    /// The children of a node as blocks.
    fn blocks_of(&mut self, node: Node<'a, '_>) -> Vec<Block> {
        let mut out = Vec::new();
        let mut run = Vec::new();
        for c in node.children() {
            if name(c) != "name" {
                self.push_node(c, &mut run, &mut out);
            }
        }
        flush(&mut run, &mut out);
        out
    }

    /// The content of an element that holds either inline content or blocks.
    fn flow(&mut self, node: Node<'a, '_>) -> Flow {
        let has_blocks = node
            .children()
            .any(|c| c.is_element() && !INLINE.contains(&name(c)));
        if has_blocks {
            Flow::Blocks(self.blocks_of(node))
        } else {
            let mut inlines = self.inlines(node);
            trim_inlines(&mut inlines);
            Flow::Inline(inlines)
        }
    }

    fn block(&mut self, node: Node<'a, '_>, out: &mut Vec<Block>) {
        let id = self.id_for(node, false);
        let kind = match name(node) {
            "t" => {
                let mut inlines = self.inlines(node);
                trim_inlines(&mut inlines);
                BlockKind::Para(inlines)
            }
            "ul" | "ol" => self.list(node),
            "dl" => {
                let mut items = Vec::new();
                for c in node.children().filter(Node::is_element) {
                    let term = name(c) == "dt";
                    if !term && name(c) != "dd" {
                        self.warn(Warning::UnknownElement(c.tag_name().name().to_string()));
                    }
                    let item_id = self.id_for(c, false);
                    items.push((
                        term,
                        Item {
                            id: item_id,
                            content: self.flow(c),
                        },
                    ));
                }
                let classes = if node.attribute("newline") == Some("true") {
                    vec!["newline"]
                } else {
                    vec![]
                };
                BlockKind::Defs { classes, items }
            }
            "table" => BlockKind::Table(self.table(node)),
            "figure" => {
                let blocks = self.blocks_of(node);
                let caption = caption(self, node, "figure-", "Figure");
                BlockKind::Figure { blocks, caption }
            }
            "artwork" => return self.artwork(node, id, out),
            "artset" => {
                let chosen = children(node, "artwork")
                    .find(|a| a.attribute("type") == Some("svg"))
                    .or_else(|| child(node, "artwork"));
                if let Some(artwork) = chosen {
                    let id = id.or_else(|| self.id_for(artwork, false));
                    self.artwork(artwork, id, out);
                }
                return;
            }
            "sourcecode" => {
                let mut text = trim_blank_lines(&raw_text(node));
                if node.attribute("markers") == Some("true") {
                    let file = node
                        .attribute("name")
                        .filter(|n| !n.trim().is_empty())
                        .map(|n| format!(" file \"{}\"", n.trim()))
                        .unwrap_or_default();
                    text = format!("<CODE BEGINS>{file}\n{text}\n<CODE ENDS>");
                }
                BlockKind::Pre {
                    class: "sourcecode",
                    content: vec![Inline::Text(text)],
                }
            }
            "blockquote" => BlockKind::Quote(self.blocks_of(node)),
            "aside" => BlockKind::Aside(self.blocks_of(node)),
            "author" => BlockKind::Address(self.address(node)),
            _ => {
                let qualified = node.tag_name().name().to_string();
                self.warn(Warning::UnknownElement(qualified));
                out.extend(self.blocks_of(node));
                return;
            }
        };
        out.push(Block { id, kind });
    }

    fn list(&mut self, node: Node<'a, '_>) -> BlockKind {
        let ordered = name(node) == "ol";
        let mut classes = Vec::new();
        let mut attrs = Vec::new();
        let mut custom = false;
        if ordered {
            let kind = node.attribute("type").unwrap_or("1");
            if matches!(kind, "1" | "a" | "A" | "i" | "I") {
                attrs.push(("type", kind.to_string()));
                if let Some(start) = node
                    .attribute("start")
                    .and_then(|s| s.trim().parse::<u32>().ok())
                {
                    attrs.push(("start", start.to_string()));
                }
            } else {
                classes.push("custom");
                custom = true;
            }
        } else {
            if node.attribute("empty") == Some("true") {
                classes.push("plain");
            }
            if node.attribute("spacing") == Some("compact") {
                classes.push("compact");
            }
        }
        let mut items = Vec::new();
        for li in node.children().filter(Node::is_element) {
            if name(li) != "li" {
                self.warn(Warning::UnknownElement(li.tag_name().name().to_string()));
            }
            let mut content = self.flow(li);
            if custom {
                let counter = li
                    .attribute("derivedCounter")
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let marker = vec![
                    Inline::Span("marker", vec![Inline::Text(counter)]),
                    Inline::Text(" ".into()),
                ];
                match &mut content {
                    Flow::Inline(inlines) => {
                        inlines.splice(0..0, marker);
                    }
                    Flow::Blocks(blocks) => match blocks.first_mut() {
                        Some(Block {
                            kind: BlockKind::Para(inlines),
                            ..
                        }) => {
                            inlines.splice(0..0, marker);
                        }
                        _ => blocks.insert(0, Block::new(BlockKind::Para(marker))),
                    },
                }
            }
            items.push(Item {
                id: self.id_for(li, false),
                content,
            });
        }
        BlockKind::List {
            ordered,
            attrs,
            classes,
            items,
        }
    }

    fn table(&mut self, node: Node<'a, '_>) -> Table {
        let mut table = Table {
            caption: caption(self, node, "table-", "Table"),
            ..Table::default()
        };
        for part in node.children().filter(Node::is_element) {
            match name(part) {
                "name" => {}
                "iref" => {
                    if let Some(id) = self.id_for(part, false) {
                        table
                            .caption
                            .get_or_insert_with(Vec::new)
                            .insert(0, Inline::Anchor(id));
                    }
                }
                "thead" => table.head.extend(self.rows(part)),
                "tbody" => table.body.extend(self.rows(part)),
                "tfoot" => table.foot.extend(self.rows(part)),
                "tr" => table.body.push(self.row(part)),
                _ => {
                    // Keep the text as a row of its own.
                    self.warn(Warning::UnknownElement(part.tag_name().name().to_string()));
                    let content = Flow::Blocks(self.blocks_of(part));
                    table.body.push(vec![Cell {
                        header: false,
                        align: None,
                        colspan: 1,
                        rowspan: 1,
                        item: Item { id: None, content },
                    }]);
                }
            }
        }
        table
    }

    fn rows(&mut self, part: Node<'a, '_>) -> Vec<Vec<Cell>> {
        children(part, "tr").map(|tr| self.row(tr)).collect()
    }

    fn row(&mut self, tr: Node<'a, '_>) -> Vec<Cell> {
        let mut cells = Vec::new();
        for cell in tr.children().filter(|c| matches!(name(*c), "td" | "th")) {
            let span = |attr: &str| {
                cell.attribute(attr)
                    .and_then(|v| v.trim().parse::<u32>().ok())
                    .filter(|v| (1..=1000).contains(v))
                    .unwrap_or(1)
            };
            cells.push(Cell {
                header: name(cell) == "th",
                align: match cell.attribute("align") {
                    Some("left") => Some("left"),
                    Some("center") => Some("center"),
                    Some("right") => Some("right"),
                    _ => None,
                },
                colspan: span("colspan"),
                rowspan: span("rowspan"),
                item: Item {
                    id: self.id_for(cell, false),
                    content: self.flow(cell),
                },
            });
        }
        cells
    }

    /// Text artwork, inline SVG, or the alt text of binary artwork.
    fn artwork(&mut self, node: Node<'a, '_>, id: Option<String>, out: &mut Vec<Block>) {
        let svg = node.children().find(|c| {
            c.is_element()
                && c.tag_name().namespace() == Some(SVG_NS)
                && c.tag_name().name() == "svg"
        });
        if let Some(svg) = svg {
            out.push(Block {
                id,
                kind: BlockKind::Svg(sanitize_svg(svg)),
            });
            return;
        }
        let text = trim_blank_lines(&raw_text(node));
        let kind = node.attribute("type").unwrap_or("");
        if text.is_empty() || kind == "binary-art" {
            let alt = collapse(node.attribute("alt").unwrap_or(""));
            let what = node.attribute("src").unwrap_or(kind);
            self.warn(Warning::OmittedContent(format!(
                "artwork without text ({what})"
            )));
            if !alt.is_empty() {
                out.push(Block {
                    id,
                    kind: BlockKind::Para(vec![Inline::Text(alt)]),
                });
            }
            return;
        }
        out.push(Block {
            id,
            kind: BlockKind::Pre {
                class: "artwork",
                content: vec![Inline::Text(text)],
            },
        });
    }

    /// An authors' address block.
    fn address(&mut self, node: Node<'a, '_>) -> Vec<Vec<Inline>> {
        let mut lines: Vec<Vec<Inline>> = Vec::new();
        let mut line = |text: String| {
            if !text.is_empty() {
                lines.push(vec![Inline::Text(text)]);
            }
        };
        if let Some(author) = author(node) {
            let suffix = if author.editor { " (editor)" } else { "" };
            if author.organization.as_deref() != Some(author.name.as_str()) {
                line(format!("{}{suffix}", author.name));
            }
        }
        line(text_of(child(node, "organization")));
        let address = child(node, "address");
        if let Some(postal) = address.and_then(|a| child(a, "postal")) {
            let mut locality = String::new();
            for part in postal.children().filter(Node::is_element) {
                let text = text_of(Some(part));
                match name(part) {
                    "city" => locality = text,
                    "region" if locality.is_empty() => locality = text,
                    "region" => locality = format!("{locality}, {text}"),
                    "code" if locality.is_empty() => locality = text,
                    "code" => locality = format!("{locality} {text}"),
                    _ => {
                        line(std::mem::take(&mut locality));
                        line(text);
                    }
                }
            }
            line(locality);
        }
        for part in address
            .into_iter()
            .flat_map(|a| a.children().filter(Node::is_element))
        {
            let text = text_of(Some(part));
            if text.is_empty() {
                continue;
            }
            match name(part) {
                "phone" => lines.push(vec![Inline::Text(format!("Phone: {text}"))]),
                "facsimile" => lines.push(vec![Inline::Text(format!("Fax: {text}"))]),
                "email" => lines.push(vec![
                    Inline::Text("Email: ".into()),
                    Inline::Link(
                        Target::External(format!("mailto:{text}")),
                        vec![Inline::Text(text)],
                    ),
                ]),
                "uri" if is_absolute(&text) => lines.push(vec![
                    Inline::Text("URI: ".into()),
                    Inline::Link(Target::External(text.clone()), vec![Inline::Text(text)]),
                ]),
                "uri" => lines.push(vec![Inline::Text(format!("URI: {text}"))]),
                _ => {}
            }
        }
        lines
    }

    fn inlines(&mut self, node: Node<'a, '_>) -> Vec<Inline> {
        let mut out = Vec::new();
        for c in node.children() {
            self.inline(c, &mut out);
        }
        out
    }

    fn inline(&mut self, node: Node<'a, '_>, out: &mut Vec<Inline>) {
        if node.is_text() {
            out.push(Inline::Text(squeeze(node.text().unwrap_or(""))));
            return;
        }
        if !node.is_element() {
            return;
        }
        match name(node) {
            "em" => out.push(Inline::Tag("em", self.inlines(node))),
            "strong" => out.push(Inline::Tag("strong", self.inlines(node))),
            "sub" => out.push(Inline::Tag("sub", self.inlines(node))),
            "sup" => out.push(Inline::Tag("sup", self.inlines(node))),
            "tt" => out.push(Inline::Tag("code", self.inlines(node))),
            "bcp14" => out.push(Inline::Span("bcp14", self.inlines(node))),
            "br" => out.push(Inline::Break),
            "contact" => {
                let name = node
                    .attribute("fullname")
                    .or(node.attribute("asciiFullname"));
                out.push(Inline::Text(collapse(name.unwrap_or(""))));
            }
            "u" => {
                let text = raw_text(node);
                let points: Vec<String> = text
                    .chars()
                    .map(|c| format!("U+{:04X}", u32::from(c)))
                    .collect();
                out.push(Inline::Text(format!("{text} ({})", points.join(" "))));
            }
            "iref" => {
                if let Some(id) = self.id_for(node, false) {
                    out.push(Inline::Anchor(id));
                }
            }
            "cref" => self.warn(Warning::OmittedContent("editorial comment (cref)".into())),
            "eref" => {
                let target = node.attribute("target").unwrap_or("").trim().to_string();
                let mut text = self.inlines(node);
                trim_inlines(&mut text);
                if text.is_empty() {
                    text.push(Inline::Text(target.clone()));
                }
                let angle = node.attribute("brackets") == Some("angle");
                if angle {
                    out.push(Inline::Text("<".into()));
                }
                if is_absolute(&target) {
                    out.push(Inline::Link(Target::External(target), text));
                } else {
                    out.extend(text);
                }
                if angle {
                    out.push(Inline::Text(">".into()));
                }
            }
            "xref" | "relref" => self.xref(node, out),
            _ => {
                let qualified = node.tag_name().name().to_string();
                self.warn(Warning::UnknownElement(qualified));
                for c in node.children() {
                    self.inline(c, out);
                }
            }
        }
    }

    /// A cross-reference.
    fn xref(&mut self, node: Node<'a, '_>, out: &mut Vec<Inline>) {
        let target = node.attribute("target").unwrap_or("");
        let dc = collapse(node.attribute("derivedContent").unwrap_or(""));
        let mut text = self.inlines(node);
        trim_inlines(&mut text);
        let resolved = self.keys.get(target).copied();
        let section = node.attribute("section").map(collapse);

        if section.is_some() || name(node) == "relref" {
            let number = section.unwrap_or_default();
            let label = if dc.is_empty() {
                format!("[{target}]")
            } else {
                format!("[{dc}]")
            };
            let label = match resolved {
                Some((id, _)) => {
                    Inline::Link(Target::Internal(id.to_string()), vec![Inline::Text(label)])
                }
                None => {
                    self.warn(Warning::BrokenLink(target.to_string()));
                    Inline::Text(label)
                }
            };
            let derived = node.attribute("derivedLink").filter(|l| is_absolute(l));
            let linked = |content: Vec<Inline>| match derived {
                Some(url) => vec![Inline::Link(Target::External(url.to_string()), content)],
                None => content,
            };
            let sec = linked(vec![Inline::Text(format!("Section {number}"))]);
            let mut formatted = Vec::new();
            match node.attribute("sectionFormat").unwrap_or("of") {
                "bare" => {
                    out.extend(linked(vec![Inline::Text(number)]));
                    if !text.is_empty() {
                        out.push(Inline::Text(" (".into()));
                        out.extend(linked(text));
                        out.push(Inline::Text(")".into()));
                    }
                    return;
                }
                "comma" => {
                    formatted.push(label);
                    formatted.push(Inline::Text(", ".into()));
                    formatted.extend(sec);
                }
                "parens" => {
                    formatted.push(label);
                    formatted.push(Inline::Text(" (".into()));
                    formatted.extend(sec);
                    formatted.push(Inline::Text(")".into()));
                }
                _ => {
                    formatted.extend(sec);
                    formatted.push(Inline::Text(" of ".into()));
                    formatted.push(label);
                }
            }
            if text.is_empty() {
                out.extend(formatted);
            } else {
                out.extend(text);
                out.push(Inline::Text(" (".into()));
                out.extend(formatted);
                out.push(Inline::Text(")".into()));
            }
            return;
        }

        let bib = resolved.is_some_and(|(_, bib)| bib);
        let content = if text.is_empty() {
            let shown = if dc.is_empty() {
                target.to_string()
            } else {
                dc.clone()
            };
            vec![Inline::Text(if bib { format!("[{shown}]") } else { shown })]
        } else if node.attribute("format") == Some("none") || dc.is_empty() {
            text
        } else {
            text.push(Inline::Text(if bib {
                format!(" [{dc}]")
            } else {
                format!(" ({dc})")
            }));
            text
        };
        match resolved {
            Some((id, _)) => out.push(Inline::Link(Target::Internal(id.to_string()), content)),
            None => {
                self.warn(Warning::BrokenLink(target.to_string()));
                out.extend(content);
            }
        }
    }
}

/// Moves a run of inline content into a paragraph, unless it is blank.
fn flush(run: &mut Vec<Inline>, out: &mut Vec<Block>) {
    trim_inlines(run);
    if !run.is_empty() {
        out.push(Block::new(BlockKind::Para(std::mem::take(run))));
    }
    run.clear();
}

/// `Figure N: name` or `Table N: name`, with N taken from `pn`.
fn caption<'a>(
    p: &mut Parser<'a>,
    node: Node<'a, '_>,
    prefix: &str,
    word: &str,
) -> Option<Vec<Inline>> {
    let number = node.attribute("pn").and_then(|pn| pn.strip_prefix(prefix));
    let mut name = child(node, "name")
        .map(|n| p.inlines(n))
        .unwrap_or_default();
    trim_inlines(&mut name);
    match (number, name.is_empty()) {
        (Some(n), true) => Some(vec![Inline::Text(format!("{word} {n}"))]),
        (Some(n), false) => {
            name.insert(0, Inline::Text(format!("{word} {n}: ")));
            Some(name)
        }
        (None, false) => Some(name),
        (None, true) => None,
    }
}

/// The id of an element: its anchor, else its pn.
fn canonical<'a>(node: Node<'a, '_>) -> Option<&'a str> {
    node.attribute("anchor").or(node.attribute("pn"))
}

/// The id and kind of the element that owns an anchor or pn. A reference
/// inside a reference group is cited through the group.
fn owner<'a>(node: Node<'a, '_>) -> (&'a str, bool) {
    let node = match node.parent_element() {
        Some(group) if name(node) == "reference" && name(group) == "referencegroup" => group,
        _ => node,
    };
    let bib = matches!(name(node), "reference" | "referencegroup");
    (canonical(node).unwrap_or(""), bib)
}

/// The bibliography label: `derivedAnchor`, else `anchor`.
fn label(node: Node) -> String {
    collapse(
        node.attribute("derivedAnchor")
            .or(node.attribute("anchor"))
            .unwrap_or(""),
    )
}

/// `<url>`, with the URL as a link.
fn angle_link(target: &str) -> Vec<Inline> {
    let target = target.trim().to_string();
    let link = if is_absolute(&target) {
        Inline::Link(Target::External(target.clone()), vec![Inline::Text(target)])
    } else {
        Inline::Text(target)
    };
    vec![Inline::Text("<".into()), link, Inline::Text(">".into())]
}

/// Whether a URL has a scheme, so that it does not resolve inside the book.
fn is_absolute(url: &str) -> bool {
    match url.find(':') {
        Some(colon) if colon > 0 => {
            let scheme = &url[..colon];
            scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
                && scheme.as_bytes()[0].is_ascii_alphabetic()
                && !url.chars().any(char::is_whitespace)
        }
        _ => false,
    }
}

/// The authors of a cited document.
fn citation_authors(front: Node) -> String {
    let mut names: Vec<(String, String)> = Vec::new();
    for author in children(front, "author") {
        let surname = collapse(author.attribute("surname").unwrap_or(""));
        let initials = collapse(author.attribute("initials").unwrap_or(""));
        let (first, last) = if !surname.is_empty() {
            if initials.is_empty() {
                (surname.clone(), surname)
            } else {
                (
                    format!("{surname}, {initials}"),
                    format!("{initials} {surname}"),
                )
            }
        } else {
            let org = text_of(child(author, "organization"));
            if org.is_empty() {
                continue;
            }
            (org.clone(), org)
        };
        let ed = if author.attribute("role") == Some("editor") {
            ", Ed."
        } else {
            ""
        };
        names.push((format!("{first}{ed}"), format!("{last}{ed}")));
    }
    match names.len() {
        0 => String::new(),
        1 => names.swap_remove(0).0,
        2 => format!("{} and {}", names[0].0, names[1].1),
        n => {
            let head: Vec<&str> = names[..n - 1]
                .iter()
                .map(|(first, _)| first.as_str())
                .collect();
            format!("{}, and {}", head.join(", "), names[n - 1].1)
        }
    }
}

/// Serializes an SVG drawing, keeping only SVG elements and safe attributes:
/// no scripts, event handlers, foreign content, styles or external links.
fn sanitize_svg(svg: Node) -> String {
    let xlink = svg.descendants().any(|n| {
        n.attributes()
            .any(|a| a.namespace() == Some(XLINK_NS) && a.value().starts_with('#'))
    });
    let mut out = String::new();
    write_svg(svg, &mut out, true, xlink);
    out
}

fn write_svg(node: Node, out: &mut String, root: bool, xlink: bool) {
    if node.is_text() {
        escape_into(out, node.text().unwrap_or(""));
        return;
    }
    if !node.is_element() || node.tag_name().namespace() != Some(SVG_NS) {
        return;
    }
    let tag = node.tag_name().name();
    if tag.eq_ignore_ascii_case("script")
        || tag.eq_ignore_ascii_case("foreignObject")
        || tag == "style"
    {
        return;
    }
    out.push('<');
    out.push_str(tag);
    if root {
        out.push_str(" xmlns=\"http://www.w3.org/2000/svg\"");
        if xlink {
            out.push_str(" xmlns:xlink=\"http://www.w3.org/1999/xlink\"");
        }
    }
    for attr in node.attributes() {
        let local = attr.name();
        let value = attr.value();
        let qualified = match attr.namespace() {
            None if local.to_ascii_lowercase().starts_with("on") || local == "style" => continue,
            None if local == "href" && !value.starts_with('#') => continue,
            None => local.to_string(),
            Some(XLINK_NS) if local == "href" && value.starts_with('#') => "xlink:href".to_string(),
            Some(XML_NS) if local == "space" || local == "lang" => format!("xml:{local}"),
            Some(_) => continue,
        };
        out.push(' ');
        out.push_str(&qualified);
        out.push_str("=\"");
        escape_into(out, value);
        out.push('"');
    }
    if node.has_children() {
        out.push('>');
        for c in node.children() {
            write_svg(c, out, false, xlink);
        }
        out.push_str("</");
        out.push_str(tag);
        out.push('>');
    } else {
        out.push_str("/>");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_numbers() {
        assert_eq!(heading_number("section-3").as_deref(), Some("3."));
        assert_eq!(heading_number("section-3.1.2").as_deref(), Some("3.1.2."));
        assert_eq!(
            heading_number("section-appendix.a").as_deref(),
            Some("Appendix A.")
        );
        assert_eq!(
            heading_number("section-appendix.a.1").as_deref(),
            Some("A.1.")
        );
        assert_eq!(heading_number("section-abstract"), None);
        assert_eq!(heading_number("section-boilerplate.1"), None);
    }

    fn cite(authors: &str) -> String {
        let xml = format!("<front>{authors}</front>");
        let doc = roxmltree::Document::parse(&xml).unwrap();
        citation_authors(doc.root_element())
    }

    #[test]
    fn citation_author_lists() {
        let a = r#"<author surname="Bishop" initials="M." role="editor"/>"#;
        let b = r#"<author surname="Thomson" initials="M."/>"#;
        let c = r#"<author surname="Benfield" initials="C."/>"#;
        let org = r#"<author><organization>IANA</organization></author>"#;
        assert_eq!(cite(a), "Bishop, M., Ed.");
        assert_eq!(cite(&format!("{b}{c}")), "Thomson, M. and C. Benfield");
        assert_eq!(
            cite(&format!("{a}{b}{c}")),
            "Bishop, M., Ed., Thomson, M., and C. Benfield"
        );
        assert_eq!(cite(org), "IANA");
        assert_eq!(cite("<author/>"), "");
    }

    #[test]
    fn package_authors() {
        let doc = roxmltree::Document::parse(
            r#"<a><author fullname="Robert M. Hinden" initials="R" surname="Hinden"><organization>Check Point</organization></author>
               <author><organization>IAB</organization></author></a>"#,
        )
        .unwrap();
        let authors: Vec<Author> = children(doc.root_element(), "author")
            .filter_map(author)
            .collect();
        assert_eq!(authors[0].file_as, "Hinden, Robert M.");
        assert_eq!(authors[0].organization.as_deref(), Some("Check Point"));
        assert_eq!(
            (authors[1].name.as_str(), authors[1].file_as.as_str()),
            ("IAB", "IAB")
        );
    }

    fn render(body: &str) -> String {
        let xml = format!(
            r#"<rfc version="3" number="1"><front><title>T</title><date year="2020" month="1"/></front>
               <middle><section pn="section-1" anchor="intro"><name slugifiedName="name-intro">Intro</name>{body}</section></middle>
               <back><references pn="section-2"><name>References</name>
               <reference anchor="RFC2119" derivedAnchor="RFC2119" target="https://www.rfc-editor.org/info/rfc2119">
               <front><title>Key words</title><author surname="Bradner" initials="S."/><date month="March" year="1997"/></front>
               <seriesInfo name="BCP" value="14"/><seriesInfo name="RFC" value="2119"/></reference></references></back></rfc>"#
        );
        let doc = parse(&xml).unwrap();
        let mut files = HashMap::new();
        for (i, s) in doc.sections.iter().enumerate() {
            crate::render::collect_ids(s, &format!("s{i}"), &mut files);
        }
        let mut w = crate::render::Writer::new(&files, "s0");
        w.blocks(&doc.sections[0].blocks);
        w.finish()
    }

    #[test]
    fn cross_references() {
        let out = render(r#"<t>See <xref target="RFC2119" derivedContent="RFC2119"/>.</t>"#);
        assert!(
            out.contains(r##"See <a href="s1#RFC2119">[RFC2119]</a>."##),
            "{out}"
        );
        let out = render(r#"<t><xref target="intro" derivedContent="Section 1"/></t>"#);
        assert!(out.contains(r##"<a href="#intro">Section 1</a>"##), "{out}");
        let out =
            render(r#"<t><xref target="name-intro" derivedContent="Section 1">here</xref></t>"#);
        assert!(
            out.contains(r##"<a href="#intro">here (Section 1)</a>"##),
            "{out}"
        );
        let out = render(
            r#"<t><xref target="RFC2119" derivedContent="RFC2119" format="none">BCP 14</xref></t>"#,
        );
        assert!(
            out.contains(r##"<a href="s1#RFC2119">BCP 14</a>"##),
            "{out}"
        );
        let out = render(
            r#"<t><xref target="RFC2119" section="2" sectionFormat="of" derivedContent="RFC2119" derivedLink="https://x/#s2"/></t>"#,
        );
        assert!(
            out.contains(
                r##"<a href="https://x/#s2">Section 2</a> of <a href="s1#RFC2119">[RFC2119]</a>"##
            ),
            "{out}"
        );
        let out = render(
            r#"<t><xref target="RFC2119" section="2" sectionFormat="comma" derivedContent="RFC2119"/></t>"#,
        );
        assert!(
            out.contains(r##"<a href="s1#RFC2119">[RFC2119]</a>, Section 2"##),
            "{out}"
        );
        let out = render(
            r#"<t><xref target="RFC2119" section="2" sectionFormat="bare" derivedContent="RFC2119" derivedLink="https://x/">t</xref></t>"#,
        );
        assert!(
            out.contains(r##"<a href="https://x/">2</a> (<a href="https://x/">t</a>)"##),
            "{out}"
        );
        let out = render(r#"<t><xref target="nowhere" derivedContent="X"/></t>"#);
        assert!(out.contains("<p>X</p>"), "{out}");
    }

    #[test]
    fn citations_and_blocks() {
        let xml = r#"<rfc version="3" number="1"><front><title>T</title><date year="2020" month="1"/></front>
            <back><references pn="section-1"><name>References</name>
            <reference anchor="RFC2119" derivedAnchor="RFC2119" target="https://www.rfc-editor.org/info/rfc2119">
            <front><title>Key words</title><author surname="Bradner" initials="S."/><date month="March" year="1997"/></front>
            <seriesInfo name="BCP" value="14"/><seriesInfo name="RFC" value="2119"/></reference></references></back></rfc>"#;
        let doc = parse(xml).unwrap();
        let BlockKind::References(entries) = &doc.sections[0].blocks[0].kind else {
            panic!("no bibliography")
        };
        assert_eq!(
            plain_text(&entries[0].citation),
            "Bradner, S., \"Key words\", BCP 14, RFC 2119, March 1997, <https://www.rfc-editor.org/info/rfc2119>."
        );
        let out = render(
            r#"<ol type="(%d)"><li derivedCounter="(1)">one</li></ol><sourcecode markers="true" name="a.txt">
x
</sourcecode><t><u>ü</u></t>"#,
        );
        assert!(
            out.contains(r#"<li><span class="marker">(1)</span> one</li>"#),
            "{out}"
        );
        assert!(
            out.contains("&lt;CODE BEGINS&gt; file &quot;a.txt&quot;\nx\n&lt;CODE ENDS&gt;"),
            "{out}"
        );
        assert!(out.contains("ü (U+00FC)"), "{out}");
    }

    #[test]
    fn svg_is_sanitized() {
        let doc = roxmltree::Document::parse(
            r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:x="http://www.w3.org/1999/xlink" xmlns:h="urn:h" onload="evil()">
                <script>evil()</script><a href="https://evil"/><use x:href="#p"/><h:div/><foreignObject/></svg>"##,
        )
        .unwrap();
        let out = sanitize_svg(doc.root_element());
        assert!(
            !out.contains("evil") && !out.contains("div") && !out.contains("foreignObject"),
            "{out}"
        );
        assert!(out.contains(r##"<use xlink:href="#p"/>"##), "{out}");
    }

    #[test]
    fn rejects_other_documents() {
        assert!(matches!(
            parse("<rfc version=\"2\" number=\"1\"/>"),
            Err(Error::InvalidSource(_))
        ));
        assert!(matches!(
            parse("<rfc version=\"3\"/>"),
            Err(Error::InvalidSource(_))
        ));
        assert!(matches!(parse("not xml"), Err(Error::InvalidSource(_))));
    }
}
