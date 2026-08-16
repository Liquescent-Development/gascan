# P5.1 milestone 4 — product wiring, artifact distribution, and the offline proof

Date: 2026-08-16
Status: Design, approved in conversation; not yet planned or implemented
Scope: Gas Can's daemon wiring onto the Arca engine, how the engine's artifacts reach a
machine, the offline capability gate, the pin bump, and the six defects milestones 2 and 3
carried forward.

Companion documents:

- Parent design: `docs/superpowers/specs/2026-08-10-p5-1-engine-service-and-wiring-design.md`
  — **§2.3 and the launchd parts of §2.5 are superseded here; see §2.1**
- Contract: `docs/superpowers/specs/2026-08-04-sandbox-engine-contract.md`
- Proto design: `docs/superpowers/specs/2026-08-07-arca-engine-proto-design.md`
- Backend design: `docs/superpowers/specs/2026-08-08-gascan-arca-backend-design.md`
- Milestone 3 design: `docs/superpowers/specs/2026-08-15-p5-1-milestone-3-rpc-surface-design.md`
- Roadmap: `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md`

---

## 1. What milestone 4 is

The last of P5.1's four milestones, and the one that makes the Arca backend reachable by a
user rather than only by a live test. Milestones 1-3 built an engine that answers all eleven
RPCs for real; nothing in a shipped Gas Can can currently select it, and no machine that has
not had artifacts hand-built can run it.

**Exit criterion.** `gascand`, selected onto `BackendSelection::Arca`, running against an
engine it started itself from the bumped pin, creates an offline sandbox, execs into it,
reads its logs and removes it — with `gascan doctor` reporting engine facts in engine
vocabulary, and `Capabilities.offline` reporting `PROVEN` for exactly the pinned revision
and `UNVERIFIED` for any other.

**Milestone 4 does not make Arca the default.** Contract §8.3-4 gates that on P5.3
conformance. `BackendSelection::Apple` remains the default and this design does not propose
moving it.

**`offline` is the only capability flag still unearned.** The other six are `true`, each
earned by a live test seen to fail against a one-line mutation.

## 2. Decisions

Two of these supersede an approved parent design. Both supersessions are recorded with the
reasoning that produced them, because the reasoning is what a later change needs.

### 2.1 The engine is started by `gascand`, dial-then-spawn — it is not a launchd job

**This supersedes parent design §2.3, and the launchd sentences of its §2.5.**

§2.3 ruled the engine a launchd job and rejected supervision on a structural defect: "after
`gascand` is `SIGKILL`ed its engine child survives holding the socket, so the next `gascand`
spawns a second engine that cannot bind. Recovery therefore *requires* dialing an existing
engine — supervision does not replace the dialing case, it adds a second one plus a fallback
branch to choose between them."

That reasoning is sound and its conclusion is already implemented in this repository, for
`gascand` itself. The CLI dials the daemon socket first and spawns only when nothing answers;
`TokioDaemonSpawner::spawn` (`crates/gascan/src/client.rs:327`) hands the child an inherited
startup-diagnostic descriptor, and `DaemonStartupMonitor` distinguishes a child that died
with an exit status from one that is merely slow to reach its first instruction. The
"second case plus a fallback branch" §2.3 feared is not an addition — dial-first *is* the
primary path, and spawn is its miss arm.

So the engine reuses that machinery rather than acquiring a supervisor of its own or a
launchd job.

**What this buys.** No plist, no `launchctl`, no new install/uninstall/upgrade surface.
§2.5's rule that "`launchctl` appears in no documentation, no error message, and no recovery
instruction" becomes true by construction instead of a property to maintain. And it removes
launchd from a product that has none today: there is no plist anywhere in this repository,
and `packaging/macos/install.sh:42` already tells the user "The per-user daemon starts on
demand."

**What is preserved.** The adoption property §2.3 rests on is untouched. A surviving engine
is still a feature: `run_daemon` calls `service.reconcile()` before serving
(`crates/gascand/src/main.rs:483`), and `ReconcileFinding` (`crates/gascand/src/reconcile.rs:5-21`)
already distinguishes `UnknownOwned`, `UnknownUnowned`, `MissingOwned` and
`OwnershipMismatch` — four of its seven variants — so a restarted daemon adopts running
sandboxes by their owner labels. Dial-then-spawn is precisely what makes that adoption
reachable.

**Not chosen: launchd socket activation.** Unchanged from §2.3 — its real benefit is letting
the engine exit when idle, and the engine must not exit while sandboxes are running.

### 2.2 The kernel and vminit are fetched after install and verified by digest

The `.pkg` continues to carry no engine payload. `packaging/macos/package.sh:83` states the
current position deliberately: "The engine is a build gate, not a payload:
build-arca-engine.sh above compiles the pinned Arca tree and fails the release if it does
not verify, but nothing from that tree is installed."

**The artifacts are large and nothing in this milestone can shrink them. MEASURED
2026-08-16** against `~/.arca/vminit` and `/Applications/Arca.app/Contents/Resources/vmlinux`:

| | |
|---|---|
| kernel, `stat -f '%z'` | **28,248,576 bytes** |
| vminit layout | 3 blobs, one manifest, **no orphans** — `index.json` names one manifest, which names one config and one layer |
| vminit layer | **186,967,040 bytes**, mediaType `application/vnd.oci.image.layer.v1.tar` — **uncompressed** |
| vminit layer contents, `tar -tvf` | **15 entries**, of which three are binaries: `./sbin/vmexec` 86,907,872, `./sbin/vminitd` 86,846,432, `./sbin/arca-services` 13,172,898 |
| those three, `file` | ELF aarch64, statically linked, **all three already `stripped`** |
| `gzip -6 -c <layer> \| wc -c` | **73,725,003 bytes**, 4.5s wall |

The binaries are static Swift and Go runtimes and are already stripped, so there is no
slimming lever inside this milestone. `vmexec` and `vminitd` are near-identical in size and
almost certainly carry near-identical copies of the Swift runtime, but they are separate
upstream binaries in the frozen submodule and deduplicating them is not this milestone's
work.

**Two figures in `docs/status/START-HERE.md` disagree about this and both are worth
correcting.** `:1446` says "163 MB vminit" and `:1283` says "an OCI layout, 178 MB". The
measured layout is 186,968,171 bytes of files, 178.3 MiB, 187.0 MB decimal.

**Not chosen: embedding both in the `.pkg`.** It is the option most consistent with §2.5's
one-install rule, and it was rejected on distribution cost — roughly +100MB on every Gas Can
release download, carried by every user including those who never select the Arca backend
— and on an unresolved obligation: `vmlinux` is a Linux kernel, so redistributing the binary
carries a GPL source-offer obligation, and on the development machine it is a symlink into
`/Applications/Arca.app` whose build provenance this design has not traced. **A fetch does
not dissolve that obligation** — see §4.

**Not chosen: keeping a separate install** (the user installs Arca.app and Gas Can reads
from it). It reinstates exactly the separate management §2.5 exists to remove, mirroring the
Apple `container` prerequisite at `install.sh:33`.

### 2.3 `offline` is gated consumer-side on the engine's build revision

`Capabilities` gains **field 20** carrying the engine's build revision. Fields 10-19 stay
reserved for the network-model phase, as `engine.proto:125-128` requires and for the reason
it gives. `contract_minor` goes from 0 to 1, which is what that field exists for: "an engine
bugfix release must not imply a contract revision" (`engine.proto:111-115`).

`gascan-arca` holds the certified revision constant and decides `Proven` versus `Unverified`
client-side.

**This mirrors the precedent Gas Can already trusts.** `crates/gascan-apple/src/probe.rs:47`
gates on `self.version == minimum && self.commit == APPLE_1_1_COMMIT`, where
`APPLE_1_1_COMMIT` is a constant held in Gas Can and compared against what the runtime
reports about itself; `probe.rs:222-227` then reports `Proven` for a certified release and
`Unsupported` otherwise. The judgement lives with the consumer, which is also the proto's own
stated rule for ownership: deciding whether a labelled resource is yours "is the consumer's
judgment and it stays there" (`engine.proto:143-148`).

The engine already knows its revision — `ArcaVersion.gitCommit`
(`Sources/ContainerBridge/Version.swift:16-18`), injected by the Makefile into
`BuildInfo.generated.swift`. It has never been on the wire.

**Not chosen: an engine-side revision gate.** One constant and one branch in `ArcaEngine`,
no contract movement — and self-certification. The component being judged would issue its own
verdict, which is the shape this project's boundary rules exist to avoid.

**Not chosen: gating on `engine_version` alone.** It is already on the wire, but
`ArcaVersion.version` is Arca's *product* version — `"0.2.4-alpha"` at
`Version.swift:7` — and does not move per engine change, so two materially different engine
builds are indistinguishable by it. Apple's precedent pairs version with commit precisely
because version alone is not enough.

**Consequence, stated rather than discovered later:** a developer build of the engine reports
`UNVERIFIED`, so `compile`'s offline arm (`crates/gascan-core/src/policy.rs:419-427`) refuses
a default-network sandbox through the policy path against an uncertified engine. That is the
intended behaviour of a gate, and §5 makes it legible in `gascan doctor` rather than
mysterious.

### 2.4 Selection is by environment variable, and the running daemon's backend becomes visible

Selection follows parent design §3.2: `GASCAN_ARCA_BACKEND` selects the backend and
`GASCAN_ENGINE_SOCKET` names the socket, following the shape of `TEST_FAKE_BACKEND_ENV`
(`crates/gascand/src/lib.rs:5`). This works: `TokioDaemonSpawner::spawn` adds three variables
to the child's environment and never clears it, so a variable set in the user's shell reaches
`gascand` through the CLI.

`BackendSelection::Arca` is a **release** variant. `Fake` is `#[cfg(debug_assertions)]`
(`lib.rs:11`) and `Arca` must not be.

**`DaemonInstanceRecord` gains a `backend` field beside `release_version`**
(`crates/gascan/src/daemon.rs:185-193`), and a handshake mismatch is an error naming both
backends.

Without it there is a silent hazard. The daemon is on demand with a 300-second idle timeout
(`crates/gascand/src/main.rs:238-241`), and the descriptor records pid, owner token,
executable, start identity, instance token, release version and start time — **and nothing
about the backend**. So `GASCAN_ARCA_BACKEND=1 gascan up` followed by a plain `gascan ps` in
another shell inside that window reaches the same Arca-backed daemon, and nothing in the
handshake reveals it; symmetrically, a daemon already running on Apple ignores the variable
on the next command. `release_version` is already carried and already compared, so the fix
follows machinery that exists.

### 2.5 The two contract defects get rulings, not engine workarounds

**(a) The proto permits offline-plus-ports and says nothing about which wins.** Three
components already agree it is refused: `gascan-core/src/policy.rs:437` refuses it as
`PolicyError::OfflinePortsForbidden` before a request is built, and
`Sources/ArcaEngine/EngineCreate.swift:245-268` refuses it engine-side with
`unsupported_capability`. Only the proto is silent. **The rule is written into
`engine.proto`.** A behaviour three implementations already share, recorded nowhere
normative, is one refactor away from being lost.

**(b) `AckResponse` cannot express a partial `Remove`.** `CreateFailed` carries
`repeated Resource created` precisely so a partial create does not leak, while `AckResponse`
is a bare `oneof { Ack ok; EngineError error }` — so a `Remove` that deletes the container
and then fails on the volume is indistinguishable on the wire from one that did nothing.
**Recorded as a contract change and deliberately not worked around in the engine.** Severity
stays bounded for the reasons already established: `Remove` validates every resource before
deleting any, so authorisation failures delete nothing; only a mid-deletion manager failure
produces a partial; nothing retries `Remove`; and `ListResources` plus reconcile can
rediscover the truth.

## 3. Architecture

### 3.1 Gas Can

- `crates/gascand` gains `BackendSelection::Arca`, engine socket configuration, an engine
  supervisor built on the existing spawner, and construction of
  `ArcaBackend<ChannelTransport>`. `crates/gascand/Cargo.toml` gains `gascan-arca` — an edge
  that does not exist today.
- `DaemonInstanceRecord` gains `backend` (§2.4).
- The doctor remedies move out of `DoctorFacts::into_report()` (§5).
- `engine/arca-pin.json` moves to schema 2 and to the new signed tag (§4).
- `packaging/macos/` gains the artifact fetch and its uninstall path; `install.sh` is
  **not** where the fetch lives (§4).
- New `gascan-e2e` coverage for a daemon-on-engine pass and for the backend-mismatch refusal.

Two requirements inherited from parent design §6, **neither of which exists yet** — recorded
as work rather than as current behaviour:

- **`gascand` validates the engine socket's owning uid before dialing.** Nothing does this
  today; `validate_peer_uid` (`crates/gascand/src/socket.rs:631`) is used at
  `crates/gascand/src/api.rs:485-491` for the opposite direction — the daemon checking the uid
  of clients connecting to *its* socket. The engine-side check is new code, and the existing
  `PeerUid` type is the piece to reuse.
- **An engine that dies mid-call fails that call** and appears as `MissingOwned` on the next
  reconcile — never as a silent reconnect implying state survived.

### 3.2 Arca

- `Capabilities` field 20 and `contract_minor = 1` (§2.3).
- The two shutdown defects, 4 and 5 in §6.
- `parseSignal`, defect 1 in §6.
- The offline-plus-ports rule written into `engine.proto` (§2.5a).

**Anchors re-derived 2026-08-16, because the ones on file have drifted.** This is recorded
rather than quietly fixed, since a successor will meet the stale ones:

| Thing | Re-derived | `START-HERE.md` says |
|---|---|---|
| `parseSignal` | `ContainerManager.swift:2931-2962` | `:2882-2911` |
| its unchecked numeric branch | `:2938-2940` | `:2889-2891` |
| `signal(number, SIG_IGN)` | `ArcaEngineCommand.swift:430` | `:381` at one point, `:434` at another |
| `source.resume()` | `:503` | `:447` |

### 3.3 The `containerization` submodule

- `EXT4.Formatter.unpack` (`Sources/ContainerizationEXT4/Formatter+Unpack.swift:27-103`)
  must refuse a blob that is not the archive its media type declares rather than producing an
  empty filesystem — defect 2 in §6.
- The host tells the guest how many layers it attached — defect 6 in §6.

**Both force a `make vminit-rebuild`, which changes the vminit layer's digest, and that
digest is what the fetch verifies.** So submodule work must land before artifacts are
published under any sequencing. §8 satisfies this by construction.

Milestone 2's rule still binds: a submodule pointer that moves must be pushed and reachable
from its own remote *before* Arca's merge, because a pointer that moves late is how a fresh
clone breaks at `git submodule update --init --recursive`.

## 4. Artifact distribution and its trust chain

### 4.1 Hosting

Both repositories are public (`gh repo view` reports `"visibility":"PUBLIC"` for
`Vas-Solutus/arca` and `Liquescent-Development/gascan`), so the kernel and the vminit layout
ship as **release assets on the signed Arca tag the pin already names**.

That ref's signature is already verified as a matter of course:
`scripts/build-arca-engine.sh:94` runs `git verify-tag refs/tags/<tag>` against
`engine/allowed-signers`, and `:103` asserts the tag resolves to the pinned revision.

### 4.2 Digests

`engine/arca-pin.json` goes from schema 1 to schema 2, gaining a sha256 and a byte length for
each artifact. The file lives in Gas Can, whose release source is signature-gated by
`gascan_verify_release_source` (`packaging/macos/package.sh:32`), so the digests inherit that
gate. `package.sh:83-95` already records engine provenance into `build-manifest.json`; it
extends to record the artifact digests a build expects.

The chain end to end: a signed Gas Can release commit fixes the pin file; the pin file names a
signed Arca tag and the digests of its assets; `build-arca-engine.sh` verifies the tag
signature and that it resolves to the pinned revision; the fetch verifies each asset against
its recorded digest.

### 4.3 Where the artifacts land

`~/Library/Application Support/dev.gascan/engine/` — per-user, durable, Gas Can-owned,
alongside the existing `dev.gascan/controller/`
(`crates/gascand/tests/controller_state.rs:48`).

**Explicitly not `~/.arca`.** That is `ArcaDaemon`'s directory, and milestone 2's thesis is
that the engine owns a private state root and writes nowhere else — a thesis that cost two
defects to establish, both of them paths that wrote outside the state root. The live tier
reads `$HOME/.arca/vmlinux` and `$HOME/.arca/vminit` today only because those are a
developer's hand-made artifacts; the four live-tier environment variables stay as they are,
undefaulted, absent meaning `panic!`.

### 4.4 Who fetches, and when

**Not `install.sh`.** The Homebrew cask installs the `.pkg` directly —
`packaging/macos/render-cask.sh` emits `pkg "gascan-#{version}-macos-arm64.pkg"` and never
invokes `install.sh` — so a fetch living there would silently never happen for cask users.

The fetch is a Gas Can command, and its absence is a `gascan doctor` fail carrying that
command as its remedy. **This is the pattern the product already uses for a prerequisite the
installer cannot satisfy**: the cask's own `caveats` say "Gas Can requires Apple container
1.1.0 and its running service. Gas Can does not install or redistribute it." and direct the
user to `gascan doctor --json`. It works identically for both install paths.

**The command's name and its place in the CLI grammar are deliberately left to the
implementation plan**, which is where this repository's other CLI surfaces were settled. What
this design fixes is that the fetch is a first-class Gas Can command rather than an installer
step, that `gascan doctor` is what tells a user to run it, and that it never runs implicitly
in the middle of another operation — a hundred-megabyte download must not surprise a user
who typed `gascan up`.

`packaging/macos/uninstall.sh` removes the per-user artifact directory. The cask's
`uninstall delete:` list stays as it is — it does not remove per-user state today and this
design does not change that.

### 4.5 Failure modes

Stated rather than discovered:

- **No network at fetch time** is a hard failure naming the command to re-run. No partial
  artifact is left in place.
- **A digest mismatch** is a refusal naming the expected and the observed digest, and it
  deletes what it fetched. It never falls back to using the file.
- **A pin that moves under an already-fetched artifact** is detected by digest and reported
  as an action the user must take, never silently tolerated and never silently re-fetched
  mid-operation.
- **A partially written artifact** — interrupted fetch, full disk — is indistinguishable from
  a corrupt one to the digest check, and is treated as one.

### 4.6 The obligation this design does not discharge

`vmlinux` is a Linux kernel binary. Distributing it — as a release asset no less than inside a
`.pkg` — carries a GPLv2 corresponding-source obligation, and on the development machine it
is a symlink into `/Applications/Arca.app/Contents/Resources/vmlinux` whose build provenance
this design has not traced. **The implementation must establish where that kernel is built
from and publish the corresponding source offer alongside the asset.** This is called out
rather than assumed because it is the kind of item that is cheap before a release and
expensive after one.

## 5. `gascan doctor`

**The remedies are currently Apple's, hardcoded, and attached where facts become a report.**
`DoctorFacts::into_report()` (`crates/gascan-core/src/doctor.rs:237-297`) pairs each fact with
prose like "install Apple container 1.1.0 in PATH", "run `container system start` and retry",
and "install matching Apple container 1.1.0 CLI and service components". An Arca-backed daemon
with a dead engine socket would today tell the user to install Apple container.

So doctor is not additive work. **The remedy moves to the backend that produced the fact.**

The `DoctorCheckId` variants are already backend-neutral — `RuntimeCli`, `RuntimeVersion`,
`RuntimeService`, `RuntimeKernel`, `RuntimeSchema` — so neither the enum nor
`render_doctor` (`crates/gascan/src/presentation.rs:109`) changes. Under the Arca backend
those five carry engine facts: the engine binary, `engine_version`, whether the socket answers
`Capabilities`, the kernel artifact, and `contract_minor`.

**Two facts are new rather than remapped**, and both exist to make a refusal legible:

1. **Engine artifacts present and digest-matching**, whose remedy is the fetch command of
   §4.4.
2. **The engine's revision against the certified constant**, which is what turns an
   `offline: UNVERIFIED` from a mystery into a statement that this engine build is not the
   certified one.

`production_doctor_report`'s gate-2 evidence strings (`crates/gascand/src/main.rs:608-745`)
are Apple's and stay Apple's. They are not a template for the engine's: Apple's evidence is an
out-of-band signed-off matrix, and the engine's is §7.2's recorded observation.

## 6. The six carried defects

| | What | Where | Status |
|---|---|---|---|
| 1 | `parseSignal` silently defaults anything unrecognised to SIGKILL, and its numeric branch range-checks nothing | `ContainerManager.swift:2931-2962`; numeric branch `:2938-2940` | reachable today from Arca's Docker surface |
| 2 | `EXT4.Formatter.unpack` accepts a blob that is not the archive its media type declares and produces an empty filesystem rather than refusing | `Formatter+Unpack.swift:27-103`, submodule | upstream, in the frozen submodule |
| 3 | Layer-cache tests use a one-layer fixture, so a multi-layer defect is invisible | `Tests/ArcaEngineTests/LayerCacheRoleTests.swift` | needs a multi-layer fixture |
| 4 | Startup SIGTERM race, **exit 143** | window closes at `ArcaEngineCommand.swift:430` | forced **12/12** immediately after spawn vs **0/12** after socket + 300ms; naturally ~2 in 440 |
| 5 | `shutdown::the_engine_exits_cleanly_with_a_client_channel_still_open`, **exit status 1** | `crates/gascan-arca/tests/live/shutdown.rs:319` | ~1 in 288; pre-existing, reproduced on `8679113` with none of milestone 3's fixes |
| 6 | The host does not tell the guest how many layers it attached, so "no layers" and "layers I could not identify" are one observation | submodule, guest-side | needs a `make vminit-rebuild` and a guest-side measurement |

**Defects 4 and 5 are not the same defect and must not be folded together.** Different tests,
different exit codes, different causes. 143 is `128+15` and therefore always the kernel: the
engine's only deliberate exit is `Foundation.exit(status)` and it has exactly one call site
(`ArcaEngineCommand.swift:336`, verified — `grep -n "Foundation.exit"` reports that line and a
doc comment at `:374`). Exit status 1 is the engine's own error exit.

**Defect 4's fix is a design change, not a one-liner.** The handler closure captures the
engine, and a bare `SIG_IGN` installed before a resumed dispatch source would make a startup
SIGTERM a silent no-op, which is worse than dying. The window is the whole of startup —
SIGTERM's disposition is default from `exec` — not merely bind-to-`SIG_IGN`.

**A second window is reasoned and has never been measured**: between `signal(number, SIG_IGN)`
(`:430`) and `source.resume()` (`:503`), the disposition is already `SIG_IGN` but libdispatch
has not registered the kevent, so a signal there is lost outright and the live tier would
report "the engine ignored SIGTERM" — which reads as a shutdown defect rather than a startup
one. **Measure it; do not assume it either way.**

Defect 5's remaining open question is attribution of the drain grace, which must be measured
rather than argued.

## 7. Testing

### 7.1 Mutation discipline is this milestone's central rule

Milestone 3's resize test **shipped the control instead of the subject** and would have
certified its own regression: the commit that introduced it recorded the variant *with* a
readiness handshake passing against the broken engine, and that is the variant that was
committed. Reverting the engine fix would have left both repositories green.

So: **every fix's test is verified by mutation by the implementer, not by a report.** The
check that catches a control-shipped-as-subject is **two mutations that fail disjoint test
sets** — which is what proves neither test rides on the other's fix. A test named for a window
that closes the window before testing it is worse than no test, because the name is what a
successor trusts.

### 7.2 The offline proof

**The shape already exists and is hardened.** `packaging/macos/release-smoke.sh:1015-1037`
asserts six ways an offline sandbox has no egress — a test-owned host endpoint, a public IP,
and public DNS, each as the sandbox user and again as guest root — and fails the release if
any succeeds.

Run against the engine, that observation is recorded under `docs/evidence/` pinned to the exact
Arca revision, and **it is what sets the certified constant of §2.3.** The evidence directory
already holds this project's other recorded proofs.

The proof observes what §2.1 of the parent design means by offline: **no network attachment at
all** — no vmnet, no WireGuard — from inside a running sandbox.

### 7.3 Defect 5 needs a rate instrument, not a run

It fires about 1 shutdown in 288, so a single green run proves nothing and must not be quoted
as if it did.

**The frozen-binary method is the one to reuse** whenever a change to the thing under test
rules out the usual empty-diff exoneration: stash the working tree, build and *separately
sign* two engines, restore, and verify every changed file byte-identical afterwards. That is
how defect 5 was attributed to pre-existing code — the identical signature reproduced on
`8679113` at 1 of 288 and did not appear with the fixes at 0 of 288. One event cannot
distinguish "unchanged" from "improved" and no such claim is to be made.

### 7.4 Baseline hygiene

Every live test stays `#[ignore]`d with a reason naming its requirements and registered in
`tests/ci/expected-ignored-tests.txt`, or `scripts/ci-check-ignored-tests.sh` fails in either
direction. The baseline is **43** at the start of this milestone.

### 7.5 The engine must be re-signed after the last `swift test`

`swift test` re-links `arca-engine` and strips its entitlements, and an unsigned engine never
creates a socket — `initialize()` dies at `VmnetNetwork()`. Verify with
`codesign -d --entitlements - <bin> 2>&1 | grep -c virtualization`, which must print `1`.

### 7.6 What the workspace suite currently is, so nobody reports it as green

Four `cargo test --workspace` runs have produced **four different single failures, all in
`crates/gascan-e2e`**, each exonerated by an empty branch diff plus an isolated green re-run.
**That the failing test is different every time is the finding** — a branch-caused failure does
not wander. A clean local workspace run has not been achieved on this line of work. This
milestone does not chase it and does not report it as green.

## 8. Sequencing

**Ruled: the pin lands in the middle, not at the end.** This is possible because §2.3 puts the
certified-revision constant in Gas Can rather than in the engine, so the offline proof no
longer has to precede the tag.

1. **All Arca and submodule work.** `Capabilities` field 20 and `contract_minor = 1`; defects
   1-6; the offline-plus-ports rule into `engine.proto`. Then **one signed tag**.
2. **The pin bump and artifact publication** against that tag. This is also what turns CI's
   `engine` job green — it is red by design against the unbumped pin, and **the pin must not
   be bumped merely to make it green**; the bump is this milestone's own work and needs the
   signed tag.
3. **All Gas Can wiring** — `BackendSelection::Arca`, the engine supervisor, the instance
   record's backend field, doctor, the fetch, `gascan-e2e` — **against the real pinned
   engine** rather than a local build.
4. **The offline proof** against the pinned revision, setting the Gas Can constant and
   flipping `offline` client-side.

**The submodule-before-artifacts constraint of §3.3 is satisfied by construction**: defects 2
and 6 land in stage 1, so the `make vminit-rebuild` they force happens before stage 2 publishes
anything.

**Not chosen: spine last** (parent design §7's ordering, one interlocked landing at the end).
Faithful to the approved plan, and rejected because CI's `engine` job would stay red for the
whole milestone and every Gas Can task would be validated only against a local unpinned build
— the configuration no user will ever have.

**Not chosen, and worth recording why: Gas Can first.** The current pin is
`gascan-engine-m1.1` / `b3390b8`, which is *milestone 1's* engine and answers
`unsupported_capability` across most of the surface. Nothing wired against it can be driven end
to end, so Gas Can-first cannot be validated at all until the pin moves.

**Known cost of the chosen order:** a defect found in stage 3 that needs an engine fix costs a
second signed tag.

## 9. Process rules this milestone inherits

Recorded here because each was paid for and each is cheap to lose.

- **Signing is inverted between the repositories.** Gas Can commits need
  `env -u SSH_AUTH_SOCK git commit`; Arca commits need the agent, because its key is in
  1Password. Probe Arca before attempting anything:
  `echo test | ssh-keygen -Y sign -n git -f <(git config --get user.signingkey)`. **Never
  `--no-gpg-sign`.** Verify `%G?` is `G` afterwards.
- **Dispatch code reviewers synchronously.** Of four reviewers backgrounded on 2026-08-16, one
  delivered only after a retrieval request, one never delivered across three probes, and two
  went silent with their work already on disk. Implementation output survives a lost report; a
  review does not. Send one retrieval request and treat a second silence as the answer.
- **Subagents have no `ReportFindings` tool.** Ask for the fields in the reply. A dispatch
  whose output contract cannot be satisfied loses the whole review.
- **A `grep | grep | awk` pipeline over a test log silently drops lines.** Write each stage to
  a file and read the file. `grep -q "test result:"` is not a completion check — it matches
  after the first target of a workspace run.
- **Merge commits only, never squash.** `allowed_merge_methods` is `["merge"]`. `ci / gate` is
  not a required check. Arca has no CI at all.
- **If the engine dies with `vmnet_return_t(rawValue: 1001)`, force-quit `InternetSharing`.**

## 10. Out of scope

- **P5.3 conformance**, and therefore making Arca the default (contract §8.3-4).
- **U5 / P5.4** — how a shipped `.pkg` gets the *workspace image* into a user's engine. §4
  distributes the kernel and vminit, which are the engine's own boot artifacts; the workspace
  image is a different question and stays P5.4's.
- **P6 network model.** Egress policy, peer channels and guest-enforced packet filtering are
  untouched; `Capabilities` fields 10-19 stay reserved.
- **Socket activation and engine idle-exit** (§2.1).
- **Deduplicating `vmexec` and `vminitd`**, which are separate upstream binaries in the frozen
  submodule (§2.2).
- **Fixing `AckResponse`'s inability to express a partial `Remove`** (§2.5b) — a contract
  change, recorded, not taken here.
- **The Minors** carried from the milestone-1 adversarial reviews, and milestone 2's deferred
  minors.
