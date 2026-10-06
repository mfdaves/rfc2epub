//! Downloads from the RFC Editor (§4, §11).

use std::time::Duration;

use serde_json::Value;

use crate::{Error, Rfc};

const HOST: &str = "https://www.rfc-editor.org";
const BODY_LIMIT: u64 = 32 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

/// Downloads an RFC from the RFC Editor and parses it (§4, §11).
///
/// Two requests are made, one after the other: the metadata record, then the
/// richest source it lists.
pub fn fetch(number: u32) -> Result<Rfc, Error> {
    if !(1..=99999).contains(&number) {
        return Err(Error::InvalidReference(number.to_string()));
    }
    let agent = agent();
    let base = format!("{HOST}/rfc/rfc{number}");
    let json = get(&agent, &format!("{base}.json"))?.ok_or(Error::NotFound(number))?;
    let info: Value = serde_json::from_str(&json)
        .map_err(|e| Error::InvalidSource(format!("invalid metadata record: {e}")))?;
    let has = |format: &str| {
        info["format"].as_array().is_some_and(|list| {
            list.iter()
                .any(|f| f.as_str().is_some_and(|f| f.eq_ignore_ascii_case(format)))
        })
    };
    let rfc = if has("XML") {
        Rfc::from_xml(&get_source(&agent, &format!("{base}.xml"))?)?
    } else if has("HTML") {
        Rfc::from_html(&get_source(&agent, &format!("{base}.html"))?, &json)?
    } else {
        return Err(Error::Unsupported(number));
    };
    if rfc.metadata().number != number {
        return Err(Error::InvalidSource(format!(
            "the source describes RFC {} instead of RFC {number}",
            rfc.metadata().number
        )));
    }
    Ok(rfc)
}

fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(60)))
        .https_only(true)
        .http_status_as_error(false)
        // Redirects are followed by hand, to keep them on the same host.
        .max_redirects(0)
        .user_agent(concat!("rfc2epub/", env!("CARGO_PKG_VERSION")))
        .build();
    ureq::Agent::new_with_config(config)
}

/// A source file, which must exist once the metadata record lists it.
fn get_source(agent: &ureq::Agent, url: &str) -> Result<String, Error> {
    get(agent, url)?.ok_or_else(|| Error::Http(format!("{url}: HTTP status 404")))
}

/// Downloads a text file. A 404 is `None`.
fn get(agent: &ureq::Agent, url: &str) -> Result<Option<String>, Error> {
    let mut url = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let mut response = agent
            .get(&url)
            .call()
            .map_err(|e| Error::Http(format!("{url}: {e}")))?;
        match response.status().as_u16() {
            200 => {
                return response
                    .body_mut()
                    .with_config()
                    .limit(BODY_LIMIT)
                    .read_to_string()
                    .map(Some)
                    .map_err(|e| Error::Http(format!("{url}: {e}")));
            }
            404 => return Ok(None),
            301 | 302 | 303 | 307 | 308 => {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                url = same_host(location).ok_or_else(|| {
                    Error::Http(format!("{url}: redirect to {location:?} refused"))
                })?;
            }
            status => return Err(Error::Http(format!("{url}: HTTP status {status}"))),
        }
    }
    Err(Error::Http(format!("{url}: too many redirects")))
}

/// A redirect target, only when it stays on the RFC Editor's host over HTTPS.
fn same_host(location: &str) -> Option<String> {
    if location.starts_with('/') && !location.starts_with("//") {
        return Some(format!("{HOST}{location}"));
    }
    let rest = location.strip_prefix(HOST)?;
    (rest.is_empty() || rest.starts_with('/')).then(|| format!("{HOST}{rest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirects_stay_on_host() {
        assert_eq!(
            same_host("/rfc/rfc1.json").as_deref(),
            Some("https://www.rfc-editor.org/rfc/rfc1.json")
        );
        assert_eq!(
            same_host("https://www.rfc-editor.org/rfc/x").as_deref(),
            Some("https://www.rfc-editor.org/rfc/x")
        );
        for refused in [
            "//evil.example/x",
            "http://www.rfc-editor.org/rfc/x",
            "https://www.rfc-editor.org.evil.example/x",
            "https://evil.example/",
        ] {
            assert!(same_host(refused).is_none(), "{refused}");
        }
    }
}
