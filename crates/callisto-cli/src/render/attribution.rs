use callisto_graph::config::{ConfigProvenance, ResolvedConfig};
use callisto_model::ConfigKey;

pub fn attribution_line(key: &ConfigKey, cfg: &ResolvedConfig) -> String {
    let key_str = key.as_str();
    let formatted_key = if let Some((table, field)) = key_str.split_once('.') {
        format!("[{table}].{field}")
    } else {
        key_str.to_string()
    };

    let prov = cfg.provenance(key);
    let val = cfg.rendered_value(key).unwrap_or_else(|| "auto".to_string());

    match prov {
        ConfigProvenance::Default => format!("governed by {formatted_key} = {val} (default)"),
        ConfigProvenance::Explicit => format!("governed by {formatted_key} = {val}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_key_renders_with_table_brackets_and_default_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = callisto_graph::config::load(tmp.path()).unwrap();
        assert_eq!(
            attribution_line(&ConfigKey::CASCADE_BUMP_SEVERITY, &cfg),
            "governed by [cascade].bump-severity = patch (default)"
        );
    }

    #[test]
    fn explicit_toml_value_drops_the_default_marker() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("callisto.toml"),
            "[cascade]\nbump-severity = \"minor\"\n",
        )
        .unwrap();
        let cfg = callisto_graph::config::load(tmp.path()).unwrap();
        assert_eq!(
            attribution_line(&ConfigKey::CASCADE_BUMP_SEVERITY, &cfg),
            "governed by [cascade].bump-severity = minor"
        );
    }

    #[test]
    fn key_with_no_dot_renders_bare_with_no_brackets() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = callisto_graph::config::load(tmp.path()).unwrap();
        assert_eq!(
            attribution_line(&ConfigKey::RELEASE_TRIGGER, &cfg),
            "governed by release-trigger = auto (default)"
        );
    }
}
