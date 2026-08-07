# Arca Engine Codegen — design

Date: 2026-08-07
Status: Implemented
Roadmap step: **P3.2**, `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md`

Governing contract: `docs/superpowers/specs/2026-08-04-sandbox-engine-contract.md`.
The proto this generates from: `docs/superpowers/specs/2026-08-07-arca-engine-proto-design.md`.
The pin this crosses: `docs/superpowers/specs/2026-08-05-arca-engine-pin-design.md`.

P3.1 produced the contract and verified that both generators run against it. It
deliberately wired neither into a build. This is that wiring.

Every claim below is marked **VERIFIED** or **PLAN**. Past-tense claims carry
their anchor inline.

## 1. Scope

P3.2 is "codegen wired both sides — Swift server, Rust client". P3's exit is
*proto exists, both sides generate, nothing implements it yet*, and this step
delivers the second clause without touching the third.

Two decisions were taken with the maintainer before drafting:

| # | Decision |
|---|---|
| 1 | The Rust build reaches the proto **across the pin at build time**, via a script that `build.rs` invokes. Not a vendored copy, and not a manual prerequisite step. |
| 2 | The generated Swift server code lives in **its own SwiftPM target**, `SandboxEngineProto`, not inside `ContainerBridge`. |

## 2. The blocker P3.1 predicted, re-confirmed

**VERIFIED 2026-08-07.** The proto was absent at the pinned revision:

```
git cat-file -e d66c320c09e1dfc4f37aafa1fb27e36aa5cabe5d:proto/arca/engine/v1/engine.proto
  ->  rc=128
```

It was re-confirmed a second time by the new sync script itself, which reported
the condition as its own designed failure (`rc=65`, "engine proto is absent at
the pinned revision") before the pin was moved. The blocker was therefore not
merely worked around; the tool built to cross the pin diagnoses it.

Ordering follows from this: **Arca first, then the tag, then Gas Can.** A pin
bump to a revision that predates the generated Swift would leave Gas Can's
`engine` job compiling a tree older than the contract it pins for.

## 3. Decision 1 — how the Rust build reaches the proto

### 3.1 The measurement that opened the design space

`scripts/build-arca-engine.sh` already crosses this pin, and it cannot be reused
here: it clones full history, initialises a submodule, and ends in
`swift build`. **VERIFIED:** its cache, `.artifacts/arca-engine/arca`, is
**1.3 GB** (`du -sh`). Nothing that expensive can sit in front of `cargo build`.

The question was whether full *provenance* is what costs 1.3 GB, or whether it
was only the *build* that did. **VERIFIED 2026-08-07:**

| Step | Result |
|---|---|
| `git fetch --depth 1 --filter=blob:none origin tag <tag>` into an empty repo | **rc=0, 1s, 108 KB** |
| `verify-tag` against `engine/allowed-signers` on that shallow repo | **rc=0**, `Good "git" signature for richard@liquescent.dev` |
| `rev-parse refs/tags/<tag>^{}` on that shallow repo | resolves, so the tag→revision assertion is available |

So the verification survives the cheap fetch intact. **A cheaper fetch is not a
weaker claim; it is the same claim over fewer bytes.** This is the A/B the
project's conventions prefer over an argument, and it is what made decision 1
affordable — without it, the honest options were a manual prerequisite step or a
vendored copy.

### 3.2 `scripts/sync-arca-proto.sh`

Fetch the tag shallow → `verify-tag` against the tracked allowed-signers file →
assert `rev-parse tag^{}` equals `.revision` → assert the proto exists at that
revision → extract `proto/` only. Prints the extract directory on stdout.

It performs **the same three assertions** as `build-arca-engine.sh`. The two
scripts differ in what they materialise, never in what they verify.

**The cache is keyed by revision** — `.artifacts/arca-proto/<revision>/` — so a
pin bump cannot be served by a stale extract and no invalidation logic has to be
written or trusted. `.artifacts/` is already gitignored (`.gitignore:3`).

**Publication is a claim, not a bare `mv`.** `mv dir existing-dir` does not fail;
it moves the source *inside* the target. A lost race would therefore have
produced `.artifacts/arca-proto/<rev>/tree/` silently, and every later build
would have read a path that does not exist. `mkdir` is atomic on POSIX, so a
claim directory decides one winner and the loser waits — **bounded at 60s**,
because a claim whose holder died must surface as an error a person can act on
rather than as a build that hangs. This mirrors `build-arca-engine.sh`'s
reasoning that "a held lock is an error and never a wait", adapted: that script
mutates one shared checkout in place, while two runs of this one produce
identical bytes and race only to publish them.

**Absence is diagnosed before extraction, not after.** `git archive` on a missing
path dies with `pathspec did not match any files`, which is true and says nothing
about which pin is wrong. The check is `cat-file -e` on the exact proto path
first, so the failure names the revision and the expectation. This is the
project's "make it say more before guessing better" habit applied at the point
where the next person will most need it — the next pin bump.

### 3.3 `crates/gascan-engine-proto`

`build.rs` runs the sync script and reads the extract path from its stdout.

**It does not parse the pin.** The script already owns what "the pinned contract"
means; a second parser in Rust would be a second definition, free to disagree.
`build.rs` therefore never reads `arca-pin.json` for content — only as a
`rerun-if-changed` input.

The script short-circuits on a warm cache, so the network cost is **once per pin
bump**, not once per build. Naming any `rerun-if-changed` replaces cargo's
default of watching the whole package, which is correct here: the proto lives
outside the package, and the pin is what decides which proto.

`build_server(false)`. Arca serves this contract from the Swift code generated in
its own tree, so a Rust server would be surface with no implementor and no
caller — and the first thing to accidentally implement it would be a test double
that made a wrong client look correct.

### 3.4 What was rejected, and why

**A vendored copy with a contract test.** It would make `cargo build` hermetic and
offline. It was rejected because it creates the second copy of a published
contract that the P3.1 design explicitly refuses — *"two copies of a contract
drift"* — and because the drift would only be caught when the comparison test
ran, which needs the network anyway. It also contradicts the 2026-08-05 reversal:
nothing of Arca's is copied into Gas Can.

**A manual prerequisite step**, with `build.rs` failing fast and naming the
command. Purest fail-fast, and it keeps cargo offline-capable. Rejected because
`cargo test --workspace` on a fresh clone would fail for a reason cargo could
have fixed itself, which erodes the governing "a green local workspace run counts
as a pass" ritual.

**The cost of the chosen option, stated plainly:** `build.rs` touches the network
on a cold cache, so a fully offline first build fails. That is a real limitation
and it is accepted, not hidden.

## 4. Decision 2 — where the generated Swift lives

`Sources/SandboxEngineProto/`, a target of its own holding only generated code.

This is P3.1's decision 4 applied one layer up. The proto was placed at Arca's
repository root rather than beside the guest-facing protos in
`Sources/ContainerBridge/proto/` because *a published contract is not a
`ContainerBridge` internal*. The code generated from it inherits that argument
unchanged. It also keeps the contract out of the target **P4.3** exists to slim.

Nothing depends on the target yet. `swift build` still compiles it, which is the
point: **a generator that silently emits an empty module also exits 0**, so a
compiled target is a stronger witness than a generated one.

**The new arm of `scripts/generate-grpc.sh` fails rather than skips.** The three
existing arms print `⚠ Skipping` and continue when their proto is missing, which
is right for them — those protos live in a submodule that may legitimately be
absent. This one is tracked at the repository root, so an absent file means a
broken tree, and exiting 0 there would let a consumer pin a revision whose server
code was never regenerated.

`--proto_path` is the repository's `proto/` root rather than the file's own
directory, so the import root stays correct if this contract ever gains a
sibling. That is why the generated files nest under `arca/engine/v1/`. The three
existing arms use the file's directory because each of their protos is alone in
one.

## 5. Consumers that had to move with it

Found by reading, not by CI failure.

| File | Change | Why |
|---|---|---|
| `scripts/ci-classify-paths.sh` | `engine/*` now sets `rust=true` as well as `engine=true`; `scripts/sync-arca-proto.sh` sets `rust` and `contracts` | The pin now decides the Rust build's codegen input. Firing `engine` alone was correct only while nothing in Rust read it. |
| `tests/ci/classify-paths-contract.sh` | expectation updated; two cases added | It asserted the old behaviour |
| `packaging/macos/release-common.sh` | `scripts/sync-arca-proto.sh` added to `inputs`, the tracked-file loop and the ignored-source scan | It is a release input on the same footing as `build-arca-engine.sh` |
| `tests/release/source-input-contract.sh` | fixture seed list and `classes` array | The pin design §4.4 recorded this exact knock-on: adding to the tracked loop without seeding the fixture fails an existing test. It was read and honoured rather than rediscovered. |
| `scripts/build-arca-engine.sh` | also builds `--target SandboxEngineProto` | Otherwise the pinned *server* half is the only end of the contract nothing ever compiles. Gas Can generates and compiles the client from the same revision; this makes the claim checkable from both ends. |

## 6. Testing — two claims, because one failure mode is invisible to the other

`crates/gascan-engine-proto/tests/generated_surface.rs`.

1. **The module carries the client and one message per RPC.** Written as type
   references, so an empty or truncated module fails to *compile* the test file.
2. **The service carries exactly the eleven contract methods**, asserted against
   the emitted `FileDescriptorSet`. A service that lost a method still compiles
   for every caller that never used it, so the Rust types cannot witness this.
   Exactness is asserted in both directions: an extra method is surface the
   policy boundary was never designed to gate.

Plus: the package is `arca.engine.v1`, asserted because the package path *is* the
major version — a change to it is a new major arriving silently.

**Both were shown to flip. VERIFIED 2026-08-07:**

| Mutation | Result |
|---|---|
| Drop `PrepareImage` from the expected set | **FAILED**, `unexpected: ["PrepareImage"] / found 11 methods, expected 10` |
| Reference a message the generator does not emit | **compile error** `E0425: cannot find type ... in module v1` |

The first mutation's message names what diverged and in which direction, which is
the standard this project holds a failure to.

## 7. Verification

Exit codes captured directly, never through a pipe.

| Check | Result |
|---|---|
| `swift build --target SandboxEngineProto` | **rc=0**, 11.75s, no new warnings |
| All 11 RPCs in the Swift output | present as `/arca.engine.v1.SandboxEngine/*` method paths |
| Swift is server-only | **0** occurrences of `ClientProtocol` |
| Swift generation is idempotent | sha256 over both files unchanged on a second run |
| `generate-grpc.sh` with the proto absent | **rc=66**, guest-arm skips unchanged at 6 |
| `sync-arca-proto.sh` against the pre-bump pin | **rc=65**, "engine proto is absent at the pinned revision" |
| `sync-arca-proto.sh` against a signed pin carrying the proto | **rc=0**, extract is **16 KB** |
| `cargo build -p gascan-engine-proto` | **rc=0** |
| `cargo test -p gascan-engine-proto` | **4 passed, 0 failed** |
| `cargo clippy -p gascan-engine-proto --all-targets -- -D warnings` | **rc=0** |

Toolchain: `protoc` 35.1, `protoc-gen-swift` 1.38.1, `protoc-gen-grpc-swift`
**1.27.0** — the version `arca/scripts/generate-grpc.sh:39` requires. Gas Can's
Rust side uses vendored protoc via `protoc_bin_vendored`, so the two sides do not
share a protoc, and neither inherits the other's version constraint.

The Rust arm was validated **before** the real tag existed, against a throwaway
signed tag over a `file://` URL — a shape the pin schema already permits
(`test("^(https|file)://")`). That removed the ordering risk of discovering a
build defect only after an annotated tag had been published, which is the one
artifact in this sequence that should not be cut twice.

## 8. Found and deliberately not fixed

**Arca's checked-in generated Swift is stale against the protos it is generated
from.** Running `scripts/generate-grpc.sh` rewrites **four tracked files** under
`Sources/ContainerBridge/Generated/` and **six `.pb.go` files** in the
`containerization` submodule.

~~**VERIFIED** by running it: the diff is entirely `nonisolated` annotations
introduced by `protoc-gen-swift` 1.38.1.~~ **WRONG, corrected 2026-08-07.** That
claim came from sampling `wireguard.pb.swift` and generalising to four files.
**VERIFIED** against the raw diff, the drift is two different things:

| File | Change |
|---|---|
| `wireguard.pb.swift`, `process.pb.swift` | `nonisolated` annotations, as described |
| `filesystem.pb.swift`, `filesystem.grpc.swift` | **8 message types and 4 RPCs that were never generated at all** — `StatPath`, `CreateVolumeOverlay`, `CreateDirectMount`, `GenerateHostsFile` |

So the committed output was not merely formatted by an older plugin; it was
generated from an **older `filesystem.proto`** and is missing real surface. The
error is left visible rather than edited away because it is the same mistake the
exec-latency probe and the proto size gate both made — trusting a sample that
measures something adjacent to the question. One file was read and four were
described.

That drift predates this change. It was reverted out of the codegen-wiring branch
rather than carried along beneath it, and landed separately in **arca#54**, Swift
only. Whether `generate-grpc.sh` should be regenerating another repository's Go
output at all is a real question that PR does not answer.

## 9. Deliberately not done

- **Nothing implements the contract, either side.** P3's exit says so. The Swift
  target has no conformer and the Rust client has no caller.
- **No `buf` breaking check.** Still P3.3's, still inert: `buf` is absent from
  this machine and Arca has no CI (P2.3 open).
- **U5 and U6 remain open**, owned by P5.4 and P6.3.
- **No `gascan-arca`.** The type mapping in the P3.1 design §9 is P5.2's to
  implement, and this crate is generated surface with no translation in it.
