use camino::{Utf8Path, Utf8PathBuf};
use gascan_arca::ChannelTransport;
use std::collections::BTreeMap;
use std::time::Duration;

/// Distinguishes the socket roots of engines started by the same process.
static SOCKET_SEQUENCE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Owns the socket root directory and removes it when dropped.
///
/// The socket root is outside the `TempDir`, so nothing else removes it. This
/// exists as a guard rather than as a `Drop` on `LiveEngine` so that the
/// directory has an owner from the moment it is created: `start()` can still
/// panic on the `sun_path` assert or on a failed spawn, and those unwind
/// through this rather than leaving an orphan under `/tmp`.
struct SocketRoot(Utf8PathBuf);

impl Drop for SocketRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Reads a required path from the environment, or panics saying how to get one.
///
/// Absence is a panic and never a skip, for the reason
/// `GASCAN_ARCA_ENGINE_BIN` was given one: a live test that silently skips is a
/// live test nobody notices has stopped running.
///
/// It deliberately does NOT check that the path exists. The engine validates
/// its own three inputs and refuses to start naming which one is missing and
/// the path it tried (design §2.3), and a second copy of that check here would
/// be a guard no test can measure -- delete either copy and the other still
/// catches it. What this owes the reader is the variable's absence, which the
/// engine cannot report because it never runs.
fn required_path(variable: &str, what: &str, directive: &str) -> String {
    std::env::var(variable).unwrap_or_else(|_| panic!("{variable} must name {what}; {directive}"))
}

/// The OCI layout every live test derives its images from.
///
/// Same shape as the other three, for the reason `required_path` records. The
/// tier never fetches: `arca-engine` refuses to, and a test that reached the
/// network would fail for reasons that have nothing to do with the engine.
pub fn base_oci_layout() -> Utf8PathBuf {
    Utf8PathBuf::from(required_path(
        "GASCAN_ARCA_BASE_OCI_LAYOUT",
        "an OCI layout holding one small linux/arm64 image with a shell and nc",
        "build one with 'skopeo copy --override-os linux --override-arch arm64 \
         docker://docker.io/library/alpine:3.20 oci:/tmp/alpine-oci:alpine:3.20'",
    ))
}

/// A `/bin/sh` program that runs its arguments and kills them when this
/// process stops holding the other end of stdin.
///
/// **This exists because `kill_on_drop(true)` does not survive the parent being
/// killed rather than dropped.** An `arca-engine` was found on this machine
/// still running four days after the live run that spawned it, orphaned to PID
/// 1 (recorded in `docs/status/START-HERE.md`). Nothing in-process can run
/// after `SIGKILL`, so the guarantee has to live in a process that outlives
/// this one and watches for its death; the pipe is the watch, because the
/// kernel closes it however the holder dies.
///
/// `exec 3<&0` and the watcher's `<&3` are load-bearing, and MEASURED to be. A
/// background command in a non-interactive shell has its stdin reassigned to
/// `/dev/null` before any explicit redirection (POSIX XCU 2.9.3), so a watcher
/// without `<&3` reads EOF at once and kills the child before the pipe is ever
/// closed. Dropping just the `<&3` fails
/// `supervision::a_supervised_child_dies_when_its_parent_stops_holding_the_pipe`
/// on `the supervisor started no child within 10s` -- `pgrep` never sees the
/// child at all, because it is already dead.
///
/// `wait "$enginepid"` rather than `exec`, so the wrapper exits with the
/// engine's own status: `await_socket`'s `try_wait()` fast-fail depends on
/// seeing it, and it is what reported `Missing expected argument
/// '--kernel-path'` as `exit status: 64` in 0.06s. MEASURED through this
/// wrapper by pointing `GASCAN_ARCA_ENGINE_BIN` at a script that exits 64:
/// `engine exited with exit status: 64 before accepting a connection on
/// /tmp/gascan-arca-live-23172-0/engine.sock`, in 0.26s.
///
/// `SIGTERM` and no escalation, because escalation would need a `sleep` this
/// wrapper cannot reliably reap. MEASURED: `arca-engine` exits **0.00s** after
/// `SIGTERM`, with status 1.
///
/// **What this does NOT guarantee**, and each of these leaks an engine exactly
/// as before: the wrapper itself being `SIGKILL`ed, a `kill -9` delivered to
/// the whole process group, or a machine that loses power. It also cannot
/// help in the window between the wrapper starting and the watcher being
/// backgrounded -- microseconds, and long before `await_socket` returns.
const SUPERVISOR: &str = r#"
exec 3<&0
engine=$1
shift
"$engine" "$@" &
enginepid=$!
{ while read -r _; do :; done; kill -TERM "$enginepid" 2>/dev/null; } <&3 &
watcher=$!
wait "$enginepid"
status=$?
kill -TERM "$watcher" 2>/dev/null
exit "$status"
"#;

/// `program` with `arguments`, under [`SUPERVISOR`], with the pipe held here.
///
/// `stdin` is piped and never written to. Closing it -- deliberately, or by
/// this process dying -- is the whole signal.
pub fn supervised(program: &str, arguments: &[&str]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(SUPERVISOR)
        // `$0`. It is what `ps` shows for the wrapper, so it says what the
        // process is rather than leaving a bare `sh` beside the engine.
        .arg("gascan-live-supervisor")
        .arg(program)
        .args(arguments)
        .stdin(std::process::Stdio::piped())
        // Belt for the ordinary case: a dropped `Child` kills the wrapper here
        // and now, rather than waiting for the watcher to notice the pipe.
        .kill_on_drop(true);
    command
}

/// An engine process on a temporary socket, killed when the test ends.
///
/// The live tier drives the engine directly rather than through `gascand`.
/// It kills streams, resets mid-exec, and kills the engine under an open
/// call, and a supervisor whose job is to react to exactly those events
/// would be fighting the tests. Supervision is exercised by `gascan-e2e`.
pub struct LiveEngine {
    child: tokio::process::Child,
    socket: Utf8PathBuf,
    /// Every image the engine's private store holds, by the tag it was loaded
    /// under, mapped to the digest the STORE recorded rather than the one the
    /// layout carried. See [`LiveEngine::image`].
    images: BTreeMap<String, String>,
    _socket_root: SocketRoot,
    _root: tempfile::TempDir,
}

impl LiveEngine {
    /// Starts the engine named by `GASCAN_ARCA_ENGINE_BIN`.
    ///
    /// Panics with a directive message when the variable is absent, because a
    /// live test that silently skips is a live test nobody notices has stopped
    /// running.
    pub async fn start() -> Self {
        Self::start_with_images(&[]).await
    }

    /// The same engine, with each named OCI layout loaded into its store first.
    ///
    /// The engine's state root is created fresh per test, so every engine
    /// starts with an empty image store and a `Create` against it would be
    /// refused as `not_found`. `arca-engine image load` binds no socket, starts
    /// no VM and needs no kernel, so the store is seeded by running the same
    /// binary to completion **before** the server is spawned.
    pub async fn start_with_images(layouts: &[&Utf8Path]) -> Self {
        let binary = required_path(
            "GASCAN_ARCA_ENGINE_BIN",
            "a built arca-engine",
            "run scripts/build-arca-engine.sh and use its second output line",
        );
        let kernel = required_path(
            "GASCAN_ARCA_KERNEL_PATH",
            "the vmlinux the engine boots guests with",
            "an installed Arca.app carries one at \
             Contents/Resources/vmlinux; ~/.arca/vmlinux symlinks it",
        );
        let vminit = required_path(
            "GASCAN_ARCA_VMINIT_LAYOUT",
            "an OCI layout holding arca-vminit:latest",
            "an installed Arca.app populates ~/.arca/vminit",
        );
        let root = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(root.path()).unwrap().to_owned();
        let state = path.join("state");
        std::fs::create_dir_all(&state).unwrap();

        for layout in layouts {
            load_image(&binary, &state, layout).await;
        }
        let images = stored_images(&state);

        // The socket does NOT live under the temp dir, and that is deliberate.
        // `sun_path` is capped at 103 bytes (swift-nio asserts it explicitly in
        // NIOCore/SocketAddresses.swift), and macOS temp dirs are
        // /var/folders/<...>/T/<...> -- a measured path came to 74 bytes, which
        // fits but leaves little room. Arca's own tests hit this exact wall
        // during Task 7 and had to move to /tmp. Build the socket path under a
        // short root and assert the length rather than meeting the cap as a
        // mystery bind failure.
        let socket_root = Utf8PathBuf::from(format!(
            "/tmp/gascan-arca-live-{}-{}",
            std::process::id(),
            SOCKET_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        // `create_dir`, not `create_dir_all`: this must fail if the directory
        // already exists. An interrupted run leaves one behind, and a recycled
        // pid would otherwise adopt it -- along with a stale `engine.sock` that
        // makes the bind fail. Say which of those happened.
        std::fs::create_dir(&socket_root)
            .unwrap_or_else(|error| panic!("could not create socket root {socket_root}: {error}"));
        let socket_root = SocketRoot(socket_root);
        let socket = socket_root.0.join("engine.sock");
        assert!(
            socket.as_str().len() <= 103,
            "socket path is {} bytes, over sun_path's 103-byte cap: {socket}",
            socket.as_str().len()
        );

        // All four options, none defaulted. The engine made `--kernel-path` and
        // `--vminit-layout` required when it took ownership of its own state
        // root, and a tier passing only the first two spawns nothing: MEASURED
        // against the branch binary as `Missing expected argument
        // '--kernel-path'`, exit 64. Every live test here was `#[ignore]`d, so
        // nothing ran them and nothing noticed -- a tier that cannot start its
        // subject and a tier nobody runs look identical from outside.
        let child = supervised(
            &binary,
            &[
                "--socket-path",
                socket.as_str(),
                "--state-root",
                state.as_str(),
                "--kernel-path",
                &kernel,
                "--vminit-layout",
                &vminit,
            ],
        )
        .spawn()
        .unwrap_or_else(|error| panic!("could not spawn {binary}: {error}"));

        let mut engine = Self {
            child,
            socket,
            images,
            _socket_root: socket_root,
            _root: root,
        };
        engine.await_socket().await;
        engine
    }

    /// The immutable reference naming what the store holds under `tag`.
    ///
    /// **THE DIGEST A REQUEST MUST NAME IS THE STORE'S, NOT THE LAYOUT'S.** The
    /// store re-wraps what it ingests: a layout whose `index.json` carries
    /// manifest `sha256:45e09956…` is recorded in
    /// `<state-root>/images/state.json` as an image *index* under
    /// `sha256:a019d0ba…`. A test that derived the digest from the layout it
    /// loaded would name content the engine does not hold, and hear
    /// `not_found` from a store that has the image.
    pub fn image(&self, tag: &str) -> String {
        let digest = self.images.get(tag).unwrap_or_else(|| {
            panic!(
                "the engine's store holds no image tagged {tag}; it holds {:?}",
                self.images.keys().collect::<Vec<_>>()
            )
        });
        format!("{}@{digest}", repository_of(tag))
    }

    /// Waits for the socket to appear, then for a connection to succeed.
    ///
    /// Both halves are needed: the file appears before the listener accepts,
    /// so waiting only for the file races the bind. Bounded, because a hang
    /// here is a failure to report rather than a condition to wait out.
    ///
    /// An engine that died at startup is a different fact from an engine that
    /// is slow, so this checks the child every pass and says which one happened
    /// rather than letting a dead engine spend the whole bound telling the slow
    /// story.
    ///
    /// The bound is 120s and not 30s because a binary's first execution is far
    /// slower than its later ones: a freshly built `arca-engine` measured 997ms
    /// on a fresh inode against 10ms warm, and freshly linked test binaries on
    /// the same machine took ~50s each to start under load. 30s failed on a
    /// cold engine. Widening a liveness wait weakens no claim this tier makes,
    /// and a false failure on a cold CI box costs more than a late true one.
    async fn await_socket(&mut self) {
        let bound = Duration::from_secs(120);
        let started = std::time::Instant::now();
        loop {
            if self.socket.exists()
                && ChannelTransport::connect(self.socket.as_std_path().to_owned())
                    .await
                    .is_ok()
            {
                return;
            }
            match self.child.try_wait() {
                Ok(Some(status)) => panic!(
                    "engine exited with {status} before accepting a connection on {}",
                    self.socket
                ),
                Ok(None) => {}
                Err(error) => {
                    panic!("could not check on the engine for {}: {error}", self.socket)
                }
            }
            assert!(
                started.elapsed() < bound,
                "engine did not accept a connection on {} within {:.1}s",
                self.socket,
                started.elapsed().as_secs_f64()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn transport(&self) -> ChannelTransport {
        ChannelTransport::connect(self.socket.as_std_path().to_owned())
            .await
            .expect("connecting to a started engine must succeed")
    }

    /// Stops the engine and waits for it to be gone.
    ///
    /// **Closing stdin rather than killing the child, and the difference
    /// matters.** The child is [`SUPERVISOR`], not the engine: killing it would
    /// leave the engine running until the watcher noticed the pipe close, which
    /// is exactly the race `a_call_against_a_killed_engine_fails_rather_than_hanging`
    /// must not have. Closing the pipe is the one signal the wrapper is built
    /// around, and `wait` then returns only once the engine itself is reaped.
    pub async fn kill(mut self) {
        drop(self.child.stdin.take());
        let stopped = tokio::time::timeout(Duration::from_secs(30), self.child.wait()).await;
        match stopped {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => panic!("could not wait on the engine supervisor: {error}"),
            // The wrapper is still up 30s after the pipe closed, so its watcher
            // did not run or the engine ignored `SIGTERM`. Say so rather than
            // hang; `kill_on_drop` cannot report it and would look like a pass.
            Err(_) => panic!(
                "the engine supervisor for {} did not exit within 30s of its pipe closing",
                self.socket
            ),
        }
    }
}

/// Seeds one OCI layout into an engine state root, before any engine serves it.
///
/// Failure is a panic carrying the subcommand's own output: a test whose store
/// is empty fails later as a `not_found` from `Create`, which reads as an
/// engine defect and is not one.
async fn load_image(binary: &str, state: &Utf8Path, layout: &Utf8Path) {
    let output = tokio::process::Command::new(binary)
        .arg("image")
        .arg("load")
        .arg("--state-root")
        .arg(state.as_str())
        .arg("--oci-layout")
        .arg(layout.as_str())
        .output()
        .await
        .unwrap_or_else(|error| panic!("could not run {binary} image load: {error}"));
    assert!(
        output.status.success(),
        "{binary} image load --state-root {state} --oci-layout {layout} exited with {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Every tag the engine's own image store records, mapped to its digest.
///
/// Read from the store rather than from the layout, for the reason
/// [`LiveEngine::image`] records. An absent file is an empty store, which is
/// what an engine started with no layouts has.
fn stored_images(state: &Utf8Path) -> BTreeMap<String, String> {
    let path = state.join("images").join("state.json");
    let Ok(source) = std::fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    let parsed: BTreeMap<String, serde_json::Value> = serde_json::from_str(&source)
        .unwrap_or_else(|error| panic!("could not parse the engine's image store {path}: {error}"));
    parsed
        .into_iter()
        .map(|(tag, descriptor)| {
            let digest = descriptor
                .get("digest")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("{path} records {tag} with no digest: {descriptor}"))
                .to_owned();
            (tag, digest)
        })
        .collect()
}

/// The repository half of a reference, split the way both sides of the wire do.
///
/// The rule is `immutable_image_identity`'s
/// (`crates/gascan-core/src/runtime.rs`), mirrored by Arca's
/// `ImageIdentity.repository(of:)`: drop anything from `@sha256:` onward, then
/// drop a tag -- the last `:` that comes *after* the last `/`, so the port in
/// `registry.example:5000/repo` is not mistaken for one. `heldImageReferences`
/// compares the request's repository against the store's, so a split that
/// disagreed with Arca's would be refused as `not_found` for content the
/// engine holds.
fn repository_of(reference: &str) -> &str {
    let reference = reference.split_once("@sha256:").map_or(reference, |a| a.0);
    match reference.rfind(':') {
        Some(colon) if !reference[colon..].contains('/') => &reference[..colon],
        _ => reference,
    }
}

/// A loopback port nothing else on this host is listening on.
///
/// Reserved by binding `127.0.0.1:0` and dropping the listener, which is the
/// technique `crates/gascan-apple/tests/live/resources.rs` uses. There is a
/// race with anything else that binds an ephemeral port in the meantime, and
/// it is the same race that tier accepts: the alternative is a fixed port that
/// collides with whatever is already listening, every run, on purpose.
pub fn reserved_loopback_port() -> u16 {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("binding an ephemeral loopback port must succeed");
    listener
        .local_addr()
        .expect("a bound listener has a local address")
        .port()
}

/// A policy-validated `CreateRequest`, which is the only kind that exists.
///
/// `CreateRequest`'s fields are `pub(crate)` to `gascan-core` and it derives no
/// `Deserialize`, so `PolicyCompiler` is the only construction path -- there is
/// deliberately no fixture constructor. This mirrors `policy_request` in
/// `tests/backend_unary.rs`, which solves the same problem the same way against
/// the fake transport. The two cannot share code: each `tests/*.rs` is its own
/// crate, and this one is reachable only from the live tier.
///
/// The `TempDir` must outlive the request: the compiled request names its
/// canonical root.
pub fn policy_request(name: &str) -> (tempfile::TempDir, gascan_core::runtime::CreateRequest) {
    policy_request_for_image(name, gascan_core::policy::PolicyCompiler::workspace_image())
}

/// The same request, against an image the engine's own store actually holds.
///
/// `PolicyCompiler::compile` pins the approved workspace image, which no engine
/// under test has: the live tier seeds a store with `arca-engine image load` and
/// must then ask for what it seeded. `compile_for_image` is the existing seam
/// for exactly this and needs no widening.
pub fn policy_request_for_image(
    name: &str,
    image: &str,
) -> (tempfile::TempDir, gascan_core::runtime::CreateRequest) {
    policy_request_from_manifest(name, image, "version = 1\nnetwork = 'networked'\n")
}

/// The same request again, over a manifest the caller wrote.
///
/// The manifest is the *only* knob, deliberately. Ports and the guest user are
/// manifest facts and nothing else in this tier may set them: `compile_ports`
/// is what decides that a declared port becomes `127.0.0.1:<port>:<port>` with
/// no mapping, and a test that reached around it would be asserting against a
/// request gascan itself cannot produce.
pub fn policy_request_from_manifest(
    name: &str,
    image: &str,
    manifest: &str,
) -> (tempfile::TempDir, gascan_core::runtime::CreateRequest) {
    use gascan_core::manifest::Manifest;
    use gascan_core::policy::PolicyCompiler;
    use gascan_core::runtime::{NetworkIsolation, RuntimeCapabilities, RuntimeVersion};
    use gascan_core::sandbox::SandboxSpec;

    let root = tempfile::tempdir().expect("a temporary project root");
    let path = Utf8Path::from_path(root.path()).expect("a utf-8 temporary path");
    std::fs::write(path.join("gascan.toml"), manifest).expect("a manifest");
    let spec = SandboxSpec::from_root(name, path, Manifest::load(path).expect("a manifest"))
        .expect("a spec");
    // Every flag true, which is the opposite of what the engine reports. The
    // compiler gates on what the runtime CLAIMS it can do, and this request only
    // has to be well-formed enough to send: what is under test is the engine's
    // refusal, and a request the compiler rejected would never reach it.
    let capabilities = RuntimeCapabilities {
        version: RuntimeVersion::new(1, 1, 0),
        bind_mounts: true,
        named_volumes: true,
        tty: true,
        signals: true,
        loopback_publish: true,
        resource_limits: true,
        offline: NetworkIsolation::Proven,
    };
    let request =
        PolicyCompiler::compile_for_image(spec, &capabilities, image).expect("a validated request");
    (root, request)
}

/// The exact retained set for `request`, derived from it rather than hardcoded.
///
/// `RetainedResources::new` requires an exact match against the request's
/// topology, and the manifest decides how many volumes and networks that is --
/// so a fixed list is a test that breaks when the manifest changes for reasons
/// unrelated to what it is testing. Same shape as `retained_for` in
/// `tests/backend_unary.rs`, for the same reason, and separate for the same
/// reason: each `tests/*.rs` is its own crate.
pub fn retained_for(
    request: &gascan_core::runtime::CreateRequest,
) -> Vec<gascan_core::runtime::RuntimeResource> {
    use gascan_core::runtime::{
        ResourceIdentity, ResourceKind, ResourceOwnership, RuntimeResource,
    };

    let mut retained: Vec<RuntimeResource> = request
        .volumes()
        .iter()
        .map(|volume| {
            RuntimeResource::discovered(
                ResourceIdentity::new(ResourceKind::Volume, volume.name.clone())
                    .expect("a policy-compiled volume name is valid"),
                Some(request.id().clone()),
                ResourceOwnership::GasCanOwned,
            )
        })
        .collect();
    if let Some(name) = request.network().managed_name() {
        retained.push(RuntimeResource::discovered(
            ResourceIdentity::new(ResourceKind::Network, name.to_owned())
                .expect("a policy-compiled network name is valid"),
            Some(request.id().clone()),
            ResourceOwnership::GasCanOwned,
        ));
    }
    retained
}

/// A one-image OCI layout that runs `command`, written beside a base layout.
///
/// **`CreateRequest` carries no argv, so this is the only way the tier can
/// decide what a sandbox runs.** `engine.proto`'s `CreateRequest` has no
/// command and no entrypoint field, and `SandboxEngineService` passes
/// `entrypoint: nil, command: nil` deliberately -- the image's own config
/// decides. The environment is no way in either: `policy.rs` sets it from
/// `guest_environment()`, a fixed map with no manifest passthrough. So
/// `gascan-apple`'s `guest_argv` technique does not transfer at all, and the
/// published-port test's responder has to be baked into an image. The port it
/// listens on is therefore known only at image-build time, which is why the
/// image is built during the test rather than prepared by a maintainer.
///
/// This is not an image builder. It reuses the base layout's layers verbatim
/// and writes three small blobs: a config with a new `Cmd`, a manifest naming
/// that config, and an index naming that manifest under `tag`. The rootfs is
/// untouched, so the `diff_ids` still describe it.
///
/// The base layout's `index.json` must name exactly one manifest. Anything
/// else would make "which image is this derived from" a choice this function
/// would have to guess at.
pub fn layout_running(
    base: &Utf8Path,
    destination: &Utf8Path,
    tag: &str,
    command: &[&str],
) -> Utf8PathBuf {
    use serde_json::{Value, json};

    copy_tree(base, destination);

    let index: Value = read_json(&destination.join("index.json"));
    let manifests = index["manifests"]
        .as_array()
        .unwrap_or_else(|| panic!("{base}/index.json has no manifests array"));
    assert_eq!(
        manifests.len(),
        1,
        "{base}/index.json must name exactly one manifest; it names {}",
        manifests.len()
    );
    let mut manifest: Value = read_json(&blob_path(destination, digest_of(&manifests[0])));
    let mut config: Value = read_json(&blob_path(destination, digest_of(&manifest["config"])));

    // `Entrypoint` is cleared as well as `Cmd` being set. A base image that
    // carried one would prepend it to the command below, and the responder
    // would run as arguments to something else.
    config["config"]["Cmd"] = json!(command);
    config["config"]["Entrypoint"] = Value::Null;
    let config_blob = write_blob(destination, &config);
    manifest["config"]["digest"] = json!(config_blob.0);
    manifest["config"]["size"] = json!(config_blob.1);
    let manifest_blob = write_blob(destination, &manifest);

    std::fs::write(
        destination.join("index.json"),
        serde_json::to_vec(&json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.index.v1+json",
            "manifests": [{
                "mediaType": manifest["mediaType"],
                "digest": manifest_blob.0,
                "size": manifest_blob.1,
                "annotations": { "org.opencontainers.image.ref.name": tag },
            }],
        }))
        .expect("an index serialises"),
    )
    .unwrap_or_else(|error| panic!("could not write {destination}/index.json: {error}"));
    destination.to_owned()
}

fn digest_of(descriptor: &serde_json::Value) -> &str {
    descriptor["digest"]
        .as_str()
        .unwrap_or_else(|| panic!("an OCI descriptor with no digest: {descriptor}"))
}

fn blob_path(layout: &Utf8Path, digest: &str) -> Utf8PathBuf {
    let (algorithm, hex) = digest
        .split_once(':')
        .unwrap_or_else(|| panic!("{digest} is not an OCI digest"));
    layout.join("blobs").join(algorithm).join(hex)
}

fn read_json(path: &Utf8Path) -> serde_json::Value {
    let source =
        std::fs::read(path).unwrap_or_else(|error| panic!("could not read {path}: {error}"));
    serde_json::from_slice(&source)
        .unwrap_or_else(|error| panic!("could not parse {path} as json: {error}"))
}

/// Writes `value` as a content-addressed blob, returning its digest and size.
///
/// The bytes that are hashed are the bytes that are written -- one
/// serialisation, used for both -- because a digest taken over a second
/// rendering would name content the layout does not contain, and the engine
/// verifies blobs it loads.
fn write_blob(layout: &Utf8Path, value: &serde_json::Value) -> (String, usize) {
    use sha2::{Digest, Sha256};

    let bytes = serde_json::to_vec(value).expect("a blob serialises");
    let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
    let path = blob_path(layout, &digest);
    std::fs::write(&path, &bytes).unwrap_or_else(|error| panic!("could not write {path}: {error}"));
    (digest, bytes.len())
}

fn copy_tree(from: &Utf8Path, to: &Utf8Path) {
    std::fs::create_dir_all(to).unwrap_or_else(|error| panic!("could not create {to}: {error}"));
    let entries = std::fs::read_dir(from)
        .unwrap_or_else(|error| panic!("could not read the base layout {from}: {error}"));
    for entry in entries {
        let entry = entry.expect("a directory entry");
        let name = entry.file_name();
        let name = name.to_str().expect("a utf-8 layout entry name");
        let source = from.join(name);
        let target = to.join(name);
        if entry.file_type().expect("a file type").is_dir() {
            copy_tree(&source, &target);
        } else {
            std::fs::copy(&source, &target)
                .unwrap_or_else(|error| panic!("could not copy {source} to {target}: {error}"));
        }
    }
}
