use std::{
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpListener, TcpStream},
    time::Duration,
};

use super::common::{LiveContext, TestError, publish_args};

#[test]
fn command_construction_rejects_non_loopback_publish() {
    assert!(publish_args(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 18080, 8080).is_err());
    assert!(publish_args(IpAddr::V6(Ipv6Addr::LOCALHOST), 18080, 8080).is_err());
    assert_eq!(
        publish_args(IpAddr::V4(Ipv4Addr::LOCALHOST), 18080, 8080).unwrap(),
        ["--publish", "127.0.0.1:18080:8080"]
    );
}

#[tokio::test]
#[ignore = "requires Apple silicon macOS 26+ with container service"]
async fn cpu_and_memory_limits_are_observable_in_guest() -> Result<(), TestError> {
    let ctx = LiveContext::new("resources").await?;
    assert_eq!(
        String::from_utf8(ctx.exec("getconf _NPROCESSORS_ONLN").await?.stdout)?.trim(),
        "1"
    );
    let memory: u64 = String::from_utf8(
        ctx.exec("awk '/MemTotal/ { print $2 * 1024 }' /proc/meminfo")
            .await?
            .stdout,
    )?
    .trim()
    .parse()?;
    assert!(
        memory <= 300_000_000,
        "guest memory exceeded requested limit: {memory}"
    );
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
        if let Ok(mut stream) = TcpStream::connect_timeout(
            &(Ipv4Addr::LOCALHOST, port).into(),
            Duration::from_millis(200),
        ) {
            stream.write_all(b"GET / HTTP/1.0\r\n\r\n")?;
            let mut body = String::new();
            stream.read_to_string(&mut body)?;
            response = Some(body);
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(response.is_some_and(|body| body.ends_with("gascan")));
    ctx.cleanup().await
}
