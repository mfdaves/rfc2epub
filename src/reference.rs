//! From a link or a number to an RFC number (§3).

use crate::Error;

/// Reduces a link or a number to an RFC number (§3).
///
/// The link is only parsed, never fetched, and its host is not checked.
pub fn parse_reference(input: &str) -> Result<u32, Error> {
    let s = input.trim();
    let invalid = || Error::InvalidReference(input.to_string());
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        return number(s).ok_or_else(invalid);
    }
    if let Some(n) = rfc_token(s, false) {
        return Ok(n);
    }
    let lower = s.to_ascii_lowercase();
    let rest = if lower.starts_with("https://") {
        &s[8..]
    } else if lower.starts_with("http://") {
        &s[7..]
    } else {
        return Err(invalid());
    };
    let end = rest.find(['?', '#']).unwrap_or(rest.len());
    let mut segments = rest[..end].split('/');
    segments.next(); // the host
    segments
        .rev()
        .find_map(|segment| rfc_token(segment, true))
        .ok_or_else(invalid)
}

/// Matches `rfc` followed by digits. In a URL segment an extension may follow;
/// otherwise one space or hyphen may separate the prefix from the digits.
fn rfc_token(s: &str, url_segment: bool) -> Option<u32> {
    let prefix = s.get(..3)?;
    if !prefix.eq_ignore_ascii_case("rfc") {
        return None;
    }
    let mut rest = &s[3..];
    if url_segment {
        if let Some(dot) = rest.find('.') {
            let ext = &rest[dot + 1..];
            if ext.is_empty() || !ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
                return None;
            }
            rest = &rest[..dot];
        }
    } else if let Some(stripped) = rest.strip_prefix([' ', '-']) {
        rest = stripped;
    }
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    number(rest)
}

/// Parses a digit string, ignoring leading zeros, within 1..=99999.
fn number(digits: &str) -> Option<u32> {
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() || digits.len() > 5 {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_table() {
        let ok = [
            ("9114", 9114),
            ("rfc9114", 9114),
            ("RFC 9114", 9114),
            ("https://www.rfc-editor.org/rfc/rfc9114.html", 9114),
            ("https://www.rfc-editor.org/info/rfc9114/", 9114),
            (
                "https://datatracker.ietf.org/doc/html/rfc9114#section-4",
                9114,
            ),
            ("https://doi.org/10.17487/RFC0791", 791),
        ];
        for (input, n) in ok {
            assert_eq!(parse_reference(input).ok(), Some(n), "{input}");
        }
        assert!(matches!(
            parse_reference("https://datatracker.ietf.org/doc/draft-ietf-quic-http/"),
            Err(Error::InvalidReference(_))
        ));
    }

    #[test]
    fn edge_cases() {
        assert_eq!(parse_reference("  rfc-0791 ").ok(), Some(791));
        assert_eq!(parse_reference("00042").ok(), Some(42));
        assert_eq!(
            parse_reference("http://x/rfc1034.txt?a=rfc1").ok(),
            Some(1034)
        );
        for bad in [
            "",
            "0",
            "100000",
            "rfc",
            "rfc 12a",
            "rfc  1",
            "ftp://x/rfc1",
            "x/rfc1",
        ] {
            assert!(parse_reference(bad).is_err(), "{bad}");
        }
        // The host is never treated as a path segment.
        assert!(parse_reference("https://rfc1/").is_err());
    }
}
