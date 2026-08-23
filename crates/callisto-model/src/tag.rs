use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Diagnostic, DiagnosticCode, DiagnosticSeverity, PackageId, Version, VersionGrammar, VersionParseError};

/// A validated tag template containing prefix, suffix, and exact `{version}` placement.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TagTemplate {
    prefix: String,
    suffix: String,
}

impl TagTemplate {
    pub fn parse(raw: &str) -> Result<Self, TagTemplateError> {
        let version_marker = "{version}";
        let count = raw.matches(version_marker).count();
        if count == 0 {
            if let Some(start) = raw.find('{') {
                if let Some(end) = raw[start..].find('}') {
                    let placeholder = &raw[start + 1..start + end];
                    return Err(TagTemplateError::UnknownPlaceholder {
                        template: raw.to_string(),
                        placeholder: placeholder.to_string(),
                    });
                }
            }
            return Err(TagTemplateError::MissingVersionPlaceholder {
                template: raw.to_string(),
            });
        }
        if count > 1 {
            return Err(TagTemplateError::MultipleVersionPlaceholders {
                template: raw.to_string(),
                count,
            });
        }

        let (prefix, suffix) = raw.split_once(version_marker).unwrap();

        if prefix.is_empty() && suffix.is_empty() {
            return Err(TagTemplateError::NoLiteralAnchor {
                template: raw.to_string(),
            });
        }

        for (part, _name) in [(prefix, "prefix"), (suffix, "suffix")] {
            for ch in part.chars() {
                if matches!(ch, '*' | '?' | '[' | ']') {
                    return Err(TagTemplateError::GlobMetacharacterInLiteral {
                        template: raw.to_string(),
                        ch,
                    });
                }
            }
        }

        let test_render = format!("{prefix}1.0.0{suffix}");
        if !is_valid_git_ref_name(&test_render) {
            return Err(TagTemplateError::InvalidGitRefName {
                template: raw.to_string(),
                rendered: test_render,
            });
        }

        Ok(TagTemplate {
            prefix: prefix.to_string(),
            suffix: suffix.to_string(),
        })
    }

    pub fn default_for(id: &PackageId) -> Self {
        TagTemplate {
            prefix: format!("{}@", id.display_name()),
            suffix: String::new(),
        }
    }

    pub fn render(&self, version: &Version) -> TagName {
        TagName(format!("{}{}{}", self.prefix, version.render(), self.suffix))
    }

    pub fn render_floating_major(&self, version: &Version) -> Option<TagName> {
        let major = version.major()?;
        let rendered = format!("{}{}{}", self.prefix, major, self.suffix);
        if is_valid_git_ref_name(&rendered) {
            Some(TagName(rendered))
        } else {
            None
        }
    }

    pub fn glob(&self) -> String {
        format!("{}*{}", self.prefix, self.suffix)
    }

    pub fn extract_version_str<'a>(&self, tag: &'a str) -> Option<&'a str> {
        if !tag.starts_with(&self.prefix) || !tag.ends_with(&self.suffix) {
            return None;
        }
        let end_idx = tag.len() - self.suffix.len();
        if end_idx < self.prefix.len() {
            return None;
        }
        Some(&tag[self.prefix.len()..end_idx])
    }

    pub fn as_str(&self) -> String {
        format!("{}{{version}}{}", self.prefix, self.suffix)
    }
}

impl Serialize for TagTemplate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.as_str())
    }
}

impl<'de> Deserialize<'de> for TagTemplate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        TagTemplate::parse(&s).map_err(serde::de::Error::custom)
    }
}

fn is_valid_git_ref_name(s: &str) -> bool {
    if s.is_empty() || s.starts_with('/') || s.ends_with('/') || s.contains("//") || s == "@" {
        return false;
    }
    // Git ref names may legally start with `-`, but every caller in this
    // codebase passes the rendered name as a bare positional to `git tag`
    // (or an equivalent CLI), whose argument parser treats a leading `-`
    // as an option rather than a ref name -- reject it here so a malicious
    // or accidental `tag-template` can never produce one.
    if s.starts_with('-') {
        return false;
    }
    if s.contains("..") || s.contains("@{") {
        return false;
    }
    for component in s.split('/') {
        if component.starts_with('.') || component.ends_with(".lock") {
            return false;
        }
    }
    for ch in s.chars() {
        if ch.is_ascii_control() || ch == ' ' || matches!(ch, '~' | '^' | ':' | '?' | '*' | '[' | '\\') {
            return false;
        }
    }
    true
}

use schemars::JsonSchema;

/// Rendered Git tag name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[schemars(with = "String")]
#[serde(transparent)]
pub struct TagName(pub String);

impl TagName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TagName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Resolved most recent release tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastTag {
    pub name: TagName,
    pub version: Version,
}

/// Output of last_tag_for pure selection algorithm.
#[derive(Clone, Debug, Default)]
pub struct LastTagSelection {
    pub chosen: Option<LastTag>,
    pub skipped: Vec<Diagnostic>,
}

/// Pure tag selection algorithm.
pub fn select_last_tag<'a>(
    template: &TagTemplate,
    grammar: VersionGrammar,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Result<LastTagSelection, VersionParseError> {
    let mut chosen: Option<LastTag> = None;
    let mut skipped = Vec::new();

    for candidate in candidates {
        let Some(extracted) = template.extract_version_str(candidate) else {
            continue;
        };

        match Version::parse(extracted, grammar) {
            Ok(ver) => {
                let tag = LastTag {
                    name: TagName(candidate.to_string()),
                    version: ver,
                };
                match &chosen {
                    None => chosen = Some(tag),
                    Some(prev) => {
                        let ord = tag.version.compare(&prev.version).map_err(|_err| VersionParseError {
                            raw: extracted.to_string(),
                            grammar,
                            message: "grammar mismatch during candidate selection".to_string(),
                        })?;
                        match ord {
                            std::cmp::Ordering::Greater => chosen = Some(tag),
                            std::cmp::Ordering::Equal => {
                                if tag.name.as_str() > prev.name.as_str() {
                                    chosen = Some(tag);
                                }
                            }
                            std::cmp::Ordering::Less => {}
                        }
                    }
                }
            }
            Err(_) => {
                skipped.push(Diagnostic {
                    code: DiagnosticCode::TagGlobNonVersionMatch,
                    severity: DiagnosticSeverity::Warning,
                    message: format!(
                        "tag candidate `{candidate}` matched glob but placeholder `{extracted}` is not a valid {grammar:?} version"
                    ),
                    package: None,
                    path: None,
                    escalated_by: None,
                    governed_by: None,
                });
            }
        }
    }

    Ok(LastTagSelection { chosen, skipped })
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum TagTemplateError {
    #[error("tag template `{template}` contains no `{{version}}` placeholder")]
    MissingVersionPlaceholder { template: String },

    #[error("tag template `{template}` contains `{{version}}` {count} times; exactly one is required")]
    MultipleVersionPlaceholders { template: String, count: usize },

    #[error("tag template `{template}` contains unknown placeholder `{{{placeholder}}}`; the only placeholder is `{{version}}`")]
    UnknownPlaceholder { template: String, placeholder: String },

    #[error("tag template `{template}` contains glob metacharacter `{ch}` outside the `{{version}}` placeholder")]
    GlobMetacharacterInLiteral { template: String, ch: char },

    #[error("tag template `{template}` has no literal text around `{{version}}`; its tag glob would be `*`")]
    NoLiteralAnchor { template: String },

    #[error("tag template `{template}` renders `{rendered}`, which is not a legal git ref name")]
    InvalidGitRefName { template: String, rendered: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_tag_template() {
        let tmpl = TagTemplate::parse("v{version}").unwrap();
        assert_eq!(tmpl.glob(), "v*");

        let ver = Version::parse("1.2.3", VersionGrammar::SemVer).unwrap();
        assert_eq!(tmpl.render(&ver).as_str(), "v1.2.3");
        assert_eq!(tmpl.render_floating_major(&ver).unwrap().as_str(), "v1");
        assert_eq!(tmpl.extract_version_str("v1.2.3"), Some("1.2.3"));
    }

    #[test]
    fn test_render_floating_major_scoped_package() {
        let tmpl = TagTemplate::parse("@scope/pkg@{version}").unwrap();
        let ver = Version::parse("2.4.0", VersionGrammar::SemVer).unwrap();
        assert_eq!(tmpl.render(&ver).as_str(), "@scope/pkg@2.4.0");
        assert_eq!(tmpl.render_floating_major(&ver).unwrap().as_str(), "@scope/pkg@2");
    }

    #[test]
    fn rejects_no_anchor() {
        let err = TagTemplate::parse("{version}").unwrap_err();
        assert!(matches!(err, TagTemplateError::NoLiteralAnchor { .. }));
    }

    #[test]
    fn rejects_template_rendering_a_leading_hyphen() {
        // "-f{version}" renders to "-f1.0.0" for the test-render probe --
        // git ref names may legally start with `-`, but `git tag`'s CLI
        // argument parser treats a leading-`-` positional as an option,
        // so this must be rejected at template-parse time rather than
        // surfacing as a broken/misinterpreted `git tag` invocation later.
        let err = TagTemplate::parse("-f{version}").unwrap_err();
        assert!(
            matches!(err, TagTemplateError::InvalidGitRefName { .. }),
            "expected InvalidGitRefName, got {err:?}"
        );
    }
}
