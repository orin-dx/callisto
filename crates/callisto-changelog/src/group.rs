use callisto_model::Severity;

use crate::{ChangelogEntry, ChangelogError};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupedEntries<'a> {
    pub major: Vec<&'a ChangelogEntry>,
    pub minor: Vec<&'a ChangelogEntry>,
    pub patch: Vec<&'a ChangelogEntry>,
}

pub fn group_entries(entries: &[ChangelogEntry]) -> Result<GroupedEntries<'_>, ChangelogError> {
    let mut grouped = GroupedEntries::default();
    for entry in entries {
        match entry.severity {
            Severity::Major => grouped.major.push(entry),
            Severity::Minor => grouped.minor.push(entry),
            Severity::Patch => grouped.patch.push(entry),
            Severity::None => return Err(ChangelogError::SeverityNoneEntry),
        }
    }
    Ok(grouped)
}
