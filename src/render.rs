//! The document model as XHTML content documents (§7.4).

use std::collections::HashMap;

use crate::model::{
    Block, BlockKind, Cell, Flow, Inline, Item, Reference, Section, Target, escape, escape_into,
};

/// Preformatted blocks, tables and figures this short avoid page breaks
/// inside them (§7.5).
const SHORT_LINES: usize = 24;
const SHORT_ROWS: usize = 12;

/// The skeleton shared by every XHTML document in the book.
pub(crate) fn page(lang: &str, title: &str, body: &str) -> String {
    let lang = escape(lang);
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html>\n\
         <html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" \
         lang=\"{lang}\" xml:lang=\"{lang}\">\n<head>\n<title>{}</title>\n\
         <link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\"/>\n</head>\n<body>\n{body}</body>\n</html>\n",
        escape(title)
    )
}

/// Writes the body of one content document.
pub(crate) struct Writer<'a> {
    out: String,
    /// The content document that holds each id.
    files: &'a HashMap<String, String>,
    /// The content document being written.
    file: &'a str,
    in_link: bool,
    /// Whether the document contains inline SVG.
    pub svg: bool,
}

impl<'a> Writer<'a> {
    pub fn new(files: &'a HashMap<String, String>, file: &'a str) -> Writer<'a> {
        Writer {
            out: String::new(),
            files,
            file,
            in_link: false,
            svg: false,
        }
    }

    pub fn finish(self) -> String {
        self.out
    }

    /// A section, its heading at `level` (1 to 6), and its subsections.
    pub fn section(&mut self, section: &Section, level: usize) {
        self.out.push_str("<section");
        self.id_attr(section.id.as_deref());
        self.out.push_str(">\n");
        if section.number.is_some() || !section.title.is_empty() {
            self.out.push_str(&format!("<h{level}>"));
            if let Some(number) = &section.number {
                self.out.push_str("<span class=\"secno\">");
                escape_into(&mut self.out, number);
                self.out.push_str("</span>");
                if !section.title.is_empty() {
                    self.out.push(' ');
                }
            }
            self.inlines(&section.title);
            self.out.push_str(&format!("</h{level}>\n"));
        }
        self.blocks(&section.blocks);
        for child in &section.children {
            self.section(child, (level + 1).min(6));
        }
        self.out.push_str("</section>\n");
    }

    fn id_attr(&mut self, id: Option<&str>) {
        if let Some(id) = id {
            self.out.push_str(" id=\"");
            escape_into(&mut self.out, id);
            self.out.push('"');
        }
    }

    fn open(&mut self, tag: &str, id: Option<&str>, classes: &[&str]) {
        self.open_with(tag, id, classes, &[]);
    }

    fn open_with(
        &mut self,
        tag: &str,
        id: Option<&str>,
        classes: &[&str],
        attrs: &[(&str, String)],
    ) {
        self.out.push('<');
        self.out.push_str(tag);
        self.id_attr(id);
        if !classes.is_empty() {
            self.out.push_str(" class=\"");
            self.out.push_str(&classes.join(" "));
            self.out.push('"');
        }
        for (name, value) in attrs {
            self.out.push_str(&format!(" {name}=\""));
            escape_into(&mut self.out, value);
            self.out.push('"');
        }
        self.out.push('>');
    }

    fn close(&mut self, tag: &str) {
        self.out.push_str("</");
        self.out.push_str(tag);
        self.out.push_str(">\n");
    }

    pub fn blocks(&mut self, blocks: &[Block]) {
        for block in blocks {
            self.block(block);
        }
    }

    fn block(&mut self, block: &Block) {
        let id = block.id.as_deref();
        match &block.kind {
            BlockKind::Para(inlines) => {
                self.open("p", id, &[]);
                self.inlines(inlines);
                self.close("p");
            }
            BlockKind::List {
                ordered,
                attrs,
                classes,
                items,
            } => {
                let tag = if *ordered { "ol" } else { "ul" };
                let attrs: Vec<(&str, String)> =
                    attrs.iter().map(|(n, v)| (*n, v.clone())).collect();
                self.open_with(tag, id, classes, &attrs);
                self.out.push('\n');
                for item in items {
                    self.item("li", item, &[]);
                }
                self.close(tag);
            }
            BlockKind::Defs { classes, items } => {
                self.open("dl", id, classes);
                self.out.push('\n');
                // HTML requires each group to start with a term and end with a
                // description.
                let mut last_term = None;
                for (term, item) in items {
                    if !term && last_term.is_none() {
                        self.out.push_str("<dt></dt>\n");
                    }
                    self.item(if *term { "dt" } else { "dd" }, item, &[]);
                    last_term = Some(*term);
                }
                if last_term == Some(true) {
                    self.out.push_str("<dd></dd>\n");
                }
                self.close("dl");
            }
            BlockKind::Pre { class, content } => {
                let lines = pre_lines(content);
                let mut classes = vec![*class];
                if lines <= SHORT_LINES {
                    classes.push("keep");
                }
                self.open("pre", id, &classes);
                if *class == "sourcecode" {
                    self.out.push_str("<code>");
                }
                self.inlines(content);
                if *class == "sourcecode" {
                    self.out.push_str("</code>");
                }
                self.close("pre");
            }
            BlockKind::Figure { blocks, caption } => {
                let short = blocks.iter().all(|b| match &b.kind {
                    BlockKind::Pre { content, .. } => pre_lines(content) <= SHORT_LINES,
                    BlockKind::Svg(_) => true,
                    _ => false,
                });
                self.open("figure", id, if short { &["keep"] } else { &[] });
                self.out.push('\n');
                self.blocks(blocks);
                if let Some(caption) = caption {
                    self.out.push_str("<figcaption>");
                    self.inlines(caption);
                    self.close("figcaption");
                }
                self.close("figure");
            }
            BlockKind::Table(table) => {
                let rows = table.head.len() + table.body.len() + table.foot.len();
                self.open(
                    "table",
                    id,
                    if rows <= SHORT_ROWS { &["keep"] } else { &[] },
                );
                self.out.push('\n');
                if let Some(caption) = &table.caption {
                    self.out.push_str("<caption>");
                    self.inlines(caption);
                    self.close("caption");
                }
                for (tag, rows) in [
                    ("thead", &table.head),
                    ("tbody", &table.body),
                    ("tfoot", &table.foot),
                ] {
                    if rows.is_empty() {
                        continue;
                    }
                    self.out.push_str(&format!("<{tag}>\n"));
                    for row in rows {
                        self.out.push_str("<tr>\n");
                        for cell in row {
                            self.cell(cell);
                        }
                        self.close("tr");
                    }
                    self.close(tag);
                }
                self.close("table");
            }
            BlockKind::Svg(svg) => {
                self.svg = true;
                self.open("div", id, &["svg", "keep"]);
                self.out.push_str(svg);
                self.close("div");
            }
            BlockKind::Quote(inner) => {
                self.open("blockquote", id, &[]);
                self.out.push('\n');
                self.blocks(inner);
                self.close("blockquote");
            }
            BlockKind::Aside(inner) => {
                self.open("aside", id, &[]);
                self.out.push('\n');
                self.blocks(inner);
                self.close("aside");
            }
            BlockKind::References(entries) => {
                self.open("dl", id, &["references"]);
                self.out.push('\n');
                for entry in entries {
                    self.reference(entry);
                }
                self.close("dl");
            }
            BlockKind::Address(lines) => {
                self.open("address", id, &["author"]);
                for (i, line) in lines.iter().enumerate() {
                    if i > 0 {
                        self.out.push_str("<br/>\n");
                    }
                    self.inlines(line);
                }
                self.close("address");
            }
        }
    }

    fn reference(&mut self, entry: &Reference) {
        self.open("dt", entry.id.as_deref(), &[]);
        self.out.push('[');
        escape_into(&mut self.out, &entry.label);
        self.out.push(']');
        self.close("dt");
        self.out.push_str("<dd>");
        self.inlines(&entry.citation);
        self.close("dd");
    }

    fn cell(&mut self, cell: &Cell) {
        let tag = if cell.header { "th" } else { "td" };
        let classes: Vec<&str> = cell.align.into_iter().collect();
        let mut attrs = Vec::new();
        if cell.colspan != 1 {
            attrs.push(("colspan", cell.colspan.to_string()));
        }
        if cell.rowspan != 1 {
            attrs.push(("rowspan", cell.rowspan.to_string()));
        }
        self.open_with(tag, cell.item.id.as_deref(), &classes, &attrs);
        self.flow(&cell.item.content);
        self.close(tag);
    }

    fn item(&mut self, tag: &str, item: &Item, classes: &[&str]) {
        self.open(tag, item.id.as_deref(), classes);
        self.flow(&item.content);
        self.close(tag);
    }

    fn flow(&mut self, flow: &Flow) {
        match flow {
            Flow::Inline(inlines) => self.inlines(inlines),
            Flow::Blocks(blocks) => {
                self.out.push('\n');
                self.blocks(blocks);
            }
        }
    }

    pub fn inlines(&mut self, inlines: &[Inline]) {
        for inline in inlines {
            match inline {
                Inline::Text(text) => escape_into(&mut self.out, text),
                Inline::Tag(tag, inner) => {
                    self.out.push_str(&format!("<{tag}>"));
                    self.inlines(inner);
                    self.out.push_str(&format!("</{tag}>"));
                }
                Inline::Span(class, inner) => {
                    self.out.push_str(&format!("<span class=\"{class}\">"));
                    self.inlines(inner);
                    self.out.push_str("</span>");
                }
                Inline::Link(target, inner) => {
                    if self.in_link {
                        self.inlines(inner);
                        continue;
                    }
                    self.out.push_str("<a href=\"");
                    let href = self.href(target);
                    escape_into(&mut self.out, &href);
                    self.out.push_str("\">");
                    self.in_link = true;
                    self.inlines(inner);
                    self.in_link = false;
                    self.out.push_str("</a>");
                }
                Inline::Anchor(id) => {
                    self.out.push_str("<span id=\"");
                    escape_into(&mut self.out, id);
                    self.out.push_str("\"></span>");
                }
                Inline::Break => self.out.push_str("<br/>"),
            }
        }
    }

    fn href(&self, target: &Target) -> String {
        match target {
            Target::External(url) => url.clone(),
            Target::Internal(id) => match self.files.get(id) {
                Some(file) if file == self.file => format!("#{id}"),
                Some(file) => format!("{file}#{id}"),
                // Unreachable after `Document::finish`; link to the document itself.
                None => format!("#{id}"),
            },
        }
    }
}

/// The number of lines of preformatted content.
fn pre_lines(content: &[Inline]) -> usize {
    fn count(inlines: &[Inline]) -> usize {
        inlines
            .iter()
            .map(|i| match i {
                Inline::Text(t) => t.matches('\n').count(),
                Inline::Tag(_, inner) | Inline::Span(_, inner) | Inline::Link(_, inner) => {
                    count(inner)
                }
                Inline::Break => 1,
                Inline::Anchor(_) => 0,
            })
            .sum()
    }
    count(content) + 1
}

/// Records which content document holds each id.
pub(crate) fn collect_ids(section: &Section, file: &str, files: &mut HashMap<String, String>) {
    let mut add = |id: &Option<String>| {
        if let Some(id) = id {
            files.entry(id.clone()).or_insert_with(|| file.to_string());
        }
    };
    add(&section.id);
    let mut ids = Vec::new();
    inline_ids(&section.title, &mut ids);
    block_ids(&section.blocks, &mut ids);
    for id in ids {
        files.entry(id).or_insert_with(|| file.to_string());
    }
    for child in &section.children {
        collect_ids(child, file, files);
    }
}

fn block_ids(blocks: &[Block], ids: &mut Vec<String>) {
    for block in blocks {
        ids.extend(block.id.clone());
        match &block.kind {
            BlockKind::Para(inlines)
            | BlockKind::Pre {
                content: inlines, ..
            } => inline_ids(inlines, ids),
            BlockKind::List { items, .. } => items.iter().for_each(|i| item_ids(i, ids)),
            BlockKind::Defs { items, .. } => items.iter().for_each(|(_, i)| item_ids(i, ids)),
            BlockKind::Figure { blocks, caption } => {
                block_ids(blocks, ids);
                if let Some(caption) = caption {
                    inline_ids(caption, ids);
                }
            }
            BlockKind::Table(table) => {
                if let Some(caption) = &table.caption {
                    inline_ids(caption, ids);
                }
                for row in table.head.iter().chain(&table.body).chain(&table.foot) {
                    row.iter().for_each(|cell| item_ids(&cell.item, ids));
                }
            }
            BlockKind::Quote(inner) | BlockKind::Aside(inner) => block_ids(inner, ids),
            BlockKind::References(entries) => {
                for entry in entries {
                    ids.extend(entry.id.clone());
                    inline_ids(&entry.citation, ids);
                }
            }
            BlockKind::Address(lines) => lines.iter().for_each(|l| inline_ids(l, ids)),
            BlockKind::Svg(_) => {}
        }
    }
}

fn item_ids(item: &Item, ids: &mut Vec<String>) {
    ids.extend(item.id.clone());
    match &item.content {
        Flow::Inline(inlines) => inline_ids(inlines, ids),
        Flow::Blocks(blocks) => block_ids(blocks, ids),
    }
}

fn inline_ids(inlines: &[Inline], ids: &mut Vec<String>) {
    for inline in inlines {
        match inline {
            Inline::Anchor(id) => ids.push(id.clone()),
            Inline::Tag(_, inner) | Inline::Span(_, inner) | Inline::Link(_, inner) => {
                inline_ids(inner, ids)
            }
            Inline::Text(_) | Inline::Break => {}
        }
    }
}
