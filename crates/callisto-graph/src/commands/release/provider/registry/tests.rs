//! Protocol-level observation tests against a loopback HTTP server.
//!
//! These run real `curl` against a real socket, so the request shape, the
//! status handling, and the retry schedule are all exercised end to end
//! without a package-manager fake standing in for the registry.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use callisto_model::{RegistryBindingDigest, RegistryKey, Version, VersionGrammar};

use super::super::http::parse_http_response;
use super::super::loopback::{fixtures, LoopbackResponse, LoopbackServer};
use super::super::policy::tests::RecordingSleeper;
use super::super::policy::OBSERVATION_MAX_ATTEMPTS;
use super::*;
use crate::commands::release::binding::PreparedRegistryBinding;
use crate::commands::release::tests::RealGitRunner;

const CKSUM: &str = "1111111111111111111111111111111111111111111111111111111111111111";

fn operation(endpoint: &str, key: &str, protocol: RegistryProtocol) -> RegistryPublishOperation {
    RegistryPublishOperation {
        package_dir: std::path::PathBuf::from("."),
        package_name: "core-crate".to_owned(),
        version: Version::parse("0.2.0", VersionGrammar::SemVer).unwrap(),
        registry: PreparedRegistryBinding {
            key: RegistryKey(key.to_owned()),
            endpoint: Some(endpoint.to_owned()),
            identity: RegistryBindingDigest::from_normalized_binding(key.as_bytes()),
            protocol,
        },
        npm_access: None,
        npm_tag: None,
    }
}

fn cargo_operation(endpoint: &str) -> RegistryPublishOperation {
    operation(endpoint, RegistryKey::CRATES_IO, RegistryProtocol::CargoSparseIndex)
}

fn pypi_operation(endpoint: &str) -> RegistryPublishOperation {
    operation(endpoint, "pypi", RegistryProtocol::Http)
}

/// Runs one observation against `server`, returning the answer and the waits
/// the retry policy would have spent.
fn observe(ecosystem: Ecosystem, operation: &RegistryPublishOperation) -> (ProviderObservationV1, Vec<Duration>) {
    let runner = RealGitRunner;
    let sleeper = RecordingSleeper::new();
    let root = std::env::temp_dir();
    let context = ProviderContext::new(&root, &runner, None).with_sleeper(&sleeper);
    let observation = registry_observation(adapter_for(ecosystem).unwrap(), &context, operation).unwrap();
    (observation, sleeper.waits())
}

/// One line of the captured sparse-index shape, renamed and re-versioned.
fn index_line(version: &str, yanked: bool) -> String {
    fixtures::crates_index_line("core-crate", version, CKSUM, yanked)
}

fn at_version(mut operation: RegistryPublishOperation, version: &str) -> RegistryPublishOperation {
    operation.version = Version::parse(version, VersionGrammar::SemVer).unwrap();
    operation
}

#[test]
fn the_sparse_index_path_follows_cargos_length_layout_and_lowercases() {
    assert_eq!(sparse_index_path("a"), "1/a");
    assert_eq!(sparse_index_path("ab"), "2/ab");
    assert_eq!(sparse_index_path("abc"), "3/a/abc");
    assert_eq!(sparse_index_path("Core-Crate"), "co/re/core-crate");
}

#[test]
fn a_served_unyanked_version_is_exact_and_carries_the_index_checksum() {
    let server = LoopbackServer::start(|_| {
        LoopbackResponse::new(
            200,
            format!("{}\n{}\n", index_line("0.1.0", false), index_line("0.2.0", false)),
        )
    });
    let (observation, waits) = observe(Ecosystem::Cargo, &cargo_operation(&server.base_url()));
    assert_eq!(
        observation,
        ProviderObservationV1::Exact {
            evidence: ProviderEvidenceV1::RegistryVersion {
                version: Version::parse("0.2.0", VersionGrammar::SemVer).unwrap(),
                checksum: Some(ArtifactDigest::parse(CKSUM).unwrap()),
                yanked: Some(false),
            }
        }
    );
    assert!(waits.is_empty());
    assert_eq!(server.paths(), vec!["/co/re/core-crate".to_owned()]);
}

#[test]
fn a_version_missing_from_a_served_index_is_absent_not_indeterminate() {
    let server = LoopbackServer::start(|_| LoopbackResponse::new(200, index_line("0.1.0", false)));
    let (observation, _) = observe(Ecosystem::Cargo, &cargo_operation(&server.base_url()));
    assert_eq!(observation, ProviderObservationV1::Absent);
}

#[test]
fn an_unknown_crate_answered_with_not_found_is_absent() {
    let server = LoopbackServer::start(|_| fixtures::crates_index_not_found());
    let (observation, _) = observe(Ecosystem::Cargo, &cargo_operation(&server.base_url()));
    assert_eq!(observation, ProviderObservationV1::Absent);
}

/// The defect this whole mechanism exists for: `cargo info` reports a yanked
/// version as absent, which would let a release "confirm" a version nobody can
/// depend on.
#[test]
fn a_yanked_version_is_a_conflict_not_an_absence() {
    let server = LoopbackServer::start(|_| LoopbackResponse::new(200, index_line("0.2.0", true)));
    let (observation, _) = observe(Ecosystem::Cargo, &cargo_operation(&server.base_url()));
    assert_eq!(
        observation,
        ProviderObservationV1::Conflict {
            reason: ProviderConflictReason::RegistryVersionYanked
        }
    );
}

#[test]
fn a_body_that_is_not_the_index_format_is_malformed_not_absent() {
    let server = LoopbackServer::start(|_| LoopbackResponse::new(200, "<html>proxy interception</html>"));
    let (observation, _) = observe(Ecosystem::Cargo, &cargo_operation(&server.base_url()));
    assert_eq!(
        observation,
        ProviderObservationV1::Indeterminate {
            cause: ProviderIndeterminateCause::MalformedResponse
        }
    );
}

#[test]
fn a_rate_limited_answer_is_retried_after_the_servers_own_delay_and_then_settles() {
    let attempts = AtomicUsize::new(0);
    let server = LoopbackServer::start(move |_| {
        if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            LoopbackResponse::new(429, "slow down").with_header("retry-after", "11")
        } else {
            LoopbackResponse::new(200, index_line("0.2.0", false))
        }
    });
    let (observation, waits) = observe(Ecosystem::Cargo, &cargo_operation(&server.base_url()));
    assert!(matches!(observation, ProviderObservationV1::Exact { .. }));
    assert_eq!(waits, vec![Duration::from_secs(11)]);
    assert_eq!(server.paths().len(), 2);
}

#[test]
fn a_persistent_server_error_exhausts_the_attempts_and_reports_its_status() {
    let server = LoopbackServer::start(|_| LoopbackResponse::new(503, "unavailable"));
    let (observation, waits) = observe(Ecosystem::Cargo, &cargo_operation(&server.base_url()));
    assert_eq!(
        observation,
        ProviderObservationV1::Indeterminate {
            cause: ProviderIndeterminateCause::ProviderStatus { status: 503 }
        }
    );
    assert_eq!(server.paths().len(), usize::try_from(OBSERVATION_MAX_ATTEMPTS).unwrap());
    assert_eq!(
        waits,
        vec![
            Duration::from_secs(2),
            Duration::from_secs(4),
            Duration::from_secs(8),
            Duration::from_secs(16)
        ]
    );
}

#[test]
fn a_plain_forbidden_answer_settles_immediately_rather_than_burning_the_attempts() {
    let server = LoopbackServer::start(|_| LoopbackResponse::new(403, "forbidden"));
    let (observation, waits) = observe(Ecosystem::Cargo, &cargo_operation(&server.base_url()));
    assert_eq!(
        observation,
        ProviderObservationV1::Indeterminate {
            cause: ProviderIndeterminateCause::ProviderStatus { status: 403 }
        }
    );
    assert_eq!(server.paths().len(), 1);
    assert!(waits.is_empty());
}

#[test]
fn a_cargo_git_index_fails_closed_with_the_unsupported_protocol_cause() {
    let operation = operation(
        "https://github.example.invalid/index",
        "private-cargo",
        RegistryProtocol::CargoGitIndex,
    );
    let (observation, _) = observe(Ecosystem::Cargo, &operation);
    assert_eq!(
        observation,
        ProviderObservationV1::Indeterminate {
            cause: ProviderIndeterminateCause::UnsupportedProtocol
        },
        "a git index must never be guessed at over HTTP"
    );
}

#[test]
fn pypi_observes_its_json_endpoint_and_reports_the_first_files_digest() {
    let server = LoopbackServer::start(|_| fixtures::pypi_version_response("core-crate", "0.2.0", CKSUM, false));
    let (observation, _) = observe(Ecosystem::Pypi, &pypi_operation(&server.base_url()));
    assert_eq!(
        observation,
        ProviderObservationV1::Exact {
            evidence: ProviderEvidenceV1::RegistryVersion {
                version: Version::parse("0.2.0", VersionGrammar::SemVer).unwrap(),
                checksum: Some(ArtifactDigest::parse(CKSUM).unwrap()),
                yanked: Some(false),
            }
        }
    );
    assert_eq!(server.paths(), vec!["/pypi/core-crate/0.2.0/json".to_owned()]);
}

#[test]
fn pypi_reports_not_found_as_absent_and_a_yanked_file_as_a_conflict() {
    let server = LoopbackServer::start(|_| fixtures::pypi_not_found());
    let (observation, _) = observe(Ecosystem::Pypi, &pypi_operation(&server.base_url()));
    assert_eq!(observation, ProviderObservationV1::Absent);

    let yanked = LoopbackServer::start(|_| fixtures::pypi_version_response("core-crate", "0.2.0", CKSUM, true));
    let (observation, _) = observe(Ecosystem::Pypi, &pypi_operation(&yanked.base_url()));
    assert_eq!(
        observation,
        ProviderObservationV1::Conflict {
            reason: ProviderConflictReason::RegistryVersionYanked
        }
    );
}

/// A PyPI publish must be able to reach a receipt: an adapter that declares no
/// version query can never confirm one.
#[test]
fn every_registry_adapter_declares_an_observable_version() {
    for ecosystem in [Ecosystem::Cargo, Ecosystem::Npm, Ecosystem::Pypi] {
        assert!(
            adapter_for(ecosystem).unwrap().can_observe_versions(),
            "{ecosystem:?} must be able to prove an exact published version"
        );
    }
}

#[test]
fn an_ecosystem_with_no_registry_adapter_is_unsupported() {
    assert!(matches!(
        adapter_for(Ecosystem::NuGet),
        Err(GraphError::UnsupportedRelease {
            feature: UnsupportedReleaseFeature::Ecosystem
        })
    ));
}

// Captured provider bytes (testing/fixtures/providers) fed to the production parsers.

fn exact(version: &str, checksum: &str) -> ProviderObservationV1 {
    ProviderObservationV1::Exact {
        evidence: ProviderEvidenceV1::RegistryVersion {
            version: Version::parse(version, VersionGrammar::SemVer).unwrap(),
            checksum: Some(ArtifactDigest::parse(checksum).unwrap()),
            yanked: Some(false),
        },
    }
}

fn observed(attempt: Attempt<ProviderObservationV1>) -> ProviderObservationV1 {
    match attempt {
        Attempt::Settled(observation) => observation,
        other => panic!("expected a settled observation, got {other:?}"),
    }
}

#[test]
fn the_captured_crates_io_response_parses_and_a_listed_version_is_exact() {
    let response = parse_http_response(fixtures::CRATES_INDEX_200).unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.header("content-type"), Some("text/plain"));
    assert_eq!(response.body.lines().count(), 3);
    let operation = at_version(cargo_operation("http://unused/"), "0.3.2");
    assert_eq!(
        observed(classify_sparse_index(&response.body, &operation)),
        exact(
            "0.3.2",
            "1d139face3a57e64cca892588fcb27693dce30c39aff26841ab3c6ef22d4b151"
        )
    );
}

#[test]
fn a_version_the_captured_index_does_not_list_is_absent() {
    let response = parse_http_response(fixtures::CRATES_INDEX_200).unwrap();
    let operation = at_version(cargo_operation("http://unused/"), "9.9.9");
    assert_eq!(
        observed(classify_sparse_index(&response.body, &operation)),
        ProviderObservationV1::Absent
    );
}

#[test]
fn the_captured_yanked_line_is_a_yanked_conflict() {
    let operation = at_version(cargo_operation("http://unused/"), "0.10.1");
    assert_eq!(
        observed(classify_sparse_index(fixtures::CRATES_YANKED_LINE, &operation)),
        ProviderObservationV1::Conflict {
            reason: ProviderConflictReason::RegistryVersionYanked
        }
    );
}

#[test]
fn a_truncated_captured_index_line_is_malformed_not_absent() {
    let line = fixtures::CRATES_YANKED_LINE;
    let operation = at_version(cargo_operation("http://unused/"), "0.10.1");
    assert_eq!(
        observed(classify_sparse_index(&line[..line.len() / 2], &operation)),
        ProviderObservationV1::Indeterminate {
            cause: ProviderIndeterminateCause::MalformedResponse
        }
    );
}

#[test]
fn the_captured_registry_answers_are_observed_end_to_end_over_the_wire() {
    let listed = LoopbackServer::start(|_| LoopbackResponse::from_raw_http(fixtures::CRATES_INDEX_200));
    let (observation, _) = observe(
        Ecosystem::Cargo,
        &at_version(cargo_operation(&listed.base_url()), "0.3.1"),
    );
    assert_eq!(
        observation,
        exact(
            "0.3.1",
            "17d477c63225dccaf4de0d0e6d576192d372c8ce8da95100ab97b9132e0c74e2"
        )
    );
    let missing = LoopbackServer::start(|_| LoopbackResponse::from_raw_http(fixtures::CRATES_INDEX_404));
    assert_eq!(
        observe(Ecosystem::Cargo, &cargo_operation(&missing.base_url())).0,
        ProviderObservationV1::Absent
    );
    let pypi_missing = LoopbackServer::start(|_| LoopbackResponse::from_raw_http(fixtures::PYPI_404));
    assert_eq!(
        observe(Ecosystem::Pypi, &pypi_operation(&pypi_missing.base_url())).0,
        ProviderObservationV1::Absent
    );
}

#[test]
fn the_captured_pypi_document_is_exact_yanked_or_malformed() {
    let response = parse_http_response(fixtures::PYPI_200).unwrap();
    assert_eq!(response.status, 200);
    let operation = at_version(pypi_operation("http://unused/"), "2.31.0");
    assert_eq!(
        observed(classify_pypi_version(&response.body, &operation)),
        exact(
            "2.31.0",
            "58cd2187c01e70e6e26505bca751777aa9f2ee0b7f4300988b709f44e013003f"
        )
    );

    let mut yanked: serde_json::Value = serde_json::from_str(&response.body).unwrap();
    yanked["urls"][1]["yanked"] = true.into();
    assert_eq!(
        observed(classify_pypi_version(&yanked.to_string(), &operation)),
        ProviderObservationV1::Conflict {
            reason: ProviderConflictReason::RegistryVersionYanked
        }
    );

    let mut no_files: serde_json::Value = serde_json::from_str(&response.body).unwrap();
    no_files["urls"] = serde_json::json!([]);
    assert_eq!(
        observed(classify_pypi_version(&no_files.to_string(), &operation)),
        ProviderObservationV1::Indeterminate {
            cause: ProviderIndeterminateCause::RegistryVersionUnverified
        }
    );

    let mut malformed: serde_json::Value = serde_json::from_str(&response.body).unwrap();
    malformed.as_object_mut().unwrap().remove("urls");
    assert_eq!(
        observed(classify_pypi_version(&malformed.to_string(), &operation)),
        ProviderObservationV1::Indeterminate {
            cause: ProviderIndeterminateCause::MalformedResponse
        }
    );
}

#[test]
fn the_captured_not_found_responses_report_status_404() {
    for raw in [fixtures::CRATES_INDEX_404, fixtures::PYPI_404] {
        assert_eq!(parse_http_response(raw).unwrap().status, 404);
    }
}
