# START HERE

This file is the session entry point. It is written to be read cold, and it is
addressed to you, the agent. Follow it as instructions — there is nothing to paste.

Written 2026-08-11 for two branches in flight; rewritten 2026-08-12 after both merged;
rewritten 2026-08-13 mid-milestone-2; rewritten again 2026-08-13 (evening) with **Landing 4
complete and Landing 5 the whole of what remains**; updated 2026-08-13 (late) after Task 13's first
hour, which **proved the spawn, got `Create` working end to end for the first time, and cost two
unplanned Arca fixes on the way** — Tasks 13a and 13b.

---

## Where the work is

**P5.1 milestone 2: LANDINGS 1-4 ARE COMPLETE. Only Landing 5 remains.** Twelve tasks, every one
reviewed. **Nothing is merged and nothing is pushed. Do not start a new branch; continue on these two.**

| | |
|---|---|
| Arca | `feat/engine-state-ownership`, HEAD `9db2f7d`, based on `cc316b65` — 37 commits |
| Gas Can | `docs/p5-1-milestone-2-design`, based on `6847d1e` — **HEAD is whatever commit last touched this file**, so read it with `git log -1`, do not trust a SHA written here — 27+ commits |
| Design | `docs/superpowers/specs/2026-08-12-p5-1-milestone-2-engine-lifecycle-design.md` |
| Plan | `docs/superpowers/plans/2026-08-12-p5-1-milestone-2-engine-lifecycle.md` |
| Parent design | `docs/superpowers/specs/2026-08-10-p5-1-engine-service-and-wiring-design.md` |
| Governing roadmap | `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md` — P0-P8; P0-P4 done, P5 current |

**Both suites were run and verified on 2026-08-13 evening, both trees clean:**

| | |
|---|---|
| `swift test --filter ArcaEngineTests` | `Executed 147 tests, with 0 failures` |
| `swift test --filter ArcaTests.NetworkPruneGateTests` | `Executed 3 tests, with 0 failures` |
| `env -u RUSTUP_TOOLCHAIN cargo test --workspace --no-fail-fast` | exit 0 — **1435 passed / 0 failed / 26 ignored across exactly 74 targets** reporting `0 filtered out` |

### FIRST THING TO DO

**Nothing is mid-flight. There is no review to collect and no fix round open.** Task 11 closed after
four fix rounds; Task 12 passed review with no fix round. Both verdicts are in the ledger.

**Start by expanding nothing and reading two things:** the milestone-2 design, then the plan's
**Landing 5 expansion** (committed at Gas Can `4e40438`, amended by `f8c5ca1` and `1726c77`). Landing 5
is already stepped out — **do not re-expand it** — and it is the only landing left.

The design records why the engine owns a private state root and why that made `initialize()` safe when
milestone 1 had rejected it. The plan carries the landings; 3, 4 and 5 were all expanded *after* the
task that preceded them ran, so they reflect what the machine actually does rather than what the code
appeared to say.

### THE ONE THING THAT MATTERS MOST ABOUT THE CURRENT STATE

**UPDATED 2026-08-13 (late). `Create` NOW WORKS END TO END, AND IT DID NOT BEFORE.** The sentence this
section led with for two days — "nothing has ever been executed end to end" — is retired for `Create`
and still true for everything after it. Measured by the controller from Gas Can's live tier against a
branch-built, ad-hoc-signed engine: a real engine on a real socket, its store seeded with `arca-engine
image load`, then `PrepareImage` → `Ok` and `Create` → `Ok` with three volumes, a network and **a
container**. That was the first container this engine has ever created, and closing it took two
unplanned Arca fixes (Tasks 13a and 13b below).

**`Start`, `Stop` and `Remove` HAVE STILL NEVER RUN.** Read the rest of this section as still true of
them. Each measured rather than assumed:

- **`Start` is entirely unproven.** `startContainer` is unreachable without a VM — it throws
  `notInitialized` first. The one test that drives it says so in its own name
  (`testStartOfAnOwnedSandboxReachesStartContainerAndStopsOnlyForWantOfAVM`).
- **Port publishing has three silent gates** and is provable only from the live tier.
- **`Remove` of a RUNNING container is untested** — that state is unreachable VM-free, because
  `loadPersistedState()` recovers every persisted `running` row as exited/137 first.
- ~~**The live tier cannot spawn the branch engine at all**~~ — **FIXED and proven**, Gas Can `776a71c`.
  It passes all four options now, with `--kernel-path` and `--vminit-layout` arriving as
  `GASCAN_ARCA_KERNEL_PATH` and `GASCAN_ARCA_VMINIT_LAYOUT`.
- **Every capability flag is still `false`.** The engine claims nothing it has not earned.

**Landing 5 exists precisely to close this gap, and Task 13 is a step change in kind, not just the next
item.** It is the first task needing a real engine process, a real kernel and a real VM.

The three RPCs still answering `unsupported_capability` are **`CreateContainer`, `Exec` and `Logs`**
(`SandboxEngineService.swift:636`, `:988`, `:998`). `Exec` and `Logs` are milestone 3's.

### What milestone 2 has landed

| Task | Arca | Gas Can |
|---|---|---|
| 1 `ContainerManager` takes its storage roots | `8fd2757`..`1ff4304` | — |
| 2 `listContainers` gains `includeInternal` | `bd80701`..`4b34bfc` | — |
| 3 `listNetworks` throws | `8b3e16f`..`1201f4a` | — |
| 3b the prune-gate swallows | `493e5ce`..`1c6a851` | — |
| 3c the gate runs DockerAPI-side tests | `fede19c` | `142d199`..`cd00388` |
| 4 the three path options | `b93ef76`..`029c01d` | — |
| 5 vminit into the engine's own store | `e1b5d9a`..`595a450` | — |
| 6 `initialize()` before serving | `a0796c4`..`85b5023` | — |
| 6b sign the engine | `014c84b`..`db6bedc` | `a45edd4`..`c8e2c5b` |
| 7 `Inspect` | `db6bedc`..`40078e7` | — |
| 8 `ListResources` | `40078e7`..`40a1d55` | — |
| 9 `image load` subcommand | `40a1d55`..`65650b2` | `0fa74fe` |
| 10 `PrepareImage` | `65650b2`..`05b909a` | — |
| 11 `Create` — closed after 4 fix rounds | `05b909a`..`7511957` | — |
| 12 `Start`, `Stop`, `Remove` — passed review, no fix round | `7511957`..`9db2f7d` | `1726c77` |
| 13 the live tier's spawn, proven | — | `776a71c` |
| 13a the create path's container directory | `9db2f7d`..`1a78ef3` | — |
| 13b the container log directory | `1a78ef3`..`5e52aae` | — |

**Tasks 3b, 3c, 6b, 13a and 13b were not in the approved plan.** Each was added on a maintainer ruling
after a review — or, for 13a, a controller spike — found something real: a `try?` that let
`docker network prune` delete an in-use network, a gate that could not see the test guarding it, an
engine that could not start a container because nothing signed it, a hardcoded container directory that
broke `Create` and wrote into Apple's shared store, and a log directory that did the same to
ArcaDaemon's. **The last two are both Task 1 misses of the same shape** — a path that should have been
rooted when `ContainerManager` gained its roots and was not.

**`ContainerBridge` now derives no state root it was not given.** That claim is Task 13b's and is worth
re-running rather than trusting, with the greps recorded in its ledger entry.

**Task 11 cost four fix rounds and every one was the same defect** — a claim that outran the code.
Seven instances across commit messages, source comments and reports; **every one caught by a reviewer
running a mutation, none by reading.** Task 12 shipped none, which is the first task this milestone
that did not. The rule that changed it is in the traps section below.

### The milestone's thesis, and that it now holds

Milestone 1 rejected calling `initialize()` because `ContainerManager`'s restore loop **writes** —
a persisted `running` container is marked exited/137 and written back (`ContainerManager.swift:317`,
write at `:333`). Against a state root shared with a live `ArcaDaemon` that declares the daemon's
containers dead.

**That hazard belongs to sharing a root, not to writing.** The engine now owns
`~/Library/Application Support/dev.gascan/engine/`, and against its own root the same write is
correct. VERIFIED by running it: the isolation probes —
`/usr/bin/find ~/.arca -newermt '-5 minutes'` and the same over
`~/Library/Application Support/com.apple.containerization` — came back **empty**, three separate
times, cross-checked with `-newer <marker>`. The engine built its own 512MB `initfs.ext4` inside
its own state root and never touched Apple's.

**THAT MEASUREMENT IS NARROWER THAN IT READS, AND CREATING A CONTAINER BROKE IT — 2026-08-13.** Those
probes ran when **no container had ever been created**, so they measured startup and nothing else. The
moment a container was created, two paths wrote outside the state root, and both are now fixed:

- **The container directory.** `ContainerManager.swift:1144` hardcoded
  `~/Library/Application Support/com.apple.containerization` while `getRootfsPath` derived the same
  directory from `manager.imageStore.path`. Task 1 moved one and not the other. **This also broke
  `Create` outright**, with `NSPOSIXErrorDomain Code=2` — Apple's manager, rooted at the private store,
  found no directory. Fixed in Arca `9b29399`, adjudicated in `1a78ef3`.
- **The container log directory.** `ContainerLogManager` derived
  `~/Library/Application Support/com.apple.arca/logs` for itself and took no root at all, so every
  engine wrote container stdout/stderr into ArcaDaemon's one shared directory **and deleted out of it on
  remove**. Fixed in Arca `5e52aae` (Task 13b).

**The re-measurement, and it is the one to quote:** with the fix, creating a container leaves Apple's
shared `containers/` directory count **unchanged (253 → 253)** and puts the directory under the
engine's own `<state-root>/images/containers/`. With `manager.imageStore.path` swapped for
`ImageStore.default.path`, the same run drives the count **253 → 254**, leaves the engine's own
directory empty, and fails `Create` — while **`swift test --filter ArcaEngineTests` stays at 149
passing**. **That pair is the whole argument for the live tier: Arca's suite cannot see this, and
nothing in that repository can.**

**The vmnet `host` network does not collide.** Two concurrent engines on different state roots
took `192.168.93.0/24` and `192.168.95.0/24`, both listening, allocation released on exit. The
`host` name is a row in each engine's own `state.db`, and `VmnetNetworkBackend`'s `isDefault`
guard reads a per-instance dictionary — not a host-wide namespace. **Limit:** no live `ArcaDaemon`
was run alongside; that case is inference from the identical code path, not observation.

### What shipped

**Arca** (`~/code/arca`, now on `main`) — seven task commits `bc03394..e74aff0`, a
dependency fix `8fc1ca5`, a comment fix `f5fde96`, and two answering the adversarial
review: `16abeec` (Inspect and ListResources) and `b3390b8` (the socket path and the
shutdown path). **30 tests pass** (`swift test --filter ArcaEngineTests`, exit 0); it was
27 before the review added three to `EngineServerTests`.

**Gas Can** (`~/code/gascan`, now on `main`) — design and plan (`33d37f9`, `4981b39`,
`77ff591`, `b36d18f`), Tasks 8-11, then the review wave `140b274..351a646`.

| Task | Commit | What |
|---|---|---|
| 8 | `f75d069`, `ddb4f6a` | `scripts/build-arca-engine.sh` builds the engine product, runs its tests in the verified clean checkout, and prints the binary path as a second stdout line |
| 9 | `cb81024` | the live harness — a real engine on a real socket, and the `connect` error paths |
| 10 | `2fe3711` | live coverage of the read RPCs — since replaced, see below |
| 11 | `c0e0cc8`, `aebf558`, `fb50d4c` | `tests/release/engine-targets-check.sh` — neither `arca-engine` nor `ArcaEngine` reaches `DockerAPI` or `ArcaDaemon` |

**1435 tests pass, 0 fail, 26 ignored** across 74 targets reporting `0 filtered out`
(`cargo test --workspace --no-fail-fast`, exit 0). It was 28 ignored: the live tier's
`Inspect` and `ListResources` tests folded into one that covers all ten unimplemented
methods, so the tier went from 8 tests / 6 ignored to 6 / 4, and nothing else moved.

### What the engine actually does

**As of `9db2f7d` EIGHT of the eleven are implemented: `Capabilities`, `Inspect`, `ListResources`,
`PrepareImage`, `Create`, `Start`, `Stop` and `Remove`.** There is also an
`arca-engine image load --state-root <R> --oci-layout <L>` subcommand. **Only `CreateContainer`, `Exec`
and `Logs` answer `unsupported_capability`** (`SandboxEngineService.swift:636`, `:988`, `:998`);
`Exec` and `Logs` are milestone 3's.

**Read the "NOTHING HAS EVER BEEN EXECUTED END TO END" section at the top before you believe any of
that means the engine works.** Eight implemented means eight that return the right answers to VM-free
unit tests.

**Three things Landings 3-4 established that change what you should believe:**

1. **`Inspect` reports what the STORE holds, deliberately** — including port bindings that were
   never published, because that is what drift detection compares against. **So `Inspect` can never
   be evidence that anything was actually done.** An engine can report a successful `Create`, a
   successful `Start`, and an `Inspect` naming a port, while publishing nothing, with every check
   green.
2. **`ListResources` reports unlabelled and internal resources with `owner` unset**, while `Inspect`
   *refuses* an unlabelled container as `foreign_resource_refused`. That looks inconsistent and is
   not: one reports what is held, the other answers about a specific claimed sandbox. **Do not
   "fix" the difference.**
3. **`ImageManager.resolveImage` was widened** to accept `repository@sha256:<hex>`, because
   `createContainer` uses ONE string both to resolve and to *record*, `startContainer` re-resolves
   that recorded string after a restart, and `Inspect` must parse it as a digest reference. One
   field, three constraints, and the third forces the form the first two rejected. **This changed
   Arca's Docker surface**: `docker run|rmi|inspect repo@sha256:...` now works where it threw.
   Deliberate, accounted for in `ba1900f`'s and `de8c880`'s messages.

The older text below described the milestone-1 state and is kept for its reasoning about *why*
`Inspect` and `ListResources` were once refused.

**`Capabilities` WAS the ONE implemented method. The other ten answered
`unsupported_capability`.** The engine runs — VERIFIED by running it: `arca-engine
--socket-path … --state-root …` logs `engine listening` and creates the socket
`srw-------`, and Gas Can's live tier drives all eleven over a real socket.

`Inspect` and `ListResources` were counted as real until 2026-08-12 and are not. The
process calls `initialize()` on no manager, so each could return exactly one answer under
every input — `absent`, and an empty list. Answering `absent` without having looked is
what makes a reconciler create a duplicate of a running sandbox; an empty `ResourceList`
is a confident report of a clean host, which is the report that hides a leak. Both now
refuse instead. The reasoning is on each method in `SandboxEngineService.swift` and in
`ArcaEngineCommand.run()`.

## What to do next — Landing 5, and Task 13 first

**Landing 5 is already expanded. Do not re-expand it.** Gas Can `4e40438`, amended by `f8c5ca1` (the
image-load seam) and `1726c77` (Task 12's routed findings). Read that section of the plan and follow it.

Remaining: **Task 13** the live tier, **Task 14** the capability flips, **Task 15** the workspace suite
run alone.

### Task 13's first hour is DONE. Both steps came back, and step 2 cost two Arca fixes.

**Do not redo this.** Steps 1 and 2 below are complete and recorded; step 3 is what remains.

1. **DONE — the spawn works**, Gas Can `776a71c`. `connect::a_real_engine_accepts_the_placeholder_authority`
   → `1 passed` against a branch-built engine. Seen to fail: dropping the two new arguments gives
   `Missing expected argument '--kernel-path'`, `engine exited with exit status: 64`, reported in 0.06s
   by `await_socket`'s `try_wait()`.
   **The module prefix in the filter is load-bearing** — `-- --ignored <bare name> --exact` reports
   `running 0 tests` and exits 0. Use `connect::<name>`.
2. **DONE — `image load` works, and `Create` now works, but only after Tasks 13a and 13b.**
   `arca-engine image load --state-root <R> --oci-layout <L>` → exit 0, no kernel, no socket. The layout
   was built with the installed `skopeo`:
   `skopeo copy --override-os linux --override-arch arm64 docker://docker.io/library/alpine:3.20
   oci:/tmp/alpine-oci:alpine:3.20`. Then `PrepareImage` → `Ok`, `Create` → `Ok`.
   **THE DIGEST A REQUEST MUST NAME IS THE STORE'S, NOT THE LAYOUT'S.** The store re-wraps what it
   loads: the layout's `index.json` carries manifest `sha256:45e09956…` and
   `<state-root>/images/state.json` records `alpine:3.20` → `sha256:a019d0ba…` as an image **index**. A
   test deriving the digest from the layout it loaded names content the store does not hold. Read
   `state.json`. `policy_request_for_image` (`common/mod.rs`) is the seam, over
   `PolicyCompiler::compile_for_image`.
3. **What remains**: the lifecycle tests, the partial-failure case, and the published-port test.

**Two things step 2 turned up that change Task 13's remaining shape:**

- **`read_rpcs::every_unimplemented_method_answers_unsupported_capability_not_a_transport_fault`
  ALREADY FAILS** against the branch engine — `Inspect: None` at `read_rpcs.rs:80`, because it calls
  `expect_err` and the branch engine now implements `Inspect` and answers `absent`. The test asserts its
  own count is 10 precisely so this would fire, and says so in its own doc comment. **The count is now
  3**: `CreateContainer`, `Exec`, `Logs`. Rewriting it is Task 13's.
- **`CreateRequest` CARRIES NO ARGV, so the published-port test cannot supply its own responder.**
  `engine.proto:254-271` has no command or entrypoint field, and
  `SandboxEngineService.swift:356-357` passes `entrypoint: nil, command: nil` **deliberately** — the
  image's own config decides what runs. The environment is no way in either:
  `policy.rs:246` sets it from `guest_environment()`, a fixed map, so a manifest cannot inject a port.
  **The plan's Landing 5 expansion assumed `gascan-apple`'s `guest_argv` technique transfers with only
  the port changed. It does not.** Maintainer's ruling 2026-08-13: **the tier builds its own OCI layout**
  — reserve a port by binding `127.0.0.1:0`, then write a layout whose config carries
  `Cmd = sh -c '…nc -l -p <reserved>'`. That keeps the ephemeral-port reservation and needs no network
  at test time.

**The two artifacts the spawn needs are host state that neither repo produces:**

| Option | Path on this machine | What |
|---|---|---|
| `--kernel-path` | `~/.arca/vmlinux` | symlink into an installed `Arca.app`, 28,248,576 bytes |
| `--vminit-layout` | `~/.arca/vminit` | an OCI layout, 178 MB |

Pass them the way `GASCAN_ARCA_ENGINE_BIN` is passed — environment variable, absent means `panic!` with
a directive message. **No hardcoded `~/.arca`, no guessing fallback.**

### Three things that will decide Landing 5, established by measurement

1. **~~THE LIVE TIER CANNOT SPAWN THE *BRANCH* ENGINE~~ — FIXED 2026-08-13, Gas Can `776a71c`.** The
   diagnosis below is kept because its *consequence* still stands: `build-arca-engine.sh` builds the
   pin, the pin bump belongs to milestone 4, and **CI still cannot run Task 13's tests this milestone.**
   `#[ignore]` remains the correct state, and "run the tier at least once" still means a local run
   against a branch build, recorded with its command and output.
   **THE LIVE TIER CANNOT SPAWN THE *BRANCH* ENGINE. IT SPAWNS THE PINNED ONE FINE.**
   **CORRECTED 2026-08-13** — earlier text here said "cannot spawn an engine, and has not since Task
   4", unqualified, and that is too strong in a way that changes Task 13's shape.
   `crates/gascan-arca/tests/live/common/mod.rs:79-86` passes only `--socket-path` and
   `--state-root`. At the **pinned** revision `b3390b8` the engine declares exactly those two plus
   `--log-level` — three `@Option`s, the last with a default
   (`git show b3390b8:Sources/arca-engine/ArcaEngineCommand.swift`). **So the tier spawns the pinned
   engine correctly, and the recorded 4/4 live pass measured a working tier.** On the branch,
   `ArcaEngineCommand.swift:68-84` makes `--kernel-path` and `--vminit-layout` required; measured
   against both the pre- and post-Task-9 branch binaries: `Missing expected argument
   '--kernel-path'`, exit 64.
   **Nobody noticed because every live test is `#[ignore]`d, so nothing runs them** — a tier that
   cannot start its subject and a tier nobody runs look identical from outside.
   **The consequence for Task 13:** `build-arca-engine.sh` builds the pin, the pin bump belongs to
   milestone 4, and the pinned engine has no lifecycle RPCs — so **CI cannot run Task 13's tests this
   milestone at all**, and `#[ignore]` is the correct state rather than a quarantine. "Run the tier at
   least once" means a **local run against a branch build, recorded with its command and output**.
   The spawn also needs two artifacts nothing in the repo produces: `~/.arca/vmlinux` (a symlink to an
   installed `Arca.app`, 28,248,576 bytes) and `~/.arca/vminit` (a 178 MB OCI layout). Landing 5's
   expansion carries the rest.
2. **PORT PUBLISHING HAS THREE SILENT GATES, and Task 11 closes only the first.**
   (a) `portMapManager == nil` — now wired; (b) `getWireGuardClient` returns nil and the `if let`
   around the publish has **no `else`** — it returns nil when the container is on no WireGuard
   network; (c) the `catch` swallows by design ("Don't fail container start on port mapping errors")
   and the container is still marked running. Gate 2 is passable: `createDefaultNetworks()` makes a
   WireGuard-backed `bridge` (`isDefault`) and a vmnet `host`, and auto-attach fires for
   `networkMode` empty/`default`/`bridge`, skipping `none`/`host`. **So an offline sandbox with ports
   publishes nothing** — Task 11 refuses that combination rather than accepting it.
   **Publication is provable ONLY from the live tier**: `publishPorts` takes a non-optional
   `WireGuardClient` built against a booted VM, and the one VM-free path that reaches the gate is a
   no-op that would pass with the setter unwired. Task 11 deliberately did not write that test.
   **Task 13's shape, already worked out:** create a sandbox with a `PortMapping`, `Start`, then
   connect to `127.0.0.1:<host_port>` from the test process. Nothing weaker distinguishes a
   published port from a stored binding.
3. **TASK 14 MAY NOT FLIP `loopback_publish` UNTIL THAT TEST EXISTS AND PASSES.** A flag whose
   machinery is unproved is a claim with no instrument, and here the machinery has three ways to
   silently do nothing.

### Two contract defects for milestone 4's design pass

**1. The contract permits a combination no engine can honour.** `engine.proto`'s `Network` is a `oneof`
of `offline`/`networked_name`, and `ports` is a separate `repeated` field on `CreateRequest`, so
offline-plus-ports is expressible and nothing says which wins. Task 11 refuses it with
`unsupported_capability`. **The proto and the design should say what happens rather than leaving each
engine to decide** — this is feedback for milestone 4, and it dies in a Swift comment otherwise.

Sharpened 2026-08-13: **three components already agree** — the proto permits it, Gas Can's
`compile_ports` (`crates/gascan-core/src/policy.rs:436-438`) refuses it as `OfflinePortsForbidden`
before a request is built, and Arca's engine refuses it. Only the proto is silent. That is an argument
for writing the rule down, not for leaving it.

**2. `Remove` cannot report a partial deletion, because `AckResponse` has nowhere to put one.** Found by
Task 12's review. `CreateFailed` carries `repeated Resource created = 1` precisely so a partial create
does not leak with nothing knowing to look for it. **`AckResponse` is a bare `oneof { Ack ok; EngineError
error }`** — so a `Remove` that deletes the container and then fails on the volume is indistinguishable
on the wire from one that did nothing, and the consumer's recorded state diverges from reality with no
signal. `RuntimeBackend::remove` returns `Result<(), RuntimeError>`, so Gas Can could not carry it
either.

**Severity is bounded, which is why it is not a blocker:** Task 12's `Remove` validates *every* resource
before deleting *any*, so authorisation failures delete nothing; only a mid-deletion manager failure
produces a partial; nothing retries `Remove` (all six `gascand` call sites are single-shot); and
`ListResources` plus reconcile can rediscover the truth. **Do not fix it in the engine — it is a contract
change.**

**Four things Task 6 found that change the remaining work.** They are in the plan; they are
repeated here because missing one is expensive:

1. **Landing 3 seeds through `loadPersistedState()`, never a stub.** It is `package func`,
   VM-free, needs no entitlement, and is the only writer of `ContainerManager.containers`
   reachable without a kernel. `Tests/ArcaEngineTests/CrashRecoveryTests.swift` is the example.
2. **Task 11 must cross-wire the managers before `Create` is written.** The engine calls
   `setVolumeManager` and `setNetworkManager` as of Task 6, but **`setPortMapManager` is still
   unwired**. `ContainerManager.swift:2482` guards `publishPorts` behind it with no `else`, so an
   unwired engine **starts a container with published ports, publishes nothing, and reports
   success**. `:2730`/`:3058` guard teardown the same way.
3. **Task 14's `named_volumes` and `loopback_publish` cannot be flipped** until that wiring
   exists. A flag whose machinery is unwired is a claim with no instrument.
4. **Signing precedes the live tier.** Task 6b landed it; do not reorder Task 13 ahead of it.
   Unsigned, `initialize()` dies at `VmnetNetwork()` and the engine never creates a socket.

**Two things the engine must keep NOT doing.** "Mirror `ArcaDaemon`" is the obvious way to close
a wiring gap and it would import both:

- **`applyRestartPolicies()` calls `startContainer` and boots VMs.** In an engine that resurrects
  sandboxes the consumer believes stopped, *before the socket binds* — the consumer never sees the
  transition and reconcile meets containers it did not start.
- **The daemon's deletion of Apple's `initfs.ext4`** is the shared-store behaviour the private
  root exists to avoid.

Deliberately correct as-is: `setHealthChecker` and `setEventEmitter` are silent when unset, and the
proto has no health or events surface. Wiring an `EventManager` would build toward one
`tests/release/engine-targets-check.sh` requires the engine **not** to have.

### Still open, not started

- **The Minors** — 6 in Gas Can, 8 in Arca, from the milestone-1 adversarial reviews. Two Gas Can
  Minors were taken along the way. Each carries its own reproduction in the review reports.
- **D7's narrowed retry.** Unblocked by evidence; maintainer's ruling 2026-08-12 was a separate PR,
  not folded into unrelated work. See its section below.
- **Milestone 2's own deferred minors** live in the plan and in
  `.superpowers/sdd/2026-08-12-p5-1-milestone-2-engine-lifecycle/progress.md`. That ledger is
  disposable scaffolding — anything in it that must outlive the milestone belongs here or in the
  handoff.

### The adversarial reviews

**Every Critical and Important from both is fixed.** The Minors are not, and are item 2
above.

| | |
|---|---|
| Gas Can PR | https://github.com/Liquescent-Development/gascan/pull/69 (merged) |
| Arca PR | https://github.com/Vas-Solutus/arca/pull/56 (merged) |
| Findings, Gas Can | `docs/status/adversarial-review-gascan-pr69.md` — Critical 1, Important 5, Minor 6 |
| Findings, Arca | `docs/status/adversarial-review-arca-pr56.md` — Critical 1, Important 6, Minor 8 |

Both report files are left exactly as written, recording what was observed at `39be145`
and `f5fde96`; each carries a status header saying what has since been fixed. They hold
file:line, a reproduced failure scenario, and a fix for each finding, plus a section on
what was attacked and *held* — which is as load-bearing as the findings, because it says
what not to re-litigate. **Read the "attacked and could not break" sections too.**

**What the two Criticals turned out to be, because they change what you believe:**

1. **Gas Can — the signed-pin gate could verify a different object than the one it
   compiled.** `verify-tag "$tag"` unqualified beside `refs/tags/${tag}` qualified: git
   tries `refs/<name>` before `refs/tags/<name>`, so the signature gate and the identity
   gate could land on different objects. Fixed by qualifying every tag name in both
   `build-arca-engine.sh` and `sync-arca-proto.sh`, and by constraining `.tag` in the pin
   schema to `^[A-Za-z0-9._-]+$`. **The old pin was never exploited** — `gascan-engine-m1`
   has no slash and was verified independently.
   `tests/release/engine-pin-contract.sh` now carries both halves of the attack as
   negative cases, and **both were confirmed to catch it**: against the unfixed script
   `slash-tag` exits 0 (the attacker's commit is compiled) and so does `shadowed-ref`.
2. **Arca — `Inspect` and `ListResources` could never report anything.** Resolved by
   option (b): both now answer `unsupported_capability`. Calling `initialize()` was
   rejected for this milestone on evidence the review had not reached — its restore loop
   *writes*, marking every persisted `running` container exited with code 137
   (`ContainerManager.swift:316-338`), so an engine pointed at a live `ArcaDaemon`'s state
   root would declare that daemon's containers dead; and `NetworkManager.initialize()`
   ends in `createDefaultNetworks()`, which creates a vmnet network. **This dissolved
   Arca's I3, I4 and I5** — all three are properties of answers that no longer exist.

   **SUPERSEDED 2026-08-13.** That paragraph ended "milestone 2 gives ContainerBridge a
   read-only load path that neither starts a VM nor writes". **That framing is retired and
   the reasoning behind it was incomplete.** The hazard belongs to *sharing a state root*,
   not to writing: given a private root the same crash-recovery write is correct, because a
   container the engine's own StateStore records as `running` at startup died with the
   previous engine process. And the read-only path could not have survived the milestone
   regardless — `createContainer` guards on `nativeManager` (`:1584`) and `startContainer`
   does too (`:2005`), so landing `Create` lands a VM-starting writer. Milestone 2 gives the
   engine its own state root and runs `initialize()` in full. **I3 and I4 are fixed on their
   merits** (Tasks 2 and 3); **I5's ordering fix returns with `Inspect`** in Task 7 — they
   were dissolved, not solved, and the answers they were properties of are coming back.

**Which Minors are left.** Two Gas Can Minors were taken along the way because they were
load-bearing for an Important — M1 (the `runtime-probe` comment orphaned onto `gate`) and
M2 (the EXIT trap that collapsed every documented exit code to 1, which the new
pin-contract cases assert exactly). M3 (the `/tmp` socket-root leak), M4 (the product
check being narrower than its comment — the comment now says so, rather than the check
being widened), M5 and M6 remain, as do all eight Arca Minors.

## The pin is real, and now on Arca's main

**`engine/arca-pin.json` names the signed annotated tag `gascan-engine-m1.1` at
`b3390b80528f425be0109298d6a95dd863747c5d` on `https://github.com/Vas-Solutus/arca.git`.**
This resolves the blocker earlier versions of this file recorded, which said the pin named
`gascan-engine-proto-v1` at `77b293e` — a revision with no engine in it, against which
`swift build --product arca-engine` exits 1, so CI's `engine` job *failed* rather than
building something old. It does not fail any more. Do not reintroduce the old wording.

**VERIFIED end to end against this pin**, not merely resolved: `./scripts/build-arca-engine.sh`
exits 0 in 6m00s from a cold clone — signature verified against `engine/allowed-signers`,
tag target matched, clean checkout, `Executed 30 tests, with 0 failures` — and prints the
checkout and binary paths. The live tier then passes 4/4 against that binary. CI's
`engine` job did the same twice on a hosted runner in run `31621889316` (14m42s the
second time), which is the only automated verification Arca has at all — see the CI
section below.

**That measurement stands as the record it is, and the gate is red right now anyway.**
Milestone 2 Task 3c widened the gate's test filter to cover `ArcaTests.NetworkPruneGateTests`
— the suite that proves `docker network prune` declines to delete an in-use network, which
`ArcaEngineTests` structurally cannot reach — and widened the listing guard beside it, so
the gate fails rather than silently running less than it names. `gascan-engine-m1.1` /
`b3390b8` carries no such suite: `git grep -l NetworkPruneGateTests b3390b8 -- Tests` exits
1 in `~/code/arca`. **So `./scripts/build-arca-engine.sh` now exits 70 against the current
pin** — `the test gate matched no tests: … declares no ArcaTests.NetworkPruneGateTests` —
and CI's `engine` job is red with it. That is the guard working, not a regression in it.
It clears when the pin moves to a signed tag carrying Arca `fede19c` (the XCTest
conversion of that suite). **Do not bump the pin ad hoc to make CI green.** The bump
belongs with the milestone's one signed tag, once the Arca branch merges; a pin moved to
an untagged or mid-branch revision buys a green check by giving up the trust model
`engine/allowed-signers` exists to enforce.

Since PR #56 merged, `b3390b8` is an ancestor of Arca's `main`, so the pinned revision no
longer depends on a tag alone to stay reachable. The older `gascan-engine-m1` at
`f5fde96` is still pushed and still valid; it was left where it was rather than moved,
because moving a pushed signed tag rewrites what an already-verified pin resolved to.

The engine build step is `.github/workflows/ci.yml:108` — re-derive that anchor with
`grep -n 'Build the pinned Arca engine' .github/workflows/ci.yml` rather than trusting it.
It has drifted on every single pass over this file so far; assume it has drifted again.
The `printf` anchor inside `ci.yml`'s own comments had drifted from `:179` to `:208` and
was corrected the same way — `grep -n "printf '%s" scripts/build-arca-engine.sh`.

`.artifacts/arca-dev-pin.json` still exists as a *development* pin naming a
`file:///Users/kiener/code/arca` URL and a local `gascan-engine-dev` tag. `.artifacts/` is
gitignored and worthless on any other machine — it is a convenience, no longer a
substitute. It now trails Arca HEAD by three commits, so **any rebuild from it must first
move the local tag** (`git tag -f -s`) and update its revision, or
`build-arca-engine.sh`'s tag-target assertion rejects it — correctly.

## What milestone 1 answered

These were unverified before this branch. Each is now an observation, and the anchors are
in `docs/status/arca-integration-handoff.md`.

- **A real engine accepts the client's placeholder authority `http://[::]:50051`.**
- **A missing socket and a non-socket render differently, and both name the path.**
  Missing: `No such file or directory (os error 2)`. A regular file: `Socket operation on
  non-socket (os error 38)`. Both carry the io cause past tonic's opaque `transport error`,
  so `source_chain` (`crates/gascan-arca/src/channel.rs:62-78`) does what it claims.
- **The engine claimed nothing it had not earned** — every capability flag came back
  `false` and `offline` came back `Unverified`.
- **The measured socket path is 41 bytes of `sun_path`'s 103.** Headroom, but the harness
  asserts the length before binding rather than meeting the cap as a mystery bind failure.
- **The engine's first-ever execution costs ~997ms against ~10ms warm**, which overran the
  plan's 30s harness bound. The harness now reports a dead child immediately via
  `try_wait()` and waits 120s.
- **`swift test --filter <no match>` exits 0 having run nothing.** Guarded now, in
  `scripts/build-arca-engine.sh`.

## Traps that will cost you if you learn them the hard way

### Added 2026-08-13 late, from Task 13's first hour.

**A MEASUREMENT'S SCOPE IS WHATEVER IT COULD REACH, NOT WHAT ITS SENTENCE SAYS.** This file recorded
"the isolation probes came back empty, three separate times, cross-checked" — true, careful, and
**taken when no container had ever been created**. It read as "the engine is isolated". It meant "the
engine's *startup* is isolated". Creating one container broke it in two places at once. **When you
write a measurement down, write what it could not have observed** — the three-times cross-check made
the claim feel stronger while the gap was in what was never exercised at all, and repetition cannot
close that kind of gap.

**A GUARD THE SUITE CANNOT FALSIFY IS WORTH SHIPPING ONLY IF IT SAYS SO IN ITS OWN NAME.** Task 13a's
call site needs a kernel and a VM, so its guards read the source as text. One was named
`...NeverNamesApplesSharedStore` — and swapping `manager.imageStore.path` for `ImageStore.default.path`
restores the defect **exactly** while leaving `Executed 149 tests, with 0 failures`. The assertion was
fine; the **name** was the overclaim, and a name is what the next person greps. Renamed to
`...CarriesNoLiteralPathToApplesSharedStore`, with both evasion vectors written into the doc comment
beside their measured results.

**WHEN A REPOSITORY STRUCTURALLY CANNOT TEST SOMETHING, SAY WHICH REPOSITORY CAN — AND RUN IT.** The
same mutation that leaves Arca green at 149 drives Gas Can's live `Create` to `NSPOSIXErrorDomain
Code=2`, empties the engine's own `containers/`, and grows Apple's shared store by one. Both halves are
now recorded on both sides. **An honest "this proves nothing about runtime" is only half the work; the
other half is running the thing that does.**

### Added 2026-08-13 evening. These five are new, and four cost real time the day they were found.

**A GATE THAT TWO PLACES ENFORCE IS A GATE NO TEST MEASURES.** Task 12's implementer wrote its
sandbox-id validation into *both* `lifecycleAck` and `lifecycleRefusal`, one belt-and-braces call apart.
**Deleting either copy left `Executed 147 tests, with 0 failures`, because the other caught it.**
Reading could never have found this — both copies were correct. It was found by mutating one and
getting a green suite. The fix was to put it in exactly one place, before the store read; the reasoning
is recorded at `EngineLifecycle.swift:83-91`. **When a guard is duplicated "for safety", the duplicate
does not add safety, it removes the instrument.**

**N IDENTICAL FIXTURES ARE NOT N SAMPLES. VARY WHAT THE ORDER DEPENDS ON.** This ledger recommended
"twenty independent stores, because the failure is per-process" from round 2 of Task 11 onward, and it
was written into two doc comments and several dispatch briefs. **It is wrong as stated.** Rebuilding an
identical store twenty times in one process gives *one* enumeration order, because the order is a
function of the data. Measured under a controlled experiment — twenty iterations and a fresh store per
iteration held constant, only the tag-name salt varied: **unsalted 60% catch rate, salted 100%.** Round
3's reviewer had measured the symptom (39 of 40, where truly independent samples would miss ~1.6 times
in a million, so the iterations had to be correlated); round 4 found the cause.

**A-THEN-B ON A DRIFTING MACHINE IS NOT A CONTROLLED COMPARISON.** Three separate agents were misled by
this in one day. The worst: Task 12's implementer saw five consecutive `SIGBUS` runs, bisected, watched
unmodified HEAD go green twice, and concluded its own change was the cause — **the identical tree then
ran 8/8 green once load fell from 5.58 to 3.74.** The technique that works is round 3's: it needed to
compare two resolver algorithms, so it **transplanted the old one beside the new one as
`probeResolveLegacy` and drove both against one store in one process, interleaved read by read.** When
a before/after would be confounded by environment, put both behaviours in one binary or interleave them.

**A RED FIGURE YOU CANNOT ACCOUNT FOR IS NOT A FAILURE, EITHER — CHECK THE INSTRUMENT BEFORE THE
SUBJECT.** The standing rule is "a green figure you cannot account for is not a pass". Its mirror bit on
2026-08-13: a bad `awk` field offset summed the workspace suite as **542 passed across 24 targets**
against a 1435/74 baseline, which read as a serious regression. The suite was fine; the arithmetic was
not. Re-extracted with an explicit `sed -E 's/.*ok\. ([0-9]+) passed; .../'` it is exact. **The
per-target baseline is what made the mismatch visible in seconds — keep recording it.**

**SUBAGENTS DIE WITH A FRAGMENT RETURN AND LEAVE MUTATIONS IN THE WORKING TREE. FOUR OF SIX DID ON
2026-08-13.** This is a different failure from the previously-recorded "idle with work committed": the
return message is a half-sentence, the report file may be complete or absent, and **the tree is dirty
with a reviewer's mutation that must not be mistaken for a fix.** Two left the tree dirty mid-mutation.
**Always, before anything else: `git status --short --untracked-files=all`, read any diff you find, and
`git stash push -m` it rather than `git checkout --` it** — you may need to know what it was. Two such
stashes exist in `~/code/arca` right now (`stash@{0}` five simultaneous Task-12 review mutations,
`stash@{1}` a Task-11 precedence inversion); both are discardable. **When an agent dies mid-review,
finishing its work directly is often faster than re-dispatching** — the last two verdicts of milestone 2
are controller adjudications for exactly this reason, and both are labelled as such in their files.

**THE MACHINE'S MEMORY IS THE HIDDEN VARIABLE.** 24 GB installed, and on 2026-08-13 it was carrying
~8 GB of swap and 7.8 GB in the compressor, with Firefox alone at 11.25 GB across 38 processes.
`swift test` died with `SIGSEGV`/`SIGBUS` intermittently and one run was OOM-killed with exit 137.
**None of it was code.** Check `sysctl vm.swapusage` and `top -l 1 -o mem -n 20` before believing a
crash, and note that `ps -A` output is filtered under this harness — `top` and `ps aux` are not.

---

**THE DEFECT'S NEXT FORM IS A CLAIM THAT OUTRUNS THE CODE, AND TASK 11 PRODUCED SIX.** Round 1 of its
review found three: the commit message and report each said offline-plus-ports was refused (it was
accepted, and a test *pinned* the acceptance), that every refusal was asserted by exact string
equality (collapsing seven messages to the literal `"unsupported"` left all 123 tests green), and
that a destructive `docker rmi` change was "Docker's own semantics" (it is not). Round 2 added three
more: a tie-break sort pinned by no test — **deleting it entirely left 137 tests green** — a claim
that resolution no longer depends on enumeration order which was **false for two of the four resolver
arms**, and a measured rate borrowed from a different fixture.

**Every one was caught by a reviewer running a mutation. None was caught by reading.**

**The rule that would have caught all six, now standing for this project:** before writing a claim
into a commit message, a source comment, or a report, ask **what mutation would falsify it, and
whether a test already fails under that mutation.** If none does, write the test or write the weaker
claim. A commit message asserting a property the suite cannot demonstrate is worse than silence,
because the next person greps for it and stops looking.

**REPORT A MUTATION BY COMPOSITION — WHICH TESTS SURVIVE, BY NAME — NEVER BY COUNT.** Task 10's
round-1 report read a *rising* failure count as evidence its new test was load-bearing. The count had
risen for an unrelated reason and that test was the one test in the file proving nothing; two
reports asserted it before a third measured which tests actually survived.

**A NONDETERMINISM IS NOT FIXED BY RUNNING IT MORE TIMES — RESTRUCTURE SO THE FAILURE IS FORCED.**
Task 11 shipped an `rmi` guard that threw on 2 of 5 runs, and in one run `getImage` and `deleteImage`
resolved the same string to different rows *inside one process*. Three of five runs looked fine. The
fix was to remove the order dependence at its root (one pass per arm, not one pass per row) and to
prove it with a test that builds 20 independent stores per run and reads one store 25 times: 10
consecutive runs, 10 GREEN, and 0 GREEN / 10 RED with the old arrangement restored. **Looping fresh
fixtures inside the test is what converts an N-in-5 flake into a deterministic failure.**

**A SUBAGENT WILL GO IDLE WITH ITS WORK STAGED AND UNCOMMITTED.** It happened once with **1755 lines
in the index**, alongside SourceKit diagnostics showing real-looking compile errors — a combination
that reads as an agent stopped mid-edit with broken code. Measuring said the opposite: `swift build`
exit 0, `Executed 123 tests, with 0 failures`. **The diagnostics were stale editor state.** Check
`git status --short --untracked-files=all`, `git diff --cached --stat`, and then actually build,
before concluding anything. Six subagents this session went idle with committed work and only the
return message missing.

**SOURCEKIT DIAGNOSTICS IN THIS REPO ARE ROUTINELY STALE AND SOMETIMES NAME FILES THAT NO LONGER
EXIST.** Reviewers write transient `ZZ*Probe*.swift` files and delete them; a diagnostic captured
mid-life outlives the file. Four of those appeared this session and all four were already gone.
**Check with `/usr/bin/find` and `git status --untracked-files=all` rather than trusting or
dismissing a diagnostic** — and note `find` is intercepted by the rtk hook for compound predicates,
so use the absolute path.

**SENDING A DECISION IS NOT THE SAME AS THE DECISION ARRIVING.** A maintainer approval crossed with a
subagent's messages **twice**; it reported itself blocked while the approval sat unread in its
mailbox, and about an hour was lost. Recording an approval in the ledger is not evidence the agent
received it. **For anything blocking, confirm the recipient acted on it.**

**THE DEFECT THIS MILESTONE FOUND EIGHT TIMES IS A TEST THAT PASSES WHILE PROVING NOTHING.**
Every task in landings 1-2 shipped one on the first attempt, and every one was caught by a
reviewer's mutation rather than by reading the diff. In order of increasing subtlety:

1. an assertion that was a **tautology** — `containerizationRoot()` returned the value the test
   handed the initializer;
2. a **one-sided** assertion — `XCTAssertTrue(hidden.isEmpty)` could not distinguish "hid the
   internal container" from "hid **every** container";
3. a **stub-driven** test that stayed green when only the production default was dropped;
4. a pair pinning the **failure path** while leaving the gate's **normal path** unpinned —
   "the read failed → don't delete" was proved, "the read said in-use → don't delete" was not;
5. six well-formed tests proving a **function** and never that `run()` **called** it;
6. an assertion on `--kernel-path` in stderr satisfied by **ArgumentParser's usage line**, printed
   on any parse error before `run()` is entered;
7. a conjunct **implied by its sibling** — `contains("arca-vminit:latest") && contains("vminit:latest")`,
   where the second string is a substring of the first, so the "which was found" half asserted
   nothing;
8. a signing step whose only proof would have been `codesign -d` output — which proves the command
   ran, not that the binary works.

**The two rules that catch these: mutate the PRODUCTION DEFAULT, not the seam; and mutate the CALL
SITE, not only the function.** A test that drives an injected stub proves the stub. In Task 3 a
reviewer dropped only the production default while leaving the stub path intact — the stub-driven
test stayed green and only the test that installed nothing caught it.

**A REVIEWER THAT CANNOT MAKE A FIX FAIL SHOULD SAY SO, INCLUDING AGAINST ITSELF.** Task 6b's
reviewer filed an Important finding backed by a real measurement, the fix landed, and it then
retracted the finding with a second measurement — its first had run `swift build --build-tests`
directly, which strips signatures, where `make test` never does. The fix was kept as defence in
depth and the commit subject asserting a relink that does not happen was corrected by a following
commit. **A fix you cannot make fail is either unnecessary or unprovable, and both deserve saying.**

**`arca-engine` CANNOT START A CONTAINER UNSIGNED, AND THE ERROR LIES.** `initialize()` constructs
`Containerization.VmnetNetwork()`, which needs `com.apple.security.virtualization`. Unentitled it
throws `vmnet_return_t(rawValue: 1002)` — the SDK header labels that `VMNET_MEM_FAILURE`; the
cause is the entitlement, not memory. The process exits and **never creates a socket**. Task 6b
signs it in Arca's `Makefile` and in `scripts/build-arca-engine.sh` (ad-hoc `--sign -`, which needs
no certificate). **Ad-hoc is sufficient for the gate and the live tier and NOT for a shipped
`.pkg`** — Developer ID signing is milestone 4's.

**LINE ANCHORS IN `SandboxEngineService.swift`, `ContainerManager.swift` AND
`NetworkManager.swift` MOVED UNDER EVERY SINGLE TASK.** `getNetworkAttachments` was cited at four
different lines across one landing. The `printf` in `build-arca-engine.sh` moved four times, twice
inside commits whose own comments say "re-derive rather than trusting the number, it has gone stale
twice". **Re-derive every anchor immediately before editing**, and re-derive again after your own
edits if you cite them.

**A SUBAGENT WILL FLIP THE SHARED TASK TRACKER TO `completed` BEFORE ANY REVIEW EXISTS.** It
happened twice. The tracker is an instruction surface, not a status board — the ledger is
authoritative. **And an idle notification is not a result:** three times a subagent went idle with
its work committed and only its return message missing. **Check `git log`, the working tree, and
the report file before re-dispatching anything.**

**SIGNING IS INVERTED BETWEEN THE TWO REPOSITORIES.** Gas Can's `user.signingkey` is a file
PATH (`~/.ssh/gascan-signing`), so commit with `env -u SSH_AUTH_SOCK git commit`. **Arca's
key lives in 1Password**, so it needs the agent and `env -u SSH_AUTH_SOCK` breaks every
commit with `unable to sign`. One rule for both aborts everything in one of them.
**NEVER `--no-gpg-sign`**, never a lightweight tag. Verify `%G?` is `G`. No co-author
trailer and no AI-tool mention in any commit message.

**1PASSWORD ANSWERS `ssh-add -l` WITHOUT APPROVAL BUT REFUSES TO SIGN WITHOUT IT.** "The
agent lists the key" is not evidence that signing will work. Signing in Arca needs a human
at the keyboard; if it fails, ask rather than working around it.

**THE SOURCE TREE IS NOT A RELIABLE PREDICTOR OF THE PINNED BUILD.** `~/code/arca` resolves
grpc-swift-2 2.4.2 successfully; an identical clean clone of the same revision does not,
and the mechanism was never found. Verify against a clean clone. Related: an executable
product build reaches dependency validation that library-target builds skip, which is how
a broken graph hid until the pinned build first produced a binary.

**AN IDLE NOTIFICATION IS NOT DEATH, AND AN EMPTY AGENT ROSTER IS NOT PROOF OF DEATH.** A
subagent went idle mid-work with output uncommitted, `ListAgents` reported nothing
reachable, a replacement was dispatched, and the two collided in the same files. **Check
file mtimes before re-dispatching a task whose work is already on disk.**

**A SUBAGENT CANNOT SUSTAIN A MULTI-MINUTE BACKGROUND BUILD.** Its session pauses and takes
the build with it — twice, the second time leaving `scripts/build-arca-engine.sh`'s `mkdir`
lock held. That lock fails closed by design, so every later run exits 75 until it is
cleared. Long builds belong in the controller session.

**WRITE PLANS THAT SAY WHERE YOU ARE GUESSING.** Nine blocks of this plan's Swift and shell
were wrong. Every one was marked "a best reading, not verified" with the command to confirm
it, and every one surfaced as a directed correction rather than a fix round. The worst
would have been silent: ContainerBridge reports container names with a **leading slash**,
which Gas Can compares against the bare sandbox id — every owned container would have
looked unrelated to its sandbox and drift detection would have seen nothing.

**THE INSTRUMENT KEEPS BEING NARROWER THAN THE CLAIM.** This is the defect this project pays
for over and over. This session alone: a permissions assertion that mis-parsed because `&`
binds tighter than `??`, so it masked a literal zero; and a stale-socket test whose fixture
created a regular file, which the code under test correctly refuses to unlink — it would
have failed against the very property it existed to prove. **Check a fix as hard as you
checked the defect, and prefer reading an artifact to grepping it.**

**COUNTING `test result:` LINES OVERCOUNTS THE WORKSPACE.** Some come from child processes
re-executing a test binary with a filter. Sum only the lines reporting `0 filtered out`,
and check that their count equals the target count.

**NEVER RUN THE WORKSPACE SUITE WHILE ANY OTHER CARGO IS RUNNING — NOT JUST A SUBAGENT'S.**
Run it alone, after `pgrep -fl "cargo test"` comes back empty. Concurrent suites against one
target directory produced **rc=101 with 59 failures**, none of them real: those tiers spawn
daemons and bind sockets, so they starve each other. Run alone it takes **93 seconds**.

**It is not only subagents, and not only this repository.** On 2026-08-12 another Claude
session was looping full `cargo test --workspace` cycles in an unrelated repo
(`capsule-os-worktrees/worker`) on the same machine, and this repo's suite failed three
times in a row while it ran. `pgrep` ancestry named the owner every time. **Read the
ancestry before assuming a stray cargo is yours, and never `pkill` it.**

**The failure count scales with the load, which is how you recognise it.** Measured that
day in this repo, same tree, same commit `351a646`: 2 concurrent cargo processes → **21
failures / 2 targets**; 3 processes → **37 / 5**; 3 processes with the run stretched
longer → **41 / 9**. Then every one of those 9 targets run *alone under the same
contention* → **318 passed, 0 failed, rc=0 for all nine**
(`gascan-apple/backend_fake_runner`, `gascan-e2e/{apple_apply,autostart,doctor}`,
`gascand/{daemon_idle,doctor_state,lifecycle,reconcile,ssh_config}`).

**`-- --test-threads=N` DOES NOT HELP; IT MAKES IT WORSE.** Bounding per-binary
parallelism to survive a loaded machine stretches the run, which overlaps *more* of the
neighbour's load — that is the 41-failure row above. Wait for a quiet machine, or verify
by isolation. Do not tune your way out of it.

**AND IT HAPPENED AGAIN, AT 92 FAILURES, AND WAS NEARLY BELIEVED.** Task 10 raised a
blocker on `rc=101, 92 failures across 12 targets`, every one of them a tier that spawns a
daemon or a helper. It was the same artifact. Settled by measurement rather than argument:
`cargo test -p gascand --test daemon_idle` is `running 11 tests`, 11 passed, exit 0 on
*both* the merge-base `9665107` and the branch tip under the same contention, and the full
suite run alone is exit 0 with **1435 passed / 0 failed / 28 ignored**. **A report claiming
"it reproduces on a quiet machine" is not evidence the machine was quiet** — check
`pgrep -fl "cargo test"` yourself and record the output.

**`git checkout <path>` IS NOT A PERSONAL UNDO IN A SHARED TREE.** It discards every
uncommitted change to that path, including another agent's in-flight work. Check
`git status` for a concurrent writer first, and undo your own edits with a targeted edit.

**NEVER PUT CONTROLLER STEPS IN A TASK OWNED BY A SUBAGENT.** The task tracker is an
instruction surface, not a status board.

**A STAGED BRIEF IS A CACHE AND IT GOES STALE.** Re-extract every brief immediately before
dispatch.

**A GREEN FIGURE YOU CANNOT ACCOUNT FOR IS NOT A PASS.** Account for every increment against
a per-target table you can re-derive by reading `running N tests` lines.

**`RUSTUP_TOOLCHAIN=1.95.0` is exported** and overrides `rust-toolchain.toml` — prefix every
cargo command with `env -u RUSTUP_TOOLCHAIN`. Use `--no-fail-fast`. Confirm the `running N
tests` line, because a bare test name silently runs zero and exits 0. `cargo clippy --fix`
is prohibited here. `ls` is aliased to something that rejects trailing-slash paths — use
`find` or `git ls-files`.

**A DOCS-ONLY CI RUN SKIPS `rust` AND `engine` ENTIRELY** (VERIFIED, run `31262534703`), so a
green docs run is not evidence about anything in Rust.

## CI: what to expect, so you do not spend a session on it

**`ci / gate` is NOT a required check and does not block merging.** VERIFIED 2026-08-12:
ruleset `20492137` carries `deletion`, `non_fast_forward`, `required_signatures` and
`pull_request`, and **zero** `required_status_checks`; PR #69 read
`mergeable=MERGEABLE, mergeStateStatus=UNSTABLE` and merged. `allowed_merge_methods` is
`["merge"]` — merge commits only, never squash.

**The `rust` job fails about 38% of the time on `main`, on a different test each time.**
Measured by reading the `rust` conclusion of every run in
`gh run list --workflow=ci.yml --branch main --limit 15`: of the 13 that completed, 8
green and 5 red, and the five reds were five distinct tests. PR #69's `rust` job then
failed **four consecutive times** — `pty_resize_driver_drains_chatty_child_without_
backpressure_timeout` (twice, missing a hard 2s wall-clock bound by 100ms and by **2.3ms**),
`concurrent_clients_converge_on_one_private_daemon` (D7, above), and
`same_image_apply_recreates_explicit_ssh_as_automatic`.

**One failure mode reproduced verbatim across branches, five days apart**, which is the
proof it is not a given branch's doing: `main`'s `31203816056` and PR #69's third attempt
both died as `KeygenRejected(KeygenRejection { outcome: Code(255), message:
KeygenMessage("/dev/fd/<N>: Bad file descriptor"), descriptor: Intact })`, fd 18 and fd 24,
both ending `error: test failed, to rerun pass \`-p gascand --test apply_setup\``.

**How to decide whether a red `rust` is yours.** Do not argue from probability — check the
diff. `git diff <merge-base>..HEAD -- crates/<the failing crate>/` empty means your branch
cannot have caused it, and that is a proof rather than an estimate. It was empty for
`crates/gascan-e2e/` throughout P5.1.

**The standing rule: a green local `cargo test --workspace` is the bar. CI reports but
must not gate, and flake-chasing waits** until someone is asked to do it. There are at
least three distinct root causes to fix when that day comes — the PTY wall-clock bound,
D7's `0200` window, and the keygen `/dev/fd` descriptor.

**Arca has NO CI AT ALL.** `gh pr checks 56` reported "no checks reported on the
'feat/sandbox-engine' branch", and `.github/workflows` does not exist in that repository.
Gas Can's `engine` job — which builds Arca from the signed tag and runs its 30 tests in a
clean checkout — is the only automated thing that ever exercises Arca. Any earlier
statement in this file that "CI is green on both" was wrong.

## D7 has fired, and the retry is now justified — write it

**The first `0200` occurrences landed on 2026-08-12, and there were two.** The instrument
did exactly what it was built for: it named which state fired. Verbatim, the local one,
from `cargo test --workspace --no-fail-fast`:

```
---- daemon_stderr_sink_survives_the_launching_cli stdout ----
daemon start failed: stdout=, stderr=Error: started daemon did not become healthy and
current (state Unsafe): protected runtime file is unsafe: mode is 0200 and the file has
content: written but never published (mode 0200, size 375, links 1, uid 501, expected
uid 501)
```

And in CI, run `31621889316`'s second `rust` attempt, a different test with the same
fault — `concurrent_clients_converge_on_one_private_daemon`, **size 382**, which is two
clients racing to autostart one daemon.

Read the message, not the test name. Of the two `0200` states
`crates/gascan/src/daemon.rs:3077-3079` distinguishes, both occurrences were **"has
content: written but never published"**.

**CORRECTION, recorded because it reverses the first reading of this evidence.** The doc
comment at `:3057-3064` calls that state "a daemon that wrote its record and died before
publishing, which never becomes 0600 on its own" — a corpse. On that basis this file
briefly said the evidence argued *against* the retry. **The code says otherwise, and the
code wins:**

- `is_interrupted_tombstone` (`:2633-2639`) is *defined* as 0200 with `st_size > 0`. It is
  a named, expected state the supervisor knows how to handle, not an unrecoverable one.
- `retire_held_record` (`:1372-1375`) resolves it — and **produces it transiently itself**:
  it `fchmod`s to `INSTANCE_TOMBSTONE_MODE` *first* and `ftruncate`s to 0 *second*, so
  between those two syscalls the file is 0200-with-content on disk.

So 0200-with-content is reachable as a publication in flight, `validate_file_stat` rejects
it as a hard `PermissionDenied`, and a client that reads during that window fails its
autostart. That is a race worth waiting out — exactly what the narrowed retry is for.
**The condition this file set ("stays unwritten until a run names which of the two `0200`
states fired") is met, and it points toward the retry.** Maintainer's ruling 2026-08-12:
write it in its own PR, not folded into unrelated work.

Two cautions that remain true:

- **It is load-dependent and does not reproduce on demand.** Both occurrences happened
  under contention (local: another repo's suite, pid 4969 recorded by `pgrep`; CI: a
  hosted runner). Quiet re-runs report **0 occurrences of `mode is 0200`**.
- **It predates the engine work.** Nothing in P5.1 touches `gascand`, the runtime record,
  or `crates/gascan`.

## P5.2 is done

`crates/gascan-arca` implements `RuntimeBackend` over Arca's contract behind an
`EngineTransport` seam, merged as `bd412b4`. `ChannelTransport` ships with no tests by
explicit ruling — the compiler checking it against `EngineTransport` was the stated
assurance, and **Tasks 9 and 10 of the current plan are what finally test it against a real
engine.** Do not add a test double for it.

The `sandbox_id`-claim rule is still duplicated verbatim between
`gascan-arca/src/translate.rs` and `gascan-apple/src/inspect.rs`, each with its own test and
a comment warning they must not diverge. Sharing it belongs to P5.3.
