use callisto_model::{
    CommandRunner, CratePublish, NpmMainPublish, PublishPlan, PublishTarget, PypiPublish,
    RegistryKey, ReleaseEntry, SCHEMA_VERSION,
};

use crate::error::GraphError;
use crate::resolver::{DependencyResolver, DependencyResolverExt};
use crate::Workspace;

#[derive(Clone, Debug, Default)]
pub struct PublishOptions {}

pub fn plan_publish<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    _opts: &PublishOptions,
) -> Result<PublishPlan, GraphError> {
    let mut rust_crates = Vec::new();
    let mut npm_main_packages = Vec::new();
    let mut npm_platform_packages = Vec::new();
    let mut pypi_packages = Vec::new();
    let mut releases = Vec::new();

    let base_versions = ws.base_versions()?;
    let inference = crate::infer::NoInference;
    let version_plan = crate::commands::version::plan_version(
        ws,
        &inference,
        &crate::commands::version::VersionOptions::default(),
    )
    .ok();

    // Build a single lookup map once — eliminates O(N) scans inside the topo loop
    // (PERF-003/004/005). Keys and values are borrowed from the graph for the
    // lifetime of this function, so no extra clones are needed for the lookups.
    let pkg_map: std::collections::HashMap<&callisto_model::PackageId, &callisto_model::Package> =
        ws.graph.packages().map(|p| (&p.id, p)).collect();
    let all_ids: std::collections::HashSet<_> = pkg_map.keys().map(|&id| id.clone()).collect();
    let topo_ids = ws.graph.toposort(&all_ids)?;

    let head_sha = if let Ok(repo) = callisto_vcs::GitRepository::discover(&ws.root) {
        repo.head_sha().ok()
    } else {
        None
    };

    for id in &topo_ids {
        let pkg = match pkg_map.get(id) {
            Some(&p) => p,
            None => continue,
        };

        let bump_info = version_plan
            .as_ref()
            .and_then(|plan| plan.bumps.iter().find(|b| b.package == pkg.id));

        let (is_release, ver) = if let Some(bump) = bump_info {
            (true, bump.to.clone())
        } else {
            let cur_ver = base_versions.get(&pkg.id).cloned().ok_or_else(|| {
                GraphError::Manifest(callisto_model::ManifestError::MissingField {
                    path: pkg
                        .manifests
                        .first()
                        .map(|m| m.path.clone())
                        .unwrap_or_default(),
                    field: "version",
                })
            })?;
            let tag_match = ws
                .tags()?
                .last_tag(&pkg.id)
                .map(|t| t.version == cur_ver)
                .unwrap_or(false);
            (!tag_match, cur_ver)
        };

        if is_release {
            let publishes_cargo = pkg
                .publish_to
                .iter()
                .any(|t| matches!(t, callisto_model::PublishTarget::CratesIo));
            let publishes_npm = pkg
                .publish_to
                .iter()
                .any(|t| matches!(t, callisto_model::PublishTarget::Npm { .. }));
            let publishes_pypi = pkg
                .publish_to
                .iter()
                .any(|t| matches!(t, callisto_model::PublishTarget::Pypi { .. }));
            let is_platform_pkg = pkg
                .manifests
                .iter()
                .any(|m| matches!(m.role, callisto_model::ManifestRole::Platform { .. }));

            if publishes_cargo {
                rust_crates.push(CratePublish {
                    name: pkg.id.name().to_string(),
                    version: ver.clone(),
                    publish_to: callisto_model::RegistryKey(
                        callisto_model::RegistryKey::CRATES_IO.to_string(),
                    ),
                    registry: None,
                });
            }

            if publishes_npm {
                let tag = if ver.is_prerelease() {
                    Some("next".to_string())
                } else {
                    None
                };

                if is_platform_pkg {
                    npm_platform_packages.push(callisto_model::NpmPublish {
                        name: pkg.id.name().to_string(),
                        version: ver.clone(),
                        publish_to: callisto_model::RegistryKey(
                            callisto_model::RegistryKey::NPM.to_string(),
                        ),
                        registry: None,
                        tag: tag.clone(),
                        // `access: None` lets npm use its ecosystem default:
                        // restricted for @scoped packages, public for unscoped.
                        // Callers that need explicit public access should set
                        // this to Some(NpmAccess::Public) before passing the
                        // plan to SubprocessRegistryClient::load_plan.
                        access: None,
                    });
                } else {
                    let platform_deps: Vec<String> = ws
                        .graph
                        .dependencies_of(&pkg.id)
                        .filter(|edge| {
                            pkg_map
                                .get(&edge.to)
                                .map(|p| {
                                    p.manifests.iter().any(|m| {
                                        matches!(
                                            m.role,
                                            callisto_model::ManifestRole::Platform { .. }
                                        )
                                    })
                                })
                                .unwrap_or(false)
                        })
                        .map(|edge| edge.to.name().to_string())
                        .collect();

                    npm_main_packages.push(NpmMainPublish {
                        name: pkg.id.name().to_string(),
                        version: ver.clone(),
                        publish_to: callisto_model::RegistryKey(
                            callisto_model::RegistryKey::NPM.to_string(),
                        ),
                        registry: None,
                        tag,
                        access: None,
                        depends_on_platforms: platform_deps,
                    });
                }
            }

            if publishes_pypi {
                // Extract the optional custom index URL from the first Pypi
                // target. Multiple Pypi entries on the same package are not
                // expected, so only the first is consulted.
                let index = pkg
                    .publish_to
                    .iter()
                    .find_map(|t| {
                        if let callisto_model::PublishTarget::Pypi { index } = t {
                            Some(index.clone())
                        } else {
                            None
                        }
                    })
                    .flatten();

                pypi_packages.push(PypiPublish {
                    name: pkg.id.name().to_string(),
                    version: ver.clone(),
                    publish_to: RegistryKey("pypi".to_string()),
                    index,
                });
            }

            if !pkg.publish_to.is_empty()
                && !pkg.publish_to.iter().all(|t| *t == PublishTarget::None)
            {
                if let Some(ref sha) = head_sha {
                    releases.push(ReleaseEntry {
                        package: pkg.id.clone(),
                        tag_name: ws.tags()?.template(&pkg.id).render(&ver),
                        sha: sha.clone(),
                        changelog_section: None,
                    });
                }
            }
        }
    }

    Ok(PublishPlan {
        schema_version: SCHEMA_VERSION,
        rust_crates,
        npm_main_packages,
        npm_platform_packages,
        pypi_packages,
        releases,
        diagnostics: Vec::new(),
    })
}

use callisto_model::{
    ApplyPermit, Ecosystem, PackageId, PublishAttempt, PublishAttemptResult, PublishOutcome,
    PublishReport, RateLimitPolicy, RegistryClient, RegistryError, TimeProvider, Version,
};
use std::time::Duration;

/// Parses a numeric retry-after value (seconds) as reported by a registry
/// tool's output. Shared by [`PublishOrchestrator::parse_http_429_ttl`] and
/// by ecosystem [`RegistryClient`] implementations that need to extract a
/// retry duration from free-form subprocess output.
pub fn parse_retry_after(raw: &str) -> Option<Duration> {
    raw.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Production [`TimeProvider`] backed by the OS clock and a real sleep.
pub struct SystemTimeProvider;

impl TimeProvider for SystemTimeProvider {
    fn now(&self) -> std::time::SystemTime {
        std::time::SystemTime::now()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Production [`RateLimitPolicy`] that always permits the retry the registry
/// asked for. `PublishOrchestrator` already bounds total wait via its 600s
/// cutoff, so this policy has no additional gating to apply.
pub struct AlwaysRetryPolicy;

impl RateLimitPolicy for AlwaysRetryPolicy {
    fn check_rate_limit(&self, _retry_after: Duration) -> Result<(), RegistryError> {
        Ok(())
    }
}

pub struct PublishOrchestrator<R, P, T> {
    client: R,
    policy: P,
    time: T,
}

impl<R, P, T> PublishOrchestrator<R, P, T>
where
    R: RegistryClient,
    P: RateLimitPolicy,
    T: TimeProvider,
{
    pub fn new(client: R, policy: P, time: T) -> Self {
        Self {
            client,
            policy,
            time,
        }
    }

    pub fn parse_http_429_ttl(retry_after_header: &str) -> Option<Duration> {
        parse_retry_after(retry_after_header)
    }

    /// Attempts to publish every package in `plan` to its ecosystem
    /// registry, recording a per-package outcome (or failure) for each one
    /// rather than aborting the whole batch on the first error — one
    /// package's registry rejection or auth failure must not silently erase
    /// the fact that earlier packages in the same run genuinely published or
    /// were already present.
    pub fn execute(&self, plan: &PublishPlan, permit: &ApplyPermit) -> PublishReport {
        let mut attempts = Vec::new();

        for rust_crate in &plan.rust_crates {
            let pkg_id = PackageId::Prefixed {
                ecosystem: Ecosystem::Cargo,
                name: rust_crate.name.clone(),
            };
            attempts.push(self.attempt_publish(pkg_id, rust_crate.version.clone(), permit));
        }

        for npm_pkg in &plan.npm_platform_packages {
            let pkg_id = PackageId::Prefixed {
                ecosystem: Ecosystem::Npm,
                name: npm_pkg.name.clone(),
            };
            attempts.push(self.attempt_publish(pkg_id, npm_pkg.version.clone(), permit));
        }

        for npm_pkg in &plan.npm_main_packages {
            let pkg_id = PackageId::Prefixed {
                ecosystem: Ecosystem::Npm,
                name: npm_pkg.name.clone(),
            };
            attempts.push(self.attempt_publish(pkg_id, npm_pkg.version.clone(), permit));
        }

        for pypi_pkg in &plan.pypi_packages {
            let pkg_id = PackageId::Prefixed {
                ecosystem: Ecosystem::Pypi,
                name: pypi_pkg.name.clone(),
            };
            attempts.push(self.attempt_publish(pkg_id, pypi_pkg.version.clone(), permit));
        }

        PublishReport {
            schema_version: callisto_model::SCHEMA_VERSION,
            attempts,
            diagnostics: Vec::new(),
        }
    }

    fn attempt_publish(
        &self,
        package: PackageId,
        version: Version,
        permit: &ApplyPermit,
    ) -> PublishAttempt {
        let result = match self.publish_with_retry(&package, &version, permit) {
            Ok(PublishOutcome::Published) => PublishAttemptResult::Published,
            Ok(PublishOutcome::AlreadyPublished) => PublishAttemptResult::AlreadyPublished,
            Err(err) => PublishAttemptResult::Failed {
                error: err.to_string(),
            },
        };

        PublishAttempt {
            package,
            version,
            result,
        }
    }

    fn publish_with_retry(
        &self,
        pkg_id: &PackageId,
        version: &Version,
        permit: &ApplyPermit,
    ) -> Result<PublishOutcome, RegistryError> {
        if self.client.is_published(pkg_id, version)? {
            return Ok(PublishOutcome::AlreadyPublished);
        }

        loop {
            match self.client.publish(pkg_id, version, permit) {
                // Both a fresh publish and a publish-time "already there"
                // classification are done-and-not-an-error: neither should
                // retry, and AlreadyPublished is treated identically to the
                // is_published short-circuit above.
                Ok(outcome @ (PublishOutcome::Published | PublishOutcome::AlreadyPublished)) => {
                    return Ok(outcome)
                }
                Err(RegistryError::RateLimited(retry_after)) => {
                    if retry_after > Duration::from_secs(600) {
                        return Err(RegistryError::RateLimited(retry_after));
                    }
                    self.policy.check_rate_limit(retry_after)?;
                    self.time.sleep(retry_after);
                }
                Err(RegistryError::AuthFailed(err)) => {
                    return Err(RegistryError::AuthFailed(err));
                }
                Err(err) => {
                    return Err(err);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {

    fn permit() -> ApplyPermit {
        ApplyPermit::force_for_tests()
    }
    use super::*;
    use std::sync::Mutex;
    use std::time::SystemTime;

    struct MockRegistryClient {
        published: Mutex<std::collections::HashSet<(PackageId, Version)>>,
        /// Stack of canned responses (popped one per `publish` call). When
        /// exhausted, `publish` defaults to a fresh `Ok(Published)`.
        responses: Mutex<Vec<Result<PublishOutcome, RegistryError>>>,
    }

    impl RegistryClient for MockRegistryClient {
        fn is_published(
            &self,
            package: &PackageId,
            version: &Version,
        ) -> Result<bool, RegistryError> {
            let published = self.published.lock().unwrap();
            Ok(published.contains(&(package.clone(), version.clone())))
        }

        fn publish(
            &self,
            package: &PackageId,
            version: &Version,
            _permit: &ApplyPermit,
        ) -> Result<PublishOutcome, RegistryError> {
            let mut responses = self.responses.lock().unwrap();
            let outcome = match responses.pop() {
                Some(res) => res?,
                None => PublishOutcome::Published,
            };

            if matches!(outcome, PublishOutcome::Published) {
                let mut published = self.published.lock().unwrap();
                published.insert((package.clone(), version.clone()));
            }
            Ok(outcome)
        }
    }

    struct MockRateLimitPolicy;
    impl RateLimitPolicy for MockRateLimitPolicy {
        fn check_rate_limit(&self, _retry_after: Duration) -> Result<(), RegistryError> {
            Ok(())
        }
    }

    struct MockTimeProvider {
        time: Mutex<SystemTime>,
    }

    impl TimeProvider for MockTimeProvider {
        fn now(&self) -> SystemTime {
            *self.time.lock().unwrap()
        }

        fn sleep(&self, duration: Duration) {
            let mut time = self.time.lock().unwrap();
            *time += duration;
        }
    }

    fn create_test_plan() -> callisto_model::PublishPlan {
        callisto_model::PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![callisto_model::CratePublish {
                name: "test-crate".to_string(),
                version: Version::parse("1.0.0", callisto_model::VersionGrammar::SemVer).unwrap(),
                publish_to: callisto_model::RegistryKey("crates.io".to_string()),
                registry: None,
            }],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        }
    }

    fn pypi_publish_entry(name: &str) -> callisto_model::PypiPublish {
        callisto_model::PypiPublish {
            name: name.to_string(),
            version: v100(),
            publish_to: callisto_model::RegistryKey("pypi".to_string()),
            index: None,
        }
    }

    #[test]
    fn test_publish_success() {
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            responses: Mutex::new(vec![]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let report = orchestrator.execute(&create_test_plan(), &permit());
        assert_eq!(report.attempts.len(), 1);
        assert!(matches!(
            report.attempts[0].result,
            PublishAttemptResult::Published
        ));
        assert_eq!(orchestrator.time.now(), SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn test_publish_already_published_is_not_an_error_and_does_not_retry() {
        // publish() itself reporting AlreadyPublished (rather than the
        // is_published pre-check short-circuiting) must be treated the same
        // way: success, no retry loop, no sleep.
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            responses: Mutex::new(vec![Ok(PublishOutcome::AlreadyPublished)]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let report = orchestrator.execute(&create_test_plan(), &permit());
        assert_eq!(report.attempts.len(), 1);
        assert!(matches!(
            report.attempts[0].result,
            PublishAttemptResult::AlreadyPublished
        ));
        assert_eq!(orchestrator.time.now(), SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn test_publish_rate_limit_retry() {
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            responses: Mutex::new(vec![Err(RegistryError::RateLimited(Duration::from_secs(
                60,
            )))]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let report = orchestrator.execute(&create_test_plan(), &permit());
        assert_eq!(report.attempts.len(), 1);
        assert!(matches!(
            report.attempts[0].result,
            PublishAttemptResult::Published
        ));
        assert_eq!(
            orchestrator.time.now(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(60)
        );
    }

    #[test]
    fn test_publish_rate_limit_exceeds_600s() {
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            responses: Mutex::new(vec![Err(RegistryError::RateLimited(Duration::from_secs(
                601,
            )))]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let report = orchestrator.execute(&create_test_plan(), &permit());
        assert_eq!(report.attempts.len(), 1);
        match &report.attempts[0].result {
            PublishAttemptResult::Failed { error } => {
                assert!(error.contains("601"));
            }
            other => panic!("expected Failed outcome, got {other:?}"),
        }
    }

    #[test]
    fn test_publish_auth_fail_fast() {
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            responses: Mutex::new(vec![Err(RegistryError::AuthFailed(
                "Invalid token".to_string(),
            ))]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let report = orchestrator.execute(&create_test_plan(), &permit());
        assert_eq!(report.attempts.len(), 1);
        match &report.attempts[0].result {
            PublishAttemptResult::Failed { error } => {
                assert!(error.contains("Invalid token"));
            }
            other => panic!("expected Failed outcome, got {other:?}"),
        }
    }

    fn v100() -> Version {
        Version::parse("1.0.0", callisto_model::VersionGrammar::SemVer).unwrap()
    }

    fn crate_publish(name: &str) -> callisto_model::CratePublish {
        callisto_model::CratePublish {
            name: name.to_string(),
            version: v100(),
            publish_to: callisto_model::RegistryKey("crates.io".to_string()),
            registry: None,
        }
    }

    #[test]
    fn test_publish_execute_reports_distinct_per_package_outcomes() {
        // crate-a publishes fresh, crate-b is already on the index, crate-c
        // fails outright. The report returned by `execute` must surface all
        // three distinctly instead of discarding per-package results.
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            responses: Mutex::new(vec![
                Err(RegistryError::AuthFailed("bad token".to_string())), // crate-c
                Ok(PublishOutcome::AlreadyPublished),                    // crate-b
                Ok(PublishOutcome::Published),                           // crate-a
            ]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let plan = callisto_model::PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![
                crate_publish("crate-a"),
                crate_publish("crate-b"),
                crate_publish("crate-c"),
            ],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        };

        let report = orchestrator.execute(&plan, &permit());

        assert_eq!(report.attempts.len(), 3);
        assert_eq!(report.attempts[0].package.name(), "crate-a");
        assert!(matches!(
            report.attempts[0].result,
            callisto_model::PublishAttemptResult::Published
        ));
        assert_eq!(report.attempts[1].package.name(), "crate-b");
        assert!(matches!(
            report.attempts[1].result,
            callisto_model::PublishAttemptResult::AlreadyPublished
        ));
        assert_eq!(report.attempts[2].package.name(), "crate-c");
        match &report.attempts[2].result {
            callisto_model::PublishAttemptResult::Failed { error } => {
                assert!(error.contains("bad token"));
            }
            other => panic!("expected Failed outcome for crate-c, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_ttl() {
        assert_eq!(PublishOrchestrator::<MockRegistryClient, MockRateLimitPolicy, MockTimeProvider>::parse_http_429_ttl("120"), Some(Duration::from_secs(120)));
        assert_eq!(PublishOrchestrator::<MockRegistryClient, MockRateLimitPolicy, MockTimeProvider>::parse_http_429_ttl("invalid"), None);
    }

    // ---------------------------------------------------------------- pypi

    /// `execute` must iterate `pypi_packages` and submit each one to the
    /// registry client under `Ecosystem::Pypi`, recording a per-package
    /// attempt just as it does for Cargo and npm packages.
    #[test]
    fn test_execute_dispatches_pypi_packages() {
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            responses: Mutex::new(vec![
                Ok(PublishOutcome::AlreadyPublished), // pypi-b
                Ok(PublishOutcome::Published),        // pypi-a
            ]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let plan = callisto_model::PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![pypi_publish_entry("pypi-a"), pypi_publish_entry("pypi-b")],
            releases: vec![],
            diagnostics: vec![],
        };

        let report = orchestrator.execute(&plan, &permit());

        assert_eq!(
            report.attempts.len(),
            2,
            "expected one attempt per pypi package"
        );
        assert_eq!(report.attempts[0].package.name(), "pypi-a");
        assert!(
            matches!(report.attempts[0].result, PublishAttemptResult::Published),
            "pypi-a should be Published"
        );
        assert_eq!(report.attempts[1].package.name(), "pypi-b");
        assert!(
            matches!(
                report.attempts[1].result,
                PublishAttemptResult::AlreadyPublished
            ),
            "pypi-b should be AlreadyPublished"
        );
    }

    /// npm platform packages must be published before npm main packages because
    /// main packages list platforms in their `optionalDependencies` and the
    /// registry resolver requires platforms to already exist.
    #[test]
    fn test_npm_platforms_published_before_mains() {
        struct RecordingClient {
            order: Mutex<Vec<String>>,
        }

        impl RegistryClient for RecordingClient {
            fn is_published(
                &self,
                _pkg: &PackageId,
                _ver: &Version,
            ) -> Result<bool, RegistryError> {
                Ok(false)
            }

            fn publish(
                &self,
                pkg: &PackageId,
                _ver: &Version,
                _permit: &ApplyPermit,
            ) -> Result<PublishOutcome, RegistryError> {
                self.order.lock().unwrap().push(pkg.name().to_string());
                Ok(PublishOutcome::Published)
            }
        }

        let client = RecordingClient {
            order: Mutex::new(Vec::new()),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let npm_version = v100();
        let plan = callisto_model::PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![],
            npm_platform_packages: vec![callisto_model::NpmPublish {
                name: "platform-linux".to_string(),
                version: npm_version.clone(),
                publish_to: callisto_model::RegistryKey(
                    callisto_model::RegistryKey::NPM.to_string(),
                ),
                registry: None,
                tag: None,
                access: None,
            }],
            npm_main_packages: vec![callisto_model::NpmMainPublish {
                name: "main-package".to_string(),
                version: npm_version.clone(),
                publish_to: callisto_model::RegistryKey(
                    callisto_model::RegistryKey::NPM.to_string(),
                ),
                registry: None,
                tag: None,
                access: None,
                depends_on_platforms: vec!["platform-linux".to_string()],
            }],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        };

        drop(orchestrator.execute(&plan, &permit()));

        let order = orchestrator.client.order.lock().unwrap();
        let platform_pos = order
            .iter()
            .position(|n| n == "platform-linux")
            .expect("platform-linux was not published");
        let main_pos = order
            .iter()
            .position(|n| n == "main-package")
            .expect("main-package was not published");
        assert!(
            platform_pos < main_pos,
            "platform packages must be published before main packages, but got order: {order:?}"
        );
    }

    /// An auth failure on a PyPI package must be recorded as `Failed` and
    /// must not propagate to abort remaining packages in the same execute run.
    #[test]
    fn test_execute_pypi_auth_failure_is_recorded_not_propagated() {
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            responses: Mutex::new(vec![Err(RegistryError::AuthFailed(
                "invalid PyPI token".to_string(),
            ))]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let plan = callisto_model::PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![pypi_publish_entry("bad-pkg")],
            releases: vec![],
            diagnostics: vec![],
        };

        let report = orchestrator.execute(&plan, &permit());

        assert_eq!(report.attempts.len(), 1);
        match &report.attempts[0].result {
            PublishAttemptResult::Failed { error } => {
                assert!(error.contains("invalid PyPI token"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }
}
