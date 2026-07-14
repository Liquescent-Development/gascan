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

    let spec = SandboxSpec::from_root("code", link, Manifest::default()).expect("valid spec");
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

    assert!(SandboxSpec::from_root("code", &file, Manifest::default()).is_err());
    assert!(SandboxSpec::from_root("code", &root.join("missing"), Manifest::default()).is_err());
}
