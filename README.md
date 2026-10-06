# rfc2epub

Turn a published IETF RFC into a clean EPUB that is pleasant to read on an
e-reader, a tablet or a phone.

```console
$ rfc2epub https://www.rfc-editor.org/rfc/rfc9114.html
rfc9114-http-3.epub
```

Give it a link to an RFC, get an EPUB back.

## What you get

- **The best source, chosen for you.** RFCs from 8650 onward are built from
  their RFCXML source, with real paragraphs, lists, tables, figures and inline
  SVG diagrams. Older RFCs are built from the RFC Editor's HTML, with the
  running headers, page footers and the old contents list taken out.
- **Navigation that works.** A real table of contents, the reader's outline
  filled from the section tree, and live cross-references.
- **Complete metadata.** Title, authors with editor roles and sort names,
  publication date, identifiers (`urn:ietf:rfc:N` and the DOI), language,
  abstract, keywords, copyright and the position in the RFC series, so the
  book files correctly in any library.
- **Predictable file names.** `rfc<number>-<title>.epub`, always built by the
  same rule and always agreeing with the metadata inside.
- **Restrained looks.** One small stylesheet. Your reader's fonts, sizes and
  night mode are respected. Code, tables and diagrams stay readable.
- **Valid and reproducible.** EPUB 3.3 with an NCX for older readers. Passes
  EPUBCheck, and the same RFC always gives the same bytes.

## Install

Requires Rust 1.89 or newer.

```console
$ cargo install --path .
```

Or build it in place with `cargo build --release`; the binary is
`target/release/rfc2epub`.

## Usage

```text
Usage: rfc2epub [OPTIONS] <RFC>...

Arguments:
  <RFC>...  One to five references: a link, "rfc9114" or "9114"

Options:
  -o, --output-dir <DIR>  Directory to write into [default: .]
  -h, --help              Print help
  -V, --version           Print version
```

A reference can be a number (`9114`), a name (`rfc9114`, `RFC 9114`) or a link
of any common form: `rfc-editor.org`, `datatracker.ietf.org` or `doi.org`.

```console
$ rfc2epub -o books rfc9000 https://datatracker.ietf.org/doc/html/rfc2616 791
books/rfc9000-quic-a-udp-based-multiplexed-and-secure-transport.epub
books/rfc2616-hypertext-transfer-protocol-http-1-1.epub
books/rfc791-internet-protocol.epub
```

- Each RFC becomes its own file. At most five RFCs per run; a duplicate is
  converted once.
- All references are checked before anything is downloaded. RFCs are then
  converted one at a time, and a failure does not stop the others.
- Written paths go to stdout. Warnings and errors go to stderr, prefixed with
  `rfc<N>:`. A file is written in full or not at all, and an existing file with
  the same name is replaced.

| Exit status | Meaning |
|---|---|
| 0 | Every RFC was converted |
| 1 | At least one RFC failed |
| 2 | Usage error, such as a bad reference or more than five RFCs |

## Network

Only `https://www.rfc-editor.org/rfc/` is contacted. A link you pass is parsed
for its RFC number and never fetched. Each RFC takes two requests made one after
the other: the metadata record `rfc<N>.json`, then the `rfc<N>.xml` or
`rfc<N>.html` source it lists. There are no retries and no cache. Requests time
out after 10 seconds to connect and 60 seconds in total, bodies are capped at
32 MiB, and the user agent is `rfc2epub/<version>`.

## Limits

- RFCs published before 8650 stay as preformatted 72-column text, because
  their source has no paragraph structure. They read well on a tablet or a
  phone held sideways; on a narrow screen held upright the lines wrap.
- RFCs published only as PDF (RFC 8, 9, 51, 418, 500, 530 and 598) and
  Internet-Drafts are not supported.

## Library

The command line is a thin layer over a small library.

```toml
[dependencies]
rfc2epub = { path = "../rfc2epub" }
# The converter alone, without the HTTP client:
# rfc2epub = { path = "../rfc2epub", default-features = false }
```

```rust
let number = rfc2epub::parse_reference("https://www.rfc-editor.org/rfc/rfc9114.html")?;
let rfc = rfc2epub::fetch(number)?; // feature `fetch`, on by default
for warning in rfc.warnings() {
    eprintln!("{warning}");
}
rfc.write_epub(std::fs::File::create(rfc.file_name())?)?;
```

Without the network, parse files you already have with `Rfc::from_xml(xml)` or
`Rfc::from_html(html, info_json)`. `Rfc::metadata()` returns the metadata as
plain fields. Errors are one `Error` enum; warnings (`UnknownElement`,
`BrokenLink`, `OmittedContent`) never stop a conversion, and the text concerned
is always kept.

## Development

Every change must keep these three passing:

```console
$ cargo fmt --check
$ cargo clippy --all-targets -- -D warnings
$ cargo test
```

The tests run offline against real RFC Editor files in `tests/fixtures/`. To
also validate every fixture's EPUB with [EPUBCheck](https://github.com/w3c/epubcheck)
5.x:

```console
$ EPUBCHECK="java -jar /path/to/epubcheck.jar" cargo test
```

A corpus survey, run by hand, converts real RFCs over the network and reports
failures and warnings. `RFC_SURVEY` picks the RFCs:

```console
$ RFC_SURVEY="791 2616 9114" cargo test --release --test epub -- --ignored --nocapture
```

| Path | Contents |
|---|---|
| `src/lib.rs` | Public API |
| `src/reference.rs` | From a link to an RFC number |
| `src/xml.rs` | RFCXML v3 parser |
| `src/legacy.rs` | Legacy HTML parser |
| `src/model.rs` | Document model, metadata, dates, names |
| `src/render.rs` | Model to XHTML |
| `src/epub.rs` | Package, navigation and ZIP container |
| `src/style.css` | The one stylesheet |
| `src/fetch.rs` | Downloads from the RFC Editor |
| `src/main.rs` | Command line |
| `tests/` | Integration tests and fixtures |
