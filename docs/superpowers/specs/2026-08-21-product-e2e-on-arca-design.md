# The product-level `gascan-e2e` suite on arca

Date: 2026-08-21
Status: Design, approved in brainstorming; implementation plan not yet written
Branch: `docs/product-e2e-on-arca-design`
Derived from `main` at `899a29f` (`git log -1`), working tree clean, no open PRs
(`gh pr list` → `[]`).

**This is P5's second exit clause.** P5's exit is *"`gascan-arca` passes conformance
and existing `gascan-e2e`"*. P5.3 covered and measured the first clause and merged as
PR #90 (`fe27646`). The second — the product-level suite running on arca — was
explicitly out of scope there (that design's §5) and is untouched.

> **BLOCKED, 2026-08-21, after this design was written and committed at `578cd14`.**
> §4 assumes the real workspace image can create a container on arca. **It cannot.**
> The engine attaches one block device per OCI layer against a 26-letter alphabet, so
> the 35-layer approved image fails at `create` with `no free indices are available for
> allocation`. Measured, with the controlled experiment and mechanism, in §4.1 below.
>
> The maintainer's decision (2026-08-21): **revert arca to upstream Containerization's
> single-composed-rootfs approach** rather than raising the ceiling or shrinking the
> image. That is arca-side work in a separate repository and is a **prerequisite** to
> everything else here. Scope is measured in §4.1; the handoff is
> `docs/status/2026-08-21-arca-layer-ceiling-handoff.md`.
>
> **Everything outside §4 survives unchanged** — the measurements in §1, the structure
> in §3, the backend proof in §5, the naming in §6, and the acceptance and failure
> discipline in §7 are unaffected.

---

## 0. Why this exists, stated once and plainly

Gas Can runs each project's sandbox on Apple's `container` runtime today, a program
the user installs and starts separately; `README.md:13-21` records the cost — macOS 26+,
`container >=1.1.0,<2.0.0`, one certified commit, and `gascan doctor` warning when the
version drifts. The destination is to stop depending on it: *"Gas Can ships one signed,
notarized package containing its own binaries and a bundled sandbox engine that Gas Can
built itself"* (`docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md`,
"Destination"). That engine is arca.

Two levels of evidence exist, and the gap between them is the whole problem.

- **The backend satisfies the interface.** P5.3's conformance suite calls the
  `RuntimeBackend` methods directly and checks each backend behaves the same.
- **A person can use the product.** `gascan-e2e` drives the real CLI as a user does —
  real daemon, real container, real workspace image.

There are **11** live product tests and all 11 run only against Apple. Arca has **2**,
and both are engine plumbing rather than product behaviour. So nothing has ever
demonstrated that the product works on the engine the project intends to ship. Closing
that is what this design is for.

**The maintainer has stated Apple's runtime will not be supported once arca works**
(2026-08-21, this session). Firecracker-on-Linux is a possibility, not a plan, and is
not designed for here. That decision shapes §3: the goal is to quarantine the
Apple-specific code so it can be deleted in one cut, not to build a permanently
pluggable abstraction.

---

## 1. What was measured, because two recorded numbers were wrong

Everything in this section was re-derived on 2026-08-21 at `899a29f`.

### The ignore counts

`grep -c '#\[ignore'` over `crates/gascan-e2e/tests/*.rs` reports apple 11 and arca 3.
**Arca's real count is 2.** The third match is `crates/gascan-e2e/tests/arca_startup.rs:10`,
a doc comment reading ``it is `#[ignore]`d``, not an attribute. Anchoring the pattern —
`grep -cE '^[[:space:]]*#\[ignore'` — gives:

| File | Ignore attributes |
|---|---|
| `apple_apply.rs` | 8 |
| `apple_lifecycle.rs` | 1 |
| `apple_recovery.rs` | 1 |
| `apple_security.rs` | 1 |
| `arca_engine.rs` | 2 |
| `arca_startup.rs` | **0** |

`docs/status/START-HERE.md` records arca's as 3 and is off by one. The P5.3 design's §5
cites 24 and 6 from an earlier revision and is stale, as that file already warns.

### The four `.env()` calls are real, and are not the whole delta

The P5.3 design says the two `command()` builders *"differ by exactly four `.env()` calls"*.
Re-derived: arca adds exactly four, unconditionally, at `arca_common/mod.rs:326-329` —
`ARCA_BACKEND_ENV`, `ENGINE_BIN_ENV`, `ENGINE_SOCKET_ENV`, `ENGINE_STATE_ROOT_ENV`.
That number is correct.

**It is not symmetric, and the design's phrasing hides two differences going the other
way.** Apple additionally carries an `error_diagnostics` branch
(`apple_common/mod.rs:987-991`) that arca has no equivalent for, and gates
`GASCAN_E2E_CANDIDATE_IMAGE` on the process environment where arca sets it
unconditionally from `self.image`. Three differences, not one.

### And the `.env()` delta is the small part of the gap

`AppleE2e` reads ground truth out-of-band through the `container` CLI, runs against the
real digest-qualified workspace image, and enables SSH. `ArcaE2e` builds a stock Alpine
whose guest-side contract is stubbed: four of the five provisioning programs are
`#!/bin/sh\nexit 0` and `sudo` is an `exec "$@"` passthrough
(`arca_common/mod.rs:79-96`). It sets `ssh.enabled = false` (`:223`) with
`user = "root"`. Parameterising `command()` yields a daemon pointed at arca; it does
not yield a test that can assert anything.

### The predecessor-image constraint, which sets the scope

`scripts/run-apple-e2e.sh` exports `GASCAN_E2E_PREDECESSOR_IMAGE` only inside the
`if test -n "${GASCAN_E2E_CANDIDATE_IMAGE_FILE:-}"` block (`:10`–`:62`, export at `:59`).
Without a release-candidate receipt, the three tests reading that variable
(`apple_apply.rs:47`, `:840`, `:1189`) fail on its absence, and
`validate_distinct_image_fixtures` (`apple_common/mod.rs:205`) additionally requires two
**distinct** digests — a second image of the same order of size. So **only 8 of the 11
apple live tests can run on this machine today**, on either backend.

### The fixture surface, which sets the design

38 distinct fixture methods are called across the four apple test files. Restricting to
the 8 in-scope tests leaves **27**, of which exactly **5** touch the Apple runtime:

| Method | The question it asks | Apple's mechanism |
|---|---|---|
| `assert_no_owned_resources` (`:1368`) | Is anything of ours left behind? | `container list --all --format json` |
| `assert_managed_network_attachment` (`:1163`) | Is our container on the managed network? | `container inspect` |
| `assert_no_network_attachments` (`:1190`) | Is it on no network at all? | `container inspect` |
| `native_ssh_endpoint` (`:1194`) | Where did SSH publish? | `container inspect` |
| `stop_owned_container` (`:755`) | Stop it behind the daemon's back | `Command::new("container")` |

The remaining 22 — `success`, `status_json`, `invoke`, `write_manifest`, `kill_daemon`,
the PTY helpers, the sentinel recorders, the path accessors — drive the CLI or touch
host files and are indifferent to the backend.

**The heavyweight Apple machinery is needed only by the 3 excluded tests**:
`owned_runtime_snapshot`, `replace_owned_container_image`, `seed_stored_image_resolution`,
`start_default_network_probe`, `assert_owned_container_running`,
`assert_image_replace_root_sentinel`, `write_image_replace_root_sentinel`,
`assert_default_network_cannot_reach_native_ssh`, `run_default_shell_pty_script_with_output_guard`,
`command`, `invoke_with_timeout`. It stays in `apple_common` untouched. Most of that
4746-line file is not restructured by this work.

### Live prerequisites, verified on this host 2026-08-21

- Engine binary: `.artifacts/arca-engine/arca/.build/arm64-apple-macosx/release/arca-engine`;
  pin `8fc1ca58b9e9d7029432a88838d1cb81713bbd75` (`.artifacts/arca-dev-pin.json`).
- Kernel and vminit: `vmlinux` (26.9 M) and `vminit/` under
  `~/Library/Application Support/dev.gascan/engine/`.
- `/tmp/alpine-oci` present with `index.json`, `oci-layout`, `blobs/`.
- `container system status` → `running`.
- `skopeo` at `/opt/homebrew/bin/skopeo`.
- The approved workspace image is **publicly pullable, no auth**, and a single-platform
  `linux/arm64` index: `skopeo inspect --raw docker://ghcr.io/liquescent-development/gascan/workspace@sha256:84f6b685002369aff5daa38add02c93d51caeac2c842d0eed49633493e7303da`
  exited 0 with one arm64/linux manifest. It is **35 layers, 2935.4 MiB compressed**
  (`skopeo inspect`, `LayersData` sizes summed).
- `arca-engine --help` lists exactly two subcommands, `serve` and `image load`. **There
  is no container list or inspect CLI**, so Apple's out-of-band mechanism has no direct
  counterpart; see §3.

---

## 2. Decisions taken

| # | Decision | Rationale |
|---|---|---|
| 1 | The arca product tier runs against the **real approved workspace image** | With the stubs, assertions about provisioning check that a no-op program exited 0. The tier would close P5's clause in name only. |
| 2 | Scope is the **8 tests needing no predecessor image** | The other 3 cannot run on this machine on either backend (§1). Excluding them costs no arca-specific coverage. |
| 3 | **One test body, per-backend fixtures** (approach A) | §3. |
| 4 | Apple's tier is **diagnostic only** — never a merge gate, never fixed for its own sake | Apple is being retired; its results exist to disambiguate arca failures. |

Rejected: duplicating the 8 tests into arca copies. It carries the retyping risk of a
migration and the maintenance cost of an abstraction while providing neither's benefit —
dominated by both alternatives.

---

## 3. Structure

Three pieces.

1. **`ProductE2e`** — one shared fixture holding the 22 backend-neutral methods:
   tempdirs, manifest, sandbox id, owner token, CLI invocation, PTY, host-side sentinels,
   path accessors.
2. **`RuntimeInspector`** — a trait with exactly the 5 methods of §1's table. That is the
   entire per-backend surface.
3. **Two implementations** — `ContainerCliInspector` (today's code, relocated) and
   `ArcaEngineInspector`.

The test bodies are **re-pointed, not retyped**. What changes is the type of the fixture
they are handed; the assertions themselves are not edited. This is the substantive
argument for this shape over a migration: a retyped assertion that silently drops a
condition still passes, and a port that asserts less than the original looks identical to
one that asserts the same.

Because Apple is being retired, the trait's purpose is **isolation for deletion**, not
extensibility. When Apple goes, `ContainerCliInspector` and the Apple entry points are
removed and the trait may be collapsed back to a concrete type. A `match` on a backend
enum inside each of the 5 methods was rejected for the same reason: it would scatter the
Apple code across every method, making that deletion a diffuse edit rather than removing
a file.

### How arca answers the 5 questions

All five map onto RPCs the `gascan-arca` client already exposes — `list_resources`,
`inspect`, `stop` — which is the mechanism the existing arca live tests already use
(`crates/gascan-arca/tests/live/lifecycle.rs:358-366`).

**A limit to record rather than smooth over.** Apple's check is arms-length: the test
runs `container inspect` while the product runs `container run`. Arca's check goes
through the *same* RPCs `gascand` itself calls, so a defect inside `ListResources` would
be invisible to both the product and the test verifying it. The arca inspector is
therefore weaker evidence than Apple's.

This is accepted rather than fixed. The alternative — reading the engine's
`stateRoot/state.db` (`arca/Sources/ArcaEngine/EnginePaths.swift:91`) — would couple the
suite to Arca's private schema across the protocol boundary the project deliberately
drew (`roadmap`, revision note 2026-08-05: Gas Can consumes arca *"across a protocol
boundary"*). Independence bought that way costs more than it returns.

---

## 4. Getting the real workspace image into the engine

Once per session — 35 layers and 2.9 GiB make per-test provisioning untenable.

1. `skopeo copy --override-os linux --override-arch arm64 docker://ghcr.io/liquescent-development/gascan/workspace@sha256:84f6b68… oci:<cache>:workspace`
2. `arca-engine image load --state-root <state> --oci-layout <cache>` — the path
   `gascan_oci_fixture::load_image` (`crates/gascan-oci-fixture/src/lib.rs:441`) already
   uses for Alpine.
3. The layout is cached outside the per-test tempdir and reused across a run.

This replaces `workspace_contract_entries()`' stubs with the five real provisioning
programs, which is what makes the assertions mean anything.

### This does not resolve U5, and the spec must not be read as claiming it does

U5 — how image digests reach the engine without registry access — is P5.4's, and
`roadmap:499-506` records it as a genuine spec gap. The harness sidesteps it legitimately
because it owns its own machine and may run `skopeo` and `image load` directly. **A
shipped `.pkg` cannot.** A green arca product suite is evidence about the product on
arca; it is not evidence that U5 is closed.

### The contradiction with the roadmap, stated deliberately

`roadmap:505` records U5 as ***Blocks:* P5 exit**. This design routes around that by
having the harness provision the image itself, and therefore asserts that P5's second
exit clause can be closed while U5 remains open.

The reasoning: U5 is a *shipping* question — how a user's engine comes to hold the image.
P5's exit clause is a *verification* question — whether the product works on arca. The
harness's ability to answer the second without answering the first is real, and the
`image load` path it depends on already exists and is already used.

This is written here rather than left implicit so that the roadmap is not silently
treated as satisfied. If the maintainer disagrees, this design is what should change —
not the roadmap quietly.

---

## 4.1. The layer ceiling that blocks §4, and the decision taken

**The controlled experiment.** Same test, same binaries, gascan `899a29f`, arca pin
`8fc1ca5`, host `newcombe` (`hostname -s`). One variable changed,
`GASCAN_ARCA_BASE_OCI_LAYOUT`:

| Base layout | `cargo test -p gascan-e2e --test arca_engine -- --ignored` |
|---|---|
| `/tmp/alpine-oci`, 1 layer | `2 passed; 0 failed ... finished in 6.44s` |
| workspace image, 35 layers | `create failed with exit code None: no free indices are available for allocation` |

**The mechanism, from the engine's own log:**
`layer_devices_start=/dev/vdc layers=36 total_mounts=38 writable_device=/dev/vdb`.
One block device per OCI layer, composed by OverlayFS inside the guest. The 36 is 35
image layers plus one the fixture adds. `vda` is initfs and `vdb` the writable
overlay, leaving **24** letters for layers. Tags come from
`Array("abcdefghijklmnopqrstuvwxyz")`
(`containerization/Sources/ContainerizationExtras/NetworkAddress+Allocator.swift:96-101`);
exhausting it throws `AllocatorError.allocatorFull`, whose text is the message above
(`AddressAllocator.swift:49-50`).

**Unpacking is not the problem.** All 36 layers unpacked in 18.74s
(`duration_seconds=18.74 layers=36`); the failure is at attachment.

**This is fork-local design, not a regression.** `git grep` against `upstream/main` in
the `containerization` submodule (`Vas-Solutus/arca-containerization`, upstream
`apple/containerization`) returns no match for `OverlayFSUnpacker`,
`ArcaBlockDeviceRole` or `ArcaLayerAttachment`; all are present at fork HEAD
`6304122`. The 26-letter allocator is upstream's — upstream stays under it because
`LinuxContainer` takes a single `rootfs: Mount` plus an optional writable upper, so
layer count never reaches the allocator.

**Why the fork's optimisation does not pay here.** Upstream caches one ext4 per
*image* (`EXT4Unpacker` unpacks to a block path and refuses if one exists,
`:152-153`). The fork caches one ext4 per *layer*, which wins when many derived
images share base layers — a registry workload. Gas Can has one pinned workspace
image, so cross-image layer sharing is worth nothing, and upstream's per-image cache
delivers the same benefit while attaching 1 device instead of 36.

**A second thing the revert fixes.** `PrepareImage` cannot materialise a rootfs
today, and `arca/Sources/ArcaEngine/SandboxEngineService.swift:594-608` documents why:
the unpacker is per-*container*, creating `upper`/`work` at a container path and
incrementing a per-layer reference count, and its per-image half is private upstream.
Upstream's per-image unpack is what `PrepareImage` would need to keep the promise the
contract states.

### Measured scope of the revert

**Submodule `arca-containerization`** — host side, almost entirely additive:
`OverlayFSUnpacker.swift` 441, `ArcaLayerAttachment.swift` 95,
`ArcaBlockDeviceRole.swift` 73, `LayerUnpackFailure.swift` 44,
`Formatter+Unpack.swift` 26 (≈679 new lines), plus `EXT4Unpacker.swift` 31 modified
lines to revert. Guest side: `vminitd/Sources/VminitdCore/ArcaBoot.swift`, 312 lines
and **absent upstream** (`git cat-file -e upstream/main:…` fails), 49 overlay
references; `AgentCommand.swift` 18 references; `Server+GRPC.swift` 12.

**Parent repo `arca`** — `Sources/ContainerBridge/ContainerManager.swift` carries 52
references, but **34 sit in one block, `1231-1353`**, the create path; the remainder
are scattered singles. `Sources/ContainerBridge/OverlayFS/` holds
`OverlayFSMounter.swift` 227 (one external caller, `ContainerManager.swift:1266`,
creating `writable.ext4`), `OverlayFSClient.swift` 197 and 525 lines of generated
protobuf — and **`OverlayFSClient` has no external references at all**, so ~722 of
those lines appear already dead.

Roughly **2,200 lines** of fork-local code across two repositories and both sides of
the VM boundary. Most of it is deletion rather than rewriting, because upstream
supplies the replacement. The fork is **70 commits ahead of upstream and 0 behind**
(`git rev-list --count`), so this is the cheapest moment to do it — there is no
upstream backlog to reconcile first.

---

## 5. Proving arca actually ran

`backend_selection` returns `Apple` from its `(false, false)` arm
(`crates/gascan-core/src/backend.rs:168`, in the function opening at `:156`). A dropped
variable therefore tests Apple and **passes**. Conformance was immune because each
instantiation constructs its backend in code; this tier is fully exposed.

Two mechanisms, because either alone is insufficient.

**Explicit selection.** `ProductE2e::new()` takes a `Backend` argument. The harness never
reads a bare environment variable and never falls back; the `(false, false)` arm is
unreachable from test code.

**Positive proof from the daemon's own record.** After the first command that starts a
daemon, the fixture reads `GASCAN_DAEMON_INSTANCE_PATH` and asserts `backend == "arca"`.
That field is the daemon reporting what it actually constructed —
`crates/gascand/src/api.rs:43`, commented *"Which backend this daemon actually runs"*,
written at `:96` from `identity.backend.as_str()`, which maps `Arca => "arca"`
(`backend.rs:140`). The record is `#[derive(serde::Serialize)]` (`api.rs:28`) and its
path is read at `api.rs:58`. Both harnesses already point that variable at
`runtime_root/daemon-instance.json`.

The assertion lives in the fixture, not in each test, so a newly added test cannot omit it.

### The guard is mutation-tested, and this is not optional

A guard that never fires is worth exactly what no guard is worth. Before any arca result
is believed: remove the arca selection, run, and confirm the suite goes **red**. If it
goes green, the guard is decorative and every downstream result is void.

This is stated as a required step because a negative result from an untested guard is
indistinguishable from a passing run, and because a probe that does not force the work to
actually happen can report success for a mutation that does fail.

---

## 6. Naming, plumbing, and the CI manifest

Tests become `<name>_on_apple` / `<name>_on_arca`, so a failure names its backend without
cross-referencing anything.

**`tests/ci/expected-ignored-tests.txt` must be updated.** It lists 50 bare test names —
`apply_installs_large_npm_tool_and_neovim_with_storage_override`, one of the 8, is among
them — and `scripts/ci-check-ignored-tests.sh` fails **in both directions**, so renamed
tests trip it as both a disappearance and an appearance. That is the intended behaviour
and means the omission cannot pass silently.

**`scripts/run-arca-e2e.sh`**, mirroring `run-apple-e2e.sh`: preflight the engine binary,
`skopeo copy` the approved image into a cached layout under `.artifacts/` if absent,
export `GASCAN_ARCA_ENGINE_BIN` and the workspace layout path, run the tier with
`--ignored`. One command rather than a hand-assembled environment — a human exporting
four variables by hand is the other entrance to §5's trap.

---

## 7. Acceptance, and what happens when a test fails

**Done** is the maintainer's standing bar (`START-HERE.md`, priority set 2026-08-21): a
green local run **plus actually running the thing changed**. Concretely:

1. The 8 tests pass on arca, with §5's backend proof in force.
2. §5's mutation test has been run and the suite went red without arca selection.
3. A manual `gascan up` on arca produces a usable sandbox.
4. Apple's tier still runs, for §3's diagnostic purpose. **Its result is not a gate.**

CI is deferred and is not a criterion.

### Failure discipline

These 8 tests have never run against arca; some will fail. Each failure sorts into
exactly one of three, and only the first is a test edit:

1. **The test encoded an Apple assumption.** Fix the test; record what was Apple-specific.
2. **Arca genuinely behaves differently.** That is a **finding**, recorded with evidence —
   not widened into green. This is the rule P5.3 already paid for: its `create`-state
   assertion is left failing on two backends on purpose, because a suite edited until it
   agrees measures nothing.
3. **The harness is wrong.** Fix the harness.

Nothing is loosened to make a run pass.

### Open item 10 is downstream of this work, not upstream

All three backends report a different state after `create` — fake `Stopped`, apple
`Running`, arca `Creating` (`crates/gascan-conformance/src/lib.rs:125-141` and
`docs/evidence/2026-08-20-backend-conformance.md`). **This does not block the product
tier.** The `up` path inspects after create and starts unless already running
(`crates/gascand/src/service.rs:1462-1469`):

```rust
let current = self.runtime.inspect(id).await?...;
if current.state != ContainerState::Running {
    self.runtime.start(id).await?;
}
```

That absorbs all three states identically; `up` never requires them to agree.

It runs the other way instead. Arca's `Creating` means `gascand` issues `Start` against a
container the engine still reports as mid-creation. Whether the engine accepts that is a
question **this suite answers with evidence**, and settling item 10 by argument first
would settle it without that evidence.

---

## 8. Out of scope

- **The 3 predecessor-image tests.** `apple_apply.rs:46`, `:839`, `:1188`. They need a
  second distinct digest that does not exist on this machine, and the release-candidate
  flow that produces one. A follow-up, not a silent omission.
- **U5 / P5.4.** §4.
- **Deleting Apple's backend or its tier.** The retirement decision is recorded here
  because it shapes §3, but the deletion is separate work with its own sequencing.
- **CI.** Deferred by the maintainer 2026-08-21.
- **The `list_resources` visibility gap** recorded as a follow-up in the P5.3 design's §4.
  Related, since §3 uses `list_resources`, but it is that follow-up's to close.
- **Production changes to `gascan-arca`.** A failure is a finding (§7), fixed as its own
  work.
