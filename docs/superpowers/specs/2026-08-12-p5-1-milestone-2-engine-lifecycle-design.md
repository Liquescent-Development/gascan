# P5.1 milestone 2 — engine state ownership and the sandbox lifecycle

Date: 2026-08-12
Status: Design, approved in conversation; not yet planned or implemented
Scope: Arca's engine startup and state ownership, the five `ContainerBridge` changes that
make it possible, and the sandbox lifecycle RPCs built on top.

Companion documents:

- Parent design: `docs/superpowers/specs/2026-08-10-p5-1-engine-service-and-wiring-design.md`
- Contract: `docs/superpowers/specs/2026-08-04-sandbox-engine-contract.md`
- Milestone 1 plan: `docs/superpowers/plans/2026-08-10-p5-1-milestone-1-engine-skeleton.md`
- Review findings: `docs/status/adversarial-review-arca-pr56.md`

Anchors below were read at Arca `cc316b6` and Gas Can `6847d1e` on 2026-08-12. Where a
symbol name locates the thing on its own, the name is used instead of a line number —
parent design §10 records that this file's line anchors have drifted before.

---

## 1. What this milestone is

The parent design's §7 sequencing lists milestone 2 as "image ingress and lifecycle". The
post-merge `docs/status/START-HERE.md` adds a first task ahead of it: "give `ContainerBridge`
a read-only load path that neither starts a VM nor writes, and restore `Inspect` and
`ListResources` on top of it."

**This milestone takes both, and it retires the read-only framing.** §2.1 explains why.

**Exit:** an engine that owns its own state, refuses to start without its inputs and says
which is missing, and can create, start, stop, inspect and remove a sandbox through
`ArcaBackend<ChannelTransport>` over a real socket.

## 2. Decisions

### 2.1 The engine owns a private state root, and `initialize()` runs in full

The read-only requirement was derived from a premise that this design removes.

Milestone 1 resolved review finding C1 by making `Inspect` and `ListResources` answer
`unsupported_capability`, and rejected calling `initialize()` on the grounds that
`ContainerManager`'s restore loop *writes*: a persisted container whose status is `running`
is marked exited with code 137 and that is written back to the `StateStore` — the
`CRASH RECOVERY` branch at `ContainerManager.swift:317`, whose write is at `:333`. Against a
state root a live `ArcaDaemon` also uses, the engine would declare that daemon's containers
dead.

**That hazard is a property of sharing a root, not of writing.** Given a private root, the
same write is correct: a container the engine's own `StateStore` records as `running`, found
at engine startup, died with the previous engine process, because its VM was that process's
child.

The read-only path also cannot survive this milestone's other half. `createContainer` guards
on `nativeManager` (`ContainerManager.swift:1584`) and so does `startContainer` (`:2005`);
`nativeManager` is assigned in exactly one place, `:240`, from a `Kernel` and a
`Containerization.VmnetNetwork`. A milestone that lands `Create` and `Start` lands a
VM-starting writer. Building a read-only load path first would mean designing, testing and
then invalidating it inside one milestone.

So: **the engine's state root is `~/Library/Application Support/dev.gascan/engine/`**, a
sibling of the controller's directory. Gas Can's own convention, not Arca's —
`crates/gascand/src/controller_state.rs:20-22` sets `APPLICATION_ID = "dev.gascan"` and
`CONTROLLER_DIRECTORY = "controller"`, and `:95-100` joins them under
`Library/Application Support`. The engine is a Gas Can-owned process (parent design §2.3,
§2.5), so its state is Gas Can state.

Not chosen: sharing `~/.arca` under an exclusive lock. It keeps one copy of the large
artifacts on disk, but it makes the engine and Arca's own Docker surface mutually exclusive
at runtime — wrong for a user who still uses Arca directly — and it requires changing
`ArcaDaemon` to cooperate.

Not chosen: a separate root *plus* a read-only load path for degraded operation when the
kernel or vminit is absent. It preserves `Inspect` on an engine that cannot act, at the cost
of two load paths to build, test and keep from diverging. §2.3 takes the fail-fast answer
instead.

### 2.2 Mutable state and read-only inputs are separate options

Three required options, none with a default, none falling back to `~/.arca`:

| Option | Kind | Contents |
|---|---|---|
| `--state-root` | mutable, private to this engine | `state.db`, `images/`, `volumes/`, `layers/` |
| `--kernel-path` | read-only input | `vmlinux` |
| `--vminit-layout` | read-only input | an OCI layout directory holding `arca-vminit:latest` |

The distinction is load-bearing. A file two processes *read* is safe to share; a state root
is not, and C1 is what sharing one costs. Collapsing both into `--state-root` would force the
large read-only artifacts to be copied per engine and would invite the reverse error — a
future reader concluding that because the kernel can be shared, the root can be.

`--state-root` is already a required `@Option` with no default
(`ArcaEngineCommand.swift:19-20`), and the existing construction is already root-relative for
`state.db`, `images/`, `volumes/` and `vmlinux` (`:76-92`). The comment at `:69` already
anticipated `--kernel-path`. What changes is that the kernel and vminit stop being derived
from the state root and become inputs in their own right.

### 2.3 A missing input is a refusal to start, not a degraded mode

`ContainerManager.initialize()` requires the kernel file to exist, an `arca-vminit:latest`
image in the store, and a live `VmnetNetwork` (`:213-245`). Milestone 1's
`ArcaEngineCommand.run()` treats refusing to start without them as a defect to avoid.

At this milestone's scope the engine needs all three to do anything it claims. Refusing is
therefore fail-fast, and the alternative — an engine that starts and answers
`unsupported_capability` for everything that matters — is the state C1 was raised against.

The refusal names which input is missing and the path tried. **Surfacing that refusal through
`gascan doctor` is milestone 4's**, not this one: doctor reads engine facts through the daemon
wiring that milestone owns (parent design §2.5). What this milestone owes milestone 4 is a
refusal message worth surfacing.

### 2.4 The engine's image store is its own, which is also how `initfs.ext4` stops being shared

`ArcaDaemon` deletes `~/Library/Application Support/com.apple.containerization/initfs.ext4`
on every start, to force regeneration when vminit changes (`ArcaDaemon.swift:95-108`). That
path is outside every state root — shared with Apple's containerization and with any other
consumer of it. An engine copying that behaviour would destroy a live `ArcaDaemon`'s
regenerated initfs.

**The engine must not coordinate that delete; it must not use that path.** `initfs.ext4` is
not a fixed location. `containerization/Sources/Containerization/ContainerManager.swift:102`
and `:146` derive it:

```swift
let initPath = self.imageStore.path.appendingPathComponent("initfs.ext4")
```

and the initializer at `:128-166` takes `root: URL? = nil` — given a root it builds
`ImageStore(path: root)`, otherwise `ImageStore.default`, whose root is
`<Application Support>/com.apple.containerization` (`ImageStore.swift`, `defaultRoot()`).
Arca's `ContainerManager.swift:240` passes no root, which is why that file is shared and why
the delete exists.

Passing the engine's own root isolates `initfs.ext4` along with everything else. No
coordination protocol, no shared path, and nothing to stomp.

**This also fixes a latent defect in the current wiring.** `ArcaEngineCommand.swift:81-84`
constructs `ImageManager(imageStorePath: root/images)`, but `ContainerManager.initialize()`
would construct Containerization's manager against `ImageStore.default`. Those are two
different stores. A vminit image loaded into `<state-root>/images` would not be found by
`getInitImage`, which reads the default store — so the engine would either fail to boot a
container or silently boot on whatever vminit another product left in the shared store. It is
invisible today only because `initialize()` never runs.

**Staleness, scoped to the engine's own root.** The engine records the loaded vminit image
digest under its state root and regenerates `initfs.ext4` only when it differs, stating why.
Unconditional deletion would regenerate a 178 MB image on every start (`du -sh ~/.arca/vminit`
→ `178M`, 2026-08-12), and `initBlock(at:)` already throws `ContainerizationError.exists` and
reuses the file otherwise, so unconditional deletion is a cost with no correctness gain.

### 2.5 vminit is a startup input; `image load` is for workspace images

`ContainerManager.initialize()` requires `arca-vminit:latest` in the store (`:242`), and the
only code that loads it is `ArcaDaemon.swift:110-128` — which `ArcaEngine` must not depend on.
Parent design §3.1 forbids that edge and `tests/release/engine-targets-check.sh` enforces it
against both `arca-engine` and `ArcaEngine`.

The engine loads `--vminit-layout` into its own store at startup, before constructing any
manager, mirroring `ArcaDaemon`'s ordering ("Load custom vminit image BEFORE creating any
ContainerManagers", `:81-82`). It reaches `ImageManager.loadFromOCILayout` in
`ContainerBridge`, on the allowed side of the edge.

This leaves `arca-engine image load --oci-layout <dir>` (parent design §2.2) doing only what
it was specified for: workspace images. Two concerns, two mechanisms, no overlap and no
install-time ordering step.

### 2.6 How the large artifacts ship is milestone 4's decision, and the seam keeps it reversible

`vmlinux` and the vminit layout are large: `/Applications/Arca.app/Contents/Resources/` holds
`vmlinux` at 26.9 MB and `vminit.zip` at 163.1 MB (`ls -lh`, 2026-08-12); `assets/README.md`
quotes ~15 MB and ~120 MB compressed for distribution.

Building them from source is not available to us. `scripts/build-kernel.sh:16-20` exits 1
unless Apple's `container` tool is installed — the dependency parent design §2.5 says
bundling the engine *removes* — and `assets/README.md` adds Go 1.24+, the Swift Static Linux
SDK, ~10 GB of disk and 20-25 minutes. Putting that in `scripts/build-arca-engine.sh` would
break CI's `engine` job and reintroduce what we are eliminating.

Two shipping options remain, and **this design does not choose between them**:

- **Vendor** the artifacts into Gas Can's `.pkg` from a maintainer-built copy.
- **Pin and fetch** them from Arca's GitHub releases, checksum-verified, alongside
  `engine/arca-pin.json`'s existing signed-tag trust model. Arca's `Makefile:214-227`
  `build-assets` target already produces `assets/vmlinux-arm64.gz`,
  `assets/vminit-oci-arm64.tar.gz` and `SHA256SUMS` for this purpose, and `assets/.gitignore`
  records the intent ("uploaded to GitHub Releases instead"). **That channel is not populated
  today**: `gh release view v0.2.4-alpha --repo Vas-Solutus/arca --json assets` lists only
  `arca-v0.2.4-alpha.dmg` and its `.sha256` (2026-08-12). Taking this option makes publishing
  the raw assets an Arca-side prerequisite.

The choice is deferred because §2.2's seam makes it free to defer: the two differ only in what
the installer places on disk and what it writes into the launchd plist. **The engine's code is
identical under both.** Milestone 4 chooses; it may not invent a third mechanism that reads
the artifacts from anywhere other than the paths given by `--kernel-path` and
`--vminit-layout`.

Not chosen: reading them out of an installed `Arca.app`. It reintroduces the prerequisite
§2.5 removes and couples Gas Can to a GUI bundle's internal layout.

Development and the live tier need no decision here: `~/.arca/vmlinux` and `~/.arca/vminit`
exist on the development machine, and the latter is a valid OCI layout
(`cat ~/.arca/vminit/oci-layout` → `"imageLayoutVersion": "1.0.0"`; `index.json` reports
`schemaVersion 2` with an image index, 2026-08-12).

## 3. The `ContainerBridge` changes

**Four land in this milestone.** The fifth, `signalExec`, belongs to milestone 3 and is listed
so that the `ContainerBridge` surface this work opens is stated in one place rather than
discovered twice.

`ContainerBridge` is shared with Arca's Docker surface, so each change is stated with its
blast radius. **None takes a default**: a default is how a caller silently keeps the old
behaviour after the reason for it has gone.

| # | Change | Why | Other callers |
|---|---|---|---|
| 1 | `ContainerManager` takes an image-store root and passes it as `root:` to `Containerization.ContainerManager` | §2.4 — isolates `initfs.ext4` and fixes the two-stores defect | `ArcaDaemon` passes the default root explicitly |
| 2 | `ContainerManager` takes the layer-cache path | `:247` hardcodes `~/.arca/layers`, so a `dev.gascan`-rooted engine would still write into Arca's tree | `ArcaDaemon` passes `~/.arca/layers` explicitly |
| 3 | `listContainers` gains `includeInternal: Bool` | review I3a — `showInternal` is false unless a label filter mentions `com.arca.internal` (`:531`), and `:556` then drops every container labelled `com.arca.internal=true` | three Docker sites pass `false`; `docker ps` (`ContainerHandlers.swift:72`) passes the filter-derived value, since it turns internal containers on today and must keep doing so |
| 4 | `listNetworks()` becomes `throws` and propagates the backend error | review I3b — `NetworkManager.swift:553` swallows a WireGuard-backend failure with `try?`, turning a real failure into a clean empty answer | Docker surface handles or propagates |
| 5 | `ExecManager.signalExec(execID:signal:)` | parent design §3.1 — **milestone 3, not this one** | none yet |

On change 3, the review proposed passing `filters: ["label": ["com.arca.internal"]]` instead.
That is rejected: it works by `contains` on a substring of a filter value, which is exactly the
narrower-instrument-than-claim defect `START-HERE` records this project paying for repeatedly.
An explicit parameter cannot be satisfied by accident.

On change 4, the `try?` is latent today because both backends are nil (review C1). Under full
`initialize()` the backends are populated and it becomes live: a WireGuard failure would drop
every bridge network from `ListResources` while reporting success.

## 4. Engine startup sequence

1. Validate `--state-root`, `--kernel-path`, `--vminit-layout`. Any missing or unreadable →
   refuse, naming which and the path tried (§2.3).
2. Open `StateStore` under the state root; construct `ImageManager` rooted at
   `<state-root>/images`.
3. Load `--vminit-layout` into that store (§2.5). Verify the loaded reference is
   `arca-vminit:latest`; a different reference is a refusal, not a warning —
   `ArcaDaemon.swift:131` logs "Loaded vminit has unexpected reference" and continues, which
   this engine must not do.
4. Compare the loaded vminit digest against the recorded one; regenerate `initfs.ext4` only
   on mismatch, logging the reason (§2.4).
5. `initialize()` the three managers, with `ContainerManager` rooted at the engine's image
   store and layer-cache path (§3, changes 1 and 2).
6. Bind the socket and serve.

Steps 1-4 all precede any manager construction, so a bad input costs a clear error rather than
a partially-initialised engine.

## 5. Per-RPC behaviour

Unchanged from parent design §5 except where a review finding applies.

**`Inspect`** returns the sandbox, `Absent`, or an error.

I4 and I5 below are **constraints on the restored method, not descriptions of live code**.
Milestone 1 deleted the body they were found in: at `cc316b6` `SandboxEngineService.swift`
contains no `ports` assignment and no call to `imageDigest`, and `inspect` returns
`notImplemented`. The review reported them against `f5fde96`. They are recorded here because
the obvious way to restore the method reintroduces both.

- **Ports (review I4).** The removed body hardcoded `sandbox.ports = []`.
  `crates/gascan-arca/src/translate.rs:436-437` reads that field as truth — an empty list means
  "publishes nothing", not "unknown" — so a sandbox publishing 8080→80 would be reported as
  publishing nothing and every port-drift comparison would run against a fabricated value. The
  restored method maps `hostConfig.portBindings`;
  `ContainerManager.convertPortBindingsToMappings` (`:881`) already performs exactly this
  mapping for the restore path.
- **Ordering (review I5).** The removed body ran the image-digest check before the owner-label
  read. A container under a colliding name created from a tag makes
  `imageDigest(fromReference:)` — which survives at `EngineTranslation.swift:27` — return nil,
  and the engine then answers `invalid_output`, the code `crates/gascan-arca/src/error.rs`
  reserves for "the engine sent me something I cannot interpret". The consumer cannot
  distinguish a broken engine from a foreign resource, which is the judgment
  `engine.proto:143-148` leaves with the consumer. **The restored method reads the owner labels
  first.** Ordering is the whole fix.

**`ListResources`** reports containers, volumes and networks, including unlabelled and
internal ones (§3, changes 3 and 4). Filtering engine-side breaks Gas Can's drift detection
silently.

**`Create`** runs volumes → network → container, reporting whatever succeeded in
`CreateFailed.created` (`engine.proto:279-286`). Offline means no network attachment. Ports
publish on loopback.

**`Remove`** refuses any resource whose stored labels differ from the caller's.
**`PrepareImage`** looks the digest up, materialises the rootfs, and fails when absent; it
never pulls.

**`Capabilities`** flips `project_mount`, `named_volumes`, `loopback_publish` and
`resource_limits` to true. `tty` and `signals` stay false until milestone 3 lands `Exec`;
`offline` stays `ISOLATION_UNVERIFIED` until milestone 4's proof exercise. A flag is flipped
only when a live test drives the capability it names.

## 6. Error handling

Unchanged from parent design §6, restated because it is the part a new method most easily
breaks:

- The vocabulary is fixed by the client — the twelve codes `crates/gascan-arca/src/error.rs:20-55`
  accepts, and anything else becomes `invalid_output` naming the offender.
- **A thrown Swift error is a contract violation.** An uncaught `throw` in a provider method
  becomes a gRPC status, which `engine.proto:52-58` reserves for transport faults. Every
  method catches everything.
- `resource` and `message` are not interchangeable; a transposition passes a `contains`
  assertion on both sides.
- Every response sets its `oneof`.

## 7. Testing

### 7.1 Arca

Translation both directions, the identity rule, the error table including the no-escaping-throw
property, and the startup sequence of §4 — including that the engine's image store is its own,
asserted on the resolved store path rather than by reading the constructor.

### 7.2 Gas Can live tier

`crates/gascan-arca/tests/live/` gains create → start → inspect → stop → remove over a real
socket, a partial-failure case asserting `CreateFailed.created`, and `Inspect` reporting real
ports. Every live test is `#[ignore]`d with a reason naming its requirements and registered in
`tests/ci/expected-ignored-tests.txt`.

### 7.3 Guards proved capable of failing

Standing rule: ship no instrument that has not been seen to fail. Each guard below is reverted
alone, the test confirmed red, and the guard restored. The revert and the observed failure are
recorded in the handoff.

| Guard | Reverted alone, must fail |
|---|---|
| `includeInternal` (§3.3) | a container labelled `com.arca.internal=true` disappears from `ListResources` |
| `listNetworks` propagation (§3.4) | an induced backend failure reports a clean list instead of `command_io` |
| owner-before-digest (§5, I5) | a tag-created foreign container comes back `invalid_output` |
| port mapping (§5, I4) | a sandbox publishing 8080→80 reports no ports |
| image-store root (§2.4) | the engine boots on the shared-store vminit rather than its own |
| input validation (§2.3) | a missing kernel or vminit layout starts the engine anyway |

The image-store-root row is the one most likely to pass for the wrong reason: on a machine
where both stores happen to hold the same vminit, a test that only checks "a container boots"
cannot distinguish them. It asserts on the resolved path and on a store seeded with a
*distinguishable* vminit reference.

### 7.4 Baseline

A green local `cargo test --workspace --no-fail-fast` (`env -u RUSTUP_TOOLCHAIN`, run when
`pgrep -fl "cargo test"` is empty) plus `swift test --filter ArcaEngineTests` at exit 0. CI
reports but does not gate — `START-HERE`'s CI section records why.

## 8. Sequencing

Five landings, each green before the next.

1. The four `ContainerBridge` changes of §3, with Arca's own tests and both call sites updated.
2. Engine startup: the three options, vminit load, digest-keyed `initfs.ext4`, fail-fast
   validation, full `initialize()` (§2.2-2.5, §4).
3. `Inspect` and `ListResources` restored, with review I3, I4 and I5 fixed (§5).
4. `image load`, `PrepareImage`, and `Create`/`Start`/`Stop`/`Remove` (§5).
5. The Gas Can live tier and the capability flips (§5, §7.2).

Landing 1 is separable and lands in Arca alone. Landing 3 is the first point at which the
engine reports anything about state, and therefore the first point at which a wrong answer is
possible — it follows startup deliberately, so that "the engine holds no state" and "the engine
answered wrongly" are never confused.

## 9. Out of scope

- **`Exec` and `Logs`** — milestone 3, including `signalExec` (§3, change 5).
- **Daemon wiring, packaging, the launchd plist, and the offline proof** — milestone 4.
- **Which mechanism ships the large artifacts** — milestone 4, constrained by §2.6.
- **P5.3 conformance**, **U5**, **P6's network model**, and the duplicated `sandbox_id`-claim
  rule — unchanged from parent design §11.
- **Arca's own CI.** Arca has none (`gh pr checks 56` reported no checks; `.github/workflows`
  does not exist in that repository, 2026-08-12). Gas Can's `engine` job remains the only
  automated thing exercising Arca, and §7.1's tests run inside it.

## 10. Documents this work must correct

- `docs/status/START-HERE.md` — the "read-only ContainerBridge load path" framing is retired
  by §2.1, and review findings I3, I4 and I5 are no longer dissolved: I3 and I4 are fixed on
  their merits in §3 and §5, and I5's ordering fix returns with `Inspect`.
- `docs/superpowers/plans/2026-08-10-p5-1-milestone-1-engine-skeleton.md` — its milestone 2
  outline predates the state-ownership decision and does not mention it.
- The parent design's §7 sequencing, for the same reason.
