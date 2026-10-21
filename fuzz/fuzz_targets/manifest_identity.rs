#![no_main]
use callisto_manifests::read_identity;
use callisto_model::ManifestFormat;
use libfuzzer_sys::fuzz_target;
use std::path::Path;

const FORMATS: [(ManifestFormat, &str); 3] = [
    (ManifestFormat::CargoToml, "Cargo.toml"),
    (ManifestFormat::PackageJson, "package.json"),
    (ManifestFormat::PyprojectToml, "pyproject.toml"),
];

fuzz_target!(|data: &[u8]| {
    let Some((selector, rest)) = data.split_first() else {
        return;
    };
    let Ok(source) = std::str::from_utf8(rest) else {
        return;
    };
    let (format, file) = FORMATS[*selector as usize % FORMATS.len()];
    let _ = read_identity(format, source, Path::new(file));
});
