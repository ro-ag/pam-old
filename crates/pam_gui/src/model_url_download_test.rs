// The fake host resolvers below stand in for system DNS, so they must match
// `HostResolver`'s fn-pointer signature — the `Result` is load-bearing even in
// the cases that cannot fail.
#![allow(clippy::unnecessary_wraps)]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::{
    model_download::ModelDownloadKind,
    model_url_download::{ModelUrlDownloadParams, acquisition_from_url},
};

fn public_resolver(_host: &str) -> Result<Vec<IpAddr>, std::io::Error> {
    Ok(vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))])
}

fn loopback_resolver(_host: &str) -> Result<Vec<IpAddr>, std::io::Error> {
    Ok(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)])
}

fn private_resolver(_host: &str) -> Result<Vec<IpAddr>, std::io::Error> {
    Ok(vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))])
}

fn link_local_resolver(_host: &str) -> Result<Vec<IpAddr>, std::io::Error> {
    Ok(vec![IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))])
}

/// A host that answers with a public address first and a loopback one
/// second: the connection may take either, so the pair must be refused.
fn mixed_resolver(_host: &str) -> Result<Vec<IpAddr>, std::io::Error> {
    Ok(vec![
        IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
    ])
}

fn unresolvable(_host: &str) -> Result<Vec<IpAddr>, std::io::Error> {
    Err(std::io::Error::other("no such host"))
}

fn params() -> ModelUrlDownloadParams {
    ModelUrlDownloadParams {
        model: "acme/pasted-model".to_owned(),
        url: "https://models.example/pasted-model.gguf".to_owned(),
        expected_size_bytes: 4096,
        sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        license_id: "Test-License".to_owned(),
        license_url: "https://example.test/license".to_owned(),
        license_notice_text: "pasted-model.gguf is under the Test-License license.".to_owned(),
        accepted: true,
    }
}

fn with_url(url: &str) -> ModelUrlDownloadParams {
    ModelUrlDownloadParams {
        url: url.to_owned(),
        ..params()
    }
}

#[test]
fn a_complete_pasted_form_becomes_a_url_acquisition() {
    let acquisition = acquisition_from_url(&params(), public_resolver).unwrap();

    assert_eq!(acquisition.kind, ModelDownloadKind::Url);
    assert_eq!(acquisition.id, "acme/pasted-model");
    assert_eq!(acquisition.url, "https://models.example/pasted-model.gguf");
    assert_eq!(acquisition.descriptor.filename, "pasted-model.gguf");
    assert_eq!(acquisition.descriptor.expected_size_bytes, 4096);
    assert_eq!(acquisition.descriptor.key.vendor(), "acme");
    assert_eq!(acquisition.descriptor.license.identifier(), "Test-License");
    // A pasted source gets no extra redirect hosts: `pam_model` appends the
    // source host itself, so only same-host hops are ever followed.
    assert!(acquisition.allowed_redirect_hosts.is_empty());
}

#[test]
fn a_digest_already_carrying_the_sha256_prefix_is_accepted() {
    let acquisition = acquisition_from_url(
        &ModelUrlDownloadParams {
            sha256: format!(
                "SHA256:{}",
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
            ),
            ..params()
        },
        public_resolver,
    );

    // Only the canonical lowercase `sha256:` prefix is a prefix; anything
    // else is treated as the hex body and refused as too long.
    assert!(acquisition.is_err());

    let canonical = acquisition_from_url(
        &ModelUrlDownloadParams {
            sha256: format!(
                "sha256:{}",
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
            ),
            ..params()
        },
        public_resolver,
    )
    .unwrap();
    assert_eq!(
        canonical.descriptor.expected_digest.as_str(),
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
}

#[test]
fn an_http_url_is_refused_by_name() {
    let failure = acquisition_from_url(&with_url("http://models.example/m.gguf"), public_resolver)
        .unwrap_err();

    assert_eq!(
        failure.detail,
        "PAM downloads models over HTTPS only; this URL uses the http scheme."
    );
    assert_eq!(
        failure.recovery.unwrap(),
        "Paste the direct https:// URL of the .gguf file itself."
    );
}

#[test]
fn a_file_url_is_refused_by_name() {
    let failure =
        acquisition_from_url(&with_url("file:///etc/passwd.gguf"), public_resolver).unwrap_err();

    assert_eq!(
        failure.detail,
        "PAM downloads models over HTTPS only; this URL uses the file scheme."
    );
}

#[test]
fn an_ftp_url_is_refused_by_name() {
    let failure = acquisition_from_url(&with_url("ftp://models.example/m.gguf"), public_resolver)
        .unwrap_err();

    assert_eq!(
        failure.detail,
        "PAM downloads models over HTTPS only; this URL uses the ftp scheme."
    );
}

#[test]
fn a_string_that_is_not_a_url_is_refused_before_anything_else() {
    let failure =
        acquisition_from_url(&with_url("models.example/m.gguf"), public_resolver).unwrap_err();

    assert_eq!(failure.detail, "PAM could not read that as a URL.");
}

#[test]
fn an_empty_url_asks_for_one() {
    let failure = acquisition_from_url(&with_url(""), public_resolver).unwrap_err();

    assert_eq!(failure.detail, "Enter the model's download URL.");
}

#[test]
fn embedded_credentials_are_refused() {
    let failure = acquisition_from_url(
        &with_url("https://user:secret@models.example/m.gguf"),
        public_resolver,
    )
    .unwrap_err();

    assert_eq!(
        failure.detail,
        "PAM refuses a download URL that carries embedded credentials."
    );
}

#[test]
fn a_query_or_fragment_is_refused_so_provenance_stays_plain() {
    for url in [
        "https://models.example/m.gguf?token=secret",
        "https://models.example/m.gguf#part",
    ] {
        let failure = acquisition_from_url(&with_url(url), public_resolver).unwrap_err();
        assert_eq!(
            failure.detail,
            "PAM refuses a download URL with a query string or fragment; provenance records only \
             the plain URL."
        );
    }
}

#[test]
fn a_non_standard_port_is_refused_by_number() {
    let failure = acquisition_from_url(
        &with_url("https://models.example:8443/m.gguf"),
        public_resolver,
    )
    .unwrap_err();

    assert_eq!(
        failure.detail,
        "PAM downloads models from port 443 only; this URL uses port 8443."
    );
}

#[test]
fn a_url_that_does_not_end_in_a_gguf_name_is_refused() {
    for url in [
        "https://models.example/models/",
        "https://models.example/model.bin",
        "https://models.example",
    ] {
        let failure = acquisition_from_url(&with_url(url), public_resolver).unwrap_err();
        assert_eq!(
            failure.detail,
            "That URL does not end in a .gguf file name PAM can save."
        );
    }
}

#[test]
fn a_loopback_host_is_refused_with_the_address_it_resolved_to() {
    let failure = acquisition_from_url(&params(), loopback_resolver).unwrap_err();

    assert_eq!(
        failure.detail,
        "models.example resolves to 127.0.0.1, which is inside your own network; PAM will not \
         download a model from it."
    );
    assert_eq!(
        failure.recovery.unwrap(),
        "Paste a URL on the public internet. PAM refuses private, loopback and link-local \
         addresses so a pasted link cannot reach into your network."
    );
}

#[test]
fn a_private_host_is_refused() {
    let failure = acquisition_from_url(&params(), private_resolver).unwrap_err();

    assert_eq!(
        failure.detail,
        "models.example resolves to 10.0.0.5, which is inside your own network; PAM will not \
         download a model from it."
    );
}

#[test]
fn a_link_local_host_is_refused() {
    let failure = acquisition_from_url(&params(), link_local_resolver).unwrap_err();

    assert_eq!(
        failure.detail,
        "models.example resolves to 169.254.169.254, which is inside your own network; PAM will \
         not download a model from it."
    );
}

#[test]
fn a_host_that_answers_with_both_public_and_loopback_addresses_is_refused() {
    let failure = acquisition_from_url(&params(), mixed_resolver).unwrap_err();

    assert!(
        failure.detail.contains("inside your own network"),
        "unexpected detail: {}",
        failure.detail
    );
}

#[test]
fn an_unresolvable_host_says_so() {
    let failure = acquisition_from_url(&params(), unresolvable).unwrap_err();

    assert_eq!(
        failure.detail,
        "PAM could not resolve the host models.example."
    );
}

#[test]
fn a_literal_private_address_is_refused_the_same_way_a_name_is() {
    // `pam_model` refuses this too, at transfer time; the form refuses it up
    // front so the message lands in the field the user typed.
    let failure = acquisition_from_url(
        &with_url("https://127.0.0.1/pasted-model.gguf"),
        loopback_resolver,
    )
    .unwrap_err();

    assert!(failure.detail.contains("inside your own network"));
}

#[test]
fn an_unaccepted_license_never_starts_a_download() {
    let failure = acquisition_from_url(
        &ModelUrlDownloadParams {
            accepted: false,
            ..params()
        },
        public_resolver,
    )
    .unwrap_err();

    assert_eq!(
        failure.detail,
        "PAM records the exact license you accept before it downloads anything."
    );
}

#[test]
fn a_malformed_model_identity_is_refused() {
    let failure = acquisition_from_url(
        &ModelUrlDownloadParams {
            model: "pasted-model".to_owned(),
            ..params()
        },
        public_resolver,
    )
    .unwrap_err();

    assert_eq!(
        failure.detail,
        "model identity must use the vendor/name form"
    );
}

#[test]
fn a_short_digest_is_refused() {
    let failure = acquisition_from_url(
        &ModelUrlDownloadParams {
            sha256: "abc123".to_owned(),
            ..params()
        },
        public_resolver,
    )
    .unwrap_err();

    assert_eq!(
        failure.detail,
        "The expected digest must be a 64-character hex SHA-256."
    );
}

#[test]
fn a_zero_or_absurd_size_is_refused() {
    for expected_size_bytes in [0, 23, (1_u64 << 40) + 1] {
        let failure = acquisition_from_url(
            &ModelUrlDownloadParams {
                expected_size_bytes,
                ..params()
            },
            public_resolver,
        )
        .unwrap_err();
        assert_eq!(
            failure.detail,
            "The expected size must be the file's exact length in bytes."
        );
    }
}

#[test]
fn an_empty_license_notice_is_refused() {
    let failure = acquisition_from_url(
        &ModelUrlDownloadParams {
            license_notice_text: "   ".to_owned(),
            ..params()
        },
        public_resolver,
    )
    .unwrap_err();

    assert_eq!(
        failure.detail,
        "PAM records the exact license notice you accept, so it cannot be empty."
    );
}

#[test]
fn a_non_https_license_url_is_refused() {
    let failure = acquisition_from_url(
        &ModelUrlDownloadParams {
            license_url: "http://example.test/license".to_owned(),
            ..params()
        },
        public_resolver,
    )
    .unwrap_err();

    assert_eq!(
        failure.detail,
        "The license identifier and notice URL are required, and the notice URL must be plain \
         HTTPS."
    );
}
