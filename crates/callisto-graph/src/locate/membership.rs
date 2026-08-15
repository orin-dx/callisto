#[cfg(test)]
mod tests {
    #[test]
    fn yaml_rust2_smoke_test_parses_a_trivial_mapping() {
        let docs = yaml_rust2::YamlLoader::load_from_str("packages:\n  - \"a\"\n").unwrap();
        assert_eq!(docs.len(), 1);
    }
}
