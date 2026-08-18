//! The daemon-on-engine harness: a real `gascand` on a real `arca-engine`.
//!
//! **What separates this from `crates/gascan-arca/tests/live/`.** That tier
//! drives `ArcaBackend` directly, over a transport it built, against an engine
//! it spawned itself with all four options in hand. This one drives the product:
//! the `gascan` CLI talks to a `gascand` that resolved its own backend from the
//! environment, dialled its own engine socket, and -- on a miss -- built the
//! engine's command line itself. Nothing here passes the engine an argument.
//!
//! That difference is the point. `TokioEngineSpawner` passed only
//! `--socket-path` until this harness existed, and MEASURED against the pinned
//! engine that exits **64** on `Missing expected argument '--state-root'`. Task
//! 11's whole suite stayed green through it, because every spawner there is a
//! fixture that never runs an engine.
//!
//! **The kernel and the vminit layout are deliberately NOT set here.** The
//! daemon resolves those from `ArtifactPaths::for_user()` -- what `gascan engine
//! fetch` installed -- and a harness that supplied them would be testing its own
//! environment instead of the product's resolution. A host that has not fetched
//! them fails with the engine's own message naming the path it tried.

#![allow(dead_code, reason = "each test binary uses a different part of this")]

use camino::{Utf8Path, Utf8PathBuf};
use gascan_core::policy::{CACHE_ROOT, CONFIG_ROOT, TOOLS_ROOT};
use gascan_core::sandbox::SandboxId;
use serde_json::Value;
use std::ffi::{OsStr, OsString};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// The tag the derived workspace layout is loaded under.
const TAG: &str = "gascan-arca-e2e:latest";

/// The two account records the guest needs, each on its own unwrapped line.
const PASSWD: &[u8] =
    b"root:x:0:0:root:/root:/bin/sh\nworkspace:x:1000:1000:workspace:/home/workspace:/bin/sh\n";
const GROUP: &[u8] = b"root:x:0:\nworkspace:x:1000:\n";

/// **The guest-side contract `gascan up` provisions against, stood in for.**
///
/// `provision_with_applied` is unconditional at `service.rs:1569`, and its first
/// step execs `/usr/bin/sudo -n /usr/bin/install -d -o workspace -g workspace`
/// inside the guest. It goes on to `/usr/local/bin/initialize-rust-home`,
/// `configure-workstation-home`, `configure-shell-home` and `select-gascamp`.
/// **Those five programs and the `workspace` account are the workspace image's
/// contract, and design §10 puts how a workspace image reaches an engine out of
/// this milestone's scope** -- it is P5.4's. MEASURED against a stock alpine
/// with none of them: `gascan up` reaches `Start`, the container runs, the
/// network attaches and port 22 publishes, and the operation then fails
/// `provisioning failed: guest provisioning transport failed`.
///
/// **These are stubs and this tier claims nothing about them.** What they make
/// reachable is everything on the Gas Can side of the boundary: the policy the
/// daemon compiled, `Create`, `Start`, every provisioning `Exec` as a real RPC
/// against a real guest, `Logs`, `reconcile`, `Stop` and `Remove`. A real
/// workspace image would exercise the same control plane and additionally
/// prove the image; nothing here should be read as having proven it.
///
/// `/etc/passwd` and `/etc/group` are written whole rather than appended to,
/// because a layer entry replaces the file it names and this fixture has no
/// reader for the base image's own. Nothing in this tier needs alpine's other
/// accounts: the sandbox runs as root and `install -o workspace` needs only the
/// account it names.
fn workspace_contract_entries() -> Vec<gascan_oci_fixture::LayerEntry<'static>> {
    use gascan_oci_fixture::LayerEntry;
    vec![
        // One line per record and no wrapping. A `\\`-continued byte literal
        // reads as two lines in the source and is one string with the
        // indentation folded in; MEASURED, that put fourteen spaces before
        // `workspace` and the guest answered `install: unknown user workspace`.
        LayerEntry::file("/etc/passwd", PASSWD),
        LayerEntry::file("/etc/group", GROUP),
        // The real sudo authenticates; this one only has to be transparent.
        // `-n` is dropped because it is sudo's "never prompt" flag and every
        // provisioning call passes it.
        LayerEntry::program(
            "/usr/bin/sudo",
            b"#!/bin/sh\nwhile [ \"$1\" = \"-n\" ]; do shift; done\nexec \"$@\"\n",
        ),
        LayerEntry::program(
            "/usr/local/bin/initialize-rust-home",
            b"#!/bin/sh\nexit 0\n",
        ),
        LayerEntry::program(
            "/usr/local/bin/configure-workstation-home",
            b"#!/bin/sh\nexit 0\n",
        ),
        LayerEntry::program(
            "/usr/local/bin/configure-shell-home",
            b"#!/bin/sh\nexit 0\n",
        ),
        // `verify_gascamp` parses this and requires a JSON object; anything
        // else is `invalid Gascamp verification output`.
        LayerEntry::program("/usr/local/bin/select-gascamp", b"#!/bin/sh\necho '{}'\n"),
        // The three managed-volume targets. A mount whose target does not exist
        // in the image is silently not mounted, so without these a missing
        // mount and a missing directory are the same observation.
        LayerEntry::directory(TOOLS_ROOT),
        LayerEntry::directory(CACHE_ROOT),
        LayerEntry::directory(CONFIG_ROOT),
    ]
}

/// The marker PID 1 writes to each stream before it settles.
///
/// `Logs` reads what the container itself produced, so a sandbox that printed
/// nothing makes an empty log indistinguishable from a broken one. Both streams
/// because they are separate channels on the wire and a `Logs` that dropped one
/// would read as a quiet guest.
pub const STARTUP_MARKER: &str = "GASCAN_ARCA_E2E_PID1";

/// What every sandbox in this tier runs as PID 1.
///
/// `CreateRequest` carries no argv and the engine passes `entrypoint: nil,
/// command: nil`, so the image's own `Cmd` is the only way to decide. A stock
/// alpine's is `/bin/sh`, which exits at once with no tty attached -- and a
/// sandbox whose PID 1 has gone cannot be exec'd into, which is most of what
/// this tier does.
fn stay_up() -> [String; 3] {
    [
        "sh".to_owned(),
        "-c".to_owned(),
        format!(
            "echo {STARTUP_MARKER}-stdout; echo {STARTUP_MARKER}-stderr >&2; while :; do sleep 1; done"
        ),
    ]
}

/// `/private/tmp` and not `std::env::temp_dir()`.
///
/// **A unix socket path has 104 bytes and the engine refuses a longer one**
/// rather than truncating it: MEASURED, an engine given a socket under this
/// session's scratch directory initialised every manager and then died on
/// `Error: unixDomainSocketPathTooLong`, having bound nothing. macOS's
/// `TMPDIR` is a per-session `/var/folders/...` path some fifty bytes deep, so
/// the default would spend half the budget before the test names anything.
/// `apple_common` chose the same root for its own reasons.
fn session_root() -> std::path::PathBuf {
    std::path::PathBuf::from("/private/tmp")
}

/// Reads a required path from the environment, or panics saying how to get one.
///
/// Absence is a panic and never a skip, for the reason the `gascan-arca` live
/// tier records: a live test that silently skips is a live test nobody notices
/// has stopped running.
fn required(variable: &str, what: &str, directive: &str) -> String {
    std::env::var(variable).unwrap_or_else(|_| panic!("{variable} must name {what}; {directive}"))
}

pub struct ArcaE2e {
    gascan: OsString,
    gascand: OsString,
    engine_binary: String,
    root: Option<tempfile::TempDir>,
    runtime: Option<tempfile::TempDir>,
    root_path: std::path::PathBuf,
    runtime_root: std::path::PathBuf,
    account_home: std::path::PathBuf,
    engine_socket: std::path::PathBuf,
    engine_state: Utf8PathBuf,
    image: String,
    id: SandboxId,
    owner_token: String,
    /// Kept so its temporary directory outlives every sandbox derived from it.
    _images: tempfile::TempDir,
}

impl ArcaE2e {
    /// A project, an engine state root seeded with one image, and no daemon yet.
    ///
    /// The image is loaded **before** anything starts an engine. `arca-engine
    /// image load` binds no socket and needs no kernel, and an engine's store is
    /// empty on a fresh state root -- a `Create` against one is refused
    /// `not_found`, which reads as an engine defect and is not one.
    pub fn new(name: &str, network: &str) -> TestResult<Self> {
        let gascan = std::env::var_os("CARGO_BIN_EXE_gascan-e2e-cli")
            .ok_or("workspace-built gascan binary is unavailable")?;
        let gascand = std::env::var_os("CARGO_BIN_EXE_gascan-e2e-daemon")
            .ok_or("workspace-built gascand binary is unavailable")?;
        let engine_binary = required(
            "GASCAN_ARCA_ENGINE_BIN",
            "a built arca-engine",
            "run scripts/build-arca-engine.sh and use its second output line",
        );
        let base = Utf8PathBuf::from(required(
            "GASCAN_ARCA_BASE_OCI_LAYOUT",
            "an OCI layout holding one small linux/arm64 image with a shell",
            "build one with 'skopeo copy --override-os linux --override-arch arm64 \
             docker://docker.io/library/alpine:3.20 oci:/tmp/alpine-oci:alpine:3.20'",
        ));

        let session = session_root();
        let root = tempfile::Builder::new()
            .prefix("gc-arca-p-")
            .tempdir_in(&session)?;
        let runtime = tempfile::Builder::new()
            .prefix("gc-arca-r-")
            .tempdir_in(&session)?;
        for path in [root.path(), runtime.path()] {
            std::fs::set_permissions(
                path,
                <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
            )?;
        }
        let root_path = root.path().canonicalize()?;
        let runtime_root = runtime.path().canonicalize()?;
        let account_home = runtime_root.join("home");
        std::fs::create_dir(&account_home)?;

        // `user = "root"` because the base layout is a stock alpine with no
        // `workspace` account: the engine hands `UserMode::Workspace` to
        // `createContainer` as the literal string `workspace`, so a start would
        // fail on the image rather than on anything this tier tests. It is an
        // ordinary manifest choice and not a test-only escape hatch.
        std::fs::write(
            root_path.join("gascan.toml"),
            format!(
                "version = 1\nname = {}\nnetwork = {}\nuser = \"root\"\n\
                 [ssh]\nenabled = false\n",
                serde_json::to_string(name)?,
                serde_json::to_string(network)?,
            ),
        )?;
        let utf8_root = Utf8Path::from_path(&root_path).ok_or("non-UTF-8 test root")?;
        let id = SandboxId::from_root(name, utf8_root);

        let images = tempfile::Builder::new()
            .prefix("gc-arca-i-")
            .tempdir_in(&session)?;
        let layout = gascan_oci_fixture::layout_running_with_entries(
            &base,
            Utf8Path::from_path(images.path()).ok_or("non-UTF-8 image root")?,
            TAG,
            &stay_up().each_ref().map(String::as_str),
            &workspace_contract_entries(),
        );

        let engine_state = Utf8Path::from_path(&runtime_root)
            .ok_or("non-UTF-8 runtime root")?
            .join("engine");
        std::fs::create_dir_all(&engine_state)?;
        gascan_oci_fixture::load_image(&engine_binary, &engine_state, &layout);
        let image = gascan_oci_fixture::stored_image_reference(
            &gascan_oci_fixture::stored_images(&engine_state),
            TAG,
        );

        Ok(Self {
            gascan,
            gascand,
            engine_binary,
            root: Some(root),
            runtime: Some(runtime),
            root_path,
            runtime_root: runtime_root.clone(),
            account_home,
            // `e.sock` and not `engine.sock`: see `session_root`, every byte of
            // the 104 counts.
            engine_socket: runtime_root.join("e.sock"),
            engine_state,
            image,
            id,
            owner_token: format!(
                "arca-e2e-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            ),
            _images: images,
        })
    }

    pub fn root(&self) -> &OsStr {
        self.root_path.as_os_str()
    }

    pub fn id(&self) -> &str {
        self.id.as_str()
    }

    pub fn image(&self) -> &str {
        &self.image
    }

    pub fn engine_socket(&self) -> &std::path::Path {
        &self.engine_socket
    }

    fn state_path(&self) -> std::path::PathBuf {
        self.runtime_root.join("state.sqlite3")
    }

    /// The CLI, with the whole environment a daemon-on-engine run needs.
    ///
    /// `TokioDaemonSpawner` adds variables to the child's environment and never
    /// clears it, so everything set here reaches the daemon the CLI starts --
    /// which is how the daemon resolves `BackendSelection::Arca` at all.
    pub fn command<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new(&self.gascan);
        command
            .args(args)
            .env("XDG_RUNTIME_DIR", &self.runtime_root)
            .env("GASCAN_STATE_PATH", self.state_path())
            .env("GASCAN_PID_PATH", self.runtime_root.join("daemon.pid"))
            .env(
                "GASCAN_DAEMON_INSTANCE_PATH",
                self.runtime_root.join("daemon-instance.json"),
            )
            .env("GASCAN_DAEMON_OWNER_TOKEN", &self.owner_token)
            .env(
                "GASCAN_DAEMON_STDERR_PATH",
                self.runtime_root.join("daemon.stderr"),
            )
            .env("GASCAN_DAEMON", &self.gascand)
            .env("GASCAN_E2E_ACCOUNT_HOME", &self.account_home)
            .env("GASCAN_E2E_CANDIDATE_IMAGE", &self.image)
            .env(gascand::ARCA_BACKEND_ENV, "1")
            .env(gascand::ENGINE_BIN_ENV, &self.engine_binary)
            .env(gascand::ENGINE_SOCKET_ENV, &self.engine_socket)
            .env(gascand::ENGINE_STATE_ROOT_ENV, self.engine_state.as_str())
            .env_remove("GASCAN_TEST_FAKE_BACKEND");
        command
    }

    pub fn invoke<I, S>(&self, args: I) -> TestResult<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Ok(self.command(args).output()?)
    }

    /// Runs the CLI and requires success, reporting both sides on a failure.
    ///
    /// The daemon's stderr as well as the CLI's, because a failure that started
    /// in the engine arrives at the CLI as a one-line refusal and the reason is
    /// two processes away.
    pub fn success<I, S>(&self, args: I) -> TestResult<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.invoke(args)?;
        if output.status.success() {
            return Ok(output);
        }
        Err(format!(
            "gascan failed with {:?}: stdout={} stderr={} daemon_stderr={} engine_alive={:?}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            self.bounded_daemon_stderr(),
            self.engine_pid(),
        )
        .into())
    }

    pub fn bounded_daemon_stderr(&self) -> String {
        use std::io::{Read as _, Seek as _};
        const MAXIMUM: u64 = 32 * 1_024;
        let result = (|| -> std::io::Result<String> {
            let mut file = std::fs::File::open(self.runtime_root.join("daemon.stderr"))?;
            let length = file.metadata()?.len();
            let omitted = length.saturating_sub(MAXIMUM);
            if omitted > 0 {
                file.seek(std::io::SeekFrom::Start(omitted))?;
            }
            let mut bytes = Vec::new();
            file.take(MAXIMUM).read_to_end(&mut bytes)?;
            Ok(format!(
                "{}{}",
                if omitted > 0 {
                    format!("<{omitted} bytes omitted>\n")
                } else {
                    String::new()
                },
                String::from_utf8_lossy(&bytes)
            ))
        })();
        result.unwrap_or_else(|error| format!("<unavailable: {error}>"))
    }

    pub fn status_json(&self) -> TestResult<Value> {
        let output = self.success(["--sandbox", self.id(), "status", "--json"])?;
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    /// The pid of the engine holding this harness's socket, if one holds it.
    ///
    /// **Read from `<socket>.lock`, which the engine writes its own pid into
    /// after taking the `flock`** (`Sources/ArcaEngine/EngineServer.swift:551`).
    /// Arca calls reading it "racy and diagnostic only", and it is -- for the
    /// refusal path that comment is about, where the holder may be anyone. Here
    /// the socket path is a fresh temporary directory this process created, so
    /// the only engine that can have taken that lock is the one this harness
    /// caused to be spawned.
    ///
    /// **The alternative is unusable on this host.** `ps -A` enumerates 31
    /// processes against `launchctl list`'s 544 and omits even the calling
    /// shell, so no `ps | grep` can find an engine, and a `pkill -f arca-engine`
    /// would reach every engine on the machine including ones this test did not
    /// start.
    pub fn engine_pid(&self) -> Option<u32> {
        let lock = self.engine_socket.with_extension("sock.lock");
        std::fs::read_to_string(lock)
            .ok()?
            .trim()
            .parse::<u32>()
            .ok()
    }

    /// The pid recorded in the daemon pid file.
    pub fn daemon_pid(&self) -> Option<u32> {
        std::fs::read_to_string(self.runtime_root.join("daemon.pid"))
            .ok()?
            .trim()
            .parse::<u32>()
            .ok()
    }

    /// Is the pid in the daemon pid file a live process?
    pub fn daemon_alive(&self) -> Option<bool> {
        self.daemon_pid().map(alive)
    }

    /// Is the pid in the engine's lock file a live process?
    ///
    /// **Separate from [`Self::engine_pid`] on purpose.** The lock file outlives
    /// the engine that wrote it, so a pid read from it is a record of who took
    /// the lock and not evidence that anyone still holds it.
    pub fn engine_alive(&self) -> Option<bool> {
        self.engine_pid().map(alive)
    }

    /// Waits for `predicate` to hold, polling, and fails rather than hanging.
    pub fn until(
        &self,
        what: &str,
        bound: Duration,
        mut predicate: impl FnMut() -> TestResult<bool>,
    ) -> TestResult {
        let started = Instant::now();
        loop {
            if predicate()? {
                return Ok(());
            }
            if started.elapsed() >= bound {
                return Err(format!(
                    "{what} did not hold within {bound:?}; daemon_stderr={}",
                    self.bounded_daemon_stderr()
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Stops the daemon without stopping its engine.
    ///
    /// **That asymmetry is the property Task 11 rests on** and the reason this
    /// helper exists separately from [`Self::stop_engine`]: a `SpawnedEngine`
    /// dropped by a dying daemon does not reap its child, so the engine -- and
    /// every sandbox it is running -- outlives the restart.
    pub fn kill_daemon(&self) -> TestResult {
        let pid = self
            .daemon_pid()
            .ok_or("the daemon wrote no pid file, so there is nothing to stop")?;
        // The pid this harness's own daemon wrote, and never a pattern: a
        // `pkill -f gascan` on this machine would take out every daemon a
        // developer has running, including ones outside this test.
        signal("-KILL", pid);
        self.until("the daemon exits", Duration::from_secs(30), || {
            Ok(!alive(pid))
        })
    }

    /// Stops the engine this harness caused to be spawned, by its own pid.
    pub fn stop_engine(&self) -> TestResult {
        let Some(pid) = self.engine_pid() else {
            return Ok(());
        };
        signal("-TERM", pid);
        let stopped = self.until("the engine exits", Duration::from_secs(30), || {
            Ok(!alive(pid))
        });
        if stopped.is_err() {
            signal("-KILL", pid);
        }
        stopped
    }
}

/// Does this pid name a live process?
///
/// **`stderr` is discarded because this is a question, not an action.**
/// `/bin/kill -0` writes `kill: <pid>: No such process` for the answer "no",
/// and a polling loop that let that through printed it once per iteration into
/// the middle of an otherwise passing run.
fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Sends one signal, ignoring a refusal.
///
/// A process that has already gone is the outcome every caller here wants, and
/// `kill` reports it as a failure; the loops that follow each call are what
/// decide whether it happened.
fn signal(name: &str, pid: u32) {
    let _ = Command::new("kill")
        .args([name, &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status();
}

/// Every process this harness caused to exist is stopped, in dependency order.
///
/// The daemon first, so it cannot reconcile against an engine that is going
/// away and report a fault on the way out; then the engine, which nothing else
/// will ever stop -- it is deliberately not reaped by the daemon that spawned
/// it, and an engine leaked here holds a vmnet interface and a state root until
/// the machine reboots. An `arca-engine` was once found still running four days
/// after the run that spawned it.
impl Drop for ArcaE2e {
    fn drop(&mut self) {
        if let Err(error) = self.kill_daemon() {
            eprintln!("arca e2e daemon cleanup: {error}");
        }
        if let Err(error) = self.stop_engine() {
            eprintln!("arca e2e engine cleanup: {error}");
        }
        drop(self.root.take());
        drop(self.runtime.take());
    }
}
