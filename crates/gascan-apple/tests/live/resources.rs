use std::{
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpListener, TcpStream},
    time::Duration,
};

use serde_json::json;

use super::common::{LiveContext, TestError, configured_resource, guest_argv, publish_args};

#[test]
fn reads_requested_limits_from_the_exact_apple_configuration_path() {
    let inspect = json!([{
        "configuration": {"resources": {"cpus": 1, "memoryInBytes": 268435456}},
        "status": {"cpuOverhead": 1, "resources": {"cpus": 2}}
    }]);
    assert_eq!(configured_resource(&inspect, "cpus"), Some(1));
    assert_eq!(
        configured_resource(&inspect, "memoryInBytes"),
        Some(268_435_456)
    );
}

#[test]
fn command_construction_rejects_non_loopback_publish() {
    assert!(publish_args(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 18080, 8080).is_err());
    assert!(publish_args(IpAddr::V6(Ipv6Addr::LOCALHOST), 18080, 8080).is_err());
    assert_eq!(
        publish_args(IpAddr::V4(Ipv4Addr::LOCALHOST), 18080, 8080).unwrap(),
        ["--publish", "127.0.0.1:18080:8080"]
    );
}

#[test]
fn published_guest_uses_persistent_netcat_listener_argv() {
    assert_eq!(
        guest_argv(true),
        [
            "docker.io/library/alpine:3.20",
            "sh",
            "-c",
            "while :; do printf 'HTTP/1.1 200 OK\\r\\nContent-Length: 2\\r\\nConnection: close\\r\\n\\r\\nok' | nc -l -p 8080; done",
        ]
    );
}

#[tokio::test]
#[ignore = "requires Apple silicon macOS 26+ with container service"]
async fn cpu_and_memory_limits_are_observable_in_guest() -> Result<(), TestError> {
    let ctx = LiveContext::new("resources").await?;
    let inspect = ctx.inspect().await?;
    assert_eq!(configured_resource(&inspect, "cpus"), Some(1));
    assert_eq!(
        configured_resource(&inspect, "memoryInBytes"),
        Some(268_435_456)
    );

    let cpu_max = String::from_utf8(ctx.exec("cat /sys/fs/cgroup/cpu.max").await?.stdout)?;
    let values: Vec<u64> = cpu_max
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    assert_eq!(values.len(), 2, "unexpected cpu.max: {cpu_max:?}");
    assert_eq!(
        values[0], values[1],
        "cpu.max does not encode one CPU: {cpu_max:?}"
    );

    let memory_max = String::from_utf8(ctx.exec("cat /sys/fs/cgroup/memory.max").await?.stdout)?;
    assert_eq!(memory_max.trim().parse::<u64>()?, 268_435_456);
    ctx.cleanup().await
}

#[tokio::test]
#[ignore = "requires Apple silicon macOS 26+ with container service"]
async fn published_port_is_reachable_only_through_loopback_binding() -> Result<(), TestError> {
    let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = reservation.local_addr()?.port();
    drop(reservation);
    let ctx = LiveContext::new_published("port", port).await?;
    let mut response = None;
    for _ in 0..50 {
        if !ctx.is_running().await? {
            return Err(format!(
                "published guest exited before readiness; {}",
                ctx.logs().await?
            )
            .into());
        }
        if let Ok(mut stream) = TcpStream::connect_timeout(
            &(Ipv4Addr::LOCALHOST, port).into(),
            Duration::from_millis(200),
        ) {
            stream.set_read_timeout(Some(Duration::from_millis(500)))?;
            if stream
                .write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n")
                .is_ok()
            {
                let mut received = Vec::new();
                loop {
                    let mut bytes = [0_u8; 1024];
                    match stream.read(&mut bytes) {
                        Ok(0) | Err(_) => break,
                        Ok(length) => {
                            received.extend_from_slice(&bytes[..length]);
                            if received.ends_with(b"ok") {
                                break;
                            }
                        }
                    }
                }
                let body = String::from_utf8_lossy(&received).into_owned();
                if body.contains("200 OK") && body.ends_with("ok") {
                    response = Some(body);
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(response.is_some(), "netcat responder did not become ready");
    ctx.cleanup().await
}
