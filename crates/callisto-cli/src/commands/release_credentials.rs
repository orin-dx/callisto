//! Credential pre-flight for a local `callisto release`: every selected
//! operation kind must have its credential before the first effect.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use callisto_model::{Ecosystem, ReleaseIntentV1, ReleaseOperationRole, ReleasePackageId};

use crate::error::CliError;

/// The ambient facts a credential check reads.
pub(crate) struct CredentialSources<'a> {
    pub var: &'a dyn Fn(&str) -> Option<String>,
    pub home: Option<PathBuf>,
    pub root: &'a Path,
    /// Absolute directory of each released package, for package-level `.npmrc`.
    pub package_dirs: &'a BTreeMap<ReleasePackageId, PathBuf>,
    pub gh_authenticated: &'a dyn Fn() -> bool,
}

impl CredentialSources<'_> {
    fn set(&self, name: &str) -> bool {
        (self.var)(name).is_some_and(|value| !value.is_empty())
    }

    fn cargo_home(&self) -> Option<PathBuf> {
        (self.var)("CARGO_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| self.home.as_ref().map(|home| home.join(".cargo")))
    }

    /// The `[registry]` table for crates.io, else `[registries.<name>]`.
    fn cargo_table<'t>(document: &'t toml::Table, registry: &str) -> Option<&'t toml::Table> {
        if registry == "crates-io" {
            document.get("registry")?.as_table()
        } else {
            document.get("registries")?.as_table()?.get(registry)?.as_table()
        }
    }

    fn cargo_token_file(&self, registry: &str) -> bool {
        let Some(home) = self.cargo_home() else {
            return false;
        };
        ["credentials.toml", "credentials"].iter().any(|name| {
            read_toml(&home.join(name)).is_some_and(|document| {
                Self::cargo_table(&document, registry)
                    .and_then(|table| table.get("token"))
                    .and_then(toml::Value::as_str)
                    .is_some_and(|token| !token.is_empty())
            })
        })
    }

    /// A configured `credential-provider` other than cargo's own token store.
    fn cargo_credential_provider(&self, registry: &str) -> bool {
        let env_name = cargo_env_name(registry);
        let env = if registry == "crates-io" {
            [
                "CARGO_REGISTRY_CREDENTIAL_PROVIDER",
                "CARGO_REGISTRY_GLOBAL_CREDENTIAL_PROVIDERS",
            ]
            .iter()
            .any(|name| (self.var)(name).is_some_and(|value| is_provider(&value)))
        } else {
            (self.var)(&format!("CARGO_REGISTRIES_{env_name}_CREDENTIAL_PROVIDER"))
                .is_some_and(|value| is_provider(&value))
                || (self.var)("CARGO_REGISTRY_GLOBAL_CREDENTIAL_PROVIDERS").is_some_and(|value| is_provider(&value))
        };
        let configs = [
            self.cargo_home().map(|home| home.join("config.toml")),
            self.cargo_home().map(|home| home.join("config")),
            Some(self.root.join(".cargo").join("config.toml")),
            Some(self.root.join(".cargo").join("config")),
        ];
        env || configs
            .into_iter()
            .flatten()
            .filter_map(|path| read_toml(&path))
            .any(|document| {
                let global = document
                    .get("registry")
                    .and_then(|registry| registry.get("global-credential-providers"))
                    .and_then(toml::Value::as_array)
                    .is_some_and(|providers| providers.iter().filter_map(toml::Value::as_str).any(is_provider));
                let own = Self::cargo_table(&document, registry)
                    .and_then(|table| table.get("credential-provider"))
                    .is_some_and(|provider| match provider {
                        toml::Value::String(provider) => is_provider(provider),
                        toml::Value::Array(parts) => {
                            parts.first().and_then(toml::Value::as_str).is_some_and(is_provider)
                        }
                        _ => false,
                    });
                global || own
            })
    }

    fn cargo(&self, registry: &str) -> bool {
        let token = if registry == "crates-io" {
            self.set("CARGO_REGISTRY_TOKEN") || self.set("CARGO_REGISTRIES_CRATES_IO_TOKEN")
        } else {
            self.set(&format!("CARGO_REGISTRIES_{}_TOKEN", cargo_env_name(registry)))
        };
        token || self.cargo_token_file(registry) || self.cargo_credential_provider(registry)
    }

    fn npm(&self, package_dir: Option<&Path>) -> bool {
        if self.set("NODE_AUTH_TOKEN") || self.set("NPM_TOKEN") || self.set("ACTIONS_ID_TOKEN_REQUEST_URL") {
            return true;
        }
        let user = ["NPM_CONFIG_USERCONFIG", "npm_config_userconfig"]
            .iter()
            .find_map(|name| (self.var)(name).filter(|value| !value.is_empty()))
            .map(PathBuf::from)
            .or_else(|| self.home.as_ref().map(|home| home.join(".npmrc")));
        let files = [
            user,
            Some(self.root.join(".npmrc")),
            package_dir.map(|dir| dir.join(".npmrc")),
        ];
        files.into_iter().flatten().any(|path| {
            std::fs::read_to_string(path).is_ok_and(|content| content.lines().any(|line| self.npmrc_auth_line(line)))
        })
    }

    /// An auth key with a non-empty value whose every `${VAR}` is set.
    fn npmrc_auth_line(&self, line: &str) -> bool {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        let key = key.trim();
        let value = value.trim();
        if key.starts_with(['#', ';']) || value.is_empty() {
            return false;
        }
        if !(key.ends_with("_authToken") || key.ends_with("_auth") || key.ends_with("_password")) {
            return false;
        }
        let mut rest = value;
        while let Some(start) = rest.find("${") {
            let Some(end) = rest[start..].find('}') else {
                return false;
            };
            if !self.set(&rest[start + 2..start + end]) {
                return false;
            }
            rest = &rest[start + end + 1..];
        }
        true
    }

    /// `TWINE_PASSWORD`, OIDC, or a `~/.pypirc` password for `repository`.
    fn pypi(&self, repository: &str) -> bool {
        if self.set("TWINE_PASSWORD") || self.set("ACTIONS_ID_TOKEN_REQUEST_URL") {
            return true;
        }
        let Some(content) = self
            .home
            .as_ref()
            .and_then(|home| std::fs::read_to_string(home.join(".pypirc")).ok())
        else {
            return false;
        };
        let mut section = String::new();
        content.lines().map(str::trim).any(|line| {
            if let Some(name) = line.strip_prefix('[').and_then(|line| line.strip_suffix(']')) {
                section = name.trim().to_owned();
                return false;
            }
            section == repository
                && line
                    .split_once(['=', ':'])
                    .is_some_and(|(key, value)| key.trim() == "password" && !value.trim().is_empty())
        })
    }
}

fn read_toml(path: &Path) -> Option<toml::Table> {
    std::fs::read_to_string(path).ok()?.parse().ok()
}

fn cargo_env_name(registry: &str) -> String {
    registry.to_ascii_uppercase().replace('-', "_")
}

fn is_provider(provider: &str) -> bool {
    let provider = provider.trim();
    !provider.is_empty() && provider != "cargo:token"
}

/// One credential a selected operation needs.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Need {
    Cargo { registry: String },
    Npm { dir: Option<PathBuf> },
    Pypi { repository: String },
    GitHub,
}

impl Need {
    fn present(&self, sources: &CredentialSources<'_>) -> bool {
        match self {
            Need::Cargo { registry } => sources.cargo(registry),
            Need::Npm { dir } => sources.npm(dir.as_deref()),
            Need::Pypi { repository } => sources.pypi(repository),
            Need::GitHub => sources.set("GH_TOKEN") || sources.set("GITHUB_TOKEN") || (sources.gh_authenticated)(),
        }
    }

    fn missing(&self, package: String) -> CliError {
        let (credential, fix) = match self {
            Need::Cargo { registry } if registry == "crates-io" => (
                "CARGO_REGISTRY_TOKEN".to_owned(),
                "set CARGO_REGISTRY_TOKEN, run `cargo login`, or configure a cargo credential-provider".to_owned(),
            ),
            Need::Cargo { registry } => {
                let name = format!("CARGO_REGISTRIES_{}_TOKEN", cargo_env_name(registry));
                let fix = format!(
                    "set {name}, run `cargo login --registry {registry}`, or configure a credential-provider for `{registry}`"
                );
                (name, fix)
            }
            Need::Npm { .. } => (
                "NODE_AUTH_TOKEN".to_owned(),
                "set NODE_AUTH_TOKEN or NPM_TOKEN, add an auth line (with every ${VAR} set) to the user, project, or package .npmrc, or use OIDC trusted publishing".to_owned(),
            ),
            Need::Pypi { repository } => (
                "TWINE_PASSWORD".to_owned(),
                format!(
                    "set TWINE_PASSWORD, add a password for `{repository}` to ~/.pypirc, or use OIDC trusted publishing; a password stored only in a keyring cannot be detected, so set one of these"
                ),
            ),
            Need::GitHub => (
                "GH_TOKEN".to_owned(),
                "set GH_TOKEN or GITHUB_TOKEN, or run `gh auth login`".to_owned(),
            ),
        };
        CliError::ReleaseCredentialMissing {
            package,
            credential,
            fix,
        }
    }
}

/// Fails on the first selected operation whose credential is missing.
pub(crate) fn check(intent: &ReleaseIntentV1, sources: &CredentialSources<'_>) -> Result<(), CliError> {
    let mut needed = BTreeMap::new();
    for operation in &intent.operations {
        let id = operation.id();
        let need = match &id.role {
            ReleaseOperationRole::RegistryPublish { registry }
            | ReleaseOperationRole::PlatformPublish { registry, .. } => match id.package.ecosystem() {
                Ecosystem::Cargo => Need::Cargo {
                    registry: callisto_graph::commands::cargo_registry_name(registry.registry_key()).to_owned(),
                },
                Ecosystem::Npm => Need::Npm {
                    dir: match &id.role {
                        ReleaseOperationRole::PlatformPublish { platform, .. } => {
                            Some(sources.root.join(platform.directory()))
                        }
                        _ => sources.package_dirs.get(&id.package).cloned(),
                    },
                },
                Ecosystem::Pypi => Need::Pypi {
                    repository: registry.registry_key().as_str().to_owned(),
                },
                _ => continue,
            },
            ReleaseOperationRole::ForgeRelease
            | ReleaseOperationRole::ForgePublish
            | ReleaseOperationRole::ArtifactUpload { .. } => Need::GitHub,
            ReleaseOperationRole::Tag => continue,
        };
        needed.entry(need).or_insert_with(|| id.package.to_string());
    }
    for (need, package) in needed {
        if !need.present(sources) {
            return Err(need.missing(package));
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
        let base = dir.path().to_string_lossy().into_owned();
        let vars: BTreeMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.replace("$DIR", &base)))
            .collect();
        let var = |name: &str| vars.get(name).cloned();
        let gh_authenticated = || gh;
        let package_dirs = intent
            .decision
            .entries
            .iter()
            .map(|entry| (entry.package.clone(), root.join("pkg")))
            .collect();
        check(
            intent,
            &CredentialSources {
                var: &var,
                home: Some(home),
                root: &root,
                package_dirs: &package_dirs,
                gh_authenticated: &gh_authenticated,
            },
        )
    }

    fn missing(result: Result<(), CliError>) -> String {
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
        let login = "[registry]\ntoken = \"t\"\n";
        assert!(check_with(&cargo, &[], &[("home/.cargo/credentials.toml", login)], false).is_ok());
        assert!(check_with(&cargo, &[], &[("home/.cargo/credentials.toml", "")], false).is_err());
        assert!(check_with(
            &cargo,
            &[("CARGO_HOME", "/nonexistent")],
            &[("home/.cargo/credentials.toml", login)],
            false
        )
        .is_err());
    }

    /// M4: an alternate registry needs its own token, login entry, or credential-provider.
    #[test]
    fn cargo_alternate_registry_uses_its_own_credential() {
        let private = intent(Ecosystem::Cargo, "my-reg", false);
        assert_eq!(
            missing(check_with(&private, &[("CARGO_REGISTRY_TOKEN", "t")], &[], true)),
            "CARGO_REGISTRIES_MY_REG_TOKEN"
        );
        assert!(check_with(&private, &[("CARGO_REGISTRIES_MY_REG_TOKEN", "t")], &[], false).is_ok());
        let crates_io_login = "[registry]\ntoken = \"t\"\n";
        assert!(check_with(
            &private,
            &[],
            &[("home/.cargo/credentials.toml", crates_io_login)],
            false
        )
        .is_err());
        let own_login = "[registries.my-reg]\ntoken = \"t\"\n";
        assert!(check_with(&private, &[], &[("home/.cargo/credentials.toml", own_login)], false).is_ok());
        let provider = "[registries.my-reg]\ncredential-provider = \"cargo:macos-keychain\"\n";
        assert!(check_with(&private, &[], &[("root/.cargo/config.toml", provider)], false).is_ok());
        let global = "[registry]\nglobal-credential-providers = [\"cargo:libsecret\"]\n";
        assert!(check_with(&private, &[], &[("home/.cargo/config.toml", global)], false).is_ok());
        let token_only = "[registries.my-reg]\ncredential-provider = \"cargo:token\"\n";
        assert!(check_with(&private, &[], &[("root/.cargo/config.toml", token_only)], false).is_err());
        assert!(check_with(
            &private,
            &[("CARGO_REGISTRIES_MY_REG_CREDENTIAL_PROVIDER", "cargo:wincred")],
            &[],
            false
        )
        .is_ok());
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

    /// M3: `${VAR}` auth counts only when VAR is set; user config and package `.npmrc` are read.
    #[test]
    fn npmrc_interpolation_userconfig_and_package_npmrc() {
        let npm = intent(Ecosystem::Npm, "npm", false);
        let interpolated = "//registry.npmjs.org/:_authToken=${MY_NPM}\n";
        assert!(check_with(&npm, &[], &[("root/.npmrc", interpolated)], false).is_err());
        assert!(check_with(&npm, &[("MY_NPM", "")], &[("root/.npmrc", interpolated)], false).is_err());
        assert!(check_with(&npm, &[("MY_NPM", "t")], &[("root/.npmrc", interpolated)], false).is_ok());
        let empty = "//registry.npmjs.org/:_authToken=\n";
        assert!(check_with(&npm, &[], &[("root/.npmrc", empty)], false).is_err());

        let auth = "//registry.npmjs.org/:_authToken=abc\n";
        assert!(check_with(&npm, &[], &[("root/pkg/.npmrc", auth)], false).is_ok());
        let user = [("NPM_CONFIG_USERCONFIG", "$DIR/elsewhere/npmrc")];
        assert!(check_with(&npm, &user, &[("elsewhere/npmrc", auth)], false).is_ok());
        assert!(
            check_with(&npm, &user, &[("home/.npmrc", auth)], false).is_err(),
            "userconfig replaces ~/.npmrc"
        );
    }

    #[test]
    fn pypi_accepts_twine_password_pypirc_or_oidc() {
        let pypi = intent(Ecosystem::Pypi, "pypi", false);
        assert_eq!(missing(check_with(&pypi, &[], &[], true)), "TWINE_PASSWORD");
        assert!(check_with(&pypi, &[("TWINE_PASSWORD", "x")], &[], false).is_ok());
        assert!(check_with(&pypi, &[("ACTIONS_ID_TOKEN_REQUEST_URL", "x")], &[], false).is_ok());
        let pypirc = "[distutils]\nindex-servers = pypi\n\n[pypi]\nusername = __token__\npassword = pypi-abc\n";
        assert!(check_with(&pypi, &[], &[("home/.pypirc", pypirc)], false).is_ok());
        let other = "[testpypi]\npassword = x\n\n[pypi]\nusername = __token__\n";
        assert!(check_with(&pypi, &[], &[("home/.pypirc", other)], false).is_err());
        let Err(CliError::ReleaseCredentialMissing { fix, .. }) = check_with(&pypi, &[], &[], false) else {
            panic!("expected a missing credential");
        };
        assert!(fix.contains("keyring") && fix.contains(".pypirc"), "{fix}");
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
