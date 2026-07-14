#[path = "../../../../tests/fixtures/network/host-server.rs"]
mod host_server;

use super::common::{LiveContext, TestError, container_record};

fn has_no_network_attachments(value: &serde_json::Value) -> bool {
    container_record(value)
        .and_then(|record| record.get("configuration"))
        .and_then(|configuration| configuration.get("networks"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(Vec::is_empty)
}

#[test]
fn exact_none_form_has_empty_structured_attachments() {
    let value = serde_json::json!([{"configuration":{"networks":[]}}]);
    assert!(has_no_network_attachments(&value));
    assert!(!has_no_network_attachments(
        &serde_json::json!([{"configuration":{"networks":[{"network":"default"}]}}])
    ));
}

#[tokio::test]
#[ignore = "requires supported Apple runtime and adversarial network probes"]
async fn offline_workspace_cannot_reach_external_or_host_networks() -> Result<(), TestError> {
    let host = host_server::HostServer::start()?;
    let targets = [
        "https://example.com".to_owned(),
        "http://1.1.1.1".to_owned(),
        "http://192.0.2.1".to_owned(),
        host.guest_url(),
    ];
    let control = LiveContext::new("network-control").await?;
    for target in [&targets[0], &targets[1], &targets[3]] {
        assert!(
            control.can_reach(target).await?,
            "networked positive control could not reach: {target}"
        );
    }
    control.cleanup().await?;

    let ctx = LiveContext::offline("network").await?;
    assert!(has_no_network_attachments(&ctx.inspect().await?));
    for target in &targets {
        assert!(
            !ctx.can_reach(target).await?,
            "offline target unexpectedly reachable: {target}"
        );
    }
    ctx.exec("test -d /workspace && ip link show lo >/dev/null")
        .await?;
    ctx.exec("ip link add gascan0 type dummy 2>/dev/null || true; ip route add default via 192.0.2.1 2>/dev/null || true").await?;
    for target in &targets {
        assert!(
            !ctx.can_reach(target).await?,
            "guest-root mutation made target reachable: {target}"
        );
    }
    ctx.cleanup().await
}
