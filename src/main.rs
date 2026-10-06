//! The `rfc2epub` command line.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rfc2epub::{Error, Rfc, parse_reference};

const MAX_REFERENCES: usize = 5;

const USAGE: &str = "Turn an RFC into a clean, navigable EPUB

Usage: rfc2epub [OPTIONS] <RFC>...

Arguments:
  <RFC>...  One to five references: a link, \"rfc9114\" or \"9114\"

Options:
  -o, --output-dir <DIR>  Directory to write into [default: .]
  -m, --metadata          Show each RFC's metadata and abstract instead of
                          converting it
  -h, --help              Print help
  -V, --version           Print version
";

enum Command {
    Help,
    Version,
    Convert { dir: PathBuf, numbers: Vec<u32> },
    Describe { numbers: Vec<u32> },
}

fn main() -> ExitCode {
    match parse_args(std::env::args_os().skip(1)) {
        Ok(Command::Help) => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            println!("rfc2epub {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Command::Convert { dir, numbers }) => run(&numbers, |number| {
            convert(number, &dir).map(|path| path.display().to_string())
        }),
        Ok(Command::Describe { numbers }) => {
            let mut first = true;
            run(&numbers, |number| {
                let text = describe(&rfc2epub::fetch(number)?);
                // A blank line separates one RFC from the next.
                let separator = if std::mem::take(&mut first) { "" } else { "\n" };
                Ok(format!("{separator}{text}"))
            })
        }
        Err(message) => {
            eprintln!("error: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Parses the arguments and validates every reference before any download.
fn parse_args(args: impl Iterator<Item = OsString>) -> Result<Command, String> {
    let mut dir = None;
    let mut metadata = false;
    let mut references = Vec::new();
    let mut args = args.map(|a| {
        a.into_string()
            .map_err(|a| format!("argument is not UTF-8: {a:?}"))
    });
    let mut options = true;
    while let Some(arg) = args.next() {
        let arg = arg?;
        if !options || !arg.starts_with('-') || arg == "-" {
            references.push(arg);
            continue;
        }
        let value = match arg.as_str() {
            "--" => {
                options = false;
                continue;
            }
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "-m" | "--metadata" => {
                metadata = true;
                continue;
            }
            "-o" | "--output-dir" => args.next().ok_or("missing value for --output-dir")??,
            _ => match arg.strip_prefix("--output-dir=").or(arg.strip_prefix("-o")) {
                Some(value) => value.to_string(),
                None => return Err(format!("unknown option {arg:?}")),
            },
        };
        if dir.replace(PathBuf::from(value)).is_some() {
            return Err("--output-dir given more than once".into());
        }
    }
    if references.is_empty() {
        return Err("no RFC given".into());
    }
    if references.len() > MAX_REFERENCES {
        return Err(format!(
            "at most {MAX_REFERENCES} RFCs per run, {} given",
            references.len()
        ));
    }
    let mut numbers = Vec::new();
    for reference in &references {
        let number = parse_reference(reference).map_err(|e| e.to_string())?;
        if !numbers.contains(&number) {
            numbers.push(number);
        }
    }
    if metadata {
        if dir.is_some() {
            return Err("--metadata writes no file, so --output-dir has no use".into());
        }
        return Ok(Command::Describe { numbers });
    }
    let dir = dir.unwrap_or_else(|| PathBuf::from("."));
    if !dir.is_dir() {
        return Err(format!("output directory {} does not exist", dir.display()));
    }
    Ok(Command::Convert { dir, numbers })
}

/// Handles the RFCs one at a time and prints what each one produced; a
/// failure does not stop the others.
fn run(numbers: &[u32], mut handle: impl FnMut(u32) -> Result<String, Error>) -> ExitCode {
    let mut failed = false;
    for &number in numbers {
        match handle(number) {
            Ok(output) => println!("{output}"),
            Err(e) => {
                eprintln!("rfc{number}: error: {e}");
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// The metadata and abstract of an RFC, to read before converting it.
fn describe(rfc: &Rfc) -> String {
    let meta = rfc.metadata();
    let mut fields: Vec<(&str, String)> = Vec::new();
    for (i, author) in meta.authors.iter().enumerate() {
        let mut line = author.name.clone();
        if author.editor {
            line.push_str(" (editor)");
        }
        if let Some(org) = author.organization.as_ref().filter(|o| **o != author.name) {
            line.push_str(", ");
            line.push_str(org);
        }
        fields.push((if i == 0 { "Authors" } else { "" }, line));
    }
    fields.push(("Published", meta.date.to_string()));
    if let Some(category) = &meta.category {
        fields.push(("Category", category.clone()));
    }
    if let Some(stream) = &meta.stream {
        fields.push(("Stream", stream.clone()));
    }
    for (label, numbers) in [
        ("Obsoletes", &meta.obsoletes),
        ("Updates", &meta.updates),
        ("Obsoleted by", &meta.obsoleted_by),
        ("Updated by", &meta.updated_by),
    ] {
        if !numbers.is_empty() {
            let list: Vec<String> = numbers.iter().map(|n| format!("RFC {n}")).collect();
            fields.push((label, list.join(", ")));
        }
    }
    if !meta.keywords.is_empty() {
        fields.push(("Keywords", meta.keywords.join(", ")));
    }
    fields.push(("Source", meta.source.clone()));
    fields.push(("File", rfc.file_name()));

    let mut out = format!("RFC {}: {}\n", meta.number, meta.title);
    for (label, value) in fields {
        let label = if label.is_empty() {
            String::new()
        } else {
            format!("{label}:")
        };
        out.push_str(&format!("{label:<14}{value}\n"));
    }
    if !meta.description.is_empty() {
        out.push('\n');
        out.push_str(&wrap(&meta.description, 76, "  "));
    }
    out.trim_end().to_string()
}

/// Wraps text into lines of at most `width` columns, each starting with
/// `indent`. A word longer than a line gets a line of its own.
fn wrap(text: &str, width: usize, indent: &str) -> String {
    let mut out = String::new();
    let mut line = String::from(indent);
    for word in text.split_whitespace() {
        if line.len() > indent.len() && line.len() + 1 + word.len() > width {
            out.push_str(&line);
            out.push('\n');
            line = String::from(indent);
        }
        if line.len() > indent.len() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if line.len() > indent.len() {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn convert(number: u32, dir: &Path) -> Result<PathBuf, Error> {
    let rfc = rfc2epub::fetch(number)?;
    for warning in rfc.warnings() {
        eprintln!("rfc{number}: warning: {warning}");
    }
    let name = rfc.file_name();
    let path = if dir == Path::new(".") {
        PathBuf::from(&name)
    } else {
        dir.join(&name)
    };
    // Write next to the target, then rename, so no partial file is left.
    let temp = dir.join(format!(".{name}.part"));
    let written = (|| -> Result<(), Error> {
        let mut out = BufWriter::new(File::create(&temp)?);
        rfc.write_epub(&mut out)?;
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        fs::rename(&temp, &path)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written.map(|()| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        parse_args(args.iter().map(OsString::from))
    }

    #[test]
    fn arguments() {
        assert!(matches!(parse(&["-h"]), Ok(Command::Help)));
        assert!(matches!(
            parse(&["9114", "--version"]),
            Ok(Command::Version)
        ));
        assert!(parse(&[]).is_err());
        assert!(parse(&["1", "2", "3", "4", "5", "6"]).is_err());
        assert!(parse(&["draft-ietf-quic-http"]).is_err());
        assert!(parse(&["--frobnicate", "1"]).is_err());
        assert!(parse(&["-o", "/nonexistent/rfc2epub", "1"]).is_err());
        let Ok(Command::Convert { dir, numbers }) = parse(&[
            "-o",
            ".",
            "rfc9114",
            "https://www.rfc-editor.org/rfc/rfc9114.html",
            "--",
            "791",
        ]) else {
            panic!("expected a conversion");
        };
        assert_eq!((dir, numbers), (PathBuf::from("."), vec![9114, 791]));
        assert!(matches!(
            parse(&["--output-dir=.", "1"]),
            Ok(Command::Convert { .. })
        ));
        let Ok(Command::Describe { numbers }) =
            parse(&["-m", "rfc2119", "--metadata", "2119", "791"])
        else {
            panic!("expected a description");
        };
        assert_eq!(numbers, vec![2119, 791]);
        assert!(parse(&["--metadata", "-o", ".", "1"]).is_err());
    }

    #[test]
    fn metadata_summary() {
        let rfc = Rfc::from_html(
            include_str!("../tests/fixtures/rfc2119.html"),
            include_str!("../tests/fixtures/rfc2119.json"),
        )
        .unwrap();
        let text = describe(&rfc);
        let expected_head = "\
RFC 2119: Key words for use in RFCs to Indicate Requirement Levels
Authors:      S. Bradner
Published:    March 1997
Category:     Best Current Practice
Updated by:   RFC 8174
Keywords:     Standards, Track, Documents
Source:       https://www.rfc-editor.org/rfc/rfc2119.html
File:         rfc2119-key-words-for-use-in-rfcs-to-indicate-requirement-levels.epub

  In many standards track documents several words are used to signify the
";
        assert!(text.starts_with(expected_head), "{text}");
        let (_, abstract_) = text.split_once("\n\n").expect("an abstract");
        assert!(abstract_.lines().all(|l| l.len() <= 76), "{text}");
    }

    #[test]
    fn wrapping() {
        assert_eq!(wrap("a b c", 5, "  "), "  a b\n  c\n");
        assert_eq!(wrap("", 5, "  "), "");
        assert_eq!(wrap("toolongword x", 6, ""), "toolongword\nx\n");
    }
}
