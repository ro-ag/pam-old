//! GUI-owned Hugging Face license discovery for manual GGUF imports.
//!
//! Given the model name a GGUF header declares (or the file's basename), one
//! bounded HTTPS search against the public Hugging Face model index yields
//! the repository id and its declared `license:` tag, so the import form can
//! prefill license details the GGUF metadata itself omitted. Discovery is an
//! enhancement: any failure falls back to manual entry, never blocks import.

use std::time::Duration;

const SEARCH_URL: &str = "https://huggingface.co/api/models";
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(8);
/// One search result page is a few KB; anything past this bound is not the
/// model index answering.
const MAX_BODY_BYTES: usize = 256 * 1024;
const MAX_QUERY_BYTES: usize = 256;
const RECOVERY: &str = "Fill in the license details under Advanced manually.";

/// A bounded, user-facing discovery failure.
#[derive(Clone, Debug)]
pub(crate) struct ModelDiscoveryFailure {
    pub(crate) detail: String,
    pub(crate) recovery: Option<String>,
}

impl ModelDiscoveryFailure {
    fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
            recovery: Some(RECOVERY.to_owned()),
        }
    }
}

/// The license a Hugging Face repository declares for a discovered model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DiscoveredLicense {
    /// The repository id, `owner/name`.
    pub(crate) repo_id: String,
    /// The raw `license:` tag value, e.g. `apache-2.0`. Not normalized to an
    /// SPDX id here; the import form owns that mapping.
    pub(crate) license_id: String,
}

/// Picks the first search result whose repository id matches the query and
/// declares a concrete license tag. Pure over the response body, so tests
/// need no network.
pub(crate) fn select_license(
    query: &str,
    body: &str,
) -> Result<DiscoveredLicense, ModelDiscoveryFailure> {
    let results: Vec<serde_json::Value> = serde_json::from_str(body)
        .map_err(|_| ModelDiscoveryFailure::new("Hugging Face returned an unexpected answer."))?;
    let needle = query.to_lowercase();
    for result in &results {
        let Some(repo_id) = result.get("id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if !repo_id.to_lowercase().contains(&needle) && !needle.contains(&repo_id.to_lowercase()) {
            continue;
        }
        let license = result
            .get("tags")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .find_map(|tag| tag.strip_prefix("license:"));
        // "other" and "unknown" are HF placeholders, not licenses a user
        // could meaningfully accept.
        if let Some(license) = license
            && license != "other"
            && license != "unknown"
        {
            return Ok(DiscoveredLicense {
                repo_id: repo_id.to_owned(),
                license_id: license.to_owned(),
            });
        }
    }
    Err(ModelDiscoveryFailure::new(
        "No matching Hugging Face model declares a license.",
    ))
}

/// Percent-encodes one query-string value: RFC 3986 unreserved characters
/// pass through, every other byte is `%XX`.
fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                use std::fmt::Write as _;
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
    }
    encoded
}

fn validate_query(query: &str) -> Result<&str, ModelDiscoveryFailure> {
    let trimmed = query.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_QUERY_BYTES
        || trimmed.chars().any(char::is_control)
    {
        return Err(ModelDiscoveryFailure::new(
            "The model name to look up must be a short plain-text query.",
        ));
    }
    Ok(trimmed)
}

/// Runs one bounded license lookup against the public Hugging Face index.
///
/// # Errors
///
/// Returns [`ModelDiscoveryFailure`] for an invalid query, an unreachable or
/// non-OK index, an oversized body, or no license-declaring match.
pub(crate) async fn discover_license(
    query: &str,
) -> Result<DiscoveredLicense, ModelDiscoveryFailure> {
    let query = validate_query(query)?;
    let client = reqwest::Client::builder()
        .timeout(DISCOVERY_TIMEOUT)
        .build()
        .map_err(|_| ModelDiscoveryFailure::new("PAM could not prepare the lookup client."))?;
    let response = client
        .get(format!(
            "{SEARCH_URL}?limit=5&search={}",
            percent_encode(query)
        ))
        .send()
        .await
        .map_err(|_| ModelDiscoveryFailure::new("Hugging Face is not reachable right now."))?;
    if !response.status().is_success() {
        return Err(ModelDiscoveryFailure::new(
            "Hugging Face did not answer the model lookup.",
        ));
    }
    let body = response
        .text()
        .await
        .map_err(|_| ModelDiscoveryFailure::new("The Hugging Face answer was cut short."))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(ModelDiscoveryFailure::new(
            "Hugging Face returned an unexpected answer.",
        ));
    }
    select_license(query, &body)
}
