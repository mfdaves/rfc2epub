//! The `rfc2epub` command line (§9).

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rfc2epub::{Error, parse_reference};

const MAX_REFERENCES: usize = 5;

const USAGE: &str = "Turn an RFC into a clean, navigable EPUB

Usage: rfc2epub [OPTIONS] <RFC>...

Arguments:
  <RFC>...  One to five references: a link, \"rfc9114\" or \"9114\"

Options:
  -o, --output-dir <DIR>  Directory to write into [default: .]
  -h, --help              Print help
  -V, --version           Print version
";

enum Command {
    Help,
    Version,
    Convert { dir: PathBuf, numbers: Vec<u32> },
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
        Ok(Command::Convert { dir, numbers }) => run(&dir, &numbers),
        Err(message) => {
            eprintln!("error: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Parses the arguments and validates every reference before any download.
fn parse_args(args: impl Iterator<Item = OsString>) -> Result<Command, String> {
    let mut dir = None;
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
    let dir = dir.unwrap_or_else(|| PathBuf::from("."));
    if !dir.is_dir() {
        return Err(format!("output directory {} does not exist", dir.display()));
    }
    Ok(Command::Convert { dir, numbers })
}

/// Converts the RFCs one at a time; a failure does not stop the others.
fn run(dir: &Path, numbers: &[u32]) -> ExitCode {
    let mut failed = false;
    for &number in numbers {
        match convert(number, dir) {
            Ok(path) => println!("{}", path.display()),
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
    }
}
