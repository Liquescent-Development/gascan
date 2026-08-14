use crate::common::{
    LiveEngine, base_oci_layout, layout_running, policy_request_from_manifest,
    reserved_loopback_port,
};
use camino::Utf8Path;
use gascan_arca::ArcaBackend;
use gascan_core::runtime::{ContainerState, RemoveRequest, RuntimeBackend};
use std::io::Read as _;
use std::time::Duration;

/// A published port must be reachable from this process, and nothing weaker
/// counts.
///
/// **THIS IS THE LANDING'S LOAD-BEARING TEST, and Task 14 may not flip
/// `loopback_publish` until it exists and passes.** Port publishing has three
/// silent gates: `portMapManager` may be unset, `getWireGuardClient` returns
/// nil for a container on no WireGuard network and the `if let` around the
/// publish has **no `else`**, and the `catch` swallows by design ("Don't fail
/// container start on port mapping errors") while the container is still marked
/// running. An engine can report a successful `Create`, a successful `Start`,
/// and an `Inspect` naming the port, having published nothing, with every check
/// green.
///
/// **So `Inspect` cannot serve as evidence.** It reports what the STORE holds,
/// including bindings that were never published, because that is what drift
/// detection compares against. The only instrument that sees past all three
/// gates is a TCP connection from the test process that reads bytes the guest
/// produced, and that is what this is.
///
/// **The port is baked into the image, and it has to be.** `CreateRequest`
/// carries no argv -- `engine.proto` has no command or entrypoint field and
/// `SandboxEngineService` passes `entrypoint: nil, command: nil` deliberately,
/// so the image's own config decides what runs -- and the environment is no way
/// in either, since `policy.rs` sets it from `guest_environment()`, a fixed map
/// with no manifest passthrough. `gascan-apple`'s `guest_argv` technique does
/// not transfer at all. The tier therefore writes its own OCI layout, and
/// because the responder's port is a fact about the image, the image is built
/// during the test.
///
/// **One number in two places, and `compile_ports` is why.** It forces
/// `host_address = 127.0.0.1` and `host_port == guest_port ==` the declared
/// manifest value; there is no mapping to exploit. So the reserved port goes
/// into `gascan.toml`'s `[ports]` *and* into the image's `Cmd`, and they are
/// the same variable here for the same reason.
///
/// **SEEN TO FAIL, twice, and the two failures are different sentences.**
///
/// - Responder moved to `port + 1`, everything else unchanged: FAILED after
///   180s with `last attempt: connected and read nothing`. The host proxy was
///   up and accepting -- so **a proxy with nothing behind it does not pass this
///   test**, which is the whole reason `Inspect` cannot substitute for it.
/// - `translate.rs`'s `ports: port_mappings(request.ports())?` replaced by an
///   empty vec, so the engine is asked to publish nothing: FAILED after 180s
///   with `last attempt: Connection refused (os error 61)`, nothing listening
///   at all. The only other test in the crate that noticed was
///   `backend_unary::create_sends_every_field_of_the_compiled_request`, which
///   sees the wire and not the host.
///
/// **What this does NOT prove.** That the binding is loopback-only -- nothing
/// here dials a non-loopback address, and `gascan-apple`'s
/// `published_port_is_reachable_only_through_loopback_binding` is the tier that
/// makes that claim. It also proves nothing about a second port, about UDP, or
/// about republishing across a restart. And of the three silent gates it is
/// arranged against, it demonstrates only that all three were passed *on this
/// path*: it does not isolate which one would have failed.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel, a vminit \
            layout and a base OCI layout; the pinned engine implements none of these RPCs"]
async fn a_published_port_is_reachable_from_the_test_process() {
    let port = reserved_loopback_port();
    let token = format!("gascan-live-{port}");
    let images = tempfile::tempdir().expect("a temporary layout root");
    // `while :;` and not a single accept: the connection below is the first
    // this responder ever serves, but a retry after a refused connect must
    // find it still listening rather than gone.
    let responder = format!("while :; do echo {token} | nc -l -p {port}; done");
    let layout = layout_running(
        &base_oci_layout(),
        Utf8Path::from_path(images.path()).expect("a utf-8 path"),
        "gascan-live-ports:latest",
        &["sh", "-c", &responder],
    );

    let engine = LiveEngine::start_with_images(&[&layout]).await;
    let backend = ArcaBackend::new(engine.transport().await);

    // `user = 'root'` for the reason `lifecycle.rs` records: a stock alpine has
    // no `workspace` user. The port is declared here and nowhere else -- the
    // manifest is the only way this tier can ask for one.
    let manifest = format!(
        "version = 1\nnetwork = 'networked'\nuser = 'root'\n\n[ports]\nresponder = {port}\n"
    );
    let (_root, request) = policy_request_from_manifest(
        "ports",
        &engine.image("gascan-live-ports:latest"),
        &manifest,
    );
    assert_eq!(
        request.ports().len(),
        1,
        "the manifest declares one port and the compiler must carry exactly it"
    );
    assert_eq!(request.ports()[0].host_port, port);
    assert_eq!(
        request.ports()[0].guest_port,
        port,
        "compile_ports forces the guest port equal to the host port; a responder \
         listening anywhere else would fail this test for the wrong reason"
    );

    let created = backend
        .create(request.clone())
        .await
        .expect("create with a port mapping must succeed");
    backend
        .start(request.id())
        .await
        .expect("start must boot the sandbox");

    let answer = read_from_loopback(port, Duration::from_secs(180)).await;
    assert_eq!(
        answer.trim(),
        token,
        "the bytes on 127.0.0.1:{port} must be the ones this image's own Cmd produces"
    );

    backend.stop(request.id()).await.expect("stop must answer");
    // Stopped rather than removed-then-asserted: the point already landed
    // above, and a running container cannot be removed.
    let mut settled = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while std::time::Instant::now() < deadline {
        let seen = backend
            .inspect(request.id())
            .await
            .expect("inspect answers");
        if seen.map(|sandbox| sandbox.state) == Some(ContainerState::Stopped) {
            settled = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(settled, "the sandbox must stop so that it can be removed");
    backend
        .remove(
            RemoveRequest::from_resources(created.created().to_vec())
                .expect("gascan-owned resources"),
        )
        .await
        .expect("remove must delete the sandbox");

    engine.kill().await;
}

/// Connects to `127.0.0.1:<port>` until something answers, and returns what it
/// said.
///
/// Retried rather than attempted once: `Start` returns before the guest's own
/// PID 1 has run the image's `Cmd`, so the first connects are refused by a host
/// proxy with nothing behind it yet. The bound is what makes this a test and
/// not a wait -- a publish that never happens fails here, naming the port.
async fn read_from_loopback(port: u16, bound: Duration) -> String {
    let deadline = std::time::Instant::now() + bound;
    let mut last = String::from("never attempted");
    while std::time::Instant::now() < deadline {
        let attempt = tokio::task::spawn_blocking(move || {
            let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
            let mut stream =
                std::net::TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
            stream.set_read_timeout(Some(Duration::from_secs(10)))?;
            let mut answer = String::new();
            stream.read_to_string(&mut answer)?;
            Ok::<String, std::io::Error>(answer)
        })
        .await
        .expect("the blocking connect task must not panic");
        match attempt {
            Ok(answer) if !answer.trim().is_empty() => return answer,
            Ok(_) => last = "connected and read nothing".to_owned(),
            Err(error) => last = error.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!(
        "nothing answered on 127.0.0.1:{port} within {:.1}s; last attempt: {last}. \
         If this reads `connected and read nothing`, the engine publishing nothing \
         and an unrelated process having taken {port} between the reservation and \
         the Create are INDISTINGUISHABLE from here -- both accept and stay silent. \
         Check what is listening on {port} before treating this as a publish failure",
        bound.as_secs_f64()
    );
}
