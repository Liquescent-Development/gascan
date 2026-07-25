use gascan::cli::{SshInvocation, ssh_invocation, wait_for_ssh};
use gascan_proto::v1;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn status(enabled: bool, active: bool) -> v1::SandboxStatus {
    v1::SandboxStatus {
        sandbox_id: "code-123".to_owned(),
        actual_state: v1::ActualState::Running as i32,
        ssh: Some(v1::SshStatus {
            enabled,
            active,
            host: active.then(|| "127.0.0.1".to_owned()),
            port: active.then_some(22222),
            alias: active.then(|| "gascan-code-123".to_owned()),
            host_key_fingerprint: active.then(|| "SHA256:host".to_owned()),
            client_key_fingerprint: active.then(|| "SHA256:client".to_owned()),
        }),
        ..Default::default()
    }
}

#[test]
fn invocation_uses_only_system_ssh_managed_config_and_stable_alias() -> TestResult {
    let invocation = ssh_invocation(
        &status(true, true),
        Path::new("/Users/test/.config/gascan/ssh/config"),
        Vec::<OsString>::new(),
    )?;
    assert_eq!(
        invocation,
        SshInvocation {
            program: "/usr/bin/ssh".into(),
            arguments: vec![
                OsString::from("-F"),
                OsString::from("/Users/test/.config/gascan/ssh/config"),
                OsString::from("gascan-code-123"),
            ],
        }
    );
    Ok(())
}

#[test]
fn invocation_preserves_every_remote_os_string_after_the_alias() -> TestResult {
    let remote = vec![
        OsString::from("printf"),
        OsString::from("--"),
        OsString::from_vec(b"spaces and \xff bytes".to_vec()),
        OsString::from("$(must-not-run)"),
    ];
    let invocation = ssh_invocation(
        &status(true, true),
        Path::new("/Users/test/.config/gascan/ssh/config"),
        remote.clone(),
    )?;
    assert_eq!(&invocation.arguments[3..], remote);
    Ok(())
}

#[test]
fn disabled_and_inactive_statuses_are_actionable() {
    let disabled = ssh_invocation(
        &status(false, false),
        Path::new("/Users/test/.config/gascan/ssh/config"),
        Vec::<OsString>::new(),
    )
    .expect_err("offline SSH must be rejected");
    assert_eq!(disabled.stable_code(), Some("ssh_disabled"));
    assert_eq!(
        disabled.message(),
        "SSH requires a networked sandbox with SSH enabled"
    );

    let inactive = ssh_invocation(
        &status(true, false),
        Path::new("/Users/test/.config/gascan/ssh/config"),
        Vec::<OsString>::new(),
    )
    .expect_err("inactive SSH must be rejected");
    assert_eq!(inactive.stable_code(), Some("ssh_not_ready"));
    assert!(inactive.message().contains("gascan up"));
}

#[test]
fn malformed_active_status_is_never_used_to_build_a_command() {
    for malformed in [
        v1::SshStatus {
            enabled: true,
            active: true,
            host: Some("0.0.0.0".to_owned()),
            port: Some(22222),
            alias: Some("gascan-code-123".to_owned()),
            host_key_fingerprint: Some("SHA256:host".to_owned()),
            client_key_fingerprint: Some("SHA256:client".to_owned()),
        },
        v1::SshStatus {
            enabled: true,
            active: true,
            host: Some("127.0.0.1".to_owned()),
            port: Some(22),
            alias: Some("other".to_owned()),
            host_key_fingerprint: Some("SHA256:host".to_owned()),
            client_key_fingerprint: Some("SHA256:client".to_owned()),
        },
    ] {
        let mut sandbox = status(true, true);
        sandbox.ssh = Some(malformed);
        assert!(
            ssh_invocation(
                &sandbox,
                Path::new("/Users/test/.config/gascan/ssh/config"),
                Vec::<OsString>::new(),
            )
            .is_err()
        );
    }
}

#[test]
fn process_execution_inherits_discrete_arguments_and_propagates_exit_code() -> TestResult {
    let temp = tempfile::tempdir()?;
    let capture = temp.path().join("arguments");
    let helper = executable(
        temp.path(),
        "capture",
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$GASCAN_CAPTURE\"\nexit 37\n",
    )?;
    let arguments = vec![
        OsString::from("one argument"),
        OsString::from("$(must-not-run)"),
        OsString::from("semi;colon"),
    ];
    let code = wait_for_ssh(
        &helper,
        &arguments,
        [(
            OsString::from("GASCAN_CAPTURE"),
            capture.as_os_str().to_owned(),
        )],
    )?;

    assert_eq!(code, 37);
    assert_eq!(
        fs::read(&capture)?,
        b"one argument\n$(must-not-run)\nsemi;colon\n"
    );
    Ok(())
}

#[test]
fn process_execution_propagates_signal_status() -> TestResult {
    let temp = tempfile::tempdir()?;
    let helper = executable(temp.path(), "signal", "#!/bin/sh\nkill -TERM $$\n")?;
    let code = wait_for_ssh(&helper, &[], std::iter::empty::<(OsString, OsString)>())?;
    assert_eq!(code, 143);
    Ok(())
}

#[test]
fn clap_accepts_ssh_remote_arguments_after_double_dash_as_os_strings() -> TestResult {
    let parsed = gascan::cli::ssh_arguments_from([
        OsString::from("gascan"),
        OsString::from("--sandbox"),
        OsString::from("code-123"),
        OsString::from("ssh"),
        OsString::from("--"),
        OsString::from("printf"),
        OsString::from("%s"),
        OsString::from("one argument"),
    ])?;
    assert_eq!(parsed.sandbox.as_deref(), Some("code-123"));
    assert_eq!(
        parsed.remote,
        [
            OsStr::new("printf"),
            OsStr::new("%s"),
            OsStr::new("one argument")
        ]
    );
    Ok(())
}

fn executable(directory: &Path, name: &str, contents: &str) -> TestResult<std::path::PathBuf> {
    let path = directory.join(name);
    fs::write(&path, contents)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    Ok(path)
}

#[test]
fn non_utf8_remote_argument_fixture_is_genuinely_non_utf8() {
    assert!(OsStr::from_bytes(b"\xff").to_str().is_none());
}
