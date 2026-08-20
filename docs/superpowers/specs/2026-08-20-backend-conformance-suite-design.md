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

## 3. What gets promoted, and what stays fake-only

`backend_contract.rs` holds **23** `#[tokio::test]` functions. They are not one population.

**Promote — assertions any backend must satisfy.** The lifecycle walk already in
`backend_contract()`, plus `duplicate_create_is_rejected_and_start_stop_are_idempotent`,
`exec_and_logs_preserve_binary_bytes_and_exact_exit_code`,
`exec_session_is_live_bidirectional_and_emits_one_exit`,
`create_collision_reports_resources_created_before_the_collision`,
`offline_fake_create_has_no_managed_network`,
`networked_fake_create_reports_network_then_volumes_then_container`,
`persistent_logs_are_isolated_by_exact_sandbox_id`. **Estimated 6–8 will promote cleanly**, and the
estimate is deliberately a range: each one has to be re-read to separate what any backend owes from
what this double happens to do. **An earlier draft of this analysis said 13. That was wrong** — see
§4 for the class of error it made.

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
it:** the fake instantiation runs in `cargo test --workspace`, so CI covers it every push. The
**arca** instantiation lands in the live tier CI already executes —
`cargo test -p gascan-arca --test live --no-fail-fast -- --ignored`, `.github/workflows/ci.yml:178`
— so it inherits CI coverage free. The **apple** instantiation runs **nowhere in CI**; no workflow
step passes `--ignored` for `gascan-apple` or `gascan-e2e`. It is a local, manual tier. This design
does not change that, and no claim that "apple passes conformance" should be made without saying
which machine it was run on and when.

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
