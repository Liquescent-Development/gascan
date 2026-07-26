use gascan_proto::{
    ssh_status::{SshState, classify},
    v1,
};

fn sandbox(actual: v1::ActualState, ssh: Option<v1::SshStatus>) -> v1::SandboxStatus {
    v1::SandboxStatus {
        sandbox_id: "code-123".to_owned(),
        actual_state: actual as i32,
        ssh,
        ..Default::default()
    }
}

fn inactive(enabled: bool) -> v1::SshStatus {
    v1::SshStatus {
        enabled,
        active: false,
        host: None,
        port: None,
        alias: None,
        host_key_fingerprint: None,
        client_key_fingerprint: None,
    }
}

fn ready() -> v1::SshStatus {
    v1::SshStatus {
        enabled: true,
        active: true,
        host: Some("127.0.0.1".to_owned()),
        port: Some(22222),
        alias: Some("gascan-code-123".to_owned()),
        host_key_fingerprint: Some("SHA256:host".to_owned()),
        client_key_fingerprint: Some("SHA256:client".to_owned()),
    }
}

#[test]
fn classification_accepts_only_explicit_disabled_and_complete_ready_states() {
    assert_eq!(
        classify(&sandbox(v1::ActualState::Running, Some(inactive(false)))),
        SshState::Disabled
    );
    assert_eq!(
        classify(&sandbox(v1::ActualState::Pending, Some(inactive(true)))),
        SshState::Starting
    );
    assert_eq!(
        classify(&sandbox(v1::ActualState::Running, Some(ready()))),
        SshState::Ready
    );
    assert_eq!(
        classify(&sandbox(v1::ActualState::Failed, Some(inactive(true)))),
        SshState::Unhealthy
    );
    assert_eq!(
        classify(&sandbox(v1::ActualState::Running, None)),
        SshState::Unavailable
    );
}

#[test]
fn contradictory_or_incomplete_wire_state_is_never_disabled_or_ready() {
    let mut contaminated_disabled = inactive(false);
    contaminated_disabled.alias = Some("gascan-code-123".to_owned());
    assert_eq!(
        classify(&sandbox(
            v1::ActualState::Running,
            Some(contaminated_disabled)
        )),
        SshState::Unhealthy
    );

    for mutation in [
        {
            let mut value = ready();
            value.host = Some("0.0.0.0".to_owned());
            value
        },
        {
            let mut value = ready();
            value.port = Some(0);
            value
        },
        {
            let mut value = ready();
            value.alias = Some("gascan-other".to_owned());
            value
        },
        {
            let mut value = ready();
            value.host_key_fingerprint = None;
            value
        },
        {
            let mut value = ready();
            value.client_key_fingerprint = Some(String::new());
            value
        },
    ] {
        assert_eq!(
            classify(&sandbox(v1::ActualState::Running, Some(mutation))),
            SshState::Unhealthy
        );
    }
}
