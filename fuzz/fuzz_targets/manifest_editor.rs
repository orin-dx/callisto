#![no_main]
use callisto_manifests::{OpenContext, open, read_identity};
use callisto_model::{ApplyPermit, ManifestDecl, ManifestFormat, ManifestRole, Version, VersionGrammar};
use libfuzzer_sys::fuzz_target;

const FORMATS: [(ManifestFormat, &str, VersionGrammar); 3] = [
    (ManifestFormat::CargoToml, "Cargo.toml", VersionGrammar::SemVer),
    (ManifestFormat::PackageJson, "package.json", VersionGrammar::SemVer),
    (ManifestFormat::PyprojectToml, "pyproject.toml", VersionGrammar::Pep440),
];

// Any manifest must open or fail cleanly, and a version write that succeeds must leave a manifest that reads back
// the version it wrote.
fuzz_target!(|data: &[u8]| {
    let Some((selector, rest)) = data.split_first() else {
        return;
    };
    let Ok(source) = std::str::from_utf8(rest) else {
        return;
    };
    let (format, file, grammar) = FORMATS[*selector as usize % FORMATS.len()];
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let path = dir.path().join(file);
    if std::fs::write(&path, source).is_err() {
        return;
    }
    let Ok(decl) = ManifestDecl::new(file, ManifestRole::Canonical, format) else {
        return;
    };
    let Ok(ctx) = OpenContext::for_workspace_root(dir.path()) else {
        return;
    };
    let Ok(mut manifest) = open(&decl, &ctx) else {
        return;
    };
    let _ = manifest.iter_dependencies().count();
    let Ok(target) = Version::parse("9.9.9", grammar) else {
        return;
    };
    let Some(permit) = ApplyPermit::granted_unless_dry_run(false) else {
        return;
    };
    if manifest.write_version(&target, &permit).is_err() || manifest.persist(&permit).is_err() {
        return;
    }
    let written = std::fs::read_to_string(&path).expect("a persisted manifest is readable");
    let identity = read_identity(format, &written, &path).expect("a manifest written by the editor parses");
    if let Some(callisto_manifests::VersionSource::Literal(version)) = identity.version {
        assert_eq!(version, "9.9.9", "the editor wrote a version it cannot read back");
    }
});
