//! Turn an RFC into a clean, navigable EPUB.
//!
//! The library behind the `rfc2epub` command. It reads an RFC from the RFC
//! Editor's RFCXML source (RFC 8650 onward) or legacy HTML (older RFCs) and
//! writes it as an EPUB 3 book with navigation and complete metadata.
//!
//! ```no_run
//! # fn main() -> Result<(), rfc2epub::Error> {
//! let number = rfc2epub::parse_reference("rfc9114")?;
//! let rfc = rfc2epub::fetch(number)?;
//! let file = std::fs::File::create(rfc.file_name())?;
//! rfc.write_epub(file)?;
//! # Ok(())
//! # }
//! ```

use std::fmt;
use std::io::{Seek, Write};

mod epub;
#[cfg(feature = "fetch")]
mod fetch;
mod legacy;
mod model;
mod reference;
mod render;
mod xml;

#[cfg(feature = "fetch")]
pub use fetch::fetch;
pub use model::{Author, Date, Metadata};
pub use reference::parse_reference;

/// A parsed RFC, independent of the source it came from.
#[derive(Debug)]
pub struct Rfc {
    doc: model::Document,
}

impl Rfc {
    /// Parses a published RFCXML v3 file.
    pub fn from_xml(xml: &str) -> Result<Rfc, Error> {
        xml::parse(xml).map(|doc| Rfc { doc })
    }

    /// Parses a legacy HTML file together with its `rfc<N>.json` record.
    pub fn from_html(html: &str, info_json: &str) -> Result<Rfc, Error> {
        legacy::parse(html, info_json).map(|doc| Rfc { doc })
    }

    /// The metadata written into the EPUB package.
    pub fn metadata(&self) -> &Metadata {
        &self.doc.meta
    }

    /// The warnings collected while parsing.
    pub fn warnings(&self) -> &[Warning] {
        &self.doc.warnings
    }

    /// The output file name: `rfc<number>-<title slug>.epub`.
    pub fn file_name(&self) -> String {
        model::file_name(self.doc.meta.number, &self.doc.meta.title)
    }

    /// Writes the book as an EPUB 3 file.
    pub fn write_epub<W: Write + Seek>(&self, out: W) -> Result<(), Error> {
        epub::write(&self.doc, out)
    }
}

/// An error that stops the conversion of one RFC.
#[derive(Debug)]
pub enum Error {
    /// The input is not a recognizable RFC reference.
    InvalidReference(String),
    /// The RFC Editor has no such RFC.
    NotFound(u32),
    /// The RFC exists but has neither XML nor HTML.
    Unsupported(u32),
    /// The source file is not what the specification describes.
    InvalidSource(String),
    /// A request failed, timed out or exceeded the size limit.
    Http(String),
    /// Writing the EPUB failed.
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidReference(input) => write!(f, "not an RFC reference: {input:?}"),
            Error::NotFound(n) => write!(f, "RFC {n} does not exist"),
            Error::Unsupported(n) => write!(f, "RFC {n} is published in neither XML nor HTML"),
            Error::InvalidSource(why) => write!(f, "invalid source: {why}"),
            Error::Http(why) => write!(f, "download failed: {why}"),
            Error::Io(err) => write!(f, "write failed: {err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err)
    }
}

impl From<zip::result::ZipError> for Error {
    fn from(err: zip::result::ZipError) -> Self {
        match err {
            zip::result::ZipError::Io(err) => Error::Io(err),
            other => Error::Io(std::io::Error::other(other)),
        }
    }
}

/// Something the converter could not fully represent. Conversion
/// continues, and the text concerned is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// An element outside the specification was met. Its text was kept.
    UnknownElement(String),
    /// A cross-reference target was not found. Its text was kept.
    BrokenLink(String),
    /// Something could not be represented, such as binary artwork.
    OmittedContent(String),
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Warning::UnknownElement(name) => write!(f, "unknown element <{name}>, text kept"),
            Warning::BrokenLink(target) => write!(f, "broken link to {target:?}, text kept"),
            Warning::OmittedContent(what) => write!(f, "omitted content: {what}"),
        }
    }
}
