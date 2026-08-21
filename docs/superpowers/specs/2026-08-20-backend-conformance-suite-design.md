# P5.3 — the backend conformance suite, design

**Roadmap step:** P5.3 of `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md`,
worded there as *"Extract the conformance suite from `fake_runtime.rs`; run against fake, apple,
and arca backends."* P5's exit is *"`gascan-arca` passes conformance and existing `gascan-e2e`"*;
this design covers the **first clause only**. The second is named under "Out of scope".

Every file:line and count below was re-derived on 2026-08-20 at `10e3342`. Re-derive rather than
trust them; this repository's durable docs have gone stale on their own numbers repeatedly.

---

## 1. What is already there, because it changes the task

The roadmap's "extract" reads as though a suite must be written. It must not. **The generic
conformance function already exists**, at `crates/gascan-core/tests/backend_contract.rs:149`:

```rust
pub async fn backend_contract(backend: &dyn RuntimeBackend)
```

It takes a trait object and walks: `inspect` absent → `create` → assert a `Container` resource
among those created → `inspect` reports `Stopped` → `start` → `exec` `true` → assert
`ExecOutput::Exit { code: 0, signal: 0 }` → `stop` → `remove` → `inspect` absent. `FakeRuntime`
runs it through `fake_runtime_satisfies_backend_contract_through_trait_object`.

**`crates/gascan-apple/tests/live/backend_contract.rs` does not call it.** It hand-rolls a
65-line walk over the same ground. That is not an oversight: `backend_contract` lives in a
`tests/` target, which is not a library, so `gascan-apple` **cannot** import it. The duplication
is a consequence of location, and location is what this design changes.

So P5.3 is: **make the contract importable, grow it, and instantiate it three times.**

## 2. Where the shared code lives, and why not `gascan-core`

The obvious move is a `pub mod` in `gascan-core/src`, following `fake_runtime` — which is
unconditionally public at `crates/gascan-core/src/lib.rs:9`, establishing that this project will
ship test-support code in that library.

**It does not work, and the reason is a hard gate rather than a preference.**
`crates/gascan-core/src/lib.rs:2` reads:

```rust
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
```

CI enforces it — `cargo clippy --workspace --all-targets -- -D warnings`, `.github/workflows/ci.yml:54`.
MEASURED at `10e3342`: `fake_runtime.rs` contains **0** occurrences of `unwrap()` and carries **no**
`#[allow(clippy::…)]`; it meets that bar honestly. `backend_contract.rs` contains **114**.

That leaves three options, and only one is honest:

| option | cost |
|---|---|
| module-level `#[allow(clippy::unwrap_used)]` in `gascan-core/src` | weakens a *production* library's deliberate discipline to host test assertions |
| rewrite the contract to propagate `Result` | ~114 sites, and assertion failures stop pointing at the assertion |
| **a separate crate** | none of the above; the denial stays intact because the code is not in that crate |

**Decision: a new crate, `gascan-conformance`.** It is a **dev-dependency only** of the crates that
run it, so unlike `fake_runtime` it is compiled into no shipped artifact at all. Panicking
assertions are correct in a test-support crate and wrong in `gascan-core`, and this puts each
where it belongs.

**The fake instantiation lives inside `gascan-conformance` itself** (`tests/fake.rs`), taking
`gascan-core` as an ordinary dependency and reaching `gascan_core::fake_runtime::FakeRuntime`.
This is deliberate: putting it in `gascan-core/tests` instead would make `gascan-core`
dev-depend on a crate that depends on it. Cargo permits that cycle, but it is a cycle a reader has
to think about, and there is no reason to mint one.

### Layout

```
crates/gascan-conformance/
  src/lib.rs        the contract + the fixtures it needs
  tests/fake.rs     instantiation 1 — FakeRuntime, free, runs in CI
crates/gascan-apple/tests/live/backend_contract.rs   instantiation 2 — replaces the 65-line duplicate
crates/gascan-arca/tests/live/conformance.rs         instantiation 3 — new
crates/gascan-core/tests/backend_contract.rs         keeps only what is genuinely fake-specific
```

The fixtures move too. `crates/gascan-core/tests/common/mod.rs` is **61 lines** and holds all three
the contract needs — `capabilities()` (`:27`), `create_request()` (`:40`),
`create_request_with_network()` (`:44`).

**`common/mod.rs` does not go away, and the duplication with `gascan-conformance` is permanent.**
Re-derived at HEAD after Task 7: `mod common;` appears in exactly one file,
`crates/gascan-core/tests/backend_contract.rs:1` — `policy.rs` has its own `capabilities()` at
`:19` and does not include it — and that one target still calls `capabilities()` on **20** lines,
`create_request(` on **24** and `create_request_with_network(` on **9**. Every export is still
referenced, so no task in this plan deletes any of them. The accepted cost is two copies of a
three-function fixture; the alternative is the one this section already rejected, since pointing
`gascan-core/tests` at `gascan-conformance` would mint a `gascan-core` dev-dependency on a crate
that depends on `gascan-core`. Drafts of the task briefs called this duplication "deliberate and
short-lived" and deferred it to a "Task 9" — the plan has eight tasks, there is no Task 9, and the
duplication is not short-lived.

## 3. What gets promoted, and what stays fake-only

`backend_contract.rs` held **23** `#[tokio::test]` functions at `10e3342`. They are not one
population. (It holds **21** after Task 7: the fake instantiation moved to `gascan-conformance` and
Task 6 consumed `duplicate_create_is_rejected_and_start_stop_are_idempotent` whole.)

**Candidates considered.** Five of the seven tests named in this paragraph promoted nothing; the
measured outcome is the table under the next heading, not this list. The lifecycle walk already in
`backend_contract()`, plus `duplicate_create_is_rejected_and_start_stop_are_idempotent`,
`exec_and_logs_preserve_binary_bytes_and_exact_exit_code`,
`exec_session_is_live_bidirectional_and_emits_one_exit`,
`create_collision_reports_resources_created_before_the_collision`,
`offline_fake_create_has_no_managed_network`,
`networked_fake_create_reports_network_then_volumes_then_container`,
`persistent_logs_are_isolated_by_exact_sandbox_id`. Each one had to be re-read to separate what any
backend owes from what this double happens to do. **An earlier draft of this analysis said 13. That
was wrong** — see §4 for the class of error it made. **This section then estimated 6–8. That was
wrong too, and the measured outcome below replaces it** rather than standing beside it.

### Triage outcome, measured across Tasks 6 and 7

**Four assertions promoted, not 6–8.** Counted the other way — by candidate rather than by
assertion — one of the seven named tests promoted whole, one promoted in part, and five promoted
nothing. Both countings are below the estimate, and nothing was promoted to close the gap.

| where | what promoted |
|---|---|
| Task 6, `22dfee8` | from `duplicate_create_is_rejected_and_start_stop_are_idempotent`: the doubled `start`, the doubled `stop`, and a second `create` of a held id failing with `resource_conflict`. Whole test consumed. |
| Task 7 | from `exec_session_is_live_bidirectional_and_emits_one_exit`: the exec stream ends at the terminal `Exit`. The rest of that test is fake-only and stays, renamed `exec_session_echoes_stdin_and_maps_a_signal_to_its_exit_code` so the name matches the assertions left in it. |

**Every promoted assertion is exercised by `FakeRuntime` alone today.** Apple and arca both fail the
contract at the post-`create` state assertion (`crates/gascan-conformance/src/lib.rs:139`, `:104`
in the recorded panic text — the comment now over it moved the line) — apple
reports `Running`, arca `Creating` — which precedes every line promoted, so neither backend has
been measured against any of it.

**Stays fake-only — the remaining candidates, and the machinery that decided each.**

| test | why it stays |
|---|---|
| `exec_and_logs_preserve_binary_bytes_and_exact_exit_code` | `set_exec_result` and `set_logs`, both fake-only. Asserting the property portably needs a command that emits known bytes on stdout *and* stderr and exits non-zero; the fake's vocabulary for that is `fake-stdout` / `fake-stderr` / `fake-exit` (`crates/gascan-core/src/fake_runtime.rs:588-636`), which no container image has, and the fake maps the portable spelling to nothing at all — `Some("true") \| Some("sh") => (Vec::new(), Vec::new(), 0)` at `:633`. Giving the contract a per-backend command means parameterising it, a design change. The exit code the walk *can* portably assert is already asserted. The log half is worse than unportable: `since` is not the same quantity across backends — apple passes `--since {n}ms` to the CLI, a duration ago (`crates/gascan-apple/src/backend.rs:630-632`), arca sends `since_unix_millis`, an absolute instant (`crates/gascan-arca/src/backend.rs:411-414`). |
| `exec_session_is_live_bidirectional_and_emits_one_exit` | **Promoted in part**, see above. What stays needs `fake-echo-stdin` to get stdin back, and its `Exit { code: 143, signal: 15 }` is the fake's own `128 + signal` arithmetic (`crates/gascan-core/src/fake_runtime.rs:1118-1122`), not something a backend owes. |
| `create_collision_reports_resources_created_before_the_collision` | `seed_volume`, fake-only. The assertion's entire content is that a failure reports exactly the resources built before a **planted** collision at a chosen index. Creating twice does produce a collision on a real backend, but with the same request — so the reported names would be the live sandbox's own, and what a same-request collision reports is unmeasured on every backend. `crates/gascan-conformance/src/lib.rs:149-164` records that open question in the contract itself, and names `gascan-apple/tests/live/storage.rs:22-37` as the precedent not to copy. |
| `offline_fake_create_has_no_managed_network` | Not machinery — fixture shape. The contract is one walk over one fixture, and arca's must be `network = 'networked'`: offline is the capability the pinned engine is proven not to honour (`docs/evidence/2026-08-18-arca-engine-offline.md`). An unconditional "no managed network" assertion fails for a networked fixture on *every* backend, so promoting it needs the contract to branch on the fixture's network. That is a design change, and it is not made here. |
| `networked_fake_create_reports_network_then_volumes_then_container` | Same fixture-conditionality — the network element exists only for a networked fixture — and, separately, **nothing owes the ordering**. `RemoveRequest::from_resources` does not reorder (`crates/gascan-core/src/runtime.rs:1001-1017`), yet the fake's recorded removal comes out container / volume / network, so re-ordering is the backend's job and no consumer reads `created()` positionally. Arca's list is in whatever order the engine's `CreateResponse` carried (`crates/gascan-arca/src/backend.rs:80-108`) — an unmeasured property of a pinned external binary. **Correction to the plan's candidate table**, which says this ordering "is asserted through the fake's call recorder": it is not. The test reads `outcome.created()` (`crates/gascan-core/tests/backend_contract.rs:509-517`) and touches neither `calls()` nor `outcomes()`. The verdict is unchanged; the stated reason was wrong. |
| `persistent_logs_are_isolated_by_exact_sandbox_id` | `FakeRuntime::persistent`, named fake-only machinery, plus `fake-stdout` to get a marker into the log. Isolation-by-id also needs two live sandboxes and `backend_contract` takes one fixture, so promoting it would mean a second design change on top of the machinery. |

**Stays fake-only — tests of the double's controllability, not of the contract.**
`named_failure_is_injected_once_at_the_call_boundary`,
`every_backend_boundary_supports_fail_once_injection`,
`injected_post_mutation_create_failure_reports_partial_resources` (all `FailureBoundary`
injection); `literal_requests_are_recorded_in_order`, `fake_recreate_records_prepare_then_container_create`,
`removal_mutates_in_container_volume_network_order` (all assert through the fake's call recorder,
`calls()`/`outcomes()`, which no real backend has); `persistent_fake_runtime_reopens_runtime_truth_without_controller_state`
and `same_name_seeded_network_conflicts_without_adoption` (persistence and seeding).

`removal_mutates_in_container_volume_network_order` is the interesting one: **the ordering it
asserts is real contract, but the instrument is not portable.** Against a real backend only the
*effect* is observable — the resources are gone — not the order they went in. It stays fake-only
and the residue is recorded here rather than papered over.

**Neither — `cancellable_exec_session_cancel_is_idempotent` and
`cancellable_exec_session_drop_signals_backend`** test `ExecSession` itself, take no backend, and
simply stay where they are.

## 4. What is NOT in this design, and the error that nearly put it here

An earlier draft scoped a mechanism for creating ownership-labelled resources out-of-band, so the
`Foreign`/`Mismatched` assertions could run against real backends. **That was an overbuild, and the
check that killed it is worth recording** because the same reasoning applies to anything else
proposed for promotion.

Ownership classification is **not per-backend**. `classify_resource_ownership` is a pure function
in shared code at `crates/gascan-core/src/runtime.rs:85-103`, taking `Option<&str>` and a
`SandboxLabel` — no backend involved. `crates/gascan-core/tests/resource_ownership.rs` already
tests it exhaustively across all three verdicts and every resource kind. Promoting those
assertions would run **the same function three times** and prove nothing new. The design note this
implements says as much: *"Ownership labels cross the wire; classification does not."*

**The general rule this yields, and the one to apply to every promotion candidate in §3:** an
assertion earns promotion only if the behaviour it names is *implemented separately by each
backend*. Shared code tested once is done.

### The one genuinely per-backend gap, recorded as a follow-up

Classification is shared; **visibility is not**. If `gascan-apple`'s listing parse drops unlabelled
entries, or the Arca engine filters them server-side, then `list_resources` returns a clean
inventory and gascand's drift detection never emits `ReconcileFinding::UnknownUnowned`
(`crates/gascand/src/service.rs:3012`). Nothing today would notice. The Arca protocol handoff
names this hazard directly: *"`ListResources` returns unlabelled resources too. Gas Can's drift
detection depends on seeing foreign ones, so filtering engine-side would break it silently."*

Closing it needs **one** assertion per backend — create a resource out-of-band, assert
`list_resources` reports it — plus a teardown story, since a leaked foreign container on a
developer's machine is a real mess. **That is a separate task, not this one**, and it is written
here so it is not lost.

## 5. Out of scope

- **The product-level `gascan-e2e` suite on arca.** P5's *second* exit clause. Apple has 24
  `#[ignore]`d product tests (`apple_apply` 17, `apple_security` 5, `apple_lifecycle` 1,
  `apple_recovery` 1); arca has 6. Closing that gap means parameterising the harness — the two
  `command()` builders in `apple_common/mod.rs:961` and `arca_common/mod.rs:303` differ by exactly
  four `.env()` calls — and it is a larger, separate piece of work.
- **P5.4 / U5**, image digests reaching the engine. Confirmed unresolved at
  `docs/status/arca-integration-handoff.md:2242`.
- **Offline / `CERTIFIED_ENGINE_REVISION`**, still `None` at `crates/gascan-arca/src/translate.rs:329`.
  Arca-side engine work; gates P7, not P5.3.
- **Production changes to `gascan-arca`.** If arca fails a promoted assertion, that is a finding
  to fix as its own work. A conformance suite that is edited until it passes measures nothing.

## 6. Testing, and the failure mode that would make this lie

**A conformance suite that silently does not run is worse than none**, because it reports success.
Two mechanisms already exist and both are used rather than reinvented.

`scripts/ci-check-ignored-tests.sh` diffs the entire `#[ignore]` set against
`tests/ci/expected-ignored-tests.txt` and **fails in both directions** — so a test that vanishes is
caught, not just one that appears. `backend_contract::backend_contract` is already an entry in that
file. Every change here moves that set: apple's entry changes shape and arca gains one. **Updating
the baseline is part of the work, and the guard is what proves the suite is still wired in.**

Absence of a live prerequisite must **panic, never skip**.
`crates/gascan-arca/tests/live/common/mod.rs:137-140` already rules this, above `required_path`
(`:147`), and states the reason: *"a live test that silently skips is a live test nobody notices
has stopped running."* The same sentence is repeated at `:333-335` on `LiveEngine::start`. The arca
instantiation uses both and inherits the rule.

**Where each instantiation actually runs, which is asymmetric and worth knowing before relying on
it.** The fake instantiation runs in `cargo test --workspace`, so CI covers it every push. **Neither
real backend's instantiation is executed by CI in any usable sense — CORRECTING what this section
said before**, which was that the arca instantiation "inherits CI coverage free". The **arca**
instantiation does land in the live tier CI executes
(`cargo test -p gascan-arca --test live --no-fail-fast -- --ignored`,
`.github/workflows/ci.yml:178`), but that step sets **one** variable, `GASCAN_ARCA_ENGINE_BIN`, and
the tier needs four. `backend_contract_holds_on_arca` calls `base_oci_layout()`, whose absence is a
`panic!` and never a skip, and the step's own comment records as a measurement that 20 of the 25
tests `--ignored` selects fail in 0.00s on exactly that missing variable and have done so since
milestone 2 — so the new test can only join the failing majority. That is a derivation, not an
observed CI run. **That `25` is from an earlier revision**: re-derived at HEAD,
`grep -rc '#\[ignore' crates/gascan-arca/tests/live/*.rs` sums to **29**, one of them added by this
plan. The **apple** instantiation runs **nowhere in CI**; no workflow step
passes `--ignored` for `gascan-apple` or `gascan-e2e`, and of the three `--ignored` occurrences in
`.github/workflows/ci.yml` — `:139` and `:162` inside comments, `:178` in arca's step — only `:178`
is executed. Both real-backend results are therefore local-only, and no claim that a real backend passes or
fails conformance should be made without naming the machine and the date. What CI *does* hold is
that the tests are still wired in, via `scripts/ci-check-ignored-tests.sh`; that is existence, not
execution. `docs/evidence/2026-08-20-backend-conformance.md` is where the local measurements live.

**Proving the extraction is behaviour-preserving.** The fake instantiation must pass before and
after the move with **no edit to any assertion body**. If an assertion had to change, the move was
not a move.

**Proving a promoted assertion is real.** For each one promoted, revert the behaviour it names in
the fake and confirm the suite fails. An assertion that cannot be made to fail is not testing
anything — this repository has measured that exact outcome, at
`docs/status/START-HERE.md:604`: a guard whose deletion *and* inversion (so that it unlinked
precisely a stranger's file) both left `cargo test -p gascan --lib` green.

**The hazard that does NOT apply here, recorded so nobody re-imports it.** The e2e harness selects
its backend from the environment and `backend_selection` returns `Apple` when nothing is requested
(`crates/gascan-core/src/backend.rs:167`), so a dropped variable there silently tests the wrong
backend. **Conformance is not exposed to this**: each instantiation constructs its backend
explicitly in code — `AppleBackend::new(ProcessRunner)`, and arca's from a live transport. There is
no selection to get wrong. This matters only if the product-e2e work in §5 is taken up later, where
it applies fully.

## 7. Acceptance

1. `gascan-conformance` exists, is a dev-dependency only, and is compiled into no shipped artifact.
2. `gascan-core/src/lib.rs`'s `deny` attribute is unchanged, and no `#[allow(clippy::unwrap_used)]`
   was added anywhere to accommodate this work.
3. The 65-line duplicate at `gascan-apple/tests/live/backend_contract.rs` is **deleted**, replaced
   by a call into the shared suite.
4. Three instantiations exist: fake, apple, arca.
5. `tests/ci/expected-ignored-tests.txt` is updated and `scripts/ci-check-ignored-tests.sh` passes.
6. Every promoted assertion has been shown to fail when the behaviour it names is reverted in the
   fake.
7. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and
   `cargo test --workspace` are clean.
8. **Arca's result against the suite is recorded as a finding, not as a pass criterion.** If arca
   fails a promoted assertion, this task's deliverable is the *measurement* and its write-up. The
   fix is separate work, and forcing green here by weakening an assertion is the one outcome that
   makes the whole exercise worthless.
