//! Structural, determinism and EPUBCheck tests on real RFC Editor files (§14).

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};
use std::process::Command;

use rfc2epub::{Date, Rfc};

const XHTML: &str = "http://www.w3.org/1999/xhtml";
const OPS: &str = "http://www.idpf.org/2007/ops";

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn xml(name: &str) -> Rfc {
    Rfc::from_xml(&fixture(&format!("{name}.xml"))).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn html(name: &str) -> Rfc {
    Rfc::from_html(
        &fixture(&format!("{name}.html")),
        &fixture(&format!("{name}.json")),
    )
    .unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn fixtures() -> Vec<(&'static str, Rfc)> {
    vec![
        ("rfc9844", xml("rfc9844")),
        ("rfc10052", xml("rfc10052")),
        ("rfc8949", xml("rfc8949")),
        ("rfc2119", html("rfc2119")),
        ("rfc8259", html("rfc8259")),
        ("rfc791", html("rfc791")),
    ]
}

fn epub(rfc: &Rfc) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    rfc.write_epub(&mut out).expect("write_epub");
    out.into_inner()
}

/// The entries of an EPUB, in ZIP order.
fn entries(bytes: &[u8]) -> Vec<(String, String)> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    (0..archive.len())
        .map(|i| {
            let mut file = archive.by_index(i).expect("entry");
            let mut text = String::new();
            file.read_to_string(&mut text).expect("utf-8 entry");
            (file.name().to_string(), text)
        })
        .collect()
}

fn ids(doc: &roxmltree::Document) -> HashSet<String> {
    doc.descendants()
        .filter_map(|n| n.attribute("id"))
        .map(String::from)
        .collect()
}

/// A table of contents as (label, href, children).
#[derive(Debug, PartialEq)]
struct Point(String, String, Vec<Point>);

fn nav_points(ol: roxmltree::Node) -> Vec<Point> {
    ol.children()
        .filter(|n| n.has_tag_name((XHTML, "li")))
        .map(|li| {
            let a = li
                .children()
                .find(|n| n.has_tag_name((XHTML, "a")))
                .expect("nav link");
            let label: String = a
                .descendants()
                .filter(|n| n.is_text())
                .filter_map(|n| n.text())
                .collect();
            let children = li
                .children()
                .find(|n| n.has_tag_name((XHTML, "ol")))
                .map(nav_points)
                .unwrap_or_default();
            Point(
                label,
                a.attribute("href").unwrap_or("").to_string(),
                children,
            )
        })
        .collect()
}

fn ncx_points(parent: roxmltree::Node) -> Vec<Point> {
    parent
        .children()
        .filter(|n| n.has_tag_name("navPoint"))
        .map(|p| {
            let label = p
                .descendants()
                .find(|n| n.has_tag_name("text"))
                .and_then(|n| n.text())
                .unwrap_or("")
                .to_string();
            let src = p
                .children()
                .find(|n| n.has_tag_name("content"))
                .and_then(|n| n.attribute("src"))
                .unwrap_or("")
                .to_string();
            Point(label, src, ncx_points(p))
        })
        .collect()
}

fn check_structure(name: &str, bytes: &[u8]) {
    // mimetype is the first entry, stored, with no extra field.
    assert_eq!(&bytes[..4], b"PK\x03\x04", "{name}");
    assert_eq!(
        u16::from_le_bytes([bytes[8], bytes[9]]),
        0,
        "{name}: mimetype is compressed"
    );
    assert_eq!(
        u16::from_le_bytes([bytes[28], bytes[29]]),
        0,
        "{name}: mimetype has an extra field"
    );
    assert_eq!(&bytes[30..38], b"mimetype", "{name}");
    assert_eq!(&bytes[38..58], b"application/epub+zip", "{name}");

    let entries = entries(bytes);
    let files: HashMap<&str, &str> = entries
        .iter()
        .map(|(n, t)| (n.as_str(), t.as_str()))
        .collect();
    let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        &names[..7],
        [
            "mimetype",
            "META-INF/container.xml",
            "EPUB/package.opf",
            "EPUB/nav.xhtml",
            "EPUB/toc.ncx",
            "EPUB/style.css",
            "EPUB/title.xhtml"
        ],
        "{name}"
    );

    // Every XML file is well-formed.
    let mut parsed = HashMap::new();
    for (file, text) in &entries {
        if file.ends_with(".xhtml")
            || file.ends_with(".xml")
            || file.ends_with(".opf")
            || file.ends_with(".ncx")
        {
            let options = roxmltree::ParsingOptions {
                allow_dtd: true,
                ..roxmltree::ParsingOptions::default()
            };
            let doc = roxmltree::Document::parse_with_options(text, options)
                .unwrap_or_else(|e| panic!("{name}/{file}: {e}"));
            parsed.insert(file.as_str(), doc);
        }
    }

    // The manifest and the container agree.
    let opf = &parsed["EPUB/package.opf"];
    let manifest: HashSet<String> = opf
        .descendants()
        .filter(|n| n.has_tag_name("item"))
        .filter_map(|n| n.attribute("href"))
        .map(|h| format!("EPUB/{h}"))
        .collect();
    for item in &manifest {
        assert!(
            files.contains_key(item.as_str()),
            "{name}: {item} is in the manifest but missing"
        );
    }
    for file in names
        .iter()
        .filter(|f| f.starts_with("EPUB/") && **f != "EPUB/package.opf")
    {
        assert!(
            manifest.contains(*file),
            "{name}: {file} is not in the manifest"
        );
    }

    // Every internal link resolves to an existing id.
    let ids: HashMap<&str, HashSet<String>> = parsed.iter().map(|(f, d)| (*f, ids(d))).collect();
    let check = |from: &str, href: &str| {
        if href
            .split(['/', '#'])
            .next()
            .is_some_and(|s| s.contains(':'))
        {
            return;
        }
        let (file, fragment) = href.split_once('#').unwrap_or((href, ""));
        let target = if file.is_empty() {
            from.to_string()
        } else {
            format!("EPUB/{file}")
        };
        let target_ids = ids
            .get(target.as_str())
            .unwrap_or_else(|| panic!("{name}: {from} links to missing {href}"));
        assert!(
            fragment.is_empty() || target_ids.contains(fragment),
            "{name}: {from} links to missing {href}"
        );
    };
    for (file, doc) in &parsed {
        if file.ends_with(".xhtml") {
            for a in doc.descendants().filter(|n| n.has_tag_name((XHTML, "a"))) {
                check(file, a.attribute("href").expect("href"));
            }
        }
    }
    let ncx = &parsed["EPUB/toc.ncx"];
    for content in ncx.descendants().filter(|n| n.has_tag_name("content")) {
        check("EPUB/toc.ncx", content.attribute("src").expect("src"));
    }

    // nav.xhtml and toc.ncx describe the same tree.
    let nav = parsed["EPUB/nav.xhtml"]
        .descendants()
        .find(|n| n.has_tag_name((XHTML, "nav")) && n.attribute((OPS, "type")) == Some("toc"))
        .and_then(|n| n.children().find(|c| c.has_tag_name((XHTML, "ol"))))
        .map(nav_points)
        .expect("toc nav");
    let map = ncx
        .descendants()
        .find(|n| n.has_tag_name("navMap"))
        .map(ncx_points)
        .expect("navMap");
    assert!(!nav.is_empty(), "{name}: empty table of contents");
    assert_eq!(nav, map, "{name}: nav.xhtml and toc.ncx differ");
}

#[test]
fn fixtures_are_structurally_valid() {
    for (name, rfc) in fixtures() {
        check_structure(name, &epub(&rfc));
    }
}

#[test]
fn output_is_deterministic() {
    for (name, rfc) in fixtures() {
        assert!(epub(&rfc) == epub(&rfc), "{name}: two runs differ");
    }
    let again = xml("rfc10052");
    assert!(
        epub(&xml("rfc10052")) == epub(&again),
        "parsing twice differs"
    );
}

fn package_of(rfc: &Rfc) -> String {
    entries(&epub(rfc))
        .into_iter()
        .find(|(n, _)| n == "EPUB/package.opf")
        .map(|(_, t)| t)
        .expect("package.opf")
}

#[test]
fn metadata_matches_the_source() {
    let rfc = xml("rfc9844");
    let m = rfc.metadata();
    assert_eq!(
        rfc.file_name(),
        "rfc9844-entering-ipv6-zone-identifiers-in-user-interfaces.epub"
    );
    assert_eq!(m.title, "Entering IPv6 Zone Identifiers in User Interfaces");
    assert_eq!(
        m.date,
        Date {
            year: 2025,
            month: 8,
            day: None
        }
    );
    assert_eq!(m.obsoletes, [6874]);
    assert_eq!(m.updates, [4007, 7622, 8089]);
    assert_eq!(m.category.as_deref(), Some("Standards Track"));
    assert_eq!(m.stream.as_deref(), Some("IETF"));
    assert!(
        m.description
            .starts_with("This document describes how the zone identifier")
    );
    assert_eq!(
        m.rights.as_deref(),
        Some(
            "Copyright (c) 2025 IETF Trust and the persons identified as the document authors. All rights reserved."
        )
    );
    let opf = package_of(&rfc);
    for expected in [
        "<dc:identifier id=\"pub-id\">urn:ietf:rfc:9844</dc:identifier>",
        "<dc:identifier>https://doi.org/10.17487/RFC9844</dc:identifier>",
        "<dc:title>RFC 9844: Entering IPv6 Zone Identifiers in User Interfaces</dc:title>",
        "<dc:creator id=\"creator-1\">Brian Carpenter</dc:creator>",
        "<meta refines=\"#creator-1\" property=\"role\" scheme=\"marc:relators\">aut</meta>",
        "<meta refines=\"#creator-1\" property=\"file-as\">Carpenter, Brian</meta>",
        "<meta refines=\"#creator-2\" property=\"file-as\">Hinden, Robert M.</meta>",
        "<dc:language>en</dc:language>",
        "<dc:date>2025-08</dc:date>",
        "<meta property=\"dcterms:modified\">2025-08-01T00:00:00Z</meta>",
        "<dc:publisher>RFC Editor</dc:publisher>",
        "<dc:source>https://www.rfc-editor.org/rfc/rfc9844.xml</dc:source>",
        "<meta refines=\"#series\" property=\"group-position\">9844</meta>",
    ] {
        assert!(opf.contains(expected), "missing {expected}");
    }

    let rfc = xml("rfc8949");
    assert_eq!(
        rfc.file_name(),
        "rfc8949-concise-binary-object-representation-cbor.epub"
    );
    assert_eq!(rfc.metadata().keywords.len(), 6);

    let rfc = html("rfc791");
    let m = rfc.metadata();
    assert_eq!(rfc.file_name(), "rfc791-internet-protocol.epub");
    assert_eq!((m.date.year, m.date.month, m.date.day), (1981, 9, None));
    assert_eq!(m.obsoletes, [760]);
    let opf = package_of(&rfc);
    for expected in [
        "<dc:identifier>https://doi.org/10.17487/RFC0791</dc:identifier>",
        "<dc:title>RFC 791: Internet Protocol</dc:title>",
        "<dc:creator id=\"creator-1\">J. Postel</dc:creator>",
        "<meta refines=\"#creator-1\" property=\"file-as\">Postel, J.</meta>",
        "<dc:source>https://www.rfc-editor.org/rfc/rfc791.html</dc:source>",
        "<dc:subject>IPv4</dc:subject>",
    ] {
        assert!(opf.contains(expected), "missing {expected}");
    }
    assert!(
        !opf.contains("<dc:description>"),
        "RFC 791 has an empty abstract"
    );
    assert!(!opf.contains("<dc:rights>"));

    let rfc = html("rfc2119");
    assert_eq!(
        rfc.metadata().category.as_deref(),
        Some("Best Current Practice")
    );
    assert_eq!(
        rfc.file_name(),
        "rfc2119-key-words-for-use-in-rfcs-to-indicate-requirement-levels.epub"
    );
}

#[test]
fn content_survives_conversion() {
    let book = entries(&epub(&xml("rfc10052")));
    let all: String = book.iter().map(|(_, t)| t.as_str()).collect();
    assert!(all.contains("<div class=\"svg keep\"><svg xmlns=\"http://www.w3.org/2000/svg\""));
    assert!(
        book.iter()
            .any(|(n, t)| n == "EPUB/package.opf" && t.contains("properties=\"svg\""))
    );

    let book = entries(&epub(&xml("rfc8949")));
    let all: String = book.iter().map(|(_, t)| t.as_str()).collect();
    assert!(all.contains("ü (U+00FC)"));
    assert!(all.contains("<caption>Table 1: "));
    assert!(all.contains("<span class=\"bcp14\">MUST</span>"));

    let rfc = html("rfc8259");
    let book = entries(&epub(&rfc));
    let nav = &book
        .iter()
        .find(|(n, _)| n == "EPUB/nav.xhtml")
        .expect("nav")
        .1;
    assert!(nav.contains(">1. Introduction</a>"), "{nav}");
    assert!(
        nav.contains(">Appendix A. Changes from RFC 7159</a>"),
        "{nav}"
    );
    assert!(rfc.warnings().is_empty(), "{:?}", rfc.warnings());
}

/// The text lines of a book's front matter and sections, markup removed.
fn text_lines(rfc: &Rfc) -> Vec<String> {
    let mut out = Vec::new();
    for (name, xhtml) in entries(&epub(rfc)) {
        let file = name.trim_start_matches("EPUB/");
        if !(file == "front.xhtml" || file.starts_with("sec-")) {
            continue;
        }
        let body = xhtml.split_once("<body>").map_or("", |(_, b)| b);
        let mut text = String::new();
        let mut in_tag = false;
        for c in body.chars() {
            match c {
                '<' => in_tag = true,
                '>' => in_tag = false,
                c if !in_tag => text.push(c),
                _ => {}
            }
        }
        let text = text
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&amp;", "&");
        out.extend(text.lines().map(str::to_string));
    }
    out
}

/// A running footer such as `Postel   [Page 4]`.
fn is_footer(line: &str) -> bool {
    let line = line.trim().to_ascii_lowercase();
    let end = line
        .rsplit_once('[')
        .is_some_and(|(_, m)| m.starts_with("page ") && m.ends_with(']'));
    let start = line.starts_with("[page ");
    end || start
}

#[test]
fn legacy_page_furniture_is_removed() {
    let lines = text_lines(&html("rfc791"));
    let count = |text: &str| lines.iter().filter(|l| l.trim() == text).count();
    assert_eq!(
        lines.iter().filter(|l| is_footer(l)).count(),
        0,
        "a footer is left"
    );
    // The running header: date, document title and section title.
    assert_eq!(
        count("September 1981"),
        1,
        "only the title block keeps the date"
    );
    for header in ["Internet Protocol", "Specification", "Overview", "Glossary"] {
        assert_eq!(count(header), 0, "running header {header:?} is left");
    }
    for kept in ["INTERNET PROTOCOL", "RFC:  791", "PREFACE"] {
        assert!(count(kept) > 0, "{kept:?} was lost");
    }
    for (name, rfc) in fixtures()
        .into_iter()
        .filter(|(n, _)| ["rfc791", "rfc2119", "rfc8259"].contains(n))
    {
        let lines = text_lines(&rfc);
        let last = lines
            .iter()
            .rev()
            .find(|l| !l.trim().is_empty())
            .expect("text");
        assert!(!is_footer(last), "{name} ends with a footer: {last:?}");
    }
}

/// Runs EPUBCheck when `EPUBCHECK` names it, for example
/// `EPUBCHECK="java -jar epubcheck.jar"`.
#[test]
fn epubcheck() {
    let Ok(command) = std::env::var("EPUBCHECK") else {
        eprintln!("skipped: set EPUBCHECK to an EPUBCheck 5.x command to run this test");
        return;
    };
    let dir = tempfile::tempdir().expect("temp dir");
    let mut words = command.split_whitespace();
    let program = words.next().expect("EPUBCHECK is empty");
    let args: Vec<&str> = words.collect();
    for (name, rfc) in fixtures() {
        let path = dir.path().join(rfc.file_name());
        std::fs::write(&path, epub(&rfc)).expect("write");
        let output = Command::new(program)
            .args(&args)
            .arg(&path)
            .output()
            .expect("run EPUBCheck");
        let report = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.status.success() && report.contains("No errors or warnings detected"),
            "{name}: EPUBCheck reported problems:\n{report}"
        );
    }
}

/// Converts RFCs from the network and reports failures and warning counts.
/// Run by hand: `cargo test --test epub -- --ignored --nocapture`. Set
/// `RFC_SURVEY` to a space-separated list of numbers to survey others.
#[cfg(feature = "fetch")]
#[test]
#[ignore = "uses the network"]
fn corpus_survey() {
    let list = std::env::var("RFC_SURVEY").unwrap_or_else(|_| {
        "1 20 791 1034 2119 2616 3986 5246 6749 7231 7540 8259 8446 8615 8649 \
         8650 8700 8792 8949 9000 9110 9114 9293 9401 9420 9562 9700 9844 10052"
            .into()
    });
    let mut failures = 0;
    for number in list
        .split_whitespace()
        .filter_map(|n| n.parse::<u32>().ok())
    {
        match rfc2epub::fetch(number) {
            Ok(rfc) => {
                let size = epub(&rfc).len();
                println!(
                    "rfc{number}: ok, {} warnings, {size} bytes",
                    rfc.warnings().len()
                );
                for warning in rfc.warnings() {
                    println!("    {warning}");
                }
            }
            Err(e) => {
                failures += 1;
                println!("rfc{number}: FAILED: {e}");
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    println!("{failures} failures");
}
