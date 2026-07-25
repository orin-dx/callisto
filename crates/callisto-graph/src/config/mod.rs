pub mod groups;
pub mod raw;
pub mod resolve;

pub use groups::{GroupDef, GroupMember, GroupMemberKind, GroupTable, RawGroupTable};
pub use resolve::{
    load, parse_pre_major_policy, CascadeBumpSeverity, CascadeConfig, CascadeMode,
    ConfigProvenance, PackageConfig, PreMajorInferencePolicy, RegistryConfig, ResolvedConfig,
    ValidationConfig,
};
