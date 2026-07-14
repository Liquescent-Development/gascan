use camino::Utf8Path;
use gascan_core::gascamp::{BUNDLED_GASCAMP_REVISION, GascampSource, resolve_gascamp};

#[test]
fn local_gascamp_must_resolve_beneath_workspace() {
    assert!(resolve_gascamp("/workspace/gascamp").is_ok());
    assert!(resolve_gascamp("/workspace/repo/../gascamp").is_ok());
    assert!(resolve_gascamp("/opt/gascan/gascamp").is_err());
    assert!(resolve_gascamp("/workspace/gascamp-link-outside").is_err());
}

#[test]
fn bundled_source_reports_the_locked_revision_as_trusted() {
    let source = resolve_gascamp("bundled").expect("bundled Gascamp source");
    assert_eq!(
        source,
        GascampSource::Bundled {
            revision: BUNDLED_GASCAMP_REVISION,
        }
    );
    assert!(source.trusted());
}

#[test]
fn workspace_override_reports_its_canonical_container_path_as_untrusted() {
    let source =
        resolve_gascamp("/workspace/repo/../gascamp").expect("contained workspace Gascamp source");
    assert_eq!(
        source,
        GascampSource::Workspace {
            path: Utf8Path::new("/workspace/gascamp").to_owned(),
        }
    );
    assert!(!source.trusted());
}
