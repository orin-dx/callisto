use callisto_model::{
    CommandRunner, CratePublish, NpmMainPublish, PublishPlan, PublishTarget, ReleaseEntry,
    SCHEMA_VERSION,
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
    let mut releases = Vec::new();

    let base_versions = ws.base_versions()?;
    let inference = crate::infer::NoInference;
    let version_plan = crate::commands::version::plan_version(
        ws,
        &inference,
        &crate::commands::version::VersionOptions::default(),
    )
    .ok();

    let all_ids: std::collections::HashSet<_> = ws.graph.packages().map(|p| p.id.clone()).collect();
    let topo_ids = ws.graph.toposort(&all_ids)?;

    let head_sha = if let Ok(repo) = callisto_vcs::GitRepository::discover(&ws.root) {
        repo.head_sha().ok()
    } else {
        None
    };

    for id in &topo_ids {
        let pkg = match ws.graph.packages().find(|p| &p.id == id) {
            Some(p) => p,
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
                .tags
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
                    });
                } else {
                    let platform_deps: Vec<String> = ws
                        .graph
                        .dependencies_of(&pkg.id)
                        .filter(|edge| {
                            ws.graph
                                .packages()
                                .find(|p| p.id == edge.to)
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
                        depends_on_platforms: platform_deps,
                    });
                }
            }

            if !pkg.publish_to.is_empty()
                && !pkg.publish_to.iter().all(|t| *t == PublishTarget::None)
            {
                if let Some(ref sha) = head_sha {
                    releases.push(ReleaseEntry {
                        package: pkg.id.clone(),
                        tag_name: ws.tags.template(&pkg.id).render(&ver),
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
        releases,
        diagnostics: Vec::new(),
    })
}

use callisto_model::{
    Ecosystem, PackageId, RateLimitPolicy, RegistryClient, RegistryError, TimeProvider, Version,
};
use std::time::Duration;

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
        if let Ok(secs) = retry_after_header.parse::<u64>() {
            Some(Duration::from_secs(secs))
        } else {
            None
        }
    }

    pub fn execute(&self, plan: &PublishPlan) -> Result<(), RegistryError> {
        for rust_crate in &plan.rust_crates {
            let pkg_id = PackageId::Prefixed {
                ecosystem: Ecosystem::Cargo,
                name: rust_crate.name.clone(),
            };
            self.publish_with_retry(&pkg_id, &rust_crate.version)?;
        }

        for npm_pkg in &plan.npm_main_packages {
            let pkg_id = PackageId::Prefixed {
                ecosystem: Ecosystem::Npm,
                name: npm_pkg.name.clone(),
            };
            self.publish_with_retry(&pkg_id, &npm_pkg.version)?;
        }

        for npm_pkg in &plan.npm_platform_packages {
            let pkg_id = PackageId::Prefixed {
                ecosystem: Ecosystem::Npm,
                name: npm_pkg.name.clone(),
            };
            self.publish_with_retry(&pkg_id, &npm_pkg.version)?;
        }

        Ok(())
    }

    fn publish_with_retry(
        &self,
        pkg_id: &PackageId,
        version: &Version,
    ) -> Result<(), RegistryError> {
        if self.client.is_published(pkg_id, version)? {
            return Ok(());
        }

        loop {
            match self.client.publish(pkg_id, version) {
                Ok(_) => return Ok(()),
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
    use super::*;
    use std::sync::Mutex;
    use std::time::SystemTime;

    struct MockRegistryClient {
        published: Mutex<std::collections::HashSet<(PackageId, Version)>>,
        rate_limit_responses: Mutex<Vec<Result<(), RegistryError>>>,
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

        fn publish(&self, package: &PackageId, version: &Version) -> Result<(), RegistryError> {
            let mut rate_limits = self.rate_limit_responses.lock().unwrap();
            if let Some(res) = rate_limits.pop() {
                res?;
            }

            let mut published = self.published.lock().unwrap();
            published.insert((package.clone(), version.clone()));
            Ok(())
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
            releases: vec![],
            diagnostics: vec![],
        }
    }

    #[test]
    fn test_publish_success() {
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            rate_limit_responses: Mutex::new(vec![]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        orchestrator.execute(&create_test_plan()).unwrap();
        assert_eq!(orchestrator.time.now(), SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn test_publish_rate_limit_retry() {
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            rate_limit_responses: Mutex::new(vec![Err(RegistryError::RateLimited(
                Duration::from_secs(60),
            ))]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        orchestrator.execute(&create_test_plan()).unwrap();
        assert_eq!(
            orchestrator.time.now(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(60)
        );
    }

    #[test]
    fn test_publish_rate_limit_exceeds_600s() {
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            rate_limit_responses: Mutex::new(vec![Err(RegistryError::RateLimited(
                Duration::from_secs(601),
            ))]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let err = orchestrator.execute(&create_test_plan()).unwrap_err();
        assert!(matches!(err, RegistryError::RateLimited(d) if d == Duration::from_secs(601)));
    }

    #[test]
    fn test_publish_auth_fail_fast() {
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            rate_limit_responses: Mutex::new(vec![Err(RegistryError::AuthFailed(
                "Invalid token".to_string(),
            ))]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let err = orchestrator.execute(&create_test_plan()).unwrap_err();
        assert!(matches!(err, RegistryError::AuthFailed(_)));
    }

    #[test]
    fn test_parse_ttl() {
        assert_eq!(PublishOrchestrator::<MockRegistryClient, MockRateLimitPolicy, MockTimeProvider>::parse_http_429_ttl("120"), Some(Duration::from_secs(120)));
        assert_eq!(PublishOrchestrator::<MockRegistryClient, MockRateLimitPolicy, MockTimeProvider>::parse_http_429_ttl("invalid"), None);
    }
}
