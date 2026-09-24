//! Credential pre-flight for a local `callisto release`: every selected
//! operation kind must have its credential before the first effect.

use std::path::{Path, PathBuf};

use callisto_model::{Ecosystem, ReleaseIntentV1, ReleaseOperationRole};

use crate::error::CliError;

/// The ambient facts a credential check reads.
pub(crate) struct CredentialSources<'a> {
    pub var: &'a dyn Fn(&str) -> Option<String>,
    pub home: Option<PathBuf>,
    pub root: &'a Path,
    pub gh_authenticated: &'a dyn Fn() -> bool,
}

impl CredentialSources<'_> {
    fn set(&self, name: &str) -> bool {
        (self.var)(name).is_some_and(|value| !value.is_empty())
    }

    fn cargo_credentials_file(&self) -> bool {
        let cargo_home = (self.var)("CARGO_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| self.home.as_ref().map(|home| home.join(".cargo")));
        cargo_home.is_some_and(|dir| dir.join("credentials.toml").is_file() || dir.join("credentials").is_file())
    }

    fn npmrc_auth(&self) -> bool {
        let files = [
            self.home.as_ref().map(|home| home.join(".npmrc")),
            Some(self.root.join(".npmrc")),
        ];
        files.into_iter().flatten().any(|path| {
            std::fs::read_to_string(path).is_ok_and(|content| {
                content.lines().any(|line| {
                    let key = line.split('=').next().unwrap_or_default().trim();
                    !key.starts_with(['#', ';'])
                        && (key.ends_with("_authToken") || key.ends_with("_auth") || key.ends_with("_password"))
                })
            })
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Cargo,
    Npm,
    Pypi,
    GitHub,
}

impl Kind {
    fn credential(self) -> &'static str {
        match self {
            Kind::Cargo => "CARGO_REGISTRY_TOKEN",
            Kind::Npm => "NODE_AUTH_TOKEN",
            Kind::Pypi => "TWINE_PASSWORD",
            Kind::GitHub => "GH_TOKEN",
        }
    }

    fn fix(self) -> &'static str {
        match self {
            Kind::Cargo => "set CARGO_REGISTRY_TOKEN, or run `cargo login`",
            Kind::Npm => "set NODE_AUTH_TOKEN or NPM_TOKEN, add an auth line to ~/.npmrc or the project .npmrc, or use OIDC trusted publishing",
            Kind::Pypi => "set TWINE_PASSWORD, or use OIDC trusted publishing",
            Kind::GitHub => "set GH_TOKEN or GITHUB_TOKEN, or run `gh auth login`",
        }
    }

    fn present(self, sources: &CredentialSources<'_>) -> bool {
        let oidc = sources.set("ACTIONS_ID_TOKEN_REQUEST_URL");
        match self {
            Kind::Cargo => sources.set("CARGO_REGISTRY_TOKEN") || sources.cargo_credentials_file(),
            Kind::Npm => sources.set("NODE_AUTH_TOKEN") || sources.set("NPM_TOKEN") || oidc || sources.npmrc_auth(),
            Kind::Pypi => sources.set("TWINE_PASSWORD") || oidc,
            Kind::GitHub => sources.set("GH_TOKEN") || sources.set("GITHUB_TOKEN") || (sources.gh_authenticated)(),
        }
    }
}

/// Fails on the first selected package whose operation kind has no credential.
pub(crate) fn check(intent: &ReleaseIntentV1, sources: &CredentialSources<'_>) -> Result<(), CliError> {
    let mut needed = std::collections::BTreeMap::new();
    for operation in &intent.operations {
        let id = operation.id();
        let kind = match &id.role {
            ReleaseOperationRole::RegistryPublish { .. } | ReleaseOperationRole::PlatformPublish { .. } => {
                match id.package.ecosystem() {
                    Ecosystem::Cargo => Kind::Cargo,
                    Ecosystem::Npm => Kind::Npm,
                    Ecosystem::Pypi => Kind::Pypi,
                    _ => continue,
                }
            }
            ReleaseOperationRole::ForgeRelease
            | ReleaseOperationRole::ForgePublish
            | ReleaseOperationRole::ArtifactUpload { .. } => Kind::GitHub,
            ReleaseOperationRole::Tag => continue,
        };
        needed.entry(kind).or_insert_with(|| id.package.to_string());
    }
    for (kind, package) in needed {
        if !kind.present(sources) {
            return Err(CliError::ReleaseCredentialMissing {
                package,
                credential: kind.credential(),
                fix: kind.fix(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeMap;

    use callisto_model::{
        ExecutionTrustProfileV1, RegistryBindingDigest, RegistryBindingId, ReleaseDecisionEntry, ReleaseDecisionV1,
        ReleaseInclusionReason, ReleaseInputSnapshotV1, ReleaseOperation, ReleasePackageId, ReleasePackageInputV1,
        SemanticInputDigest, SourceIdentity, Version, VersionGrammar,
    };

    use super::*;

    pub(crate) fn intent(ecosystem: Ecosystem, registry: &str, forge: bool) -> ReleaseIntentV1 {
        let package = ReleasePackageId::new(ecosystem, "demo").unwrap();
        let version = Version::parse("1.0.0", VersionGrammar::SemVer).unwrap();
        let binding = RegistryBindingId::new(registry, RegistryBindingDigest::parse(&"a".repeat(64)).unwrap()).unwrap();
        let publish = ReleaseOperation::registry_publish(package.clone(), version.clone(), binding, vec![]).unwrap();
        let tag = ReleaseOperation::tag(package.clone(), version.clone(), vec![publish.id().clone()]).unwrap();
        let mut operations = vec![publish, tag.clone()];
        if forge {
            let release =
                ReleaseOperation::forge_release(package.clone(), version.clone(), vec![tag.id().clone()]).unwrap();
            let published =
                ReleaseOperation::forge_publish(package.clone(), version.clone(), vec![release.id().clone()]).unwrap();
            operations.extend([release, published]);
        }
        let decision = ReleaseDecisionV1::new(vec![ReleaseDecisionEntry {
            package: package.clone(),
            target_version: version,
            reasons: vec![ReleaseInclusionReason::UnreleasedVersion],
        }])
        .unwrap();
        let snapshot = ReleaseInputSnapshotV1::new(
            SourceIdentity::GitCommit {
                sha: callisto_model::CommitSha::parse(&"b".repeat(40)).unwrap(),
            },
            vec![ReleasePackageInputV1 {
                package,
                fingerprint: SemanticInputDigest::parse(&"c".repeat(64)).unwrap(),
            }],
        )
        .unwrap();
        ReleaseIntentV1::new(
            decision,
            snapshot,
            ExecutionTrustProfileV1::GitCommit,
            operations,
            vec![],
        )
        .unwrap()
    }

    fn check_with(
        intent: &ReleaseIntentV1,
        vars: &[(&str, &str)],
        files: &[(&str, &str)],
        gh: bool,
    ) -> Result<(), CliError> {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let root = dir.path().join("root");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        for (path, body) in files {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        let vars: BTreeMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let var = |name: &str| vars.get(name).cloned();
        let gh_authenticated = || gh;
        check(
            intent,
            &CredentialSources {
                var: &var,
                home: Some(home),
                root: &root,
                gh_authenticated: &gh_authenticated,
            },
        )
    }

    fn missing(result: Result<(), CliError>) -> &'static str {
        match result {
            Err(CliError::ReleaseCredentialMissing { credential, .. }) => credential,
            other => panic!("expected a missing credential, got {other:?}"),
        }
    }

    #[test]
    fn cargo_needs_a_token_or_a_login_file() {
        let cargo = intent(Ecosystem::Cargo, "cratesIo", false);
        assert_eq!(missing(check_with(&cargo, &[], &[], true)), "CARGO_REGISTRY_TOKEN");
        assert_eq!(
            missing(check_with(&cargo, &[("CARGO_REGISTRY_TOKEN", "")], &[], true)),
            "CARGO_REGISTRY_TOKEN"
        );
        assert!(check_with(&cargo, &[("CARGO_REGISTRY_TOKEN", "t")], &[], false).is_ok());
        assert!(check_with(&cargo, &[], &[("home/.cargo/credentials.toml", "")], false).is_ok());
        assert!(check_with(
            &cargo,
            &[("CARGO_HOME", "/nonexistent")],
            &[("home/.cargo/credentials.toml", "")],
            false
        )
        .is_err());
    }

    #[test]
    fn npm_accepts_tokens_npmrc_auth_or_oidc() {
        let npm = intent(Ecosystem::Npm, "npm", false);
        assert_eq!(missing(check_with(&npm, &[], &[], true)), "NODE_AUTH_TOKEN");
        for var in ["NODE_AUTH_TOKEN", "NPM_TOKEN", "ACTIONS_ID_TOKEN_REQUEST_URL"] {
            assert!(check_with(&npm, &[(var, "x")], &[], false).is_ok(), "{var}");
        }
        let auth = "//registry.npmjs.org/:_authToken=abc\n";
        assert!(check_with(&npm, &[], &[("home/.npmrc", auth)], false).is_ok());
        assert!(check_with(&npm, &[], &[("root/.npmrc", auth)], false).is_ok());
        assert!(check_with(&npm, &[], &[("root/.npmrc", "registry=https://example.com/\n")], false).is_err());
        assert!(check_with(&npm, &[], &[("root/.npmrc", "# //r/:_authToken=abc\n")], false).is_err());
    }

    #[test]
    fn pypi_accepts_twine_password_or_oidc() {
        let pypi = intent(Ecosystem::Pypi, "pypi", false);
        assert_eq!(missing(check_with(&pypi, &[], &[], true)), "TWINE_PASSWORD");
        assert!(check_with(&pypi, &[("TWINE_PASSWORD", "x")], &[], false).is_ok());
        assert!(check_with(&pypi, &[("ACTIONS_ID_TOKEN_REQUEST_URL", "x")], &[], false).is_ok());
    }

    #[test]
    fn forge_release_needs_a_github_token_or_gh_login() {
        let forge = intent(Ecosystem::Cargo, "cratesIo", true);
        let cargo = [("CARGO_REGISTRY_TOKEN", "t")];
        assert_eq!(missing(check_with(&forge, &cargo, &[], false)), "GH_TOKEN");
        assert!(check_with(&forge, &cargo, &[], true).is_ok());
        for var in ["GH_TOKEN", "GITHUB_TOKEN"] {
            assert!(check_with(&forge, &[cargo[0], (var, "x")], &[], false).is_ok(), "{var}");
        }
    }

    #[test]
    fn the_error_names_the_package_and_the_credential() {
        let error = check_with(&intent(Ecosystem::Cargo, "cratesIo", false), &[], &[], true).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("CARGO_REGISTRY_TOKEN") && message.contains("cargo/demo"),
            "{message}"
        );
    }
}
