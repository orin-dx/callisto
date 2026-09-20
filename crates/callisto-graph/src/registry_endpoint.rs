//! The one canonical registry endpoint, shared by config validation and release binding.

use callisto_model::{CanonicalTranscript, RegistryBindingDigest, RegistryKey};

/// Credential-free URL projection used only inside a digest transcript.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RegistryBindingV1 {
    pub(crate) scheme: String,
    pub(crate) host: String,
    pub(crate) effective_port: Option<u16>,
    pub(crate) path: String,
}

impl RegistryBindingV1 {
    pub(crate) fn digest(&self) -> RegistryBindingDigest {
        let mut transcript = CanonicalTranscript::semantic_input_v1();
        transcript.push_str("registry.scheme", &self.scheme);
        transcript.push_str("registry.host", &self.host);
        transcript.push_str(
            "registry.port",
            &self.effective_port.map_or_else(String::new, |port| port.to_string()),
        );
        transcript.push_str("registry.path", &self.path);
        RegistryBindingDigest::from_normalized_binding(transcript.as_bytes())
    }

    pub(crate) fn endpoint(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let port = match (&self.scheme[..], self.effective_port) {
            ("https", Some(443)) | ("http", Some(80)) | (_, None) => String::new(),
            (_, Some(port)) => format!(":{port}"),
        };
        format!("{}://{host}{port}{}", self.scheme, self.path)
    }
}

/// Maps a `url::ParseError` to a specific, static diagnostic reason for
/// [`GraphError::UnsafeRegistryBinding`]/[`GraphError::UnsafeGitRemote`],
/// rather than discarding it behind one generic "invalid URL" message.
pub(crate) fn url_parse_error_reason(error: url::ParseError) -> &'static str {
    match error {
        url::ParseError::RelativeUrlWithoutBase => "URL has no scheme (e.g. missing `https://`)",
        url::ParseError::EmptyHost => "URL host is empty",
        url::ParseError::InvalidPort => "URL port is invalid",
        url::ParseError::InvalidIpv4Address | url::ParseError::InvalidIpv6Address => {
            "URL host is not a valid IP address"
        }
        url::ParseError::InvalidDomainCharacter => "URL host contains an invalid character",
        url::ParseError::Overflow => "URL exceeds the maximum length",
        _ => "invalid URL",
    }
}

/// Cargo's `sparse+` marker on a registry index URL. Its presence is how cargo
/// itself distinguishes an HTTP sparse index from a git index.
pub(crate) const SPARSE_INDEX_PREFIX: &str = "sparse+";

/// Canonicalises a registry URL (scheme/host case, default port, path) or names the unsafe part.
pub(crate) fn canonical_registry_url(raw: &str) -> Result<RegistryBindingV1, &'static str> {
    let raw = raw.strip_prefix(SPARSE_INDEX_PREFIX).unwrap_or(raw);
    let parsed = url::Url::parse(raw).map_err(url_parse_error_reason)?;
    if parsed.cannot_be_a_base() || parsed.host_str().is_none() {
        return Err("URL must have an authority");
    }
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("URL scheme must be http or https");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("userinfo is forbidden");
    }
    if parsed.query().is_some() {
        return Err("query string is forbidden");
    }
    if parsed.fragment().is_some() {
        return Err("fragment is forbidden");
    }
    Ok(RegistryBindingV1 {
        scheme: parsed.scheme().to_ascii_lowercase(),
        host: parsed.host_str().expect("validated authority").to_ascii_lowercase(),
        effective_port: parsed.port_or_known_default(),
        path: parsed.path().to_string(),
    })
}

/// The URL a built-in registry key stands for when no `url` is configured.
pub(crate) fn builtin_registry_url(key: &str) -> Option<&'static str> {
    match key {
        RegistryKey::CRATES_IO => Some("https://index.crates.io/"),
        RegistryKey::NPM => Some("https://registry.npmjs.org/"),
        RegistryKey::PYPI => Some("https://pypi.org/"),
        RegistryKey::NUGET => Some("https://api.nuget.org/v3/index.json"),
        _ => None,
    }
}

/// The canonical destination of a registry: its configured URL, else its built-in one.
pub(crate) fn registry_destination(key: &str, url: Option<&str>) -> Option<RegistryBindingV1> {
    canonical_registry_url(url.or_else(|| builtin_registry_url(key))?).ok()
}
