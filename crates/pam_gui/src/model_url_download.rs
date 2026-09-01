//! The pasted-URL model download: everything Pam checks before a source it
//! did not hand-verify is allowed to move bytes onto disk.
//!
//! A curated preset ([`crate::model_presets`]) is safe partly because its URL
//! is a checked-in constant and its redirects are confined to a bounded
//! Hugging Face CDN allowlist. A URL the owner pastes has neither, so this
//! module is the substitute: HTTPS only, no credentials, no query or
//! fragment, port 443, a `.gguf` file name, and a host that does not resolve
//! into the user's own network. The digest is the real gate — `pam_model`
//! refuses to publish bytes that do not hash to it — but these checks stop a
//! pasted link being used as a probe of the local network before any of that
//! matters.
//!
//! Redirects: a pasted URL gets an EMPTY extra-host allowlist, so
//! `pam_model` will follow a redirect only back to the very host that was
//! pasted (it appends the source host itself), re-running its HTTPS, port,
//! credential and address-literal checks at every hop, bounded by its own
//! redirect limit. Inheriting the preset list's CDN allowlist would let an
//! arbitrary pasted host bounce onto Hugging Face's infrastructure, and
//! allowing free redirects would hand a pasted link the cross-host hop this
//! module exists to prevent.

use std::net::{IpAddr, ToSocketAddrs as _};

use pam_core::ContentDigest;
use pam_model::{LicenseSnapshot, ModelDescriptor, address_is_non_public};
// `url::Url` itself, re-exported: this crate already links reqwest, and the
// pinned `url` version is the one `pam_model` parses the same source with.
use reqwest::Url;

use crate::{
    model_download::{ModelAcquisition, ModelDownloadFailure, ModelDownloadKind},
    model_import::{notice_digest, parse_model_key},
};

/// Everything the owner types into the pasted-URL form: the same fields
/// `pam model import` demands, plus the source URL and the explicit license
/// acceptance the preset flow also requires.
///
/// Deliberately not `Debug`: `license_notice_text` is consent material.
#[derive(Clone)]
pub struct ModelUrlDownloadParams {
    pub model: String,
    pub url: String,
    pub expected_size_bytes: u64,
    pub sha256: String,
    pub license_id: String,
    pub license_url: String,
    pub license_notice_text: String,
    pub accepted: bool,
}

/// Resolves a host name to the addresses Pam would connect to. Injectable so
/// the address gate can be tested without a network or a DNS server.
pub(crate) type HostResolver = fn(&str) -> Result<Vec<IpAddr>, std::io::Error>;

/// The production resolver: the same system resolution `reqwest` will do a
/// moment later, on port 443.
///
/// # Errors
///
/// Returns the underlying resolution error when the host cannot be resolved.
pub(crate) fn resolve_host(host: &str) -> Result<Vec<IpAddr>, std::io::Error> {
    Ok((host, 443_u16)
        .to_socket_addrs()?
        .map(|address| address.ip())
        .collect())
}

/// Validates one pasted-URL form submission into the acquisition the shared
/// download runtime executes.
///
/// Every refusal names what was wrong with the value the user typed, because
/// a form that only says "invalid" leaves an owner guessing at a field they
/// copied from a publisher's page.
///
/// # Errors
///
/// Returns [`ModelDownloadFailure`] for a missing license acceptance, a
/// malformed model key, digest, size, or license, a URL that is not
/// credential-free HTTPS on port 443 ending in a `.gguf` name, or a host that
/// cannot be resolved or resolves outside the public internet.
pub(crate) fn acquisition_from_url(
    params: &ModelUrlDownloadParams,
    resolve: HostResolver,
) -> Result<ModelAcquisition, ModelDownloadFailure> {
    if !params.accepted {
        return Err(ModelDownloadFailure {
            detail: "Pam records the exact license you accept before it downloads anything."
                .to_owned(),
            recovery: Some("Accept this model's license, then start the download.".to_owned()),
        });
    }
    let key = parse_model_key(params.model.trim()).map_err(|failure| ModelDownloadFailure {
        detail: failure.detail,
        recovery: failure.recovery,
    })?;
    let source = validate_pasted_url(params.url.trim())?;
    let filename = source_filename(&source)?;
    let digest = parse_expected_digest(params.sha256.trim())?;
    validate_expected_size(params.expected_size_bytes)?;
    let license = build_license(params)?;
    let host = source
        .host_str()
        .ok_or_else(|| failure("That download URL has no host.", HOST_RECOVERY))?;
    reject_non_public_host(host, resolve)?;
    let descriptor = ModelDescriptor::new(
        key.clone(),
        filename,
        digest,
        params.expected_size_bytes,
        license,
    )?;
    Ok(ModelAcquisition {
        // A pasted download has no catalog id; the model key is the identity
        // the form already asked for, and it is what the GUI matches its own
        // in-flight download against.
        id: key.id(),
        kind: ModelDownloadKind::Url,
        descriptor,
        // Stored and downloaded as the canonical URL: `validate_pasted_url`
        // has already refused any query or fragment, so what reaches the
        // durable `ModelSource::Https` record is the plain URL and nothing a
        // signed CDN link would carry.
        url: source.to_string(),
        allowed_redirect_hosts: Vec::new(),
    })
}

const URL_RECOVERY: &str = "Paste the direct https:// URL of the .gguf file itself.";
const HOST_RECOVERY: &str = "Paste the direct https:// URL of the .gguf file on a public host.";

fn failure(detail: impl Into<String>, recovery: &str) -> ModelDownloadFailure {
    ModelDownloadFailure {
        detail: detail.into(),
        recovery: Some(recovery.to_owned()),
    }
}

/// Mirrors `pam_model`'s own source rule so the refusal arrives in the form,
/// naming the offending part, instead of surfacing as a mid-download error.
fn validate_pasted_url(value: &str) -> Result<Url, ModelDownloadFailure> {
    if value.is_empty() {
        return Err(failure("Enter the model's download URL.", URL_RECOVERY));
    }
    let url = Url::parse(value)
        .map_err(|_| failure("Pam could not read that as a URL.", URL_RECOVERY))?;
    if url.scheme() != "https" {
        return Err(failure(
            format!(
                "Pam downloads models over HTTPS only; this URL uses the {} scheme.",
                url.scheme()
            ),
            URL_RECOVERY,
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(failure(
            "Pam refuses a download URL that carries embedded credentials.",
            "Remove the user:password@ part of the URL and paste it again.",
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(failure(
            "Pam refuses a download URL with a query string or fragment; provenance records only \
             the plain URL.",
            "Paste the URL with everything from the ? or # onward removed.",
        ));
    }
    match url.port_or_known_default() {
        Some(443) => {}
        Some(port) => {
            return Err(failure(
                format!("Pam downloads models from port 443 only; this URL uses port {port}."),
                "Paste a plain https:// URL with no explicit port.",
            ));
        }
        None => return Err(failure("That download URL has no host.", HOST_RECOVERY)),
    }
    if url.host_str().is_none() {
        return Err(failure("That download URL has no host.", HOST_RECOVERY));
    }
    Ok(url)
}

/// The destination file name, taken from the URL's own last path segment:
/// the download lands at `<models dir>/<vendor>/<file name>`, so it has to be
/// one safe `.gguf` segment.
fn source_filename(source: &Url) -> Result<String, ModelDownloadFailure> {
    let candidate = source
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or_default();
    if pam_model::validate_model_filename(candidate).is_err() {
        return Err(failure(
            "That URL does not end in a .gguf file name Pam can save.",
            URL_RECOVERY,
        ));
    }
    Ok(candidate.to_owned())
}

fn parse_expected_digest(value: &str) -> Result<ContentDigest, ModelDownloadFailure> {
    // Publishers list the digest either bare or already prefixed; accept both
    // and store Pam's canonical `sha256:` form either way.
    let canonical = if value.starts_with("sha256:") {
        value.to_owned()
    } else {
        format!("sha256:{value}")
    };
    ContentDigest::parse(canonical.to_ascii_lowercase()).map_err(|_| {
        failure(
            "The expected digest must be a 64-character hex SHA-256.",
            "Copy the SHA-256 the publisher lists for this exact file.",
        )
    })
}

fn validate_expected_size(expected_size_bytes: u64) -> Result<(), ModelDownloadFailure> {
    if (ModelDescriptor::MIN_SIZE_BYTES..=ModelDescriptor::MAX_SIZE_BYTES)
        .contains(&expected_size_bytes)
    {
        Ok(())
    } else {
        Err(failure(
            "The expected size must be the file's exact length in bytes.",
            "Copy the byte count the publisher lists for this exact file.",
        ))
    }
}

fn build_license(params: &ModelUrlDownloadParams) -> Result<LicenseSnapshot, ModelDownloadFailure> {
    let notice = params.license_notice_text.trim();
    if notice.is_empty() {
        return Err(failure(
            "Pam records the exact license notice you accept, so it cannot be empty.",
            "Paste the license notice text exactly as the publisher states it.",
        ));
    }
    LicenseSnapshot::new(
        params.license_id.trim(),
        params.license_url.trim(),
        notice_digest(&params.license_notice_text),
    )
    .map_err(|_| {
        failure(
            "The license identifier and notice URL are required, and the notice URL must be plain \
             HTTPS.",
            "Fill in the SPDX identifier and the https:// URL of the license notice.",
        )
    })
}

/// Refuses a host that resolves into the machine's own network.
///
/// `pam_model` already rejects a private *literal* address in the URL, but a
/// pasted host name can point anywhere, so Pam resolves it here and refuses
/// loopback, private, link-local and every other reserved range using
/// `pam_model`'s own table rather than a second copy of it. A host that
/// resolves to a mix is refused on the first non-public answer: `reqwest`
/// may pick any of them.
fn reject_non_public_host(host: &str, resolve: HostResolver) -> Result<(), ModelDownloadFailure> {
    let addresses = resolve(host).map_err(|_| {
        failure(
            format!("Pam could not resolve the host {host}."),
            "Check the host name and this Mac's network connection, then retry.",
        )
    })?;
    if addresses.is_empty() {
        return Err(failure(
            format!("Pam could not resolve the host {host}."),
            "Check the host name and this Mac's network connection, then retry.",
        ));
    }
    if let Some(address) = addresses
        .into_iter()
        .find(|address| address_is_non_public(*address))
    {
        return Err(failure(
            format!(
                "{host} resolves to {address}, which is inside your own network; Pam will not \
                 download a model from it."
            ),
            "Paste a URL on the public internet. Pam refuses private, loopback and link-local \
             addresses so a pasted link cannot reach into your network.",
        ));
    }
    Ok(())
}
