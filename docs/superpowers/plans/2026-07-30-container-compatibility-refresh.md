# Apple Container Compatibility and Runtime Refresh Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Accept coherent Apple Container 1.x releases at or above 1.1.0, warn when they are uncertified, keep offline isolation fail-closed, and refresh runtime evidence without restarting `gascand`.

**Architecture:** `gascan-apple` classifies release evidence and derives networked capabilities separately from certified offline proof. `gascan-core` adds a nonblocking doctor warning status, while `gascand` refreshes production doctor evidence and backend capabilities for each relevant request instead of retaining daemon-lifetime results.

**Tech Stack:** Rust 1.95, Tokio, tonic/protobuf v1 compatibility envelope, serde JSON, Apple Container structured CLI output, Cargo integration tests.

## Global Constraints

- Accept only Apple Container semantic versions `>=1.1.0, <2.0.0`.
- The sole certified identity remains 1.1.0 at commit `5973b9cc626a3e7a499bb316a958237ebe14e2ed`.
- Compatible untested releases may use networked capabilities but never receive `NetworkIsolation::Proven`.
- CLI and API service release versions and full commits must match.
- Warning-only doctor reports are ready and exit zero; failed or unknown readiness prerequisites remain blocking.
- Existing protobuf fields and API major remain unchanged.
- Runtime evidence collection remains bounded to 60 seconds.
- No daemon restart may be required after changing Apple Container.

---

### Task 1: Classify Apple Container release evidence

**Files:**
- Modify: `crates/gascan-apple/src/probe.rs`
- Modify: `crates/gascan-apple/src/lib.rs`
- Modify: `crates/gascan-apple/tests/probe.rs`
- Create: `crates/gascan-apple/tests/fixtures/system-version-1.2.0.json`
- Create: `crates/gascan-apple/tests/fixtures/system-status-1.2.0.json`

**Interfaces:**
- Consumes: existing `RuntimeVersion`, `RuntimeCapabilities`, `NetworkIsolation`, `APPLE_1_1_COMMIT`.
- Produces:
  - `AppleReleaseEvidence { version: RuntimeVersion, commit: String }`
  - `AppleCompatibility::{Certified, CompatibleUntested}`
  - `AppleProbe::release_evidence() -> Result<AppleReleaseEvidence, RuntimeError>`
  - `AppleReleaseEvidence::compatibility() -> Result<AppleCompatibility, RuntimeError>`
  - `AppleSystemStatus.api_server_commit` and `.api_server_version` for later coherence checks.

- [ ] **Step 1: Add failing range and capability tests**

Add table-driven tests asserting:

```rust
let cases = [
    ("1.0.9", false),
    ("1.1.0", true),
    ("1.1.1", true),
    ("1.2.0", true),
    ("1.99.99", true),
    ("2.0.0", false),
];
```

Assert the exact certified commit classifies as `Certified`; another valid
commit in the accepted range classifies as `CompatibleUntested`. For the
untested tier assert every ordinary capability is true and
`offline == NetworkIsolation::Unsupported`.

- [ ] **Step 2: Run the probe tests and verify RED**

Run:

```bash
rtk cargo test --locked -p gascan-apple --test probe
```

Expected: failures show that 1.0 is currently accepted, 1.2 ordinary
capabilities are false, and no compatibility type exists.

- [ ] **Step 3: Implement release evidence and compatibility classification**

Introduce:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppleCompatibility {
    Certified,
    CompatibleUntested,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppleReleaseEvidence {
    pub version: RuntimeVersion,
    pub commit: String,
}

impl AppleReleaseEvidence {
    pub fn compatibility(&self) -> Result<AppleCompatibility, RuntimeError> {
        let minimum = RuntimeVersion::new(1, 1, 0);
        if self.version.major != 1 || self.version < minimum {
            return Err(RuntimeError::UnsupportedVersion {
                found: self.version,
                supported: ">=1.1.0, <2.0.0".to_owned(),
            });
        }
        Ok(if self.version == minimum && self.commit == APPLE_1_1_COMMIT {
            AppleCompatibility::Certified
        } else {
            AppleCompatibility::CompatibleUntested
        })
    }
}
```

If `RuntimeVersion` does not implement ordering, compare `(major, minor,
patch)` tuples explicitly rather than adding an unrelated dependency.

Make `base_capabilities` set ordinary capabilities for both accepted tiers and
set offline to `Proven` only for `Certified`.

- [ ] **Step 4: Preserve strict structured schema checks**

Keep release build type, exactly one `container` entry, 40-character lowercase
commit, embedded seven-character service commit, absolute app root, and exact
semantic-version parsing. Export the evidence types through
`crates/gascan-apple/src/lib.rs`.

- [ ] **Step 5: Run focused tests and verify GREEN**

Run:

```bash
rtk cargo test --locked -p gascan-apple --test probe
rtk cargo test --locked -p gascan-apple --test backend_fake_runner
```

Expected: all pass.

- [ ] **Step 6: Commit**

```bash
rtk git add crates/gascan-apple/src crates/gascan-apple/tests
rtk git commit -m "feat: accept compatible Apple Container 1.x releases"
```

---

### Task 2: Add a nonblocking doctor warning status

**Files:**
- Modify: `crates/gascan-core/src/doctor.rs`
- Modify: `crates/gascan-core/tests/doctor.rs`
- Modify: `crates/gascand/src/api.rs`
- Modify: `crates/gascand/tests/doctor_state.rs`
- Modify: `crates/gascan/src/presentation.rs`
- Modify: `crates/gascan/src/cli.rs`

**Interfaces:**
- Consumes: existing `DoctorStatus`, `DoctorFact`, `DoctorReport`, protobuf `Capability`.
- Produces:
  - `DoctorStatus::Warning`
  - `DoctorFact::warning(detail)`
  - `DoctorStatus::is_available()`
  - warning-aware readiness, API findings, JSON, exit status, and human rendering.

- [ ] **Step 1: Write failing core warning tests**

Add assertions equivalent to:

```rust
let mut facts = DoctorFacts::all_supported_for_tests();
facts.version = DoctorFact::warning("untested 1.2.0");
let report = facts.into_report();
assert!(report.is_ready());
assert!(report.runtime_readiness_failure().is_none());
assert_eq!(
    report.check("runtime.version").unwrap().status,
    DoctorStatus::Warning
);
```

- [ ] **Step 2: Write failing API and presentation tests**

In `doctor_state.rs`, assert a warning capability has `available == true`,
structured status `"warning"`, and produces no `findings`.

In `presentation.rs`, assert exact plain output:

```text
⚠ Gascan is ready with warnings
  Runtime  10/12 checks passed, 2 warnings
```

and warning check headings use `⚠`, while failure headings remain `✗`.

- [ ] **Step 3: Run focused tests and verify RED**

Run:

```bash
rtk cargo test --locked -p gascan-core --test doctor
rtk cargo test --locked -p gascand --test doctor_state
rtk cargo test --locked -p gascan presentation::tests
```

Expected: compilation or assertions fail because `Warning` is absent.

- [ ] **Step 4: Implement warning semantics**

Add:

```rust
pub enum DoctorStatus {
    Pass,
    Warning,
    Fail,
    Unknown,
}

impl DoctorStatus {
    pub const fn is_available(self) -> bool {
        matches!(self, Self::Pass | Self::Warning)
    }
}
```

Make `DoctorReport::is_ready` accept pass or warning. Make
`runtime_readiness_failure` select only `Fail` or `Unknown` readiness
prerequisites.

In the API, set:

```rust
available: check.status.is_available()
```

and create `findings` only for blocking `Fail` and `Unknown` checks.

- [ ] **Step 5: Implement warning rendering and exit behavior**

Compute pass, warning, and blocking counts per group. Render the warning-ready
heading when there are warnings and no blocking checks. Keep JSON status
derived from serialized structured detail.

The CLI already exits based on `doctor.findings`; confirm warning-only reports
return zero once warnings are excluded from findings.

- [ ] **Step 6: Run focused tests and verify GREEN**

Run the three commands from Step 3.

Expected: all pass.

- [ ] **Step 7: Commit**

```bash
rtk git add crates/gascan-core/src/doctor.rs crates/gascan-core/tests/doctor.rs \
  crates/gascand/src/api.rs crates/gascand/tests/doctor_state.rs \
  crates/gascan/src/presentation.rs crates/gascan/src/cli.rs
rtk git commit -m "feat: report nonblocking doctor warnings"
```

---

### Task 3: Produce coherent certified and untested runtime reports

**Files:**
- Modify: `crates/gascand/src/main.rs`
- Modify: `crates/gascand/tests/doctor_state.rs`
- Modify: `crates/gascan-core/src/doctor.rs`
- Modify: `crates/gascan-core/tests/policy.rs`

**Interfaces:**
- Consumes: `AppleReleaseEvidence`, `AppleCompatibility`, `AppleSystemStatus`, `DoctorFact::warning`.
- Produces:
  - `apply_runtime_evidence(facts: &mut DoctorFacts, cli: Result<AppleReleaseEvidence, RuntimeError>, service: Result<AppleSystemStatus, RuntimeError>)`;
  - focused offline error text containing the installed version.

- [ ] **Step 1: Add failing doctor matrix tests**

Extract or add tests for these exact cases:

| CLI | Service | Expected |
| --- | --- | --- |
| certified 1.1.0 | certified 1.1.0 | all pass |
| untested 1.2.0 commit A | untested 1.2.0 commit A | version/offline warnings, networked capabilities pass |
| 1.2.0 commit A | 1.2.0 commit B | blocking service/schema failure |
| 1.2.0 | 1.1.0 | blocking service/schema failure |
| 1.0.9 | 1.0.9 | blocking version failure |
| 2.0.0 | 2.0.0 | blocking version failure |

Assert warning detail names installed `1.2.0` and tested `1.1.0`.

- [ ] **Step 2: Run focused tests and verify RED**

Run:

```bash
rtk cargo test --locked -p gascand --test doctor_state
```

Expected: untested 1.2.0 is still represented as failed/unsupported.

- [ ] **Step 3: Extract coherent runtime fact construction**

Move classification away from nested boolean `exact && matrix` conditions into
the deterministic `apply_runtime_evidence` helper above. After separately
mapping malformed CLI and service results to blocking facts, coherent evidence
uses:

```rust
match (cli.compatibility()?, versions_match, commits_match) {
    (AppleCompatibility::Certified, true, true) => {
        apply_certified_facts(facts, &cli, &service)
    }
    (AppleCompatibility::CompatibleUntested, true, true) => {
        apply_compatible_facts(facts, &cli, &service)
    }
    (_, false, _) | (_, _, false) => {
        apply_mismatched_facts(facts, &cli, &service)
    }
}
```

Define all three helpers in `crates/gascand/src/main.rs` with parameters
`facts: &mut DoctorFacts`, `cli: &AppleReleaseEvidence`, and
`service: &AppleSystemStatus`, returning `()`. The certified helper assigns
Gate 2 pass facts; the compatible helper assigns the version/offline warnings
and ordinary capability passes; the mismatch helper assigns blocking service
and schema failures naming both identities.

Certified detail retains Gate 2 evidence. Compatible detail must not cite Gate
2 as proof for the untested release.

- [ ] **Step 4: Make offline policy errors version-specific**

When policy compilation sees `NetworkIsolation::Unsupported`, include:

```text
hard offline isolation has not been verified with Apple Container 1.2.0; use networked mode or install the certified 1.1.0 release
```

Preserve the rule that mount construction occurs only after the offline gate.

- [ ] **Step 5: Run doctor and policy tests**

Run:

```bash
rtk cargo test --locked -p gascand --test doctor_state
rtk cargo test --locked -p gascan-core --test policy
rtk cargo test --locked -p gascan-apple --test probe
```

Expected: all pass.

- [ ] **Step 6: Commit**

```bash
rtk git add crates/gascand/src/main.rs crates/gascand/tests/doctor_state.rs \
  crates/gascan-core/src/doctor.rs crates/gascan-core/tests/policy.rs
rtk git commit -m "feat: warn on uncertified Container runtimes"
```

---

### Task 4: Refresh runtime evidence and capabilities per request

**Files:**
- Modify: `crates/gascand/src/service.rs`
- Modify: `crates/gascand/src/main.rs`
- Modify: `crates/gascand/tests/doctor_state.rs`
- Modify: `crates/gascand/tests/service.rs` if service refresh tests belong in the shared service suite

**Interfaces:**
- Consumes: production `production_doctor_report()` and `RuntimeBackend::capabilities()`.
- Produces:
  - `DoctorState::refreshing<C, F>(timeout: Duration, collector: C) -> DoctorState where C: Fn() -> F + Send + Sync + 'static, F: Future<Output = DoctorReport> + Send + 'static`
  - fresh owned `RuntimeCapabilities` from `runtime_capabilities()`.

- [ ] **Step 1: Add failing refresh tests**

Create a collector backed by an atomic counter:

```rust
let calls = Arc::new(AtomicUsize::new(0));
let state = DoctorState::refreshing(Duration::from_secs(1), {
    let calls = Arc::clone(&calls);
    move || {
        let calls = Arc::clone(&calls);
        async move {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            report_for_version(if call == 0 { "1.2.0" } else { "1.1.0" })
        }
    }
});
assert_ne!(state.report().await, state.report().await);
assert_eq!(calls.load(Ordering::SeqCst), 2);
```

Add a mutable fake backend test proving two lifecycle requests observe two
different `RuntimeCapabilities` values without reconstructing the service.

- [ ] **Step 2: Run refresh tests and verify RED**

Run:

```bash
rtk cargo test --locked -p gascand --test doctor_state refresh
rtk cargo test --locked -p gascand runtime_capabilities
```

Expected: `DoctorState::refreshing` is missing and the second capability read
returns the cached first value.

- [ ] **Step 3: Implement a fixed-or-refreshing DoctorState**

Represent sources explicitly:

```rust
type DoctorFuture = Pin<Box<dyn Future<Output = DoctorReport> + Send>>;
type DoctorCollector = dyn Fn() -> DoctorFuture + Send + Sync;

enum DoctorSource {
    Fixed(DoctorReport),
    Refreshing {
        timeout: Duration,
        collector: Arc<DoctorCollector>,
    },
}
```

`ready` uses `Fixed`. `refreshing` invokes a new future on every `report()`.
Keep the existing timeout fallback text. Retain one-shot `collect` only where
existing tests require its pending/completion semantics; production must use
`refreshing`.

- [ ] **Step 4: Remove daemon-lifetime capability caching**

Remove `capabilities: tokio::sync::OnceCell<RuntimeCapabilities>` from
`SandboxService`. Change:

```rust
async fn runtime_capabilities(&self) -> Result<RuntimeCapabilities, ServiceError> {
    self.runtime.capabilities().await.map_err(ServiceError::Runtime)
}
```

Update callers to borrow the owned local value during policy compilation.

- [ ] **Step 5: Wire production to refreshing evidence**

Construct:

```rust
let doctor = DoctorState::refreshing(Duration::from_secs(60), || {
    production_doctor_report()
});
```

Verify every doctor request and every `require_runtime_ready` path gets fresh
evidence, while SSH facts remain merged per request.

- [ ] **Step 6: Run focused and workspace tests**

Run:

```bash
rtk cargo test --locked -p gascand --test doctor_state
rtk cargo test --locked -p gascand --lib
rtk cargo test --locked -p gascan-core --test policy
```

Expected: all pass, including two sequential reports with different runtime
identities.

- [ ] **Step 7: Commit**

```bash
rtk git add crates/gascand/src/service.rs crates/gascand/src/main.rs \
  crates/gascand/tests/doctor_state.rs
rtk git commit -m "fix: refresh runtime evidence without daemon restart"
```

---

### Task 5: Document and verify the compatibility contract

**Files:**
- Modify: `README.md`
- Modify: `docs/release/macos-checklist.md`

**Interfaces:**
- Consumes: final warning text, accepted version range, certified identity.
- Produces: user-facing install requirements and release acceptance evidence.

- [ ] **Step 1: Update user documentation**

Document:

- Apple Container `>=1.1.0, <2.0.0` is accepted.
- 1.1.0 is the certified release.
- newer 1.x releases display doctor warnings;
- networked sandboxes remain usable;
- offline sandboxes require a certified release;
- changing Apple Container is detected without restarting Gas Can.

Replace caveat text that says Gas Can requires exactly 1.1.0.

- [ ] **Step 2: Add documentation contract assertions**

Confirm `tests/release/source-input-contract.sh` does not freeze this prose.
No release script change is required because the contract currently lives
only in `README.md` and `docs/release/macos-checklist.md`; do not create a new
script solely to grep prose.

- [ ] **Step 3: Run formatting and complete verification**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo test --locked -p gascan-apple --test probe
rtk cargo test --locked -p gascan-core --test doctor
rtk cargo test --locked -p gascan-core --test policy
rtk cargo test --locked -p gascand --test doctor_state
rtk cargo test --locked -p gascan presentation::tests
rtk cargo test --locked --manifest-path scripts/Cargo.toml
rtk git diff --check
```

Expected: all pass. Tests requiring localhost/process inspection must be rerun
outside the filesystem sandbox if they fail only with `Operation not
permitted`.

- [ ] **Step 4: Commit**

```bash
rtk git add README.md docs/release/macos-checklist.md
rtk git commit -m "docs: explain Apple Container compatibility policy"
```
