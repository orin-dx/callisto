use callisto_model::Severity;

use crate::{ConventionalCommit, ParsedCommit};

pub fn raw_severity(commit: &ConventionalCommit) -> Severity {
    if commit.breaking {
        return Severity::Major;
    }
    match commit.commit_type.as_str() {
        "feat" => Severity::Minor,
        "fix" | "perf" => Severity::Patch,
        _ => Severity::None,
    }
}

pub fn raw_severity_of(commit: &ParsedCommit) -> Severity {
    match commit {
        ParsedCommit::Conventional(c) => raw_severity(c),
        ParsedCommit::NonConventional { .. } => Severity::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_commit;
    use callisto_model::CommitSha;

    #[test]
    fn classifies_severities() {
        let sha = CommitSha::parse("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0").unwrap();
        assert_eq!(
            raw_severity_of(&parse_commit(sha.clone(), "fix: bug")),
            Severity::Patch
        );
        assert_eq!(
            raw_severity_of(&parse_commit(sha.clone(), "feat: feature")),
            Severity::Minor
        );
        assert_eq!(
            raw_severity_of(&parse_commit(sha.clone(), "feat!: breaking")),
            Severity::Major
        );
        assert_eq!(
            raw_severity_of(&parse_commit(sha.clone(), "chore: cleanup")),
            Severity::None
        );
    }
}
