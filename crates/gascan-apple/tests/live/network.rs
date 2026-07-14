#[path = "../../../../tests/fixtures/network/host-server.rs"]
mod host_server;

use super::common::{LiveContext, TestError, container_record};

fn ipv4_gateway(value: &serde_json::Value) -> Option<std::net::Ipv4Addr> {
    let records = value.as_array()?;
    if records.len() != 1 {
        return None;
    }
    records[0]
        .get("status")?
        .get("ipv4Gateway")?
        .as_str()?
        .parse()
        .ok()
}

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

#[test]
fn default_gateway_comes_from_exact_structured_network_status() {
    let value = serde_json::json!([{"configuration":{"id":"default"},"status":{"ipv4Gateway":"192.168.64.1"}}]);
    assert_eq!(ipv4_gateway(&value), Some("192.168.64.1".parse().unwrap()));
    assert!(ipv4_gateway(&serde_json::json!([{"status":{}}])).is_none());
    assert!(
        ipv4_gateway(&serde_json::json!([
            {"status":{"ipv4Gateway":"192.168.64.1"}},
            {"status":{"ipv4Gateway":"192.168.65.1"}}
        ]))
        .is_none()
    );
}

#[tokio::test]
#[ignore = "requires supported Apple runtime and adversarial network probes"]
async fn offline_workspace_cannot_reach_external_or_host_networks() -> Result<(), TestError> {
    let host = host_server::HostServer::start()?;
    let control = LiveContext::new("network-control").await?;
    let gateway = ipv4_gateway(&control.inspect_network("default").await?)
        .ok_or("default network inspect is missing a unique IPv4 gateway")?;
    let targets = [
        "https://example.com".to_owned(),
        "http://1.1.1.1".to_owned(),
        "http://192.0.2.1".to_owned(),
        host.url_for_host(gateway),
    ];
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
