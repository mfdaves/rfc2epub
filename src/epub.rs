//! The package, navigation and ZIP container.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{Seek, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

use crate::Error;
use crate::model::{Document, Section, escape, rfc_list};
use crate::render::{Writer, collect_ids, page};

const STYLE: &str = include_str!("style.css");

const CONTAINER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n\
<rootfiles>\n\
<rootfile full-path=\"EPUB/package.opf\" media-type=\"application/oebps-package+xml\"/>\n\
</rootfiles>\n\
</container>\n";

/// One content document of the book.
struct Content {
    id: String,
    file: String,
    xhtml: String,
    svg: bool,
}

/// One entry of the table of contents.
struct NavPoint {
    label: String,
    href: String,
    children: Vec<NavPoint>,
}

/// Writes the EPUB for a document.
pub(crate) fn write<W: Write + Seek>(doc: &Document, out: W) -> Result<(), Error> {
    let lang = doc.meta.language.as_str();
    let has_front = !doc.front.is_empty() || doc.sections.is_empty();

    // Content documents are named in document order.
    let mut parts: Vec<(String, String, &[Section])> = Vec::new();
    if has_front {
        parts.push(("front".into(), "front.xhtml".into(), &doc.front));
    }
    for (i, section) in doc.sections.iter().enumerate() {
        let id = format!("sec-{:03}", i + 1);
        let file = format!("{id}.xhtml");
        parts.push((id, file, std::slice::from_ref(section)));
    }

    let mut files = HashMap::new();
    for (_, file, sections) in &parts {
        for section in *sections {
            collect_ids(section, file, &mut files);
        }
    }

    let mut contents = Vec::new();
    for (id, file, sections) in &parts {
        let mut writer = Writer::new(&files, file);
        for section in *sections {
            writer.section(section, 1);
        }
        let svg = writer.svg;
        let title = if id == "front" {
            match sections.first() {
                Some(first) if !first.label().is_empty() => first.label(),
                _ => doc.front_label.to_string(),
            }
        } else {
            sections.first().map(Section::label).unwrap_or_default()
        };
        contents.push(Content {
            id: id.clone(),
            file: file.clone(),
            xhtml: page(lang, &title, &writer.finish()),
            svg,
        });
    }

    let nav = nav_tree(doc, has_front);
    let first_body = contents
        .iter()
        .find(|c| c.id != "front")
        .or(contents.first())
        .map(|c| c.file.clone())
        .unwrap_or_default();

    let mut zip = ZipWriter::new(out);
    let stored = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(DateTime::default())
        .unix_permissions(0o644);
    let deflated = stored
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(6));
    zip.start_file("mimetype", stored)?;
    zip.write_all(b"application/epub+zip")?;
    let mut entry = |name: &str, data: &str| -> Result<(), Error> {
        zip.start_file(name, deflated)?;
        zip.write_all(data.as_bytes())?;
        Ok(())
    };
    entry("META-INF/container.xml", CONTAINER)?;
    entry("EPUB/package.opf", &package(doc, &contents))?;
    entry("EPUB/nav.xhtml", &nav_document(doc, &nav, &first_body))?;
    entry("EPUB/toc.ncx", &ncx(doc, &nav))?;
    entry("EPUB/style.css", STYLE)?;
    entry("EPUB/title.xhtml", &title_page(doc))?;
    for content in &contents {
        entry(&format!("EPUB/{}", content.file), &content.xhtml)?;
    }
    zip.finish()?;
    Ok(())
}

/// The navigation tree shared by `nav.xhtml` and `toc.ncx`.
fn nav_tree(doc: &Document, has_front: bool) -> Vec<NavPoint> {
    let mut points = Vec::new();
    if has_front {
        points.push(NavPoint {
            label: doc.front_label.to_string(),
            href: "front.xhtml".into(),
            children: Vec::new(),
        });
    }
    for (i, section) in doc.sections.iter().enumerate() {
        if section.toc {
            let file = format!("sec-{:03}.xhtml", i + 1);
            points.push(NavPoint {
                label: section.label(),
                href: file.clone(),
                children: nav_children(section, &file, 2, doc.toc_depth),
            });
        }
    }
    points
}

fn nav_children(section: &Section, file: &str, depth: usize, max: usize) -> Vec<NavPoint> {
    if depth > max {
        return Vec::new();
    }
    section
        .children
        .iter()
        .filter(|child| child.toc)
        .map(|child| NavPoint {
            label: child.label(),
            href: match &child.id {
                Some(id) => format!("{file}#{id}"),
                None => file.to_string(),
            },
            children: nav_children(child, file, depth + 1, max),
        })
        .collect()
}

fn package(doc: &Document, contents: &[Content]) -> String {
    let meta = &doc.meta;
    let mut m = String::new();
    let mut line = |s: String| {
        m.push_str(&s);
        m.push('\n');
    };
    line(format!(
        "<dc:identifier id=\"pub-id\">{}</dc:identifier>",
        meta.urn()
    ));
    line(format!("<dc:identifier>{}</dc:identifier>", meta.doi()));
    line(format!(
        "<dc:title>{}</dc:title>",
        escape(&meta.full_title())
    ));
    for (i, author) in meta.authors.iter().enumerate() {
        let id = format!("creator-{}", i + 1);
        line(format!(
            "<dc:creator id=\"{id}\">{}</dc:creator>",
            escape(&author.name)
        ));
        let role = if author.editor { "edt" } else { "aut" };
        line(format!(
            "<meta refines=\"#{id}\" property=\"role\" scheme=\"marc:relators\">{role}</meta>"
        ));
        line(format!(
            "<meta refines=\"#{id}\" property=\"file-as\">{}</meta>",
            escape(&author.file_as)
        ));
    }
    line(format!(
        "<dc:language>{}</dc:language>",
        escape(&meta.language)
    ));
    line(format!("<dc:date>{}</dc:date>", meta.date.iso()));
    line(format!(
        "<meta property=\"dcterms:modified\">{}</meta>",
        meta.date.modified()
    ));
    line("<dc:publisher>RFC Editor</dc:publisher>".into());
    if !meta.description.is_empty() {
        line(format!(
            "<dc:description>{}</dc:description>",
            escape(&meta.description)
        ));
    }
    for keyword in &meta.keywords {
        line(format!("<dc:subject>{}</dc:subject>", escape(keyword)));
    }
    if let Some(rights) = &meta.rights {
        line(format!("<dc:rights>{}</dc:rights>", escape(rights)));
    }
    line(format!("<dc:source>{}</dc:source>", escape(&meta.source)));
    line("<meta property=\"belongs-to-collection\" id=\"series\">RFC</meta>".into());
    line("<meta refines=\"#series\" property=\"collection-type\">series</meta>".into());
    line(format!(
        "<meta refines=\"#series\" property=\"group-position\">{}</meta>",
        meta.number
    ));
    for (property, value) in [
        ("schema:accessMode", "textual"),
        ("schema:accessModeSufficient", "textual"),
        ("schema:accessibilityFeature", "tableOfContents"),
        ("schema:accessibilityFeature", "readingOrder"),
        ("schema:accessibilityFeature", "structuralNavigation"),
        ("schema:accessibilityHazard", "none"),
    ] {
        line(format!("<meta property=\"{property}\">{value}</meta>"));
    }

    let mut manifest = String::from(
        "<item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>\n\
         <item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>\n\
         <item id=\"css\" href=\"style.css\" media-type=\"text/css\"/>\n\
         <item id=\"title\" href=\"title.xhtml\" media-type=\"application/xhtml+xml\"/>\n",
    );
    let mut spine = String::from("<itemref idref=\"title\"/>\n<itemref idref=\"nav\"/>\n");
    for content in contents {
        let properties = if content.svg {
            " properties=\"svg\""
        } else {
            ""
        };
        let _ = writeln!(
            manifest,
            "<item id=\"{}\" href=\"{}\" media-type=\"application/xhtml+xml\"{properties}/>",
            content.id, content.file
        );
        let _ = writeln!(spine, "<itemref idref=\"{}\"/>", content.id);
    }

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"pub-id\" xml:lang=\"{}\">\n\
         <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n{m}</metadata>\n\
         <manifest>\n{manifest}</manifest>\n\
         <spine toc=\"ncx\">\n{spine}</spine>\n\
         </package>\n",
        escape(&meta.language)
    )
}

fn nav_document(doc: &Document, nav: &[NavPoint], first_body: &str) -> String {
    fn list(points: &[NavPoint], out: &mut String) {
        out.push_str("<ol>\n");
        for point in points {
            let _ = write!(
                out,
                "<li><a href=\"{}\">{}</a>",
                escape(&point.href),
                escape(&point.label)
            );
            if !point.children.is_empty() {
                out.push('\n');
                list(&point.children, out);
            }
            out.push_str("</li>\n");
        }
        out.push_str("</ol>\n");
    }
    let mut body =
        String::from("<nav epub:type=\"toc\" id=\"toc\" role=\"doc-toc\">\n<h1>Contents</h1>\n");
    list(nav, &mut body);
    body.push_str("</nav>\n");
    let _ = write!(
        body,
        "<nav epub:type=\"landmarks\" id=\"landmarks\" hidden=\"hidden\">\n<h2>Landmarks</h2>\n<ol>\n\
         <li><a epub:type=\"titlepage\" href=\"title.xhtml\">Title Page</a></li>\n\
         <li><a epub:type=\"toc\" href=\"nav.xhtml#toc\">Contents</a></li>\n\
         <li><a epub:type=\"bodymatter\" href=\"{}\">Start of Content</a></li>\n</ol>\n</nav>\n",
        escape(first_body)
    );
    page(&doc.meta.language, "Contents", &body)
}

fn ncx(doc: &Document, nav: &[NavPoint]) -> String {
    fn depth(points: &[NavPoint]) -> usize {
        points
            .iter()
            .map(|p| 1 + depth(&p.children))
            .max()
            .unwrap_or(0)
    }
    fn nav_points(points: &[NavPoint], order: &mut usize, out: &mut String) {
        for point in points {
            *order += 1;
            let _ = writeln!(
                out,
                "<navPoint id=\"nav-{order}\" playOrder=\"{order}\">\
                 <navLabel><text>{}</text></navLabel><content src=\"{}\"/>",
                escape(&point.label),
                escape(&point.href)
            );
            nav_points(&point.children, order, out);
            out.push_str("</navPoint>\n");
        }
    }
    let mut map = String::new();
    nav_points(nav, &mut 0, &mut map);
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\" xml:lang=\"{}\">\n\
         <head>\n<meta name=\"dtb:uid\" content=\"{}\"/>\n<meta name=\"dtb:depth\" content=\"{}\"/>\n\
         <meta name=\"dtb:totalPageCount\" content=\"0\"/>\n<meta name=\"dtb:maxPageNumber\" content=\"0\"/>\n\
         </head>\n<docTitle><text>{}</text></docTitle>\n<navMap>\n{map}</navMap>\n</ncx>\n",
        escape(&doc.meta.language),
        doc.meta.urn(),
        depth(nav).max(1),
        escape(&doc.meta.full_title())
    )
}

fn title_page(doc: &Document) -> String {
    let meta = &doc.meta;
    let mut body = String::from("<section class=\"titlepage\" epub:type=\"titlepage\">\n");
    let _ = writeln!(body, "<p class=\"series\">RFC {}</p>", meta.number);
    let _ = writeln!(body, "<h1>{}</h1>", escape(&meta.title));
    if !meta.authors.is_empty() {
        body.push_str("<ul class=\"authors\">\n");
        for author in &meta.authors {
            body.push_str("<li>");
            body.push_str(&escape(&author.name));
            if author.editor {
                body.push_str(" (editor)");
            }
            if let Some(org) = author.organization.as_ref().filter(|o| **o != author.name) {
                let _ = write!(body, "<br/><span class=\"org\">{}</span>", escape(org));
            }
            body.push_str("</li>\n");
        }
        body.push_str("</ul>\n");
    }
    let _ = writeln!(body, "<p class=\"date\">{}</p>", meta.date.display());
    let mut facts = String::new();
    if let Some(category) = &meta.category {
        let _ = writeln!(facts, "<dt>Category</dt><dd>{}</dd>", escape(category));
    }
    if let Some(stream) = &meta.stream {
        let _ = writeln!(facts, "<dt>Stream</dt><dd>{}</dd>", escape(stream));
    }
    for (label, numbers) in [("Obsoletes", &meta.obsoletes), ("Updates", &meta.updates)] {
        if !numbers.is_empty() {
            let links: Vec<String> = numbers
                .iter()
                .map(|n| {
                    format!(
                        "<a href=\"https://www.rfc-editor.org/rfc/rfc{n}\">{}</a>",
                        rfc_list(&[*n])
                    )
                })
                .collect();
            let _ = writeln!(facts, "<dt>{label}</dt><dd>{}</dd>", links.join(", "));
        }
    }
    if !facts.is_empty() {
        let _ = writeln!(body, "<dl class=\"facts\">\n{facts}</dl>");
    }
    body.push_str("</section>\n");
    page(&meta.language, &meta.full_title(), &body)
}
