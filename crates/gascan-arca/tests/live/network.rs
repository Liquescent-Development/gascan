//! **The offline proof: a sandbox with no network attachment has no egress.**
//!
//! This is the observation that licenses `CERTIFIED_ENGINE_REVISION`
//! (`translate.rs`). Until it existed, `NetworkIsolation::Proven` was a claim
//! with no instrument, which is the defect this project has written traps about
//! since milestone 1.
//!
//! **It lives here and not in `gascan-e2e`, and that is not a convenience.**
//! `PolicyCompiler::validate_capabilities` (`policy.rs:417-427`) refuses to
//! compile an offline sandbox unless `capabilities.offline` is already `Proven`
//! -- so `gascan up` on an offline manifest cannot run until the constant this
//! evidence licenses is set. Recording the evidence through the product would
//! mean setting the constant first, which is the order Task 15 exists to
//! forbid. This tier compiles against a stated capability set rather than the
//! engine's claim (`common::policy_request_from_manifest` says so in its own
//! comment), so the observation is of the ENGINE and owes nothing to Gas Can's
//! opinion of it.
//!
//! **The shape is `packaging/macos/release-smoke.sh:1015-1037`'s** -- a
//! test-owned host endpoint, a public IP and public DNS, each as the sandbox
//! user and again as guest root -- with two additions this tier can make and a
//! shell script cannot:
//!
//! - **A positive control on the same image and the same probes.** Six failures
//!   from a guest whose `nc` is broken read exactly like six failures from a
//!   guest with no network. The release smoke's own host endpoint is its
//!   positive control; this adds the public IP and DNS to that role, because
//!   all three are asserted negative offline and none of the three is evidence
//!   until it has been seen to succeed.
//! - **The guest-root mutation attempt**, which `gascan-apple`'s tier makes and
//!   the release smoke does not: root adds an interface and a default route,
//!   and every target is probed again afterwards. "No egress" that survives an
//!   adversary inside the guest is a different claim from "no egress by
//!   default".

use crate::common::{LiveEngine, await_state, base_oci_layout, policy_request_from_manifest};
use camino::{Utf8Path, Utf8PathBuf};
use gascan_arca::ArcaBackend;
use gascan_core::runtime::{
    ContainerState, CreateRequest, ExecOutput, ExecRequest, ExecSession, RuntimeBackend,
    RuntimeNetwork,
};
use gascan_oci_fixture::LayerEntry;
use std::collections::BTreeMap;
use std::io::Read as _;
use std::time::{Duration, Instant};

/// `user = 'root'` in both manifests, and the unprivileged half is reached with
/// `su workspace` rather than by running the sandbox as `workspace`.
///
/// **`ExecRequest` carries no user**, so an exec runs as whatever the container
/// runs as; there is no way to ask the engine for a root exec into a
/// non-root sandbox. Root-and-drop is the only ordering that reaches both
/// privilege levels through one sandbox, and it is the stronger one: the
/// mutation attempt below needs `CAP_NET_ADMIN`, which only the root half has.
const OFFLINE_MANIFEST: &str = "version = 1\nnetwork = 'offline'\nuser = 'root'\n";
const NETWORKED_MANIFEST: &str = "version = 1\nnetwork = 'networked'\nuser = 'root'\n";

/// The account the unprivileged half of every probe drops to.
///
/// A stock alpine has no `workspace` user, so the layout writes one. `/etc/passwd`
/// and `/etc/group` are written whole because a layer entry replaces the file it
/// names, and nothing here needs alpine's other accounts.
const PASSWD: &[u8] =
    b"root:x:0:0:root:/root:/bin/sh\nworkspace:x:1000:1000:workspace:/home/workspace:/bin/sh\n";
const GROUP: &[u8] = b"root:x:0:\nworkspace:x:1000:\n";

/// What every probe below is run under, so the two halves differ in exactly one
/// thing: the uid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum As {
    GuestRoot,
    SandboxUser,
}

impl As {
    fn wrap(self, script: &str) -> Vec<String> {
        match self {
            Self::GuestRoot => vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()],
            // `su` and not `setpriv` or `runuser`: busybox has `su`, and run by
            // root it needs no password.
            Self::SandboxUser => vec![
                "su".to_owned(),
                "workspace".to_owned(),
                "-c".to_owned(),
                script.to_owned(),
            ],
        }
    }

    const fn described(self) -> &'static str {
        match self {
            Self::GuestRoot => "guest root",
            Self::SandboxUser => "the sandbox user",
        }
    }
}

/// One way out of the guest, and the command that tries it.
///
/// Three mechanisms and not one, because they fail independently: a guest with
/// a route but no resolver reaches `1.1.1.1` and not `example.com`, and one on
/// an isolated host-only network reaches the host and neither of the others.
/// The release smoke asserts all three for the same reason.
#[derive(Clone, Debug)]
struct Target {
    mechanism: &'static str,
    /// A `/bin/sh` command that exits 0 exactly when the target is reached.
    probe: String,
}

fn targets(host_address: &str, host_port: u16) -> Vec<Target> {
    vec![
        Target {
            mechanism: "a test-owned host endpoint",
            // `nc -z` and not `wget`: the listener speaks no HTTP, and a
            // completed TCP handshake is the whole question.
            probe: format!("nc -w 3 -z {host_address} {host_port}"),
        },
        Target {
            mechanism: "a public IP",
            probe: "wget -T 5 -q -O /dev/null http://1.1.1.1/".to_owned(),
        },
        Target {
            mechanism: "public DNS",
            // `nslookup` and not `getent`: busybox's `getent hosts` consults
            // `/etc/hosts` as well, so a name that resolved from the file would
            // read as a resolver that worked.
            probe: "nslookup example.com".to_owned(),
        },
    ]
}

/// What guest root tries, in order, to give itself a way out.
///
/// Reported rather than asserted on. Whether the guest CAN add an interface is
/// the engine's business and may change; what this test asserts is that having
/// tried, the guest still reaches nothing. `gascan-apple`'s tier makes the same
/// distinction.
const MUTATION: &str = "ip link add gascan0 type dummy 2>&1; printf 'link-add-exit=%s\\n' $?; \
                        ip link set gascan0 up 2>&1; printf 'link-up-exit=%s\\n' $?; \
                        ip addr add 192.0.2.2/24 dev gascan0 2>&1; printf 'addr-add-exit=%s\\n' $?; \
                        ip route add default via 192.0.2.1 2>&1; printf 'route-add-exit=%s\\n' $?; \
                        printf 'post-mutation-links:\\n'; ip -o link show; \
                        printf 'post-mutation-routes:\\n'; ip route show";

/// A TCP listener on this host that accepts and immediately closes.
///
/// **The one target this test owns end to end.** `1.1.1.1` and `example.com`
/// depend on the machine having internet at all; this one does not, so a run
/// on a disconnected laptop still has one mechanism whose positive control is
/// meaningful.
struct HostEndpoint {
    address: String,
    port: u16,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HostEndpoint {
    fn start() -> Self {
        let listener =
            std::net::TcpListener::bind("0.0.0.0:0").expect("a loopback-and-LAN listener");
        let port = listener.local_addr().expect("a bound address").port();
        listener
            .set_nonblocking(true)
            .expect("a pollable listener, so the thread can be told to stop");
        let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = std::sync::Arc::clone(&shutdown);
        let thread = std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Read nothing and answer nothing. `nc -z` completes the
                        // handshake and closes; anything more would be a
                        // protocol this test does not need.
                        let mut sink = [0_u8; 1];
                        let _ = stream.read(&mut sink);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            // The address the guest must use, which is NOT loopback: the guest
            // is a virtual machine and `127.0.0.1` there is its own. Derived by
            // connecting a UDP socket, which sends nothing and reports which
            // local address the routing table would use to leave this host.
            address: host_address(),
            port,
            shutdown,
            thread: Some(thread),
        }
    }
}

impl Drop for HostEndpoint {
    fn drop(&mut self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn host_address() -> String {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").expect("an ephemeral UDP socket");
    // Connecting a UDP socket sends no packet; it only fixes the local address.
    socket
        .connect("1.1.1.1:80")
        .expect("a route to somewhere off this host");
    socket
        .local_addr()
        .expect("a local address")
        .ip()
        .to_string()
}

/// A booted sandbox and the exec surface the probes run through.
struct Sandbox {
    engine: LiveEngine,
    backend: ArcaBackend<gascan_arca::ChannelTransport>,
    request: CreateRequest,
    _project: tempfile::TempDir,
    _images: tempfile::TempDir,
}

impl Sandbox {
    async fn boot(name: &str, manifest: &str) -> Self {
        let tag = format!("gascan-live-network-{name}:latest");
        let images = tempfile::tempdir().expect("a temporary layout root");
        let layout = layout(
            Utf8Path::from_path(images.path()).expect("a utf-8 path"),
            &tag,
        );

        let engine = LiveEngine::start_with_images(&[&layout]).await;
        let backend = ArcaBackend::new(engine.transport().await);
        let (project, request) = policy_request_from_manifest(name, &engine.image(&tag), manifest);
        // **The confounder this closes.** Every observation below is about what
        // the engine did with `Network { mode: Offline }`, and the manifest is
        // three files away from the wire. A request that had silently compiled
        // to `Networked` would produce exactly the reachability this test would
        // then report as an engine defect.
        //
        // Matched by variant and not by value: `Networked` carries a
        // compiler-chosen name this test has no business predicting, and which
        // variant was chosen is the whole question.
        let compiled_offline = matches!(request.network(), RuntimeNetwork::Offline);
        assert_eq!(
            compiled_offline,
            manifest.contains("network = 'offline'"),
            "the compiled request does not carry the manifest's network mode: {:?} from {manifest:?}",
            request.network()
        );

        backend
            .prepare_image(request.image())
            .await
            .expect("the store holds the image the request names");
        backend
            .create(request.clone())
            .await
            .expect("create against a seeded store must succeed");
        backend
            .start(request.id())
            .await
            .expect("start must boot the sandbox");
        await_state(
            &backend,
            &request,
            ContainerState::Running,
            Duration::from_secs(180),
        )
        .await;

        Self {
            engine,
            backend,
            request,
            _project: project,
            _images: images,
        }
    }

    async fn run(&self, argv: Vec<String>) -> Completed {
        let opening = self.backend.exec(ExecRequest {
            id: self.request.id().clone(),
            argv: argv.clone(),
            stdin: Vec::new(),
            environment: BTreeMap::new(),
            tty: false,
        });
        let mut session = match tokio::time::timeout(Duration::from_secs(60), opening).await {
            Err(_) => panic!("Exec did not open a session for {argv:?} within 60s"),
            Ok(Err(error)) => panic!("Exec must open a session for {argv:?}: {error}"),
            Ok(Ok(session)) => session,
        };
        drain(&mut session, Duration::from_secs(120)).await
    }

    /// Whether `target` is reachable, run as `who`.
    ///
    /// **Every probe is bounded inside the guest as well as outside it.** A
    /// `wget` with no `-T` against a black hole waits for a TCP timeout the
    /// host cannot see, so the outer bound would fire and report a hung engine
    /// for what is actually the answer this test wanted.
    async fn reaches(&self, target: &Target, who: As) -> Completed {
        self.run(who.wrap(&target.probe)).await
    }

    async fn teardown(self) {
        self.backend
            .stop(self.request.id())
            .await
            .expect("stop must answer for a running sandbox");
        await_state(
            &self.backend,
            &self.request,
            ContainerState::Stopped,
            Duration::from_secs(120),
        )
        .await;
        self.engine.kill().await;
    }
}

fn layout(destination: &Utf8Path, tag: &str) -> Utf8PathBuf {
    gascan_oci_fixture::layout_running_with_entries(
        &base_oci_layout(),
        destination,
        tag,
        &["sh", "-c", "while :; do sleep 1; done"],
        &[
            LayerEntry::file("/etc/passwd", PASSWD),
            LayerEntry::file("/etc/group", GROUP),
        ],
    )
}

#[derive(Debug)]
struct Completed {
    stdout: String,
    stderr: String,
    code: i32,
}

impl Completed {
    const fn reached(&self) -> bool {
        self.code == 0
    }

    fn described(&self) -> String {
        format!(
            "exit {} stdout={:?} stderr={:?}",
            self.code, self.stdout, self.stderr
        )
    }
}

async fn drain(session: &mut ExecSession, bound: Duration) -> Completed {
    let started = Instant::now();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    loop {
        let remaining = bound.saturating_sub(started.elapsed());
        match tokio::time::timeout(remaining, session.next()).await {
            Err(_) => panic!(
                "no Exit frame within {:.1}s; stdout so far {:?} and stderr {:?}",
                bound.as_secs_f64(),
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            ),
            Ok(None) => panic!(
                "the exec stream ended with no Exit frame; stdout {:?}, stderr {:?}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            ),
            Ok(Some(Err(error))) => panic!("the engine refused the exec: {error}"),
            Ok(Some(Ok(ExecOutput::Stdout(bytes)))) => stdout.extend_from_slice(&bytes),
            Ok(Some(Ok(ExecOutput::Stderr(bytes)))) => stderr.extend_from_slice(&bytes),
            Ok(Some(Ok(ExecOutput::Exit { code, .. }))) => {
                return Completed {
                    stdout: String::from_utf8_lossy(&stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&stderr).into_owned(),
                    code,
                };
            }
        }
    }
}

/// **THE OFFLINE PROOF, AND IT FAILS TODAY. THAT IS NOT A BROKEN TEST.**
///
/// Run against the engine `engine/arca-pin.json` names, revision `c545612b`,
/// this reports **thirteen violations**: an `offline` sandbox carries a vmnet
/// `eth0` with a default route and a resolver, and reaches a test-owned host
/// endpoint, a public IP and public DNS -- as guest root and as the sandbox
/// user, before and after the mutation attempt. The run is recorded in
/// `docs/evidence/2026-08-18-arca-engine-offline.md`.
///
/// It asserts the PROPERTY and not the observation, deliberately. A test
/// written to assert what this engine does would pass today and fail on the
/// build that fixes it, which is the wrong way round for the one instrument
/// that decides whether `CERTIFIED_ENGINE_REVISION` may ever be set. **Do not
/// weaken it to make the tier green.** It turns green on an engine that
/// attaches nothing, and that engine is the one that earns the constant.
///
/// Six negatives, each with a positive control, plus a mutation attempt and the
/// interfaces the guest can see.
///
/// The two sandboxes are booted in sequence and not together: each costs a
/// virtual machine, and the networked one exists only to establish that the
/// probes work at all. Its assertions are therefore REQUIRED -- a positive
/// control that is allowed to fail is not a control.
///
/// **What this proves.** That an engine given `Network { mode: Offline }`
/// attaches nothing: from inside the running guest, no test-owned host
/// endpoint, no public IP and no public name is reachable, at either privilege
/// level, and none becomes reachable after guest root has added an interface
/// and a default route.
///
/// **What it does NOT prove.** Anything about egress policy, peer channels or
/// packet filtering -- P6's, and `Capabilities` fields 10-19 stay reserved. Nor
/// anything about a sandbox the PRODUCT created: `gascan up` on an offline
/// manifest is refused until the constant this licenses is set, and the
/// daemon-on-engine tier is where that becomes reachable.
#[tokio::test]
#[ignore = "FAILS BY DESIGN against the pinned engine -- see the doc comment and \
            docs/evidence/2026-08-18-arca-engine-offline.md; requires a built arca-engine named \
            by GASCAN_ARCA_ENGINE_BIN, a kernel, a vminit layout and a base OCI layout, and \
            boots two virtual machines"]
async fn an_offline_sandbox_has_no_egress_at_either_privilege_level() {
    let host = HostEndpoint::start();
    let targets = targets(&host.address, host.port);

    // The control. Every probe must SUCCEED here, or its failure below is not
    // evidence of isolation.
    let networked = Sandbox::boot("net-control", NETWORKED_MANIFEST).await;
    for target in &targets {
        for who in [As::GuestRoot, As::SandboxUser] {
            let outcome = networked.reaches(target, who).await;
            assert!(
                outcome.reached(),
                "the positive control failed: {} could not reach {} on a NETWORKED sandbox, so \
                 its failure on an offline one would prove nothing about isolation -- {}",
                who.described(),
                target.mechanism,
                outcome.described()
            );
        }
    }
    let control_interfaces = networked.run(As::GuestRoot.wrap("ip -o addr show")).await;
    eprintln!(
        "networked control interfaces:\n{}",
        control_interfaces.stdout
    );
    networked.teardown().await;

    let offline = Sandbox::boot("offline", OFFLINE_MANIFEST).await;

    // What the guest can see of its own topology, recorded rather than asserted
    // on: the assertions that matter are the reachability ones, and an
    // interface list is what a reader needs to believe them.
    let interfaces = offline.run(As::GuestRoot.wrap("ip -o addr show")).await;
    eprintln!("offline interfaces:\n{}", interfaces.stdout);
    let routes = offline.run(As::GuestRoot.wrap("ip route show")).await;
    eprintln!("offline routes:\n{}", routes.stdout);
    let resolver = offline
        .run(As::GuestRoot.wrap("cat /etc/resolv.conf 2>&1 || true"))
        .await;
    eprintln!("offline resolver:\n{}", resolver.stdout);

    // **Every observation is collected and asserted at the end**, rather than
    // each one aborting the run. Two virtual machines cost minutes, and a proof
    // that stops at its first violation makes a reader pay that again for every
    // further fact. What a refutation needs is the whole picture in one run.
    let mut violations: Vec<String> = Vec::new();

    // The structural half. `lo` is the loopback every network namespace has;
    // any other interface is an attachment, and offline means the absence of
    // one. Named rather than counted, so the message says WHICH.
    let attached =
        offline
            .run(As::GuestRoot.wrap(
                "ip -o link show | awk -F': ' '{print $2}' | cut -d@ -f1 | grep -vx lo || true",
            ))
            .await;
    let attached: Vec<&str> = attached.stdout.split_whitespace().collect();
    if !attached.is_empty() {
        violations.push(format!(
            "an offline sandbox carried non-loopback interfaces {attached:?}"
        ));
    }

    // The six, before any mutation.
    for target in &targets {
        for who in [As::GuestRoot, As::SandboxUser] {
            let outcome = offline.reaches(target, who).await;
            eprintln!(
                "offline probe: {} as {} -> reached={}",
                target.mechanism,
                who.described(),
                outcome.reached()
            );
            if outcome.reached() {
                violations.push(format!(
                    "an offline sandbox reached {} as {}: {}",
                    target.mechanism,
                    who.described(),
                    outcome.described()
                ));
            }
        }
    }

    // Guest root tries to give itself a way out, and then the six again.
    let mutation = offline.run(As::GuestRoot.wrap(MUTATION)).await;
    eprintln!("guest-root mutation attempt:\n{}", mutation.stdout);
    for target in &targets {
        for who in [As::GuestRoot, As::SandboxUser] {
            let outcome = offline.reaches(target, who).await;
            if outcome.reached() {
                violations.push(format!(
                    "after a guest-root mutation, an offline sandbox reached {} as {}: {}",
                    target.mechanism,
                    who.described(),
                    outcome.described()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "an offline sandbox is not isolated. {} violations:\n  {}\n\ninterfaces:\n{}\nroutes:\n{}\nresolver:\n{}",
        violations.len(),
        violations.join("\n  "),
        interfaces.stdout,
        routes.stdout,
        resolver.stdout
    );

    offline.teardown().await;
}
