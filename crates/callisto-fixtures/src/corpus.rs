pub fn valid_changeset_sample() -> &'static str {
    r#"---
"my-pkg": patch
"@scoped/web-app": minor
---

A sample patch and minor polyglot changeset.
"#
}

pub fn valid_pre_json_sample() -> &'static str {
    r#"{
  "mode": "pre",
  "tag": "beta",
  "initialVersions": {
    "my-pkg": "1.0.0",
    "@scoped/web-app": "2.0.0"
  },
  "changesets": []
}"#
}

pub fn cargo_workspace_toml_sample() -> &'static str {
    r#"[workspace]
members = ["crates/*"]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2021"

[workspace.dependencies]
serde = "1"
"#
}

pub fn npm_package_json_sample() -> &'static str {
    r#"{
  "name": "@scoped/web-app",
  "version": "2.0.0",
  "dependencies": {
    "react": "^18.0.0"
  }
}"#
}

pub fn pyproject_toml_sample() -> &'static str {
    r#"[project]
name = "py-service"
version = "0.4.0"
dependencies = [
    "requests>=2.28.0"
]
"#
}

pub fn go_mod_sample() -> &'static str {
    r#"module github.com/myorg/goservice

go 1.22
"#
}

pub fn cargo_toml_bom_sample() -> &'static str {
    "\u{FEFF}[package]\nname = \"bom-crate\"\nversion = \"1.0.0\"\nedition = \"2021\"\n"
}

pub fn cargo_toml_tab_indented_sample() -> &'static str {
    "[package]\n\tname = \"tabbed-crate\"\n\tversion = \"1.0.0\"\n\tedition = \"2021\"\n"
}

pub fn pyproject_toml_bom_crlf_sample() -> &'static str {
    "\u{FEFF}[project]\r\nname = \"bom-crlf-lib\" # keep me\r\nversion = \"1.0.0\" # release version\r\n"
}

pub fn pyproject_toml_tab_indented_sample() -> &'static str {
    "[project]\nname = \"tabbed-lib\"\nversion = \"1.0.0\"\ndependencies = [\n\t\"requests>=2.28.0\",\n]\n"
}

pub fn pyproject_toml_crlf_no_bom_sample() -> &'static str {
    "[project]\r\nname = \"crlf-lib\" # keep me\r\nversion = \"1.0.0\" # release version\r\n"
}

pub fn pyproject_toml_bom_no_crlf_sample() -> &'static str {
    "\u{FEFF}[project]\nname = \"bom-lf-lib\" # keep me\nversion = \"1.0.0\" # release version\n"
}
