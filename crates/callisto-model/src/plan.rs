use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// npm package access level, controlling the `--access` flag passed to `npm publish`.
///
/// When `None` is set on a publish entry, no `--access` flag is passed and
/// npm uses its default: `restricted` for scoped packages (`@scope/name`),
/// `public` for unscoped packages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum NpmAccess {
    /// Publish as publicly accessible. Required for scoped packages that
    /// should be available without authentication.
    Public,
    /// Publish as restricted (private). Only accessible to authorized users
    /// and teams. This is npm's default for scoped packages.
    Restricted,
}
