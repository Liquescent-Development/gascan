use camino::Utf8Path;
use gascan_core::manifest::Manifest;
use gascan_core::sandbox::{SandboxId, SandboxSpec, WORKSPACE_TARGET};

#[test]
fn canonical_path_produces_stable_noncolliding_id() {
    let first = SandboxId::from_root("code", Utf8Path::new("/Users/me/code"));
    let again = SandboxId::from_root("code", Utf8Path::new("/Users/me/code"));
    let other = SandboxId::from_root("code", Utf8Path::new("/Volumes/code"));
    assert_eq!(first, again);
    assert_ne!(first, other);
    assert_eq!(first.as_str().len(), "code-".len() + 12);
}

#[test]
fn names_are_slugged_without_affecting_path_identity() {
    let id = SandboxId::from_root("  My Project_2026!  ", Utf8Path::new("/workspace/code"));
    assert!(id.as_str().starts_with("my-project-2026-"));
    assert!(!id.as_str().contains("--"));
}

#[test]
fn fixture_paths_do_not_collide() {
    let roots = [
        "/a/code",
        "/b/code",
        "/a/Code",
        "/a/code-2",
        "/Volumes/a/code",
    ];
    let ids = roots
        .map(|root| SandboxId::from_root("code", Utf8Path::new(root)))
        .map(|id| id.to_string());
    let unique = ids.into_iter().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), roots.len());
}

#[test]
fn spec_canonicalizes_symlinks_and_mounts_only_the_root() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let real = temp.path().join("real");
    std::fs::create_dir(&real).expect("real root");
    let link = temp.path().join("link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &link).expect("root symlink");
    let link = Utf8Path::from_path(&link).expect("UTF-8 fixture path");

    let manifest = Manifest::load(link).expect("default manifest");
    let spec = SandboxSpec::from_root("code", link, manifest).expect("valid spec");
    let canonical_path = std::fs::canonicalize(&real).expect("canonical fixture root");
    let canonical = Utf8Path::from_path(&canonical_path).expect("UTF-8 canonical path");
    assert_eq!(spec.canonical_root(), canonical);
    assert_eq!(spec.bind_mounts().len(), 1);
    assert_eq!(spec.bind_mounts()[0].source(), canonical);
    assert_eq!(
        spec.bind_mounts()[0].target(),
        Utf8Path::new(WORKSPACE_TARGET)
    );
    assert!(spec.bind_mounts()[0].is_writable());
}

#[test]
fn spec_rejects_missing_roots_and_non_directories() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = Utf8Path::from_path(temp.path()).expect("UTF-8 fixture path");
    let file = root.join("file");
    std::fs::write(&file, "data").expect("file fixture");

    let manifest = Manifest::load(root).expect("default manifest");
    assert!(SandboxSpec::from_root("code", &file, manifest).is_err());
    assert!(Manifest::load(&root.join("missing")).is_err());
}

#[test]
fn manifest_loaded_for_another_root_is_rejected() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let base = Utf8Path::from_path(temp.path()).expect("UTF-8 fixture path");
    let first = base.join("first");
    let second = base.join("second");
    std::fs::create_dir(&first).expect("first root");
    std::fs::create_dir(&second).expect("second root");
    let manifest = Manifest::load(&first).expect("first manifest");

    let error = SandboxSpec::from_root("code", &second, manifest)
        .expect_err("manifest provenance must match the sandbox root");
    assert!(error.to_string().contains("loaded for a different root"));
}

#[test]
fn sandbox_id_deserialization_rejects_unchecked_strings() {
    for invalid in [
        "code",
        "Code-0123456789ab",
        "code--0123456789ab",
        "code-0123456789a",
        "code-0123456789az",
        "-0123456789ab",
    ] {
        let encoded = format!("\"{invalid}\"");
        assert!(
            serde_json::from_str::<SandboxId>(&encoded).is_err(),
            "accepted {invalid}"
        );
    }

    let generated = SandboxId::from_root("code", Utf8Path::new("/workspace/code"));
    let encoded = serde_json::to_string(&generated).expect("serialize generated ID");
    assert_eq!(
        serde_json::from_str::<SandboxId>(&encoded).expect("deserialize checked ID"),
        generated
    );
}
