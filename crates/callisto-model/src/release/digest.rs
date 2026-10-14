use std::fmt;
use std::io::Read;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::identity::string_json_schema;

macro_rules! release_digest {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        string_json_schema!($name);

        impl $name {
            /// Parses a lowercase hexadecimal SHA-256 digest.
            pub fn parse(raw: &str) -> Result<Self, DigestParseError> {
                if raw.len() != 64
                    || !raw
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                {
                    return Err(DigestParseError {
                        digest_kind: stringify!($name),
                        raw: raw.to_string(),
                    });
                }
                Ok(Self(raw.to_string()))
            }

            /// Returns the canonical lowercase hexadecimal digest.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            fn from_sha256(bytes: impl AsRef<[u8]>) -> Self {
                let rendered = format!("{:x}", Sha256::digest(bytes.as_ref()));
                Self(rendered)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let raw = String::deserialize(deserializer)?;
                Self::parse(&raw).map_err(serde::de::Error::custom)
            }
        }
    };
}

release_digest!(ArtifactDigest, "SHA-256 over exact artifact bytes.");

release_digest!(DecisionDigest, "SHA-256 over a release-decision transcript.");

release_digest!(SemanticInputDigest, "SHA-256 over a semantic-input transcript.");

release_digest!(IntentDigest, "SHA-256 over a release-intent transcript.");

release_digest!(
    RegistryBindingDigest,
    "SHA-256 over a normalized, credential-free registry binding."
);

/// Error returned when a durable digest is not canonical lowercase SHA-256.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{digest_kind} must be exactly 64 lowercase hexadecimal characters, got {raw}")]
pub struct DigestParseError {
    digest_kind: &'static str,
    raw: String,
}

impl ArtifactDigest {
    /// Hashes the exact artifact bytes without normalization.
    pub fn from_bytes(bytes: impl AsRef<[u8]>) -> Self {
        Self::from_sha256(bytes)
    }

    /// Streams exact artifact bytes into SHA-256 without retaining the full
    /// asset in memory. Release artifacts can be substantially larger than
    /// manifests, so executor verification must use this path.
    pub fn from_reader(mut reader: impl Read) -> std::io::Result<(Self, u64)> {
        let mut hasher = Sha256::new();
        let mut length = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            length = length.checked_add(read as u64).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "artifact exceeds supported length")
            })?;
        }
        Ok((Self(format!("{:x}", hasher.finalize())), length))
    }
}

impl DecisionDigest {
    /// Hashes a versioned release-decision transcript.
    pub fn from_transcript(transcript: &CanonicalTranscript) -> Self {
        Self::from_sha256(transcript.as_bytes())
    }
}

impl SemanticInputDigest {
    /// Hashes a versioned semantic-input transcript.
    pub fn from_transcript(transcript: &CanonicalTranscript) -> Self {
        Self::from_sha256(transcript.as_bytes())
    }
}

impl IntentDigest {
    /// Hashes a versioned release-intent transcript.
    pub fn from_transcript(transcript: &CanonicalTranscript) -> Self {
        Self::from_sha256(transcript.as_bytes())
    }
}

impl RegistryBindingDigest {
    /// Hashes a graph-normalized registry binding without retaining its source
    /// endpoint in a durable model value.
    ///
    /// The caller is responsible for URL parsing, rejecting credentials and
    /// query/fragment data, and producing a canonical binding before calling
    /// this method. Only this digest is retained.
    pub fn from_normalized_binding(bytes: impl AsRef<[u8]>) -> Self {
        let mut transcript = CanonicalTranscript::registry_binding_v1();
        transcript.push_bytes("binding", bytes);
        Self::from_sha256(transcript.as_bytes())
    }
}

/// Deterministic length-prefixed bytes for a durable release digest.
///
/// The transcript starts with a fixed protocol marker, a one-byte schema
/// version, and a length-prefixed domain. Each appended field is a
/// length-prefixed UTF-8 tag followed by a length-prefixed byte value. Domain
/// types own ordering of fields; this primitive preserves that order exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalTranscript {
    bytes: Vec<u8>,
}

impl CanonicalTranscript {
    const MARKER: &'static [u8] = b"callisto-release-transcript";
    const VERSION: u8 = 1;

    /// Starts the v1 transcript used for semantic input snapshots.
    pub fn semantic_input_v1() -> Self {
        Self::v1(b"semantic-input")
    }

    /// Starts the v1 transcript used for release decisions.
    pub fn decision_v1() -> Self {
        Self::v1(b"release-decision")
    }

    /// Starts the v1 transcript used for release intents.
    pub fn intent_v1() -> Self {
        Self::v1(b"release-intent")
    }

    /// Starts the v1 transcript used for normalized registry bindings.
    fn registry_binding_v1() -> Self {
        Self::v1(b"registry-binding")
    }

    fn v1(domain: &[u8]) -> Self {
        let mut bytes = Vec::with_capacity(Self::MARKER.len() + domain.len() + 16);
        Self::push_length_prefixed(&mut bytes, Self::MARKER);
        bytes.push(Self::VERSION);
        Self::push_length_prefixed(&mut bytes, domain);
        Self { bytes }
    }

    /// Appends one named UTF-8 field.
    pub fn push_str(&mut self, tag: &str, value: &str) {
        self.push_bytes(tag, value.as_bytes());
    }

    /// Appends one named byte field.
    pub fn push_bytes(&mut self, tag: &str, value: impl AsRef<[u8]>) {
        Self::push_length_prefixed(&mut self.bytes, tag.as_bytes());
        Self::push_length_prefixed(&mut self.bytes, value.as_ref());
    }

    /// Returns the exact canonical bytes, suitable only for hashing or test vectors.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn push_length_prefixed(out: &mut Vec<u8>, value: &[u8]) {
        let length = u64::try_from(value.len()).expect("usize always fits in u64 on supported platforms");
        out.extend_from_slice(&length.to_be_bytes());
        out.extend_from_slice(value);
    }
}
