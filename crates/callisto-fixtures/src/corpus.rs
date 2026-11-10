pub fn cargo_toml_bom_sample() -> &'static str {
    "\u{FEFF}[package]\nname = \"bom-crate\"\nversion = \"1.0.0\"\nedition = \"2021\"\n"
}

pub fn cargo_toml_tab_indented_sample() -> &'static str {
    "[package]\n\tname = \"tabbed-crate\"\n\tversion = \"1.0.0\"\n\tedition = \"2021\"\n"
}

pub fn cargo_toml_crlf_no_bom_sample() -> &'static str {
    "[package]\r\nname = \"crlf-crate\" # keep me\r\nversion = \"1.0.0\" # release version\r\nedition = \"2021\"\r\n"
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
