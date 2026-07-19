use std::{
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
};

use gascan_apple::AppleAttach;
use gascan_core::runtime::RuntimeError;

fn executable(path: &Path) {
    fs::write(path, "fixture").unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn absent_override_keeps_packaged_helper_contract() {
    let attach = AppleAttach::configured(None).unwrap();
    assert_eq!(attach.helper_path(), Path::new("gascan-apple-attach"));
}

#[test]
fn executable_override_is_canonicalized_exactly() {
    let directory = tempfile::tempdir().unwrap();
    let helper = directory.path().join("helper");
    executable(&helper);

    let attach = AppleAttach::configured(Some(helper.clone().into_os_string())).unwrap();
    assert_eq!(attach.helper_path(), fs::canonicalize(helper).unwrap());
}

#[test]
fn empty_override_is_rejected_with_actionable_typed_error() {
    let error = AppleAttach::configured(Some(OsString::new())).unwrap_err();
    assert!(matches!(
        error,
        RuntimeError::CommandIo { operation, message }
            if operation == "GASCAN_APPLE_ATTACH_HELPER"
                && message.contains("must not be empty")
    ));
}

#[test]
fn missing_or_non_executable_override_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing");
    let error = AppleAttach::configured(Some(missing.into_os_string())).unwrap_err();
    assert!(matches!(
        error,
        RuntimeError::CommandIo { operation, message }
            if operation == "GASCAN_APPLE_ATTACH_HELPER"
                && message.contains("cannot resolve")
    ));

    let helper = directory.path().join("not-executable");
    fs::write(&helper, "fixture").unwrap();
    let error = AppleAttach::configured(Some(helper.into_os_string())).unwrap_err();
    assert!(matches!(
        error,
        RuntimeError::CommandIo { operation, message }
            if operation == "GASCAN_APPLE_ATTACH_HELPER"
                && message.contains("is not executable")
    ));
}

#[test]
fn non_file_override_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let error = AppleAttach::configured(Some(PathBuf::from(directory.path()).into_os_string()))
        .unwrap_err();
    assert!(matches!(
        error,
        RuntimeError::CommandIo { operation, message }
            if operation == "GASCAN_APPLE_ATTACH_HELPER"
                && message.contains("is not a regular file")
    ));
}
