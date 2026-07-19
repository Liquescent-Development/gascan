use std::{
    collections::BTreeMap,
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

#[tokio::test]
async fn exec_rejects_unapproved_or_malformed_environment_before_spawn() {
    let attach = AppleAttach::new("/definitely/missing/gascan-apple-attach");
    for (name, value) in [("PATH", "/host/bin"), ("LC_", "C"), ("LANG", "C\0UTF-8")] {
        let result = attach
            .exec_with_environment(
                "container-id",
                ["true"],
                false,
                BTreeMap::from([(name.to_owned(), value.to_owned())]),
            )
            .await;
        let error = match result {
            Ok(_) => panic!("invalid environment unexpectedly reached the helper"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            RuntimeError::CommandIo { operation, message }
                if operation == "gascan-apple-attach"
                    && message.contains("invalid environment variable")
        ));
    }
}

fn set_mode(path: &Path, mode: u32) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(mode);
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
fn relative_executable_override_is_canonicalized_exactly() {
    let current = std::env::current_dir().unwrap();
    let helper = tempfile::Builder::new()
        .prefix("gascan-attach-config-")
        .tempfile_in(&current)
        .unwrap();
    set_mode(helper.path(), 0o755);
    let relative = helper.path().strip_prefix(&current).unwrap().to_path_buf();

    let attach = AppleAttach::configured(Some(relative.into_os_string())).unwrap();
    assert_eq!(
        attach.helper_path(),
        fs::canonicalize(helper.path()).unwrap()
    );
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
fn other_only_execute_bit_is_not_executable_by_its_current_owner() {
    let directory = tempfile::tempdir().unwrap();
    let helper = directory.path().join("other-only");
    fs::write(&helper, "fixture").unwrap();
    set_mode(&helper, 0o001);

    let error = AppleAttach::configured(Some(helper.into_os_string())).unwrap_err();
    assert!(matches!(
        error,
        RuntimeError::CommandIo { operation, message }
            if operation == "GASCAN_APPLE_ATTACH_HELPER"
                && message.contains("is not executable by the effective user")
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
