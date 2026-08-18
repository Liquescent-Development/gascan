# Whole-landing review — P5.1 milestone 4, 2026-08-18

Four reviewers over the whole `main...feat/milestone-4-product-wiring` diff (56 files,
~7863 insertions), one per dimension: trust boundaries, cross-task wiring, the truth of
written claims, and release/packaging.

**All four went idle without ever delivering a reply, and all four files survived**, which is
why the dispatch required the file to be written before the reply. That is the second
milestone running in which the reply channel lost reviews and the file channel did not.

## What was acted on, and where

Six findings were fixed in `3882a52` and in the two task commits before it. Each has a test
that fails without it:

| Finding | Severity | Found by | Fixed in |
|---|---|---|---|
| `package.sh` emits an engine block `verify-package.sh` rejects; every package build fails | Critical | release | `3882a52` |
| `gascan engine fetch` leaves its directory 0755, which `gascand` and `uninstall.sh` both refuse | Major | boundary, release | `3882a52` |
| A doctor fallback emits Apple's remedies on the Arca path | Major | wiring, boundary | `3882a52` |
| Client readiness (15s) is under the daemon's engine readiness (20s), and both under a measured cold start | Major | wiring | `3882a52` |
| A comment says the revision gate is "still to come", in the file that defines it | Major | claims | `3882a52` |
| START-HERE is stale and prescribes an acceptance the landing refutes | Major | claims | this rewrite |

## What was NOT acted on, and why

Two Majors are design decisions rather than repairs, and are the first two open items in
`docs/status/START-HERE.md`:

- **The controller store is not backend-scoped**, and the `BackendMismatch` message steers the
  user into the gap. Needs a decision between a backend-scoped store path, a per-record
  backend column, and a `reconcile()` consumer that quarantines `MissingOwned`.
- **Every Arca startup failure goes to a null stderr**, and the doctor fact that names
  `gascan engine fetch` sits behind the daemon that cannot start without the artifacts.

The remaining Minors are unaddressed and are recorded verbatim below.

---



=====================================================================
# Reviewer: boundary
=====================================================================

# Milestone 4 landing — trust boundaries and security review

Branch `feat/milestone-4-product-wiring`, `git diff main...HEAD` (56 files).
Scope reviewed: `crates/gascan-arca/` (transport, channel, translate, backend),
`crates/gascand/src/engine.rs`, `crates/gascan-core/src/engine_artifacts.rs`,
`crates/gascan/src/cli.rs` `engine fetch`, `engine/arca-pin*.{json,jq}`,
`scripts/build-arca-engine.sh`, `scripts/sync-arca-proto.sh`,
`packaging/macos/uninstall.sh`, and the offline gate
(`policy.rs::validate_capabilities` / `translate.rs::certified_isolation` /
`api.rs::service_status`).

Known-and-decided items named in the brief (`CERTIFIED_ENGINE_REVISION = None`,
the by-design-failing `network.rs` live test, `Unsupported`/`Unverified`
pass-through, the `gascan-e2e` guest stubs) are excluded and are not reported
below.

**Critical: none found.** Nothing in this diff lets an unearned `Proven` reach
`validate_capabilities`, and no digest/signature gate I could find verifies over
bytes an attacker chooses. The `Proven` arm is reached from exactly one place
(`translate.rs:371`), and every sandbox creation funnels through
`policy.rs:186` → `validate_capabilities` (`policy.rs:400`) via
`compile_for_image_internal`; `service.rs:4496` is the only production caller.

---

## Major 1 — `engine fetch` creates `dev.gascan/` at 0755, which the daemon's own directory guard then refuses forever

`crates/gascan-core/src/engine_artifacts.rs:433`
(`std::fs::create_dir_all(paths.root())`), against
`crates/gascand/src/controller_state.rs:2452-2457` +
`controller_state.rs:3042-3051` (`validate_directory`, `private_mode` requires
`mode == DIRECTORY_MODE` = `0o700`) and
`packaging/macos/uninstall.sh:145-155` + `uninstall.sh:44-51,117-118`.

**Failure scenario A (fresh machine, fetch first).**
`ArtifactPaths::for_user()` (`engine_artifacts.rs:180-189`) resolves to
`$HOME/Library/Application Support/dev.gascan/engine`, and `fetch` creates the
whole chain with `create_dir_all`, i.e. `mkdir(0o777)` under the process umask.
On a default macOS host the umask is `022`.

MEASURED on this host just now: `umask` prints `022`; a `rustc`-compiled program
calling `std::fs::create_dir_all` on
`<tmp>/fakehome/Library/Application Support/dev.gascan/engine` produced
`755 …/dev.gascan/engine` and `755 …/dev.gascan`.

`gascan engine fetch` is reachable with no daemon at all and is documented as
the thing you run when the daemon cannot start (`crates/gascan/src/cli.rs:450-457`,
"Requiring a daemon here would make the remedy depend on the thing it repairs";
`gascand/src/main.rs:737-741` names it as the doctor remedy). So the realistic
first-run order on a clean install that has selected the Arca backend is: fetch,
then start the daemon.

The daemon then calls `open_controller_store` → `open_controller_directory`
(`controller_state.rs:2448`) → `ensure_private_child_directory(application_support,
"dev.gascan", …)`. Because `dev.gascan` now **exists**, the `created` branch is
not taken (`controller_state.rs:2587-2606`), no `fchmod` runs, and
`validate_directory(…, private_mode = true)` compares `0o755 != 0o700` and
returns `ControllerStateError::Unsafe("application directory ownership, type, or
mode is unsafe")` → code `controller_state_unsafe`. The daemon cannot start, on
either backend, until the user manually `chmod 700`s a directory nothing tells
them about. Note the error text says "ownership, type, or mode", so the message
does not even isolate the mode as the cause.

**Failure scenario B (daemon first, then fetch, then uninstall).** If the daemon
ran first, `dev.gascan` is `0700` and scenario A does not fire — but `fetch`
still creates the `engine` child at `0755`. `gascan_uninstall_remove_engine_data`
(`uninstall.sh:145-155`) passes `parent_private=true` and the helper's final
check on the child is `gascan_uninstall_validate_directory_entry "./$child" …
true true` (`uninstall.sh:117-118`), whose `private` branch (`uninstall.sh:44-47`)
requires exactly `mode_value == 0700`. `0755` fails → `gascan_uninstall_refuse_path`
→ `return 65`, which is not the tolerated `2`, so
`uninstall.sh:237` (`|| exit $?`) aborts `gascan uninstall --remove-data` at exit
65 — after the controller data has already been removed and before the
`sudo rm -f` of the installed binaries. The uninstall leaves the machine
half-torn-down.

**Why I believe it is real.** Both consumers require exactly `0700`, both are in
this diff's blast radius, and the producer sets no mode at all. `grep` for
`set_permissions|from_mode|0o700` across `crates/gascan-core/src` shows nothing
in `engine_artifacts.rs`; `crates/gascan-core/tests/engine_artifacts.rs` has no
occurrence of `mode` or `permissions`; and nothing under `tests/` references
`remove_engine_data`, so no gate covers either path.

**Secondary, same root cause.** Every other Gas Can-owned state directory is
opened through the fd-based, `SYMLINK_NOFOLLOW`, uid- and mode-validating
traversal in `controller_state.rs`. `engine_artifacts.rs` uses plain path APIs
throughout and validates nothing: `fetch` does `remove_dir_all(&staging)`
(line 439, 443), `remove_file(&kernel)` and `remove_dir_all(&vminit)` (lines
515-516) on paths under a directory it never confirms is a real directory owned
by this uid. A symlink planted at `…/dev.gascan/engine` redirects all three. That
requires write access to `dev.gascan`, which is the user's own — so it is not a
cross-user boundary today, but it is the discipline the rest of the product
enforces and it is absent here.

**What would refute it.** Any of: the artifacts root being chmod'd to `0700`
somewhere I did not find; `effective_account_home()` resolving somewhere other
than the home `ControllerStatePaths` uses (it does not — both call
`gascan_core::account::effective_account_home`, `engine_artifacts.rs:181` and
`controller_state.rs:81`); a shipped umask of `077` for the process that runs
`gascan engine fetch`; or `ensure_private_child_directory` repairing rather than
rejecting a pre-existing wrong-mode directory (it does not — `fchmod` runs only
under `if created`).

**Fix shape.** Create the artifact root with an explicit `0o700` (and `fchmod`
it when it already exists and is ours), the same way
`ensure_private_child_directory` does — ideally by reusing that helper rather
than growing a second directory discipline.

---

## Minor 1 — `verify_installed` claims the vminit layout "matches the pin" while hashing 478 of its 73,739,738 bytes

`crates/gascan-core/src/engine_artifacts.rs:354-407` (`require_oci_manifest`),
`:529-541` (`verify_installed`), reported by
`crates/gascand/src/main.rs:751-755` as the doctor's kernel fact.

**Failure scenario.** After a successful fetch, anything running as this user
(or plain disk corruption) rewrites
`~/Library/Application Support/dev.gascan/engine/vminit/blobs/sha256/<layer-digest>`
— the rootfs layer that is the guest-side init every sandbox boots.
`require_oci_manifest` reads `index.json`, requires one manifest descriptor,
compares its `digest`/`size` to the pin, and then hashes exactly one blob: the
manifest itself (`engine_artifacts.rs:401-406`, `pin.artifacts.vminit.content.bytes`
= **478** in `engine/arca-pin.json:26`). The manifest's `config` and `layers`
descriptors are parsed by nobody. `verify_installed` returns `Ok(())` and
`gascan doctor` prints "engine artifacts under … match gascan-engine-m4".

**Why I believe it is real.** The chain that would make the 478-byte digest
sufficient — manifest → layer digests → layer bytes — exists in the data but no
code in this repository walks it. The function's own docstring justifies hashing
the manifest blob because "`index.json` is the one file in the layout that is not
content-addressed"; the same argument applies one level down and is not followed
there. At fetch time the layers *are* covered, by the asset-level sha256 over
the `.tar.gz` (`engine_artifacts.rs:489-494`) which runs before `untar` — so this
is specifically a gap in the *re*-verification that `verify_installed` performs
and that the doctor advertises as "the SAME verification the fetch performs"
(`engine_artifacts.rs:524-528`).

**What would refute it.** `arca-engine` re-verifying every blob it loads against
the manifest, which would make the pinned manifest digest a sufficient root of
trust end to end. `crates/gascan-oci-fixture/src/lib.rs:393-394` asserts exactly
that ("the engine verifies blobs it loads") — but as a bare claim with no
citation, about a different codebase, and Gas Can's own reported fact would still
be false in the meantime.

**Fix shape.** Parse the verified manifest blob and `require_file` each `config`
and `layers` descriptor against its own digest and size. Everything needed is
already on disk and already verified.

---

## Minor 2 — an Arca daemon whose engine hangs gets Apple's remedies, which is the defect `arca_doctor_report` says it fixed

`crates/gascand/src/service.rs:298-304` (`doctor_timeout_report`) and
`service.rs:360-363` (collection abandoned), both hardcoding `&AppleRemedies`;
reached from `service.rs:367-371`.

**Failure scenario.** The Arca arm builds `DoctorState::refreshing(60s,
arca_doctor_report)` (`main.rs:355-361`). `arca_doctor_report` awaits
`engine_report()` (`main.rs:713-716`) with no deadline of its own, over a gRPC
channel to an engine that may be adopted, wedged, or mid-VM-boot. When that
exceeds 60s, `report()` discards the collector and returns
`doctor_timeout_report`, which pairs `DoctorFacts::unavailable(...)` with
`AppleRemedies`. `into_report` (`doctor.rs:244-281`) attaches a remedy to
**every** check including `Unknown` ones (`unwrap_or_else(|| remedies.remedy(id))`,
`doctor.rs:275-277`). The user of an Arca daemon is told "install Apple container
1.1.0 in PATH" (`doctor.rs:321`), "run `container system start` and retry"
(`doctor.rs:323`), and — for the offline check that is the whole point of the
certification gate — "install a supported Apple container release with proven
offline isolation" (`doctor.rs:357-358`).

**Why I believe it is real.** `main.rs:657-667` states this as the defect being
corrected: "an Arca-backed daemon with a dead engine socket told the user to
install Apple container — advice that would have changed nothing." The fix
(`main.rs:726`, `into_report(&ArcaRemedies)`) covers only the path where the
collector *returns*. A dead or wedged engine is precisely the case that reaches
the timeout instead. `service.rs:3504` (`default_doctor_report`, release build)
is the same shape.

**What would refute it.** `arca_doctor_report` being unable to exceed 60s — but
nothing bounds the `Capabilities` RPC, and the engine is spawned/adopted with no
liveness contract beyond having bound its socket.

**Fix shape.** Carry the backend's `&dyn DoctorRemedies` into `DoctorState`
alongside the collector, so the fallback reports use the same table as the
collector it replaced.

---

## Minor 3 — the engine socket's ownership check does not survive to the dial, and checks less than the daemon's own socket guard does

`crates/gascand/src/engine.rs:237-250` (`require_own_socket`), `:262-267`
(`ensure_engine`), `:283`; the real dial is `main.rs:362`
(`ChannelTransport::connect(launch.socket)` → `channel.rs:33`,
`UnixStream::connect(socket)`).

**Failure scenario.** `ensure_engine` stats `GASCAN_ENGINE_SOCKET` and compares
`st_uid` to `PeerUid::current()`, then returns. The caller opens the path again,
by name, in a separate syscall. Anyone who can write the socket's parent
directory can unlink and rebind between the two, and the daemon then speaks gRPC
— `Create`, `Exec` with the user's bind mounts — to a process it never validated.
Nothing in this diff validates that parent directory, and the variable is
undefaulted and user-supplied (`backend.rs:54-60`).

**Why I believe it is real.** The module's own claim is stronger than what the
code delivers: "Refused BEFORE any connection is attempted … every byte after the
dial would be trusted output from a process this user does not control"
(`engine.rs:60-65`). A name-based check followed by a separate name-based open is
not that property. Note also the asymmetry with the daemon's own socket guard,
which validates type, uid, `st_nlink == 1`, `mode == 0o600` and a `0o700` parent
(`gascand/src/socket.rs:14-15,318-327,389-393`); `require_own_socket` validates
uid alone, and `std::fs::metadata` follows symlinks where `socket.rs` uses
`statat`/`fstat`.

**What refutes most of the impact.** The window only opens where the socket's
directory is writable by an attacker. `/tmp` on macOS is sticky, so the obvious
placement is safe, and a socket pre-created by another user is caught by the uid
comparison at first sight. This is a hardening gap and a doc-vs-code overclaim,
not a demonstrated cross-user compromise — which is why it is Minor.

**Fix shape.** Validate through the connected fd rather than the path: connect
first, then `getsockopt(LOCAL_PEERCRED)` / `SO_PEERCRED` on the stream and refuse
a foreign peer before any request is written. That closes the window instead of
narrowing it, and it is the same direction `api.rs` already validates in.

---

## Minor 4 — an engine that starts but never binds is left running, once per failed daemon start, with no bound

`crates/gascand/src/engine.rs:266-267`, `:299-304`, `:313-325`.

**Failure scenario.** `spawner.spawn()` succeeds, the engine process comes up and
never binds (a `--state-root` it cannot write, a vmnet failure after
initialisation, a hang in image-store recovery). After 20s `wait_until_listening`
returns `NotListening` and `ensure_engine` propagates it; `main.rs:355` `?`s out
and the daemon exits. `SpawnedEngine` is dropped with `kill_on_drop(false)`
(`engine.rs:317-321`), so the engine keeps running. The next `gascan up` starts a
daemon, `listening()` is still false, and it spawns **another** engine. Nothing
in the loop caps this, and nothing kills an engine it did not start
(`engine.rs:12-17`), so a user retrying a broken configuration accumulates engine
processes each holding VM and vmnet resources.

**Why I believe it is real.** It follows directly from three deliberate choices
in the module (no kill on drop, no unlink of a stale socket, dial-before-spawn)
and there is no counting or exclusion anywhere on the path.

**What would refute it.** An engine that fails this way exiting on its own —
which the `Exited` arm (`engine.rs:296-298`) would then catch and report, leaving
nothing behind. That is the common case; this is about the one that hangs.

---

## Minor 5 — the tag signature gate binds only SSH-format signatures

`scripts/build-arca-engine.sh:98-103`.

**Failure scenario.** `git verify-tag` selects its verifier from the signature
payload in the tag object, not from configuration. `gpg.ssh.allowedSignersFile`
constrains only the SSH path. A tag carrying an OpenPGP signature is handed to
`gpg`, which exits 0 for a good signature from any key present in the builder's
keyring regardless of trust level — and `git` reports `GOODSIG` as success. The
`allowed-signers` file (one `ssh-ed25519` key, `engine/allowed-signers:1`)
constrains nothing on that path.

**Why I believe it is real.** The gate's entire authority is that one file, and
nothing asserts the signature is the format that file governs. The two gates
immediately around it were hardened precisely against a resolver disagreeing with
the verifier (`build-arca-engine.sh:84-97`); this is the same class one level
over, in the verifier's choice of algorithm.

**What refutes most of it.** It requires an attacker key already in the builder's
GPG keyring. On a hosted CI runner the keyring is empty and `gpg` fails closed,
so the release path is not exposed; a developer's Mac with an imported key is.

**Fix shape.** Assert the format before trusting the verdict — e.g. require the
tag's signature block to begin `-----BEGIN SSH SIGNATURE-----` (via
`git for-each-ref --format='%(contents:signature)' "refs/tags/$tag"`) and exit 65
otherwise.

---

## Refuted candidates (checked and not reported)

- **Tar extraction escaping the staging directory.** `untar`
  (`engine_artifacts.rs:288-297`) runs `/usr/bin/tar -xzf` with no
  `--no-same-owner`/path guard, but the archive's own sha256 is verified at
  `:489-494` *before* `:497-502` unpacks it, so the bytes are pinned.
- **`asset` used as a path component** (`:454`, `:482`). Constrained to
  `^[A-Za-z0-9._-]+$` by `engine/arca-pin-schema.jq:67`, and the pin is
  `include_str!`-compiled, not read at run time.
- **Promotion leaving a half-installed set** (`:513-518`). `rename` of the vminit
  failing after the kernel's succeeded leaves no vminit — but `verify_installed`
  then returns `Io(NotFound)` and the doctor reports "not installed" with the
  fetch remedy. Fails closed.
- **A `Proven` reaching policy from a second path.** `certified_isolation` is the
  only producer (`translate.rs:371`) and `validate_capabilities` the only
  consumer (`policy.rs:186`, one production call site at `service.rs:4496`).
- **Zombie engine children.** `tokio::process::Child` dropped with
  `kill_on_drop(false)` is handed to tokio's orphan queue and reaped by the
  signal driver.
- **`signaling_record`'s `backend: "endpoint-attested"`**
  (`gascan/src/daemon.rs:740-746`) matching no real selection. It never reaches
  `require_matching_backend`, which runs only in `connected_outcome`
  (`daemon.rs:2065`).
- **`backend_selection(true, false)` returning `Apple` in release**
  (`gascan-core/src/backend.rs:128-129`). Unreachable: `fake_requested` is a
  compile-time `false` there (`backend.rs:149-150`).
- **Secrets in logs/errors.** `curl` is invoked with only the pinned URL
  (`engine_artifacts.rs:263-273`); `SystemTools::run` surfaces stderr only;
  `TransportError` carries the operation, the socket path and the gRPC status
  (`channel.rs:39-43,80-85`). No token, credential, or environment dump reaches
  any of them.


=====================================================================
# Reviewer: wiring
=====================================================================

# Milestone 4 whole-landing review — cross-task correctness

Branch `feat/milestone-4-product-wiring`, `git diff main...HEAD` (56 files, ~7863 insertions).
Dimension: the seams between Tasks 9–15, which task-scoped review cannot see.

Known-and-decided items excluded from findings per the brief: `CERTIFIED_ENGINE_REVISION = None`
and the failing-by-design `live/network.rs`; the `gascan-e2e` arca image's guest-side stubs;
`wait_until_listening`'s untested check order.

**Counts: 0 Critical, 4 Major, 4 Minor.** No Critical finding survived refutation.

Candidates I killed by reading the surrounding code, recorded so they are not re-raised:

- *A stale engine socket permanently wedges the daemon.* `engine.rs:206-217` treats `ECONNREFUSED`
  as "not listening" and deliberately declines to unlink, and `live/common/mod.rs:42` says a stale
  `engine.sock` "makes the bind fail". **Refuted:** the engine removes it itself —
  `EngineServer.start` does `SocketPathLock.acquire` then `removeStaleSocket(at:)`
  (`.artifacts/arca-engine/arca/Sources/ArcaEngine/EngineServer.swift:91-92`).
- *`serve_arguments()` names options the engine does not have.* **Refuted:** verified against
  `ArcaEngineCommand.swift:152-166` — `--socket-path`, `--state-root`, `--kernel-path`,
  `--vminit-layout`, and `serve` is `defaultSubcommand` so omitting it is correct.
- *The `BackendMismatch` message tells the user to run a command that is itself refused.*
  **Refuted:** `stop_with`/`restart_with` (`daemon.rs:1533`, `daemon.rs:1741`) do not pass through
  `connected_outcome`, so `gascan daemon stop` works across backends.
- *Task 9's jq pin schema is a shell-script-only gate that the Rust consumer bypasses (`.asset`
  path traversal).* **Refuted:** `.github/workflows/ci.yml:111` runs `scripts/build-arca-engine.sh`,
  which validates the real `engine/arca-pin.json` against `engine/arca-pin-schema.jq` before any
  release build.
- *A `DoctorCheckId` lacks an Arca remedy.* **Refuted:** `ArcaRemedies::remedy`
  (`crates/gascan-core/src/doctor.rs:353-420`) matches all 21 ids exhaustively and no arm names
  Apple's runtime. (But see Major 4 and Minor 4 for the two ways Apple prose and stale prose still
  reach an Arca user.)

---

## Major 1 — The client gives up on the daemon (15s) before the supervisor gives up on the engine (20s), and both are under the cold-engine start this repository has measured

`crates/gascand/src/engine.rs:52` — `EngineReadiness::default().timeout = 20s`
`crates/gascan/src/daemon.rs:594` — `SupervisorTimeouts::readiness = 15s`
`crates/gascan-arca/tests/live/common/mod.rs:400-405` — the measurement

**Failure scenario.** Correct configuration, cold engine (first execution of a freshly built
binary, or a fresh state root, or a loaded machine). `gascan up` → client spawns `gascand` → the
Arca arm calls `ensure_engine` → miss → spawn. The engine must `validateEngineInputs`, construct
`EngineManagers`, `loadVminit` a 73,739,738-byte OCI layout into its store and resolve an initfs
(`ArcaEngineCommand.swift:183-216`) — all **before** `EngineServer.start` binds at line 307. The
client's 15s deadline elapses first and it returns
`SupervisorError::Readiness { state: Stopped|Unreachable, .. }` (`daemon.rs:1298-1301`). Five
seconds later the daemon's own `NotListening` error — the one that names the socket — is produced
for a client that has already gone. `gascan up` fails on a correctly configured host.

**Why I believe it is real.** The bound is not speculative: this repository measured it and wrote it
down. `await_socket`'s doc at `live/common/mod.rs:400-405` says, in its own words, "The bound is
120s and not 30s because a binary's first execution is far slower than its later ones … **30s
failed on a cold engine**." Task 11 chose 20s and Task 10/earlier chose 15s; the live tier that
measured 30s-is-not-enough is Task 14/15's neighbour and neither number was revisited. The only
instrument that runs this path end to end (`arca_engine.rs`) is `#[ignore]`d and its own comment
says it measures "a warm store on this host … a few seconds", so a warm run cannot see this.

**What would refute it.** A measurement of `arca-engine` binding on a cold state root in under 15s,
or evidence that `loadVminit` and `ContainerManager.initialize()` are lazy rather than pre-bind.
I read `ArcaEngineCommand.run()` and they are not: both complete before line 307.

**Aggravating.** The two bounds are also in the wrong order relative to each other. Even if both
were raised, 20s > 15s means the supervisor's `NotListening` error can never reach a user by
construction: the client always abandons first.

---

## Major 2 — Every Arca startup failure is written to a null stderr and to no structured channel, and the doctor fact that names `gascan engine fetch` is behind the daemon that cannot start without the artifacts

`crates/gascand/src/main.rs:254` — `drop(startup_diagnostic);`
`crates/gascand/src/main.rs:296-340` — the Arca arm's `required(...)` errors and `ensure_engine`
`crates/gascand/src/main.rs:747-752` — the `run \`gascan engine fetch\`` remedy
`crates/gascan/src/client.rs:394-396` — `command.stderr(Stdio::null())`
`crates/gascan/src/daemon.rs:1223`, `1237` — the client's only structured channel

**Failure scenario.** A user follows the Arca path on a fresh host: sets `GASCAN_ARCA_BACKEND`,
`GASCAN_ENGINE_BIN`, `GASCAN_ENGINE_SOCKET`, `GASCAN_ENGINE_STATE_ROOT`, and has not run
`gascan engine fetch`. `gascan up` → daemon starts → `ensure_engine` spawns the engine → the engine
refuses at `validateEngineInputs` with `--kernel-path names nothing that exists: <path>`
(`.artifacts/arca-engine/arca/Sources/ArcaEngine/EngineStartup.swift:56-60`) and exits without
binding → `ensure_engine` returns `EngineError::Exited { status }` → `run()` returns `Err` → the
daemon exits. What the user sees, 15s later, is
`SupervisorError::Readiness { state: Stopped, detail: <generic> }`. The engine's message went to the
daemon's stderr, which `TokioEngineSpawner` inherits and which `TokioDaemonSpawner` set to
`Stdio::null()` (`GASCAN_DAEMON_STDERR_PATH` is a test-only variable). The daemon's own carefully
worded errors — "`GASCAN_ARCA_BACKEND` selects the Arca engine backend, so `GASCAN_ENGINE_SOCKET`
must name its socket" (`main.rs:301-306`) — go to the same place.

The startup-diagnostic descriptor exists precisely to carry a startup failure to the client, and it
is `drop`ped at `main.rs:254`, *before* the backend match. `DaemonStartupMonitor::controller_error`
(`daemon.rs:514`) is the only structured channel the client reads, and only
`report_controller_startup_error` (`main.rs:461-484`) ever writes it — for `ControllerStateError`
alone.

**The cross-task half.** Task 13 built the correct remedy: `engine_artifact_fact()` reports "engine
artifacts are not installed under `<root>`" `.with_remedy("run \`gascan engine fetch\`")`
(`main.rs:747-752`). Task 11's startup ordering makes that fact unreachable in exactly the state it
describes — the fact lives inside a daemon that cannot start until the artifacts exist. `gascan
doctor` connects to the daemon before dispatching (`cli.rs:470`, `cli.rs:598`); only
`Command::Engine` returns early (`cli.rs:455-458`). So the one command whose comment says
"Requiring a daemon here would make the remedy depend on the thing it repairs" is right about
`fetch` and the doctor that *names* fetch has exactly that dependency.

**Why I believe it is real.** Every link is code I read, not inference: the drop site, the null
stderr default, the single writer of the diagnostic file, the engine's pre-bind validation, and the
CLI's connect-before-dispatch.

**What would refute it.** A second path by which the Arca arm's errors reach the client — I grepped
`startup_diagnostic` in `main.rs` and its four uses are all in the store-open arm — or a
`Stdio::inherit()`/log file for the daemon in production, which `daemon_launch` does not provide.

---

## Major 3 — The controller store is shared across backends, so the sequential switch the mismatch error recommends produces exactly the confusion the backend field exists to prevent

`crates/gascan/src/daemon.rs:400-403` — the error that recommends the switch
`crates/gascand/src/controller_state.rs:87-104` — one database per user, backend not in the path
`crates/gascand/src/main.rs:243-253` — the store is opened before the backend is resolved
`crates/gascand/src/main.rs:585` — `let _ = service.reconcile().await?;`
`crates/gascand/src/api.rs:1857-1867` — `list` reads the store, with no runtime cross-check

**Failure scenario.** `gascan up` on Apple creates sandbox A; the Apple container is running.
The user sets `GASCAN_ARCA_BACKEND=1` and runs `gascan ps`. Task 10 refuses correctly:
`BackendMismatch`, and the message says "stop it with `gascan daemon stop` or clear the backend
environment to match it". The user does the first. Now `gascan ps` starts an Arca daemon, which
opens *the same* `~/Library/Application Support/dev.gascan/controller/<db>` — there is no backend in
`ControllerStatePaths` and no backend column in the store — and `list` returns record A from the
store verbatim. `reconcile()` does raise `ReconcileFinding::MissingOwned(A)`, and `run_daemon`
discards the whole report at `main.rs:585`. So `gascan ps` under Arca reports the Apple sandbox as
running; `gascan down A` fails against an engine that never heard of it; `gascan destroy A` removes
the record while the Apple container keeps running, unreferenced.

That is, near-verbatim, the failure `api.rs:36-42` says the backend field exists to stop: "silently
reaches the Arca daemon and reports its sandboxes as though they were Apple's." Task 10 closed the
concurrent case and the persistent case is wide open — and the refusal message steers the user into
it.

**Why I believe it is real.** `grep -n backend crates/gascand/src/store.rs
crates/gascand/src/controller_state.rs` returns nothing; the store path is derived from the account
home and a fixed application id; the reconcile report is dropped at the one production call site.

**What would refute it.** A backend-scoped database path, a per-record backend column, a
`reconcile()` consumer that quarantines `MissingOwned` records, or a `list` that cross-checks the
runtime. None exists on this branch.

---

## Major 4 — A doctor refresh that times out on the Arca path emits Apple's remedies

`crates/gascand/src/service.rs:298-303` — `doctor_timeout_report(..).into_report(&AppleRemedies)`
`crates/gascand/src/service.rs:363` — the abandoned-collector arm, same
`crates/gascand/src/main.rs:349-354` — the Arca daemon installs `DoctorState::refreshing(60s, ..)`

**Failure scenario.** An Arca daemon is up; the engine is alive but wedged (stopped, deadlocked, or
merely slower than 60s under load — `arca_doctor_report`'s `engine_report()` is a tonic call with no
deadline of its own). The user runs `gascan doctor`. `DoctorState::report()` takes the
`Refreshing` arm, `tokio::time::timeout` fires, and the report becomes
`DoctorFacts::unavailable(..).into_report(&AppleRemedies)` — all 21 checks `Unknown`, every remedy
Apple's. `render_human_doctor` prints a `Fix:` line for every non-pass check
(`presentation.rs:193-195`), so the user of an Arca-backed daemon is told
`runtime.cli → install Apple container 1.1.0 in PATH` and
`runtime.service → run \`container system start\` and retry`.

**Why I believe it is real.** This is word for word the defect Task 12's own doc comment declares
eliminated: "An Arca-backed daemon whose engine socket was dead told the user to 'install Apple
container 1.1.0 in PATH' — advice that is not merely unhelpful but actively misdirecting"
(`doctor.rs:310-315`). The trait threading covered every construction site the Arca arm builds
directly, and missed the two inside `service.rs` that build a report *about* a collector rather than
from one. A hung engine is not an exotic state — it is the state the 60s timeout exists for.

**What would refute it.** A `DoctorRemedies` carried on `DoctorState` so the timeout report could use
the right set, or evidence that the `Refreshing` timeout arm is unreachable for Arca. It is the
Arca daemon's only doctor source (`main.rs:349`).

---

## Minor 1 — A spawn that never binds abandons the engine process it started

`crates/gascand/src/engine.rs:266-267`, `crates/gascand/src/engine.rs:169-176`,
`crates/gascand/src/engine.rs:316-320`

`ensure_engine` owns `spawned: SpawnedEngine` locally. `SpawnedEngine` has no `Drop`, and
`TokioEngineSpawner` sets `kill_on_drop(false)` — correct and load-bearing on the success path,
where the surviving engine is the adoption property. On the `NotListening` path it means the daemon
returns an error, exits, and leaves behind an engine it started that never became usable, which
nothing will ever reap. Under Major 1 this is the ordinary outcome of a cold first run, so the
sequence is: `gascan up` fails, an engine keeps starting in the background, the retry adopts it.
The harness's own comment records the shape of the cost — "An `arca-engine` was once found still
running four days after the run that spawned it" (`arca_common/mod.rs`, `Drop for ArcaE2e`).

**What would refute it:** a `Drop` on `SpawnedEngine` that kills only on the error return, or a kill
at the `ensure_engine` error site. Neither exists. I checked that the `Exited` arm is unaffected —
that child is already gone.

---

## Minor 2 — `runtime.kernel` describes Gas Can's installed artifacts, which an adopted engine may not be using

`crates/gascand/src/main.rs:309-322` (the asymmetry comment), `crates/gascand/src/main.rs:733-736`,
`crates/gascand/src/engine.rs:1-22` (adoption)

The comment argues the kernel and vminit must not be environment-read so that "the doctor
[cannot] report one pair while the spawn used another". That holds for the spawn arm. It does not
hold for the *dial* arm, which is the primary path by design: an adopted engine was pointed at
`--kernel-path` and `--vminit-layout` by whoever started it, and nothing compares those to
`ArtifactPaths::for_user()`. So `runtime.kernel: pass — engine artifacts under <root> match <tag>`
can be true while the running engine boots guests on entirely different bytes; and the inverse
refuses `gascan up` through `require_runtime_ready` (`service.rs:1058-1065`) against a perfectly
functional adopted engine whose own artifacts are fine.

This is not fixable within the contract — `Capabilities` carries no artifact paths, which
`main.rs:333-338` already acknowledges for the state root. The finding is that the fact's *wording*
claims more than the evidence supports on the adoption path. The same argument applies to
`GASCAN_ENGINE_STATE_ROOT`: an adopted engine may be serving a different one, and nothing detects it.

---

## Minor 3 — `gascan engine fetch` destroys the installed artifacts before promoting, and takes no lock

`crates/gascan-core/src/engine_artifacts.rs:487-495`, `crates/gascan-core/src/engine_artifacts.rs:509-519`

The promotion comment claims "a reader either sees the previous artifact or this one, never a
partial". That is false for `vminit`: `remove_dir_all(&vminit)` at line 516 precedes
`rename(&staged_vminit, &vminit)` at line 518, so there is a window in which no layout exists, and
any failure or interruption inside it leaves the host with a new kernel and no vminit — an engine
that will not start. `remove_file(&kernel)` at line 515 is not even necessary; `rename` over an
existing file is atomic on its own.

Concurrency makes it reachable without an interruption: `fetch` has no lock, and line 493's
`remove_dir_all(&staging)` deletes an in-flight run's staging directory. Two `gascan engine fetch`
runs → run A verifies `staging/vmlinux`, run B wipes staging, A's `remove_file(&kernel)` succeeds
and A's `rename` fails `ENOENT` → no installed kernel. `scripts/build-arca-engine.sh:55-60` takes an
`mkdir` lock over the same class of hazard for the same reason; the Rust fetch does not.
Recoverable by re-running fetch — hence Minor, not Major.

A related, smaller cost on the same code: `engine_artifact_fact()` runs `verify_installed`, which
sha256s the whole 28,248,576-byte kernel synchronously inside an `async fn` on a 2-worker Tokio
runtime (`main.rs:212-216`), on every `gascan doctor`, `up` and `apply`.

---

## Minor 4 — Two prose defects that reach the user

1. **`SupervisorError::BackendMismatch`'s message has eighteen literal spaces in it.**
   `crates/gascan/src/daemon.rs:402` — a missing `\` line continuation inside the string literal, so
   the user reads `…and this command expects apple;                  stop it with \`gascan daemon
   stop\`…`. Verified with `cat -A`; `cargo fmt` cannot see inside a string literal.

2. **The Arca `RuntimeKernel` remedy predates `gascan engine fetch` and never names it.**
   `crates/gascan-core/src/doctor.rs` (`ArcaRemedies`, `RuntimeKernel`) says "fetch the engine
   artifacts recorded in engine/arca-pin.json, then run `gascan daemon restart`" — a description of
   a command rather than the command. Task 13 added `gascan engine fetch` afterwards and attached it
   per-fact at `main.rs:747-752`, but only on two of `engine_artifact_fact()`'s four arms: the
   malformed-pin arm (`main.rs:737-741`) and the unresolvable-`ArtifactPaths` arm
   (`main.rs:742-745`) return a bare `DoctorFact::fail`, fall through to the backend default, and
   tell the user to do something without saying how.


=====================================================================
# Reviewer: claims
=====================================================================

# Milestone 4 whole-landing review — claims and test sufficiency

Branch `feat/milestone-4-product-wiring`, `git diff main...HEAD` (56 files, +7863).
Reviewed dimension: whether the written claims are true, and whether the tests are
instruments or decoration.

Method: every repo-local `file:line` anchor added by this diff was opened and read;
every arithmetic claim was recomputed; the ignore baseline was regenerated by running
the checker; the central exit-code claim was reproduced by running the pinned engine.

---

## Findings by severity

**Critical: none found.**

**Major: 2.**

**Minor: 8.**

---

### MAJOR 1 — the handoff doc is stale at HEAD and prescribes an acceptance the landing refutes

`docs/status/START-HERE.md:11`, `:29`, `:60-64`

**Claim as written** (line 11): "MILESTONE 4: LANDINGS 2 AND 3 ARE DONE. **TASKS 14 AND 15
REMAIN**, THEN THE WHOLE-LANDING REVIEW."
(line 29): "The last commit that changed code is `629ca27` (Task 13)."
(lines 60-64, Task 15 instructions): "**Only then** set the constant… **Acceptance: with the
constant set, a live `Capabilities` yields `Proven`**; with it altered by ONE CHARACTER, the
same engine yields `Unverified`. **That pair is the instrument.**"

**What I found instead.** Two commits land after `d9af25c` (the last commit that touched
START-HERE):

```
$ git log --oneline main..HEAD | head -3
fd05780 feat(arca): the offline proof was run and it refutes; the constant st...
cd3e2a5 feat(e2e): the daemon passed the engine one argument of the four it r...
d9af25c docs: the head row went stale the moment it was committed...
```

`cd3e2a5` is Task 14 (it adds `crates/gascan-e2e/tests/arca_engine.rs`, 152 lines, plus the
whole `gascan-oci-fixture` crate). `fd05780` is Task 15 (it adds
`crates/gascan-arca/tests/live/network.rs` and `docs/evidence/2026-08-18-arca-engine-offline.md`).
Both changed code. So all three past/present-tense claims above are false at HEAD.

The third is the one that matters. `docs/evidence/2026-08-18-arca-engine-offline.md:1-9` records
that the offline proof **refutes**, that `CERTIFIED_ENGINE_REVISION` stays `None`, and that
"there is no engine build to certify". A reader following START-HERE's step 3-4 would set the
constant against an instrument that refuted it — which
`crates/gascan-arca/src/translate.rs:317-319` calls out by name as "worse" than a claim with no
instrument. The entry-point document now instructs the opposite of what the landing concluded.

**How I checked.** `git log --oneline main..HEAD`; `git show --stat cd3e2a5`; `sed -n '1,70p'
docs/status/START-HERE.md`; read `docs/evidence/2026-08-18-arca-engine-offline.md` in full.

---

### MAJOR 2 — a comment says the revision gate does not exist, in the landing that adds it

`crates/gascan-arca/src/translate.rs:784-788`

**Claim as written:**

```rust
offline: v1::Isolation::Proven as i32,
// Empty, because nothing reads it yet and a plausible-looking
// revision here would be a claim this test does not make. Field 20
// arrived with the schema-2 pin; the certified-revision comparison
// that turns it into a Proven/Unverified verdict is still to come,
// and the tests that earn it belong with it rather than here.
build_revision: String::new(),
```

**What I found instead.** Both halves are false in the tree this comment ships in:

- `translate.rs:371` — `Ok(v1::Isolation::Proven) => certified_isolation(&capabilities.build_revision),`
  reads the field.
- `translate.rs:356-361` — `fn certified_isolation(build_revision: &str) -> NetworkIsolation` is
  the comparison the comment says is "still to come".
- `translate.rs:826-861` — `an_uncertified_engine_cannot_claim_proven_and_its_refusals_are_left_alone`
  is "the tests that earn it", 40 lines below this comment in the same file.

The comment is also load-bearing in the wrong direction: it is the explanation for why the fixture
carries an empty revision, and the real reason today is the opposite — an empty revision is now the
*input to a gate*, exercised deliberately at `translate.rs:833-836` as "the empty revision a build
with a broken build-info generator reports".

**How I checked.** `grep -n build_revision crates/gascan-arca/src/translate.rs` (hits at 356, 358,
371, 789, 835, 941), then read each site. Blame confirms the ordering:

```
$ git log --oneline -S "the certified-revision comparison" main..HEAD -- crates/gascan-arca/src/translate.rs
28cc656 build(engine): the pin carries both digests per asset...
$ git log --oneline -S "fn certified_isolation" main..HEAD -- crates/gascan-arca/src/translate.rs
7f9e8e6 feat(daemon): the Arca backend is selectable...
```

The comment was written at `28cc656` and made false by the very next commit, `7f9e8e6`.

---

### MINOR 1 — `release-smoke.sh:1015-1037` is short by one line, in five places

`docs/evidence/2026-08-18-arca-engine-offline.md:96`,
`crates/gascan-arca/tests/live/network.rs:19`,
`docs/status/START-HERE.md:56`, plus two occurrences in the plan/design docs.

**Claim as written:** "The three mechanisms are the ones `packaging/macos/release-smoke.sh:1015-1037`
asserts, each as the sandbox user and again as guest root."

**What I found instead.** The six-probe block is `1015-1038`. Line 1015 opens the first `if`, line
1037 is the last `exit 1`, and line 1038 is the closing `fi`. The cited range truncates the last
probe mid-statement. The substance of the claim — three mechanisms × two privilege levels, in that
file — is correct.

**How I checked.**

```
$ grep -n "offline sandbox reached the test-owned endpoint\|offline guest root resolved public DNS" \
    packaging/macos/release-smoke.sh
1016:  printf 'offline sandbox reached the test-owned endpoint\n' >&2
1036:  printf 'offline guest root resolved public DNS\n' >&2
$ sed -n '1038p' packaging/macos/release-smoke.sh
fi
```

`packaging/macos/release-smoke.sh` is unchanged by this diff, so this is not drift — it was off by
one when written.

---

### MINOR 2 — `gascan engine fetch` has no instrument at all

`crates/gascan/src/cli.rs:944-966` (`execute_engine`), `:453-459` (the early-return arm)

**Claim as written** (`cli.rs:453-457`): "Handled BEFORE the daemon connection, and deliberately so…
Requiring a daemon here would make the remedy depend on the thing it repairs."

**What I found instead.** The property is real and well-argued, and nothing defends it. The
`engine_artifacts` module underneath is well covered (`crates/gascan-core/tests/engine_artifacts.rs`,
seven tests, all real — see the positive notes below), but no test invokes the CLI command. Deleting
the early return at `cli.rs:453-459` — which would push `gascan engine fetch` behind a daemon
connection, exactly the defect the comment names — leaves the whole workspace green.

**How I checked.**

```
$ grep -rn "engine.*fetch\|engine_fetch" crates/gascan-e2e/tests/*.rs
crates/gascan-e2e/tests/arca_engine.rs:41:  ...  the artifacts `gascan engine fetch`
crates/gascan-e2e/tests/arca_engine.rs:63:  ...  the boot artifacts gascan engine fetch \
crates/gascan-e2e/tests/arca_engine.rs:178: ...  the boot artifacts gascan engine fetch \
```

All three are prose in doc comments and ignore reasons; none is an invocation. Also noted in
passing: `Command::Engine { .. } => Ok(0)` at `cli.rs:475-476` is unreachable, since the arm at
`:453` returns first.

---

### MINOR 3 — `repository_of` carries a subtle rule and no test

`crates/gascan-oci-fixture/src/lib.rs:505-521`

**Claim as written:** "The rule is `immutable_image_identity`'s (`crates/gascan-core/src/runtime.rs`),
mirrored by Arca's `ImageIdentity.repository(of:)`: drop anything from `@sha256:` onward, then drop
a tag — the last `:` that comes *after* the last `/`, so the port in `registry.example:5000/repo` is
not mistaken for one. `heldImageReferences` compares the request's repository against the store's,
so a split that disagreed with Arca's would be refused as `not_found` for content the engine holds."

**What I found instead.** The implementation is correct — I traced it by hand against
`registry.example:5000/repo`, `repo:tag` and `registry.example:5000/repo:tag`, and it agrees with
`immutable_image_identity` (`crates/gascan-core/src/runtime.rs:704-715`) on all three. But the
function has zero tests, and its only caller (`lib.rs:501`, inside `stored_image_reference`) is
reached only from `#[ignore]`d live tiers. The whole crate has exactly one unit test
(`lib.rs:548`), and it is about `layer_archive`, not this. Inverting the `!` at `lib.rs:517`
— which is precisely the port-vs-tag confusion the comment warns about — leaves the default suite
green.

**How I checked.** `grep -rn "repository_of" crates/` returns only the definition and the one
internal call; `grep -n "#\[test\]" crates/gascan-oci-fixture/src/lib.rs` returns one line.

---

### MINOR 4 — a test docstring claims more than the test checks

`crates/gascan-core/tests/engine_artifacts.rs:379-381`

**Claim as written:** "The pin this binary was built from parses, **and describes the release that
exists.**"

**What I found instead.** `the_compiled_in_pin_parses_and_names_both_artifacts` checks shape only:
`schema == 2`, revision length 40, digest lengths 64, positive byte counts, and that each asset URL
contains the tag. Nothing establishes that the release exists — and by the file's own opening
paragraph (`:5-7`, "Nothing here reaches the network"), nothing here could. The shape checks are
worthwhile; the sentence overstates them.

**How I checked.** Read `crates/gascan-core/tests/engine_artifacts.rs:382-399` in full.

---

### MINOR 5 — one new ignore-baseline entry's reason understates its requirements

`crates/gascan-arca/tests/live/read_rpcs.rs:108`

**Claim as written:** `#[ignore = "requires the engine BUILT FROM THE PIN by
scripts/build-arca-engine.sh, named by GASCAN_ARCA_ENGINE_BIN"]`

**What I found instead.** `the_engine_reports_the_revision_the_pin_names` calls `LiveEngine::start()`,
which builds `EngineOptions` from **four** required variables — `GASCAN_ARCA_ENGINE_BIN`,
`GASCAN_ARCA_KERNEL_PATH` and `GASCAN_ARCA_VMINIT_LAYOUT`
(`crates/gascan-arca/tests/live/common/mod.rs:94-105`) — each of which panics if absent. A host with
the pinned engine but no kernel gets a panic naming `GASCAN_ARCA_KERNEL_PATH`, not the skip the
reason implies. The other five new entries name their requirements accurately; the neighbouring
pre-existing entry at `read_rpcs.rs:60` has the same gap, so this is a copied pattern rather than a
new mistake.

**How I checked.** Read `crates/gascan-arca/tests/live/common/mod.rs:88-131` and
`read_rpcs.rs:107-140`.

---

### MINOR 6 — a claim in START-HERE is falsified by its own presence

`docs/status/START-HERE.md:49`

**Claim as written:** "`sha256:a61c4cd9…` remains a superseded vminit digest. **It appears in no
tracked file anywhere.**"

**What I found instead.**

```
$ git grep -n "a61c4cd9"
docs/status/START-HERE.md:49:`sha256:a61c4cd9…` remains a superseded vminit digest. It appears in no tracked file anywhere.
```

The sentence is the only tracked occurrence, and writing it made it false. The intent — that no
pin, script or manifest names it as live — is true and worth recording; the wording is not.

---

### MINOR 7 — two shipped strings carry a folded-indentation artifact

`crates/gascan/src/daemon.rs:402`, `crates/gascan-arca/src/translate.rs:829`

`daemon.rs:402` is the user-facing `BackendMismatch` message. It contains eighteen literal spaces
mid-sentence, from a line break that was wrapped without a `\` continuation:

```
$ grep -n "stop it with" crates/gascan/src/daemon.rs | cat -A
402: ..."the·running·daemon·uses·the·{running}·backend·and·this·command·expects·{expected};··················stop·it·with·`gascan·daemon·stop`..."
```

`translate.rs:829` has the same artifact in an assertion message ("…evidence exists;
&nbsp;&nbsp;… this test's other assertions…"). Cosmetic, but the first one is prose a user reads
at the moment they are already confused about which daemon they reached.

---

### MINOR 8 — several plan/design anchors have drifted because the branch moved the lines

`docs/superpowers/plans/2026-08-16-p5-1-milestone-4-product-wiring.md:71,72,501,502,511`
`docs/superpowers/specs/2026-08-16-p5-1-milestone-4-product-wiring-design.md:76,163,302`

Six anchors named lines this branch then rewrote. I verified each was **correct when written** by
resolving it against `main`:

| anchor | on `main` | at HEAD |
|---|---|---|
| `scripts/build-arca-engine.sh:31` | `(.schema == 1) and` | `}` |
| `scripts/build-arca-engine.sh:35` | `(.revision \| … "^[0-9a-f]{40}$"))` | a comment line |
| `scripts/build-arca-engine.sh:94` | `verify-tag "refs/tags/${tag}" …` | a comment line |
| `scripts/sync-arca-proto.sh:43` | `(.schema == 1) and` | a comment line |
| `crates/gascan-core/src/doctor.rs:237` | `pub fn into_report(self) -> DoctorReport {` | a doc-comment line |
| `crates/gascand/src/main.rs:483` | `let _ = service.reconcile().await?;` | now at `:585` |

This is expected of a plan that says "modify line N", not fabrication — recorded so a later reader
does not chase them. Every anchor in these docs that points at code the branch did **not** move
still resolves exactly: `crates/gascan/src/daemon.rs:185`, `crates/gascand/src/socket.rs:631`,
`crates/gascand/tests/controller_state.rs:48`, `crates/gascand/src/reconcile.rs:5-21`.

---

## What I verified and found sound

Recorded because these are the claims most worth doubting, and they hold.

**The exit-64 claim — reproduced empirically.** Asserted at `crates/gascand/src/engine.rs:118-120`,
`crates/gascan-e2e/tests/arca_engine.rs:9-12`, `crates/gascand/tests/engine_supervisor.rs:82-84`,
and in `cd3e2a5`'s commit message. I ran the pinned binary:

```
$ arca-engine --socket-path ./probe.sock ; echo "exit=$?" ; ls probe.sock
Error: Missing expected argument '--state-root <state-root>'
exit=64
ls: probe.sock: No such file or directory
```

Exit code, message text and "binds nothing" all confirmed, against
`.artifacts/arca-engine/arca/.build/release/arca-engine` at
`c545612b056e028d5885968a7b9f586d694f994c`. (`arca_common/mod.rs:12` quotes the message as
`'--state-root'` without the ` <state-root>` placeholder — an inexact quotation of a message the
two other sites quote exactly. Not worth a finding.)

**The ignore baseline is exactly right.** Ran the checker rather than reading the file:

```
$ ./scripts/ci-check-ignored-tests.sh
ci-check-ignored-tests: 49 ignored test(s), matching the baseline
```

All six new entries correspond to real `#[ignore]`d tests
(`arca_engine.rs:62`, `:177`, `network.rs:433`, `read_rpcs.rs:108`, `shutdown.rs:439`,
`startup.rs:436`). Five of the six reasons name their actual requirements; the sixth is Minor 5
above. `startup.rs`'s "starts and stops 36 engines" checks out: `ROUNDS = 12` × three arms
(`startup.rs:118`).

**Every statistical claim in `shutdown.rs` recomputes exactly.** This is the densest numeric block
in the diff and all eleven numbers are right:

- `1324` is the smallest n clearing 1% at p = 1/288: n=1323 → 1.00340%, n=1324 → 0.99993%. ✓
- `ln(0.01)/ln(287/288) = 1323.985` ✓, and the first-order `-ln(0.01)/(1/288) = 1326.289` ✓,
  overshooting by two exactly as stated.
- 96 rounds at p = 1/288 → 71.6116% all-clean ✓ (the number that justifies the resize).
- λ = 1324/288 = 4.597 ✓; `P(X ≤ 1 | λ=4.597) = 5.64%` ✓.
- Pooled 2/1612 → p ≈ 1/806 ✓; `(1−1/806)^1324 = 19.3%` ✓.
- Unchanged rows too: `Untouched` 440 → 0.99778% ✓, 439 → 1.00834% ✓;
  `RemovedContainer` `(0.625)^32 = 2.94e-7` ✓.

The self-critical paragraph at `shutdown.rs:130-138` — which says the clean sweep of 1324 is
"consistent with fixed", not "99% sure" — is the correct reading of its own data.

**`network.rs` and the evidence doc agree.** Thirteen violations in both
(`network.rs:400`, evidence doc's list). The violation strings in the doc match the `format!`
templates at `network.rs:495-497`, `:511-516`, `:528-533` exactly, including the
"after a guest-root mutation" prefix. The probe commands match (`wget http://1.1.1.1/`,
`nslookup example.com`, `nc -w 3 -z`). The interface list from `ip -o link show` and the address
list from `ip -o addr show` are consistent with each other (only `lo` and `eth0` carry addresses).
The command's `-p gascan-arca --test live … network::` filter selects exactly one test — the other
baseline entry `network::offline_workspace_cannot_reach_external_or_host_networks` is in
`gascan-apple` — so "0 passed; 1 failed" is the right shape. The evidence doc's Gas Can anchor is
exact: `git log -1 --format='%H %P' fd05780` → parent `cd3e2a5904600bf034e904316366717870434b7d`.

**Repo-local anchors added in code all resolve.** Six were added inside `crates/`, `scripts/`,
`packaging/`; five are repo-local and four are exactly right:

- `policy.rs:417-427` — precisely the `if spec.manifest().network() == NetworkMode::Offline` block. ✓
- `policy.rs:419-425` — the three match arms it describes. ✓
- `service.rs:1569` — the first statement of `provision_with_applied`; and that statement is
  `initialize_managed_volume_roots`, whose first exec is
  `/usr/bin/sudo -n /usr/bin/install -d -o workspace -g workspace` (`service.rs:2150-2168`),
  exactly as `arca_common/mod.rs:44-46` says. ✓
- `gascan-apple/src/probe.rs:47` — `if self.version == minimum && self.commit == APPLE_1_1_COMMIT`. ✓
- `packaging/macos/release-smoke.sh:1015-1037` — Minor 1 above.

**Swift-side anchors resolve too**, against the pinned checkout at
`.artifacts/arca-engine/arca` (`git rev-parse HEAD` = `c545612b…`, the pinned revision):
`SandboxEngineService.swift:182` is `capabilities.buildRevision = ArcaVersion.buildRevision`;
`:190` is `capabilities.offline = .unverified`, so the evidence doc's ".unverified stays" is
observable; `EngineServer.swift:551` is `let path = socketPath + ".lock"`, which is what
`arca_common`'s `engine_pid()` reads. `ArcaEngineCommand.swift:132-137` carries verbatim the words
`crates/gascand/src/engine.rs:116-118` quotes as "the engine's own". The engine does have a `serve`
subcommand and it is the `defaultSubcommand` (`:139`), so `serve_arguments()` omitting it is
correct.

**`images/workspace/Dockerfile:142-143`** (`gascan-oci-fixture/src/lib.rs:62`) names all three of
`.local`, `.cache` and `.config`. ✓ **`packaging/macos/package.sh`** does say the engine is "a build
gate, not a payload" (line 83), as `gascan-core/src/backend.rs:88-90` claims. ✓
**`~83MB compressed`** = 9,092,349 + 73,739,738 = 82,832,087 bytes. ✓ The `9092349` in
`engine_artifacts.rs:325`'s example message is the real kernel asset size. ✓
**`DoctorCheckId` has exactly 21 variants** (`doctor.rs:26-46`), matching the `21` asserted in
`every_check_has_a_non_empty_remedy_under_every_backend`. ✓ Both `DoctorRemedies` impls match
exhaustively with no `_` arm. ✓ **The pin validates against its own schema**:
`jq -e --from-file engine/arca-pin-schema.jq engine/arca-pin.json` → true. ✓
**`a_supervised_child_dies_when_its_parent_stops_holding_the_pipe`** (cited at
`common/mod.rs:181`) exists at `supervision.rs:51` and is correctly absent from the ignore
baseline, because it is not `#[ignore]`d. ✓

**Tests I looked for tautologies in and did not find them.** Naming the one-line production change
that turns each red:

- `tests/release/engine-pin-contract.sh` — the strongest suite in the diff. Twenty new refusal cases,
  each editing exactly one field of a good pin. The `buildinfo-lies` case is the answer to "is the
  build-revision assertion load-bearing": the fixture's Makefile takes the revision as a parameter
  precisely so a *lying* generator is expressible, and the case then greps the refusal for the lie
  (`0123456789abcdef…`). Deleting the `[[ $built_revision == "$revision" ]]` check turns it red.
  The warm-cache assertion was *strengthened* rather than relaxed when regeneration made the
  checkout dirty — "exactly one tracked file differs and it holds the pinned revision" — which is
  the opposite of silencing a check to make a step pass.
- `crates/gascan-core/tests/engine_artifacts.rs` — `Fault::CorruptKernel` flips one byte without
  changing length, so only the digest can catch it; `Fault::TruncatedVminit` halves the file, so
  only the length check can. `a_pin_that_moved…` asserts `downloads().len() == 2`, which is what
  makes "detected, not re-fetched" a real claim. `a_layout_whose_index_was_edited…` replaces the
  blob while keeping the index's claim — deleting the `require_file` call at
  `engine_artifacts.rs:401-406` turns it red and nothing else does.
- `crates/gascand/tests/engine_supervisor.rs` — `adopts_a_listening_engine_without_spawning_a_second`
  fails if the `listening()` early return at `engine.rs:264` is removed;
  `a_socket_with_nothing_behind_it_is_treated_as_no_engine` fails if `ConnectionRefused` is dropped
  from the match at `engine.rs:212-215`; `a_socket_owned_by_another_user_is_refused_before_dialing`
  fails if `require_own_socket` at `engine.rs:262` is removed. The stale-socket test *waits* for the
  socket to begin refusing rather than assuming it, and says why — that is a fixture defending
  itself, not decoration.
- `crates/gascan-core/tests/doctor.rs::no_arca_remedy_names_apples_runtime` sweeps the whole
  report, not the five runtime checks, and would fail on any single Apple string reintroduced
  into `ArcaRemedies`.
- `crates/gascan/src/daemon.rs::a_daemon_on_another_backend_is_refused_and_left_running` — deleting
  `require_matching_backend` at `daemon.rs:2071` turns it red.

**Honesty I checked rather than assumed.** Four places in this diff mark a property as reasoned but
undefended, and in each case I confirmed the disclaimer is true rather than modest:

- `crates/gascand/src/engine.rs:275-283` — the exited-before-timeout check order. Confirmed: the
  fixture engine (`/usr/bin/false`) exits inside the first 5ms tick, so both orders pass.
- `crates/gascan-oci-fixture/src/lib.rs:541-546` — ancestor derivation. Confirmed: `tar` creates
  missing parents implicitly, so the assertions at `lib.rs:575-578` would still pass.
- `crates/gascand/tests/engine_supervisor.rs:79-86` — the argv test names itself a list-against-list
  comparison and points at the e2e as the real instrument. That is exactly what it is.
- `crates/gascan/src/daemon.rs` — the two negative assertions in the backend-mismatch test are
  marked as not having fired under the mutation, with the reason (`NeverSignaler` fails first).

**On `network::an_offline_sandbox_has_no_egress_at_either_privilege_level`.** Per the brief I did
not treat its by-design failure as a finding. Its docstring's numbers do match the evidence doc:
thirteen violations, revision `c545612b`, the same three mechanisms at two privilege levels before
and after mutation. Its ignore reason leads with "FAILS BY DESIGN", which is the right place for
that warning.

---

## Two claims I could not substantiate and am not calling findings

Recorded so they are not mistaken for verified.

1. `docs/evidence/2026-08-18-arca-engine-offline.md:117-122` — "MEASURED end to end — `gascan up`
   on an offline manifest, against a real `gascand` on a real engine, is refused". The instrument
   named (`an_offline_manifest_is_refused_because_no_engine_build_is_certified`) exists and is a
   real test, and it was added one commit earlier, so the claim is plausible. But unlike the network
   run beside it, this one carries no command, no exit code and no output — the section's other
   claims all do. The same measurement is asserted at `crates/gascand/src/api.rs:980-982` and
   `:3581-3583`, where the observed output (`Error: invalid_request`) *is* quoted, which is why I am
   not calling it bare.

2. `docs/evidence/…:57` — "Result: **FAILED**, `test result: FAILED. 0 passed; 1 failed`, in 9.68s."
   Two virtual-machine boots plus roughly twenty exec round-trips in 9.68 s is fast, but nothing in
   the repository contradicts it and `arca_engine.rs:22-26` describes a warm-store boot as "a few
   seconds". Not refuted; noted only because it is the one number in the doc I could neither
   recompute nor cross-check.


=====================================================================
# Reviewer: release
=====================================================================

# Whole-landing review — release, packaging, CI and repository hygiene

Repo: `/Users/kiener/code/gascan`, branch `feat/milestone-4-product-wiring`.

**Baseline note.** I started against `d9af25c`. While I was reviewing, HEAD advanced to
`fd05780` (`cd3e2a5`, `fd05780` landed mid-review) and another agent left uncommitted edits to
`crates/gascand/src/service.rs` and `crates/gascand/src/main.rs` in the shared working tree.
Every finding below was **re-verified against committed `fd05780`** with `git show HEAD:<path>`
after that move. Two verifications I could not complete because of the dirty tree are listed at
the end under "Not verified".

---

## Critical

### C1 — `package.sh` now emits an engine block that `verify-package.sh` rejects; every package build fails

`packaging/macos/package.sh:95` (was `{name, url, tag, revision}`):

```
engine_json=$(jq -cS '{name, url, tag, revision, artifacts}' "$repo_root/engine/arca-pin.json")
```

`packaging/macos/verify-package.sh:64` was not changed with it:

```
(.engine | keys == ["name", "revision", "tag", "url"]) and
```

`jq`'s `keys` is sorted, so the manifest `package.sh` now writes yields
`["artifacts","name","revision","tag","url"]` and the equality is false.

**Failure scenario.** `packaging/macos/package.sh:127` runs `verify-package.sh` on the package
it just built. It exits 65 with `build manifest is invalid`, so `package.sh` fails. The same
verifier gates `packaging/macos/install.sh:22`, `packaging/macos/publish.sh:45` and
`packaging/macos/release.sh:193`. Nothing can be packaged, installed from a package, published
or released from this branch.

**Why I believe it is real.** Reproduced, not inferred. I built the manifest the way
`package.sh` does from the real `engine/arca-pin.json` and ran `verify-package.sh`'s own jq
program over it:

```
$ engine_json=$(jq -cS '{name, url, tag, revision, artifacts}' engine/arca-pin.json)
  ... jq -e '<the program at verify-package.sh:56-71>' ...
verify-package manifest jq exit=1
$ printf '%s' "$manifest" | jq -c '.engine | keys'
["artifacts","name","revision","tag","url"]
```

**What would refute it.** A second copy of the key list somewhere that `verify-package.sh`
actually consults instead of line 64 — there is none; `grep -n 'keys ==' packaging/macos/verify-package.sh`
returns only line 64. Or a `verify-package.sh` change in a commit not on this branch.

**Related (folded in, same root):** nothing in `tests/release/*-contract.sh` builds a package
from the real pin, which is why the contract suite stays green. Both fixtures hand-write the
old 4-key block — `tests/release/installer-contract.sh:132` and
`tests/release/publish-contract.sh:206-213` — so they satisfy line 64 while the real packager
does not. `tests/release/clean-host.sh:7-8` does drive the real `package.sh`, but it is not a
`*-contract.sh`, so `scripts/ci-run-release-contracts.sh` never runs it.

---

### C2 — `gascan uninstall --remove-data` fails outright once `gascan engine fetch` has run

`crates/gascan-core/src/engine_artifacts.rs:433` creates the artifact root with

```
std::fs::create_dir_all(paths.root())?;
```

and the file contains no permission handling at all (`grep -in 'perm|chmod|umask|set_mode'`
matches nothing). Under the default macOS umask 022 that leaves the directory at **0755**.

`packaging/macos/uninstall.sh:117-119` validates the child it is about to remove with
`private=true`, and `gascan_uninstall_validate_directory_entry` (`uninstall.sh:44-47`) requires
**exactly 0700**:

```
gascan_uninstall_validate_directory_entry "./$child" "$parent/$child" "$expected_uid" true true || exit $?
/bin/rm -rf -- "./$child" || exit $?
```

`gascan_uninstall_remove_engine_data` (`uninstall.sh:145-155`) is called at `uninstall.sh:237`
as `... || exit $?`, so a 65 aborts the whole uninstall **before** the `sudo rm -f` block at
`uninstall.sh:239-246`. The binaries are not removed either.

**Failure scenario.** A user runs `gascan engine fetch`, later runs
`packaging/macos/uninstall.sh --remove-data`. It prints
`refusing unsafe uninstall path: .../dev.gascan/engine`, exits 65, removes nothing, and leaves
`/usr/local/bin/gascan{,d}` installed. `tests/release/clean-host.sh:43` and `:100` both call
`--remove-data`, so the clean-host gate breaks on any machine that has fetched artifacts.

**Why I believe it is real.** Two independent anchors.

1. On this machine, the directory `gascan engine fetch` created is 0755 while the daemon's
   sibling is 0700:
   ```
   $ stat -f '%Lp %u %N' "$HOME/Library/Application Support/dev.gascan"{,/engine,/controller}
   700 501 .../dev.gascan
   755 501 .../dev.gascan/engine
   700 501 .../dev.gascan/controller
   ```
2. I extracted `uninstall.sh:7-155` plus `gascan_user_engine_root` into a harness and ran it
   against a throwaway `$HOME` fixture (script kept at
   `.../scratchpad/probe.sh`, library at `.../scratchpad/unlib.sh`):
   ```
   engine mode=755
   refusing unsafe uninstall path: <fixture>/Library/Application Support/dev.gascan/engine
   A: remove_engine_data rc=65
   A: engine dir STILL PRESENT
   B: remove_engine_data rc=0        # after chmod 700
   B: engine dir removed
   ```

**What would refute it.** `engine fetch` chmodding its root to 0700 somewhere I did not find,
or a umask of 077 being guaranteed for the process. Neither holds: the file has no chmod, and
the observed on-disk mode is 0755.

---

### C3 — `gascan engine fetch` on a fresh machine creates `dev.gascan` at 0755 and the daemon then refuses to start, on every backend

Same `create_dir_all` at `crates/gascan-core/src/engine_artifacts.rs:433`. `paths.root()` is
`~/Library/Application Support/dev.gascan/engine`
(`crates/gascan-core/src/engine_artifacts.rs:180-190`), so on a machine where the daemon has
never run, `create_dir_all` also creates the **parent** `dev.gascan` — at 0755.

The daemon requires that parent to be exactly 0700:

- `crates/gascand/src/controller_state.rs:23` — `const DIRECTORY_MODE: u32 = 0o700;`
- `crates/gascand/src/controller_state.rs:3042-3051` — `validate_directory` with
  `private_mode` requires `mode == DIRECTORY_MODE`, else `ControllerStateError::Unsafe`.
- `crates/gascand/src/controller_state.rs:2603-2607` — `ensure_private_child_directory` chmods
  **only `if created`**, then validates. A pre-existing 0755 `dev.gascan` is therefore never
  corrected, only rejected.
- `crates/gascand/src/controller_state.rs:2452-2457` — that is the call used for `dev.gascan`.

**Failure scenario.** Fresh install. `gascan doctor` reports the boot artifacts missing and
names `gascan engine fetch` as the remedy (`crates/gascand/src/main.rs:760`). The user runs it.
`dev.gascan` is created at 0755. Every subsequent `gascan daemon start` — Apple backend
included, since this is the controller store and not engine-specific — fails with
`controller_state_unsafe`, and there is no remedy that repairs the mode.

**Why I believe it is real.** The refusal is already an asserted property of the codebase:
`crates/gascand/tests/controller_state.rs:174-181` sets `dev.gascan` to `0o755` and asserts
`error.code() == "controller_state_unsafe"`. And `create_dir_all`'s two-level mode is measured,
not assumed — I compiled a 10-line program calling `std::fs::create_dir_all` on
`<tmp>/Library/Application Support/dev.gascan/engine`:
```
755  .../Application Support/dev.gascan
755  .../Application Support/dev.gascan/engine
```
This is masked on the reviewer's machine only because the daemon happened to create
`dev.gascan` first here — which is exactly why it is invisible.

**What would refute it.** A guarantee that `gascand` always runs before `gascan engine fetch`
on a new machine. The opposite is documented: `cli.rs:453-457` explains that `engine` is handled
*before* the daemon connection precisely so it can be run when the daemon cannot start.

---

## Major

### M1 — `crates/gascan-oci-fixture` is not classified as an engine-area path, so live-tier changes skip the engine job

`scripts/ci-classify-paths.sh` has no case for the new crate; it falls through to
`scripts/ci-classify-paths.sh:79` (`crates/*|rust-toolchain.toml|proto/*`). Measured:

```
$ printf 'crates/gascan-oci-fixture/src/lib.rs\n' | ./scripts/ci-classify-paths.sh
rust=true
contracts=false
engine=false
```

The crate is a dev-dependency of the live tier (`crates/gascan-arca/Cargo.toml:22`) and supplies
every OCI layout the tier boots: `crates/gascan-arca/tests/live/common/mod.rs:8,355,357,386` and
`crates/gascan-arca/tests/live/network.rs:43,333`.

**Failure scenario.** A change confined to `crates/gascan-oci-fixture/src/lib.rs` breaks the
layouts the live tier builds. `engine` is false, the engine job is skipped, `gate`
(`.github/workflows/ci.yml:165+`, `if: always()`, accepts skipped) goes green, and the change
merges without the tier ever having run.

**Why I believe it is real.** This is the identical failure the script's own comment records
having already happened once, for `crates/gascan-arca/src/channel.rs`
(`scripts/ci-classify-paths.sh:41-62`): "editing crates/gascan-arca/src/channel.rs ... skipped
the engine job entirely, `gate` accepted the skip, and the change merged without the live tier
ever running." The fix applied then was to name the whole crate; the new crate was not given the
same treatment.

**What would refute it.** The fixture crate being unreachable from the live tier — it is not; it
is `pub use`d at `common/mod.rs:8`.

---

### M2 — `docs/status/START-HERE.md:174` states the daemon's engine environment as two variables; there are three

```
The daemon's own two are `GASCAN_ENGINE_BIN` and `GASCAN_ENGINE_SOCKET`, both undefaulted for
the same reason and both required when `GASCAN_ARCA_BACKEND` is set.
```

`crates/gascand/src/main.rs:326-328` requires three:

```
executable: required(gascand::ENGINE_BIN_ENV, "the engine executable")?,
socket:     required(gascand::ENGINE_SOCKET_ENV, "its socket")?,
state_root: required(gascand::ENGINE_STATE_ROOT_ENV, "its state root")?,
```

`GASCAN_ENGINE_STATE_ROOT` (`crates/gascan-core/src/backend.rs:77`) is new in this landing and
appears in exactly three files repo-wide (`backend.rs`, `gascand/tests/backend_selection.rs:57`,
`gascan-e2e/tests/arca_engine.rs:41`) — no doc anywhere records it.

**Failure scenario.** A successor follows the handoff, exports the two named variables, and the
daemon exits with `GASCAN_ARCA_BACKEND selects the Arca engine backend, so
GASCAN_ENGINE_STATE_ROOT must name its state root`. The doc gives no path and no hint.

**Why I believe it is real.** The claim is new in this landing —
`git show main:docs/status/START-HERE.md | grep "daemon's own two"` returns nothing (rc=1), and
the line is present at `fd05780`. It is the durable handoff doc, which is the deliverable this
project treats as load-bearing.

**What would refute it.** `state_root` being optional — it is not; `required()`
(`crates/gascand/src/main.rs:296-308`) returns `InvalidInput` on absence.

---

### M3 — the installer contract exercises engine-data removal only through its "absent" branch

`tests/release/installer-contract.sh:229-241` (`prepare_uninstall_roots`) creates and chmods
only the controller and runtime roots. No `engine` directory is ever created, so
`gascan_uninstall_remove_engine_data` always takes the `status == 2` (absent) early return at
`packaging/macos/uninstall.sh:152` and the contract passes without touching the new code path.

**Failure scenario.** This is the direct cause of C2 shipping undetected: a fixture built the
way `gascan engine fetch` builds the directory (i.e. `mkdir -p` with the default umask, 0755)
would have failed. Instead the contract green-lights removal logic it never runs.

**Why I believe it is real.** Read directly; `grep -n engine tests/release/installer-contract.sh`
matches only the hardcoded build-manifest `engine` key at line 132.

**What would refute it.** Another contract creating an engine artifact directory before calling
`--remove-data` — `grep -rn gascan_user_engine_root packaging/ tests/` shows the helper has
exactly one caller, `uninstall.sh`.

---

### M4 — the CI live-tier step now selects a test that fails by design, with nothing marking it

`.github/workflows/ci.yml:151-161` runs, in the engine job:

```
GASCAN_ARCA_ENGINE_BIN=$binary cargo test -p gascan-arca --test live --no-fail-fast -- --ignored
```

`-- --ignored` selects every `#[ignore]`d test in the tier — now **25** across 12 files
(`grep -c '^#\[ignore' crates/gascan-arca/tests/live/*.rs`), up from 21 on `main`. Two of the
four the landing adds are new modules wired into `crates/gascan-arca/tests/live.rs`
(`mod network`, `mod startup`), and one of them —
`crates/gascan-arca/tests/live/network.rs:433`,
`network::an_offline_sandbox_has_no_egress_at_either_privilege_level` — is marked "FAILS BY
DESIGN against the pinned engine".

**Failure scenario.** The engine job runs the step, the by-design failure is reported as a
failure, the job is red, `gate` requires the job to have succeeded or been skipped, and the
required check blocks the merge — with the only explanation living in a Rust doc comment.

**Why I believe it is real.** The step has no filter and no allowance for expected failures; it
is a bare `cargo test ... -- --ignored` and its status is the step's status.

**Important qualifier, stated so this is not overweighted.** The step is *already* non-viable on
`main` for a related reason: every live test resolves `GASCAN_ARCA_KERNEL_PATH` and
`GASCAN_ARCA_VMINIT_LAYOUT` through `required_path`
(`crates/gascan-arca/tests/live/common/mod.rs:148`), which **panics** on absence and is
explicitly documented as "absence is a panic and never a skip". The CI step sets only
`GASCAN_ARCA_ENGINE_BIN`. So this landing compounds an existing red rather than introducing the
first one. It is filed Major and not Critical for that reason, but the by-design failure is new
and unmarked.

**What would refute it.** A CI-only environment file or runner-level export of the kernel /
vminit / base-layout variables — `grep -rn GASCAN_ARCA_KERNEL_PATH .github/` finds none.

---

## Minor

### m1 — `.github/workflows/ci.yml:138-141` states a measured live-tier count that is off by 21

> "`running 4 tests` is the expectation: four `#[ignore]` attributes exist across
> `crates/gascan-arca/tests/live` (2 in connect.rs, 2 in read_rpcs.rs) ... MEASURED at this
> revision: ... `2 passed; 0 failed; 4 ignored`."

Actual at `fd05780`: 25 `#[ignore]` attributes across 12 files in that directory. The comment
labels itself "a reader's orientation, not a gate", so nothing breaks — but it is a MEASURED
claim in a durable file that is now false. Already stale on `main` (21); this landing widens it
to 25 without touching `.github/workflows/ci.yml` at all (`git diff main...HEAD -- .github/` is
empty).

**Refuted by.** Nothing; `grep -c '^#\[ignore'` is the derivation.

### m2 — `docs/status/START-HERE.md:143` says the ignored-test baseline is 46; it is 49

`tests/ci/expected-ignored-tests.txt` has 49 lines at `fd05780`, and a static count agrees:
`git grep -h '^\s*#\[ignore' HEAD -- 'crates/*/tests/*' 'crates/*/src/*' | wc -l` → 49. The
handoff names a gate's number and gets it wrong by three.

Note the **baseline file itself is correct** — 49 recorded, 49 attributes in source. Only the
prose is stale.

### m3 — a corrupt pin *schema* is reported as a malformed pin *file*

`scripts/build-arca-engine.sh:41` and `scripts/sync-arca-proto.sh:51` both do

```
jq -e --from-file "$pin_schema" "$pin_file" >/dev/null 2>&1 || {
  printf 'engine pin file is malformed: %s\n' "$pin_file" >&2
  exit 64
}
```

A syntactically broken `engine/arca-pin-schema.jq` makes jq exit 3, which is caught by the same
`||` and reported as the *pin* being malformed. Measured:

```
$ printf 'this is not ( valid jq\n' > bad.jq
$ jq -e --from-file bad.jq engine/arca-pin.json >/dev/null 2>&1; echo $?
3
```

The existence check at `build-arca-engine.sh:37-40` / `sync-arca-proto.sh:47-50` covers a missing
schema but not a broken one. Exit code is still 64, so only the message misleads. Low
likelihood, cheap fix (distinguish rc 3).

### m4 — `Pin` reads `schema` and never enforces it

`crates/gascan-core/src/engine_artifacts.rs:75` has `pub schema: u32` and nothing compares it to
2 — a third parser of `engine/arca-pin.json` beside the two shell consumers, and the one that
does *not* consult `engine/arca-pin-schema.jq`. A schema-1 pin does fail closed (serde rejects
the missing `artifacts`), and a bump would be caught by
`crates/gascan-core/tests/engine_artifacts.rs:385` (`assert_eq!(pin.schema, 2)`), so the residual
risk is only a future schema 3 that is structurally compatible but semantically different.
Filed Minor for that reason.

---

## Things I checked that are sound (so nobody re-derives them)

- **The two scripts agree on what a valid pin is.** Both load the same non-overridable
  `engine/arca-pin-schema.jq` (`build-arca-engine.sh:9-11,37-44`,
  `sync-arca-proto.sh:23-25,47-54`), both guard its existence, both exit 64. The build gate
  cannot pass while verifying nothing: `jq -e` makes `false`/`null` exit 1.
  `jq -e --from-file engine/arca-pin-schema.jq engine/arca-pin.json` → `true`, exit 0.
- **Exit codes are preserved and extended coherently** in `build-arca-engine.sh`: 69 for the new
  `make` prerequisite, 70 for a missing/failed generator, 65 for a build revision that is not the
  pinned revision — and 65 is the same class as a tag resolving to the wrong commit, which is
  consistent. `make` runs only after `verify-tag refs/tags/<tag>`, the tag-target assertion,
  `checkout --detach --force`, `clean -qffdx` and the submodule clean
  (`scripts/build-arca-engine.sh:96-134`), so it never executes an unverified Makefile.
- **`tests/release/engine-pin-contract.sh`** was moved to schema 2 throughout, adds 18 refusal
  cases for the artifact block, adds the schema-1 refusal (the case that fails if only one of the
  two scripts is migrated), and replaced the now-invalid `git diff --quiet` warm-cache assertion
  with a strictly stronger one (exactly one tracked file differs, it is the generated build info,
  and it holds the pinned revision). No assertion in it reads false to me.
- **Release input tracking covers the new files.** `engine/arca-pin-schema.jq` was added to the
  tracked list (`packaging/macos/release-common.sh:41-43`) and to the ignored-source extension
  set (`...|json|jq)$/`, `release-common.sh:52-55`), and `tests/release/source-input-contract.sh:12,20`
  seeds and classes it. `crates/gascan-oci-fixture` needs no entry: `crates` is covered as a
  directory at `release-common.sh:30-35` and `:53`.
- **The new crate does not change the release inputs or the dependency surface.** `Cargo.lock`
  adds only the workspace member; every external dep it pulls (`camino`, `serde_json`, `sha2`,
  `tempfile`) was already in the lock. `cargo metadata --locked --offline --no-deps` exits 0, so
  the lock is in sync. Version selection in `package.sh:25`, `publish.sh:18` and
  `release-gates.sh:22` is `select(.name == "gascan")`, so a new member cannot displace it.
- **`scripts/ci-run-release-contracts.sh`** globs `tests/release/*-contract.sh` and
  `tests/ci/*-contract.sh`, so no enumeration needs updating.
- **The cask is correctly left alone.** `packaging/macos/render-cask.sh` removes no per-user
  state today, and `uninstall.sh:141-144` records the decision not to make the artifacts the one
  exception. `tests/release/cask-contract.sh` passes.
- **Contracts I ran, all green:** `tests/release/documentation-contract.sh`,
  `tests/release/source-input-contract.sh`, `tests/release/cask-contract.sh`,
  `tests/release/source-signature-contract.sh`, `tests/ci/classify-paths-contract.sh`.
- **`~/Library/Application Support/dev.gascan/engine/` has exactly one path assumption outside
  Rust** — `gascan_user_engine_root` at `packaging/macos/release-common.sh:13-15`, whose only
  caller is `uninstall.sh`. Nothing else assumes it.
- **`SystemTools` uses absolute system paths** (`/usr/bin/curl`, `/usr/bin/gunzip`,
  `/usr/bin/tar`, `engine_artifacts.rs:264,280,290`), all present on macOS. No new packaging
  prerequisite.

## Gaps in the contract suite this landing made assertable but did not assert

Beyond M3 (engine-data removal) and C1's fixture problem:

- **`gascan engine fetch` is a new user-facing subcommand** (`crates/gascan/src/cli.rs:100-105,
  123-130`) and `tests/release/documentation-contract.sh` requires nothing about it. `README.md`
  does not mention the engine, the command, or the artifact paths
  (`grep -n 'engine\|Arca' README.md` matches nothing). The contract enumerates every `gascan
  daemon` subcommand by hand (`documentation-contract.sh:88-91`); the same treatment for
  `gascan engine fetch` is now assertable and absent.

## Not verified

- **`cargo test --workspace` / `cargo clippy --workspace --all-targets` /
  `scripts/ci-check-ignored-tests.sh`.** I ran `cargo test --workspace -- --ignored --list` and it
  failed to compile `gascand`'s `doctor_state` test (6 errors, E0061/E0308). **That failure is
  not this landing's** — `git status --porcelain` showed another agent's uncommitted edits to
  `crates/gascand/src/service.rs` and `crates/gascand/src/main.rs` adding a
  `remedies: &'static dyn DoctorRemedies` parameter, with `crates/gascand/tests/doctor_state.rs`
  mid-edit. The committed `HEAD:crates/gascand/src/service.rs:313` still reads
  `pub fn pending() -> (Self, DoctorCompleter)`. I did not stash or check out to get a clean
  build, because that would have destroyed work in progress. **These three CI steps remain
  unverified for this branch.** I substituted a static count for the baseline check (49 recorded,
  49 `#[ignore]` attributes in source — they agree).
- **`tests/release/engine-pin-contract.sh`, `installer-contract.sh`, `publish-contract.sh`,
  `release-script-contract.sh`.** Not run. The first creates signed git tags (`git tag -s`) and
  drives a Swift build; the others drive `uninstall.sh` and `sudo` stubs. Running them would have
  needed the signing key and could have mutated machine state. C2 was verified instead by
  extracting the uninstall functions into an isolated harness, which is a narrower but sufficient
  check for that finding.
