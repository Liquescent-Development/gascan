# Backend conformance across three backends — MEASURED, 2026-08-20

**The shared `RuntimeBackend` contract was run against all three backends for the
first time. All three report a *different* state after `create`, and only the
test double satisfies the contract's assertion.**

| backend | state after `create` | mechanism |
|---|---|---|
| `FakeRuntime` | `Stopped` | what the double models |
| apple | `Running` | `create` translates to `container run` — `crates/gascan-apple/src/translate.rs:100`, inside the `create` opening at `:94` |
| arca | `Creating` | the pinned engine maps container status `"created"` → `.creating` |

Both real backends fail at `crates/gascan-conformance/src/lib.rs:104`, the
walk's third assertion, reached after `inspect`-absent and `create`.
**Everything after it — the duplicate
`create`, the doubled `start`, the exec session, the doubled `stop`, `remove`,
and the final absent `inspect` — was NOT REACHED on either backend.** Nobody may
write that apple or arca passed or failed the exec walk. It was not run.

This document is what P5.3's acceptance criterion 8 required: arca's result is a
*finding*, not a pass criterion. The deliverable is the measurement, and the
failing tests are committed asserting the contract as written. **No assertion was
weakened to produce a green tier.**

## What was under test

| | |
|---|---|
| Gas Can revision | branch `feat/backend-conformance-suite`. The eleven commits under "How this branch got here" are the work; the commit that adds this document sits on top of them, at `ba458c9`'s child. Re-derive with `git log --oneline main..feat/backend-conformance-suite`. |
| Contract | `crates/gascan-conformance/src/lib.rs`, `pub async fn backend_contract(&dyn RuntimeBackend, &CreateRequestFixture)` |
| Instantiations | `crates/gascan-conformance/tests/fake.rs`, `crates/gascan-apple/tests/live/backend_contract.rs`, `crates/gascan-arca/tests/live/conformance.rs` |
| Host | `newcombe`, Darwin 25.6.0 arm64 |
| Date | 2026-08-20 |
| Apple's runtime | `container` CLI 1.1.0, service running |
| Arca revision | `c545612b056e028d5885968a7b9f586d694f994c`, the revision `engine/arca-pin.json` names under tag `gascan-engine-m4`. `git -C .artifacts/arca-engine/arca rev-parse HEAD` returns it. |

**The three backends were NOT given the same request, and the contract does not
require them to be.** What is constant is the contract; the fixture is a
parameter. The fake and apple use `CreateRequestFixture::pinned` — the pinned
workspace image, `network = 'offline'` — and arca uses `for_image` over a stock
alpine layout seeded into the live engine's store, with `network = 'networked'`
and `user = 'root'`, both forced and both explained in `conformance.rs`'s header
comment. The names differ too. Any claim that one compiled request was fed to
three backends is false; see "Two claims corrected" below.

## The commands, and what each returned

### Fake — PASSES, and CI runs it every push

```
cargo test -p gascan-conformance --test fake
```

Exit **0**, 1 passed. It is not `#[ignore]`d, so it runs inside
`cargo test --workspace`, which is exactly `.github/workflows/ci.yml`'s `rust`
job. This is the only one of the three with continuous coverage.

### Apple — FAILS at the post-`create` state

```
cargo test -p gascan-apple --test live -- --ignored backend_contract_holds_on_apple
```

Exit **101**:

```
panicked at crates/gascan-conformance/src/lib.rs:104:5:
assertion `left == right` failed
  left: Running
 right: Stopped
```

`create` for apple compiles to `container run`
(`crates/gascan-apple/src/translate.rs:100`), so the container is started by the
same command that creates it. There is no window in which apple's `create` has
produced a `Stopped` container.

### Arca — FAILS at the same assertion, for a different reason

```
GASCAN_ARCA_ENGINE_BIN=... GASCAN_ARCA_KERNEL_PATH=... \
GASCAN_ARCA_VMINIT_LAYOUT=... GASCAN_ARCA_BASE_OCI_LAYOUT=... \
  cargo test -p gascan-arca --test live -- --ignored --test-threads=1 --nocapture \
    backend_contract_holds_on_arca
```

Exit **101**, `left: Creating`, `right: Stopped`,
`test result: FAILED. 0 passed; 1 failed; 28 filtered out; finished in 1.16s`.
Reproduced **four times** — 1.30s, 1.13s, 1.00s, 1.16s — same assertion, same
values, every time.

## Why arca's `Creating` is not the contract reading too early

This is the obvious alternative explanation and it is closed against the
engine's own source, not against a probe. At revision `c545612b`,
`Sources/ArcaEngine/EngineTranslation.swift:127-134` is a bare `switch` over the
status string:

```swift
public func sandboxState(fromStatus status: String) -> Arca_Engine_V1_SandboxState {
    switch status {
    case "created": return .creating
    ...
```

No clock, no retry, no stored history. **`create` performs no autonomous state
transition; the sandbox sits in the engine's `created` status until something
starts it.** Waiting cannot change what `inspect` returns, so a poll loop in the
contract would not have found `Stopped`.

Note what this does *not* say: `Creating` is not a state the sandbox is stuck
in, and this document deliberately avoids the words "terminal" and
"indefinitely". `start` leaves it in under two seconds — the positive control
below drives `create` → `start` → `inspect` → `stop` → `remove` end to end in
1.99s.

## The positive control, run BEFORE the conformance test existed

Deliberately ordered that way, so a failure would be attributable to arca rather
than to the machine or the environment:

```
lifecycle::create_start_inspect_stop_and_remove_drive_a_real_container
test result: ok. 1 passed; 0 failed; 27 filtered out; finished in 1.99s
```

Exit **0**. Same host, same day, same four `GASCAN_ARCA_*` variables, same
engine. A real container was created, started, inspected, stopped and removed.
The engine, the kernel, the vminit layout and the base OCI layout were all
working when the conformance test was run against them.

The captured log is at
`.superpowers/sdd/2026-08-20-backend-conformance-suite/arca-positive-control.log`,
which is **git-ignored and exists only on this machine** — `.gitignore:1`. The
result quoted above is the durable copy.

## Neither real-backend measurement is reproducible in CI, and the design said otherwise

**Correcting §6 of `docs/superpowers/specs/2026-08-20-backend-conformance-suite-design.md`,
which claimed the arca instantiation "inherits CI coverage free".** That is false
in effect, and the design has been corrected as part of this work.

- **Arca.** `.github/workflows/ci.yml:170-178` runs the live tier, but sets
  **one** variable, `GASCAN_ARCA_ENGINE_BIN`. The tier needs four.
  `backend_contract_holds_on_arca` calls `base_oci_layout()`
  (`crates/gascan-arca/tests/live/common/mod.rs:156-159`), whose absence is a
  `panic!` and never a skip — deliberately, per the rule at `:137-140`. The
  step's own comment records, as a measurement at this revision, that 20 of the
  25 tests `--ignored` selects fail in 0.00s on exactly that missing variable,
  and have done so since milestone 2. **The new test can only join them.** That
  is a derivation from the panic-not-skip rule and the step's recorded
  measurement; no CI run of this test has been observed.
- **Apple.** No CI job runs its tier at all. `--ignored` appears in exactly one
  place in `.github/workflows/ci.yml` — line 178, arca's step.

**So both real-backend measurements are local-only, on a named machine, on a
named date, and this document is the only evidence for them that will ever
exist** until someone puts a kernel, a vminit layout and a base OCI layout on a
runner. The three instantiations do stay wired in:
`scripts/ci-check-ignored-tests.sh` diffs the whole `#[ignore]` set against
`tests/ci/expected-ignored-tests.txt` and fails in both directions, so a test
that vanishes is caught. That guard proves the tests exist. It does not prove
they ran.

## Apple's residue was cleaned up, and here is the anchor

The failing apple run creates a real container before it panics. Measured
afterwards on `newcombe`, 2026-08-20:

| command | what it listed |
|---|---|
| `container list --all` | `buildkit`, `code-3fd063e3b68e` |
| `container volume list` | `gascan-cache-code-3fd063e3b68e`, `containerization-linux-build`, `gascan-config-code-3fd063e3b68e`, `gascan-mise-code-3fd063e3b68e` |
| `container network list` | `default`, `gascan-network-code-3fd063e3b68e` |

Grepping all three for the failed run's container,
`gascan-live-backend-92391-1787258495344035000-2e7e3b521ca5`, returns **0**. No
`gascan-live-backend-*` resource survives. The remaining entries all belong to
the user's own `code` sandbox and to the containerization build image.

## An open question this branch surfaced and deliberately did not answer

**What a *same-request* duplicate `create` reports in `created()` is unmeasured
on every backend.**

The contract's walk issues its second `create` with the same request as the
first (`crates/gascan-conformance/src/lib.rs:97` and `:112`) and asserts only
that the failure's code is `resource_conflict`. `conflict.created()` is neither
inspected nor removed, and the reason is recorded in the contract itself: a
rejected `create` may report resources it built before the collision, but with
an identical request those names are the **live sandbox's own**. Removing them
would tear down the sandbox the walk still has to start, exec, stop and remove.

That is not a hypothetical. A cleanup of exactly that shape was written
(`9f2c0bf`) and **reversed** (`e7e55e4`) before it left the branch, because it
would have destroyed the sandbox under test and then double-removed the
container.

The one live measurement that exists does not settle it.
`crates/gascan-arca/tests/live/lifecycle.rs:259-278` observes a conflicting
create reporting what it made — but there the container and volumes had been
removed first and only the network name was still held, so the three volumes it
reported were genuinely orphaned. **The place to measure the same-request case is
arca's live tier, mirroring `lifecycle.rs:264` without the preparatory remove.**

### A trap left in the tree, named here because nothing else warns about it

`crates/gascan-apple/tests/live/storage.rs:22-37` holds
`create_with_partial_cleanup`, which does precisely the
`if !failure.created().is_empty() { remove }` shape that was reversed above. **It
is correct there** — its callers use it on creates expected to fail against
independently-seeded state, not on a duplicate of a live sandbox. But it is the
nearest precedent a future reader will find, and copying it into the conformance
walk would reintroduce `9f2c0bf`'s defect.

## What promoted, and the estimate it came in under

**Four assertions across Tasks 6 and 7, against an estimated 6-8.** Counted by
candidate rather than by assertion: of seven candidate tests, **one promoted
whole**, **one promoted in part**, and **five promoted nothing**.

| where | what promoted |
|---|---|
| `22dfee8` | the doubled `start`, the doubled `stop`, and a second `create` of a held id failing with `resource_conflict` |
| `a32a29e` | an exec session's stream ends at the terminal `Exit` |

**The spec was corrected, not the work.** Nothing was promoted to close the gap
to the estimate, and §3 of the design now carries the measured outcome in place
of the estimate, with the reason each of the five non-promoting candidates was
left where it is. Every promoted assertion is exercised by `FakeRuntime` alone
today, because all four sit *after* `lib.rs:104`.

## Two claims corrected

**`049b4ba`'s commit message states two things that are wrong, and `766eb6e`
corrects both.** A reader who runs `git log 049b4ba` alone gets the uncorrected
text, so it is repeated here:

1. It says the three backends were given **"the same compiled request"**. They
   were not — the requests differ in image, network, user and name. What is
   constant across the three instantiations is the *contract*, not the request.
2. It calls arca's `Creating` **"terminal"**. It is not. `start` leaves it in
   under two seconds. The accurate statement is the one this document uses:
   `create` performs no autonomous state transition.

## How this branch got here

| commit | what |
|---|---|
| `f06e96c` | created `crates/gascan-conformance`, dev-dependency only, with `CreateRequestFixture` and `capabilities()` |
| `9dcca6a` | moved the contract in from `gascan-core/tests/`, verified byte-identical apart from three sanctioned changes |
| `daa687b` | deleted the duplicated original from `gascan-core`; test count 175 → 174 |
| `0e1f3fb` | apple instantiation, replacing a 65-line hand-rolled duplicate |
| `049b4ba` | arca instantiation — the measurement the plan exists for |
| `766eb6e` | corrected the two claims above |
| `22dfee8` | promoted 3 assertions |
| `9f2c0bf` | corrected `22dfee8`'s line anchors; added a residue cleanup |
| `e7e55e4` | **reversed** that cleanup — it would have torn down the sandbox under test |
| `a32a29e` | promoted 1 assertion |
| `ba458c9` | design §3 updated with the measured triage outcome |

## What this does not say

- **Nothing about whether apple or arca satisfies the rest of the contract.**
  Neither reached `start`. `Running` and `Creating` are the only real-backend
  facts here.
- **Nothing about which backend is wrong.** Three implementations disagree about
  what `create` means, and this document records the disagreement. Whether the
  contract should assert `Stopped`, or assert a set, or make the post-`create`
  state a fixture-declared expectation, is a design question that has not been
  answered and must not be answered by editing the assertion to fit whatever a
  backend happens to do.
- **Nothing about a user seeing this.** The contract is a test-tier instrument
  and the failures are in the test tier. **Whether any production path reads
  `inspect` immediately after `create` and depends on `Stopped` was NOT
  surveyed.** Do not read this document as saying the product is unaffected; it
  says only that the question was not asked.
- **Nothing about offline.** Arca's fixture is `networked` on purpose —
  `docs/evidence/2026-08-18-arca-engine-offline.md` is the reason, and it stands
  unchanged.

## What follows

1. **The two failures stay in the tree asserting the contract as written.** They
   fail today, on a real backend, for a real reason. Weakening `lib.rs:104` to
   accept three states would make the suite green and would make it worthless —
   that is the outcome acceptance criterion 8 exists to forbid.
2. **Deciding what a backend owes after `create` is the next piece of work**, and
   it is a design decision with three live candidates in front of it. It is not
   in P5.3's scope.
3. **P5's exit is not met.** Its first clause — extract the conformance suite and
   run it against apple and arca — is done, and its result is above. Its second
   clause, `gascan-e2e` on arca, is untouched and was explicitly out of scope.
4. **A same-request `create` collision should be measured in arca's live tier**
   before anything asserts what `created()` holds on that path.
