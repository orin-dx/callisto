use callisto_model::CommitSha;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConventionalCommit {
    pub sha: CommitSha,
    pub commit_type: String,
    pub scope: Option<String>,
    pub breaking: bool,
    pub description: String,
    pub body: Option<String>,
    pub footers: Vec<CommitFooter>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitFooter {
    pub token: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedCommit {
    Conventional(ConventionalCommit),
    NonConventional { sha: CommitSha, subject: String },
}

impl ParsedCommit {
    pub fn sha(&self) -> &CommitSha {
        match self {
            ParsedCommit::Conventional(c) => &c.sha,
            ParsedCommit::NonConventional { sha, .. } => sha,
        }
    }

    pub fn subject(&self) -> &str {
        match self {
            ParsedCommit::Conventional(c) => &c.description,
            ParsedCommit::NonConventional { subject, .. } => subject,
        }
    }
}

pub fn parse_commit(sha: CommitSha, message: &str) -> ParsedCommit {
    let mut lines = message.lines();
    let Some(header) = lines.next() else {
        return ParsedCommit::NonConventional {
            sha,
            subject: String::new(),
        };
    };

    let header_trim = header.trim();
    if header_trim.is_empty() {
        return ParsedCommit::NonConventional {
            sha,
            subject: String::new(),
        };
    }

    let Some((type_and_scope_and_bang, description)) = header_trim.split_once(": ") else {
        return ParsedCommit::NonConventional {
            sha,
            subject: header_trim.to_string(),
        };
    };

    if description.is_empty() {
        return ParsedCommit::NonConventional {
            sha,
            subject: header_trim.to_string(),
        };
    }

    let mut rest = type_and_scope_and_bang;
    let mut breaking_from_header = false;
    if let Some(r) = rest.strip_suffix('!') {
        breaking_from_header = true;
        rest = r;
    }

    let mut commit_type = rest;
    let mut scope = None;

    if let Some((t, s_rest)) = rest.split_once('(') {
        if let Some(s) = s_rest.strip_suffix(')') {
            commit_type = t;
            scope = Some(s.to_string());
        } else {
            return ParsedCommit::NonConventional {
                sha,
                subject: header_trim.to_string(),
            };
        }
    }

    if commit_type.is_empty() || commit_type.contains(char::is_whitespace) {
        return ParsedCommit::NonConventional {
            sha,
            subject: header_trim.to_string(),
        };
    }

    let rest_body: Vec<&str> = lines.collect();
    let mut body_lines = Vec::new();
    let mut footers = Vec::new();
    let mut footer_lines = Vec::new();
    let mut breaking_from_footer = false;

    let mut blocks: Vec<Vec<&str>> = Vec::new();
    let mut current_block: Vec<&str> = Vec::new();

    for line in rest_body {
        if line.trim().is_empty() {
            if !current_block.is_empty() {
                blocks.push(current_block);
                current_block = Vec::new();
            }
        } else {
            current_block.push(line);
        }
    }
    if !current_block.is_empty() {
        blocks.push(current_block);
    }

    if let Some(last_block) = blocks.pop() {
        if let Some(first) = last_block.first() {
            if is_footer_line(first.trim()) {
                footer_lines = last_block;
            } else {
                blocks.push(last_block);
            }
        }
    }

    for block in blocks {
        if !body_lines.is_empty() {
            body_lines.push("");
        }
        body_lines.extend(block);
    }

    for line in footer_lines {
        let trimmed = line.trim();
        if let Some((token, val)) = parse_footer_line(trimmed) {
            if token == "BREAKING CHANGE" || token == "BREAKING-CHANGE" {
                breaking_from_footer = true;
            }
            footers.push(CommitFooter {
                token: token.to_string(),
                value: val.to_string(),
            });
        } else if let Some(last) = footers.last_mut() {
            last.value.push('\n');
            last.value.push_str(trimmed);
        }
    }

    let body_str = body_lines.join("\n").trim().to_string();
    let body = if body_str.is_empty() {
        None
    } else {
        Some(body_str)
    };

    ParsedCommit::Conventional(ConventionalCommit {
        sha,
        commit_type: commit_type.to_string(),
        scope,
        breaking: breaking_from_header || breaking_from_footer,
        description: description.to_string(),
        body,
        footers,
    })
}

fn is_footer_line(line: &str) -> bool {
    parse_footer_line(line).is_some()
}

fn parse_footer_line(line: &str) -> Option<(&str, &str)> {
    if let Some((token, val)) = line.split_once(": ") {
        if token == "BREAKING CHANGE" || token == "BREAKING-CHANGE" || is_valid_footer_token(token)
        {
            return Some((token, val.trim()));
        }
    }
    if let Some((token, val)) = line.split_once(" #") {
        if is_valid_footer_token(token) {
            return Some((token, val.trim()));
        }
    }
    None
}

fn is_valid_footer_token(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_feat_commit() {
        let sha = CommitSha::parse("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0").unwrap();
        let msg = "feat(core)!: add groundbreaking feature\n\nBREAKING CHANGE: change api";
        let parsed = parse_commit(sha, msg);
        match parsed {
            ParsedCommit::Conventional(c) => {
                assert_eq!(c.commit_type, "feat");
                assert_eq!(c.scope, Some("core".to_string()));
                assert!(c.breaking);
                assert_eq!(c.description, "add groundbreaking feature");
            }
            _ => panic!("expected conventional"),
        }
    }

    #[test]
    fn test_body_with_colon_not_treated_as_footer() {
        let sha = CommitSha::parse("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0").unwrap();
        let msg = "feat(api): update endpoint\n\nNote: this is a body paragraph.\n\nSigned-off-by: Developer <dev@example.com>";
        let parsed = parse_commit(sha, msg);
        match parsed {
            ParsedCommit::Conventional(c) => {
                assert_eq!(c.body, Some("Note: this is a body paragraph.".to_string()));
                assert_eq!(c.footers.len(), 1);
                assert_eq!(c.footers[0].token, "Signed-off-by");
            }
            _ => panic!("expected conventional"),
        }
    }
}
