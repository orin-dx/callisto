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
    let val = cfg
        .rendered_value(key)
        .unwrap_or_else(|| "auto".to_string());

    match prov {
        ConfigProvenance::Default => format!("governed by {formatted_key} = {val} (default)"),
        ConfigProvenance::Explicit => format!("governed by {formatted_key} = {val}"),
    }
}
