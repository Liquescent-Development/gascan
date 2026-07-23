# Per-Sandbox Managed Network Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give every newly created networked Gas Can sandbox its own explicitly attached, ownership-labeled Apple Container NAT network.

**Architecture:** The sealed runtime request carries the deterministic managed network name for networked sandboxes. The Apple backend inventories networks as first-class removal-proof resources, creates the network before volumes and the container, and deletes it last. Offline requests continue to use `--network none`; existing sandboxes are neither detected nor migrated.

**Tech Stack:** Rust 2024, Tokio, Serde/serde_json, Apple Container CLI 1.1 structured JSON, shell-based Apple E2E cleanup harness.

## Global Constraints

- A networked sandbox uses exactly `gascan-network-<sandbox-id>`.
- Every managed network has `dev.gascan.managed-by=gascan` and `dev.gascan.sandbox-id=<sandbox-id>`.
- Gas Can never adopts or deletes a network based only on its name.
- Offline sandboxes emit `--network none` and create no network resource.
- Do not pass `--internal`, `--subnet`, `--subnet-v6`, `--plugin`, `--option`, or any DNS override.
- Do not hard-code `10.10.10.53`, a NAT gateway, or an Apple Container subnet.
- Do not fall back to Apple's built-in `default` network.
- Do not add migration, database schema, legacy detection, or `apply` recreation behavior.
- Preserve loopback-only published ports.
- Mutations occur in create order network → volumes → container and delete order container → volumes → network.
- All ownership decisions remain fail-closed and use fresh structured inventory plus the existing opaque removal proof.

---

### Task 1: Model a Managed Network in the Core Runtime Contract

**Files:**
- Modify: `crates/gascan-core/src/runtime.rs`
- Modify: `crates/gascan-core/src/policy.rs`
- Modify: `crates/gascan-core/src/fake_runtime.rs`
- Modify: `crates/gascan-core/tests/common/mod.rs`
- Modify: `crates/gascan-core/tests/policy.rs`
- Modify: `crates/gascan-core/tests/backend_contract.rs`

**Interfaces:**
- Produces: `ResourceKind::Network`.
- Produces: `RuntimeNetwork::Networked { name: String }` and `RuntimeNetwork::Offline`.
- Produces: `RuntimeNetwork::managed_name(&self) -> Option<&str>`.
- Produces: `PolicyCompiler::managed_network_name(id: &SandboxId) -> String`.
- Produces: `create_request_with_network(name: &str, network: &str) -> CreateRequestFixture` for core integration tests.
- Produces: `FakeRuntime::seed_network(name, sandbox_id, ownership)` and `FakeRuntime::network_exists(name)`.

- [ ] **Step 1: Write failing policy and validation tests for the exact network identity**

In `crates/gascan-core/tests/policy.rs`, extend the expected-resource test and add mode-specific assertions:

```rust
#[test]
fn expected_resource_identities_include_the_managed_network() {
    let id = SandboxId::test("expected-network");
    let identities = PolicyCompiler::expected_resource_identities(&id).unwrap();
    let network = identities
        .iter()
        .find(|identity| identity.kind() == ResourceKind::Network)
        .expect("managed network identity");

    assert_eq!(
        network.name(),
        PolicyCompiler::managed_network_name(&id)
    );
    assert_eq!(network.name(), format!("gascan-network-{id}"));
}

#[test]
fn networked_policy_seals_the_exact_managed_network_name() {
    let (_temp, spec) = spec("version = 1\nnetwork = 'networked'\n");
    let request = PolicyCompiler::compile(spec, &capabilities()).unwrap();
    let expected = PolicyCompiler::managed_network_name(request.id());
    assert_eq!(
        request.network().managed_name(),
        Some(expected.as_str())
    );
}

#[test]
fn offline_policy_has_no_managed_network_name() {
    let (_temp, spec) = spec("version = 1\nnetwork = 'offline'\n");
    let request = PolicyCompiler::compile(spec, &capabilities()).unwrap();
    assert_eq!(request.network().managed_name(), None);
}
```

Update `expected_resource_identities_are_derived_from_the_sealed_sandbox_id`
to expect five identities in this exact order: container, three volumes,
network.

In `crates/gascan-core/tests/backend_contract.rs`, add outcome authorization:

```rust
#[test]
fn network_create_evidence_is_authorized_only_for_networked_requests() {
    let networked = create_request_with_network("network-evidence", "networked");
    let name = networked.network().managed_name().unwrap().to_owned();
    let resource = RuntimeResource::discovered(
        ResourceIdentity::new(ResourceKind::Network, name).unwrap(),
        Some(networked.id().clone()),
        ResourceOwnership::GasCanOwned,
    );
    let container = RuntimeResource::discovered(
        ResourceIdentity::new(ResourceKind::Container, networked.id().to_string()).unwrap(),
        Some(networked.id().clone()),
        ResourceOwnership::GasCanOwned,
    );
    assert!(CreateOutcome::new(&networked.request(), vec![resource.clone(), container]).is_ok());

    let offline = create_request_with_network("offline-evidence", "offline");
    let error = CreateFailure::from_created_evidence(
        &offline.request(),
        vec![RuntimeResource::discovered(
            ResourceIdentity::new(ResourceKind::Network, resource.name()).unwrap(),
            Some(offline.id().clone()),
            ResourceOwnership::GasCanOwned,
        )],
        RuntimeError::InjectedFailure { boundary: "test".into() },
    );
    assert!(error.created().is_empty());
}
```

- [ ] **Step 2: Run the focused tests to verify they fail**

Run:

```bash
cargo test -p gascan-core --test policy expected_resource_identities_include_the_managed_network
cargo test -p gascan-core --test backend_contract network_create_evidence_is_authorized_only_for_networked_requests
```

Expected: compilation fails because `ResourceKind::Network`,
`managed_network_name`, `managed_name`, and the network-selectable fixture do
not exist.

- [ ] **Step 3: Implement the sealed network request and resource authorization**

In `crates/gascan-core/src/runtime.rs`, change the runtime network type and
accessor:

```rust
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeNetwork {
    Networked { name: String },
    Offline,
}

impl RuntimeNetwork {
    pub fn managed_name(&self) -> Option<&str> {
        match self {
            Self::Networked { name } => Some(name),
            Self::Offline => None,
        }
    }
}

impl CreateRequest {
    pub const fn network(&self) -> &RuntimeNetwork {
        &self.network
    }
}

pub enum ResourceKind {
    Container,
    Volume,
    Network,
}
```

Add a helper used by both create outcome validators:

```rust
fn allowed_network(request: &CreateRequest) -> Option<ResourceIdentity> {
    request
        .network()
        .managed_name()
        .map(|name| ResourceIdentity {
            kind: ResourceKind::Network,
            name: name.to_owned(),
        })
}
```

In both `CreateFailure::from_created_evidence` and
`validate_created_resources`, accept a network only when its identity equals
`allowed_network(request)`, its sandbox ID equals the request ID, and ownership
is `GasCanOwned`. When `require_container` is true, require both the container
and the managed network for a networked request; offline successful outcomes
continue to require only the container. Add a test showing that a networked
`CreateOutcome` containing a container but no managed network is rejected with
`invalid_state`.

In `crates/gascan-core/src/policy.rs`, centralize naming and seal it into the
request:

```rust
impl PolicyCompiler {
    pub fn managed_network_name(id: &SandboxId) -> String {
        format!("gascan-network-{id}")
    }
}
```

Append a `ResourceKind::Network` identity in
`expected_resource_identities`. Compile manifest modes as:

```rust
let network = match manifest.network() {
    NetworkMode::Networked => RuntimeNetwork::Networked {
        name: Self::managed_network_name(spec.id()),
    },
    NetworkMode::Offline => RuntimeNetwork::Offline,
};
```

Update call sites that previously copied `RuntimeNetwork` to borrow or clone
it.

- [ ] **Step 4: Add conditional managed-network behavior to the fake runtime**

In `crates/gascan-core/tests/common/mod.rs`, make the existing helper delegate:

```rust
pub fn create_request(name: &str) -> CreateRequestFixture {
    create_request_with_network(name, "offline")
}

pub fn create_request_with_network(name: &str, network: &str) -> CreateRequestFixture {
    assert!(matches!(network, "offline" | "networked"));
    // Existing temporary-root setup remains unchanged.
    std::fs::write(
        root.join("gascan.toml"),
        format!("version = 1\nnetwork = '{network}'\n"),
    )
    .expect("write backend-contract manifest");
    // Load, compile, and return the existing fixture type.
}
```

In `FakeRuntime::create`, before the volume loop:

```rust
if let Some(name) = request.network().managed_name() {
    let identity = match ResourceIdentity::new(ResourceKind::Network, name) {
        Ok(identity) => identity,
        Err(error) => return Err(create_failure(&request, created, error)),
    };
    if state.resources.contains_key(&identity) {
        return Err(create_failure(
            &request,
            created,
            RuntimeError::Conflict {
                resource: name.to_owned(),
                message: "network already exists".to_owned(),
            },
        ));
    }
    let resource = RuntimeResource::discovered(
        identity.clone(),
        Some(request.id().clone()),
        ResourceOwnership::GasCanOwned,
    );
    state.resources.insert(identity, resource.clone());
    created.push(resource);
    fail_after_create_mutation(&mut state, &request, &created)?;
}
```

Add exact-kind seed/query helpers:

```rust
pub async fn seed_network(
    &self,
    name: &str,
    sandbox_id: Option<SandboxId>,
    ownership: ResourceOwnership,
) -> Result<(), RuntimeError> {
    let identity = ResourceIdentity::new(ResourceKind::Network, name)?;
    self.inner.lock().await.resources.insert(
        identity.clone(),
        RuntimeResource::discovered(identity, sandbox_id, ownership),
    );
    Ok(())
}

pub async fn network_exists(&self, name: &str) -> bool {
    self.inner.lock().await.resources.values().any(|resource| {
        resource.kind() == ResourceKind::Network && resource.name() == name
    })
}
```

Add tests proving a networked create reports network → volumes → container
evidence, an offline create has no `Network`, a same-name seeded network
conflicts without adoption, and removal deletes the exact fake network.

- [ ] **Step 5: Run core tests and commit**

Run:

```bash
cargo test -p gascan-core --test policy
cargo test -p gascan-core --test backend_contract
cargo test -p gascan-core --lib
```

Expected: all pass.

Commit:

```bash
git add crates/gascan-core/src/runtime.rs crates/gascan-core/src/policy.rs crates/gascan-core/src/fake_runtime.rs crates/gascan-core/tests/common/mod.rs crates/gascan-core/tests/policy.rs crates/gascan-core/tests/backend_contract.rs
git commit -m "feat: model managed sandbox networks"
```

---

### Task 2: Translate Networked Requests to an Explicit Apple Network

**Files:**
- Modify: `crates/gascan-apple/src/translate.rs`
- Modify: `crates/gascan-apple/tests/translate.rs`

**Interfaces:**
- Consumes: `CreateRequest::network() -> &RuntimeNetwork`.
- Consumes: the policy-sealed network name from Task 1.
- Produces: `container run --network gascan-network-<sandbox-id>` for networked requests.

- [ ] **Step 1: Replace the old implicit-default assertion with an exact-network assertion**

In `crates/gascan-apple/tests/translate.rs`, change the networked test:

```rust
#[test]
fn networked_create_uses_the_managed_network_and_loopback_publish() {
    let (_root, request) = request(
        "web",
        "version = 1\nnetwork = 'networked'\n[ports]\nweb = 3000\n",
    );
    let expected_network = request.network().managed_name().unwrap();
    let spec = AppleCommandBuilder::create(&request).unwrap();

    assert!(spec.args.windows(2).any(
        |pair| pair == ["--publish", "127.0.0.1:3000:3000"]
    ));
    assert!(spec.args.windows(2).any(
        |pair| pair[0] == "--network" && pair[1] == expected_network
    ));
    assert!(!spec.args.windows(2).any(
        |pair| pair == ["--network", "default"]
    ));
}
```

Retain the literal offline fixture assertion so it continues to prove
`--network none`.

- [ ] **Step 2: Run the translation test to verify it fails**

Run:

```bash
cargo test -p gascan-apple --test translate networked_create_uses_the_managed_network_and_loopback_publish
```

Expected: FAIL because networked translation emits no `--network`.

- [ ] **Step 3: Translate both runtime network variants explicitly**

Make `CreateView.network` a cloned `RuntimeNetwork` and emit:

```rust
match &view.network {
    RuntimeNetwork::Networked { name } => {
        args.extend(["--network".to_owned(), name.clone()]);
    }
    RuntimeNetwork::Offline => {
        args.extend(["--network".to_owned(), "none".to_owned()]);
    }
}
```

Do not derive the name in the translator and do not emit any network, subnet,
plugin, internal-network, or DNS flags beyond this exact pair.

- [ ] **Step 4: Run translation tests and commit**

Run:

```bash
cargo test -p gascan-apple --test translate
```

Expected: all pass, including the unchanged literal offline argv fixture.

Commit:

```bash
git add crates/gascan-apple/src/translate.rs crates/gascan-apple/tests/translate.rs
git commit -m "feat: attach sandboxes to managed networks"
```

---

### Task 3: Inventory Apple Networks with Fail-Closed Ownership

**Execution note:** Tasks 3 and 4 are one implementation and review unit.
Complete the inventory work, continue directly through Task 4, and commit and
review only after the full Apple backend suite is green. Do not create the
intermediate intentionally-red commit described below.

**Files:**
- Modify: `crates/gascan-apple/src/backend.rs`
- Modify: `crates/gascan-apple/tests/backend_fake_runner.rs`

**Interfaces:**
- Consumes: `ResourceKind::Network`.
- Produces: network `RuntimeResource` observations from `container network list --format json`.
- Produces: strict `NetworkRecord { id, configuration: { name, labels } }` deserialization.

- [ ] **Step 1: Extend the stateful runner and write failing inventory tests**

Add `networks: BTreeMap<String, (String, String)>` to the fake runner state and
support:

```rust
["network", "list", "--format", "json"] => json!(
    state.networks.iter().map(|(name, (sandbox, manager))| json!({
        "id": name,
        "configuration": {
            "name": name,
            "labels": {
                "dev.gascan.managed-by": manager,
                "dev.gascan.sandbox-id": sandbox
            }
        },
        "status": {}
    })).collect::<Vec<_>>()
),
```

Add an injectable raw network-list record so an ID/name disagreement can be
tested without weakening the normal map-backed fixture.

Add these tests:

```rust
#[tokio::test]
async fn inventory_reports_owned_foreign_and_mismatched_networks() {
    // Seed three distinct network records through the runner state.
    // Assert ResourceKind::Network and each exact ownership classification.
}

#[tokio::test]
async fn inventory_rejects_network_id_and_name_disagreement() {
    // Return id "gascan-network-a" with configuration.name "gascan-network-b".
    // Assert RuntimeError::InvalidOutput with code "invalid_output".
}
```

- [ ] **Step 2: Run the inventory tests to verify they fail**

Run:

```bash
cargo test -p gascan-apple --test backend_fake_runner inventory_reports_owned_foreign_and_mismatched_networks
cargo test -p gascan-apple --test backend_fake_runner inventory_rejects_network_id_and_name_disagreement
```

Expected: FAIL because the backend never requests or parses network inventory.

- [ ] **Step 3: Parse and classify network inventory**

In `crates/gascan-apple/src/backend.rs`, add:

```rust
#[derive(Deserialize)]
struct NetworkRecord {
    id: String,
    configuration: NetworkConfiguration,
}

#[derive(Deserialize)]
struct NetworkConfiguration {
    name: String,
    #[serde(default)]
    labels: BTreeMap<String, String>,
}
```

After volume inventory, run:

```rust
let output = self.runner.run(CommandSpec::new(
    "container",
    ["network", "list", "--format", "json"],
)).await?;
let records: Vec<NetworkRecord> = serde_json::from_slice(&output.stdout)
    .map_err(|error| invalid_output("container network list", error.to_string()))?;
for record in records {
    if record.id != record.configuration.name {
        return Err(invalid_output(
            "container network list",
            "network id and name differ".into(),
        ));
    }
    let sandbox_id = record.configuration.labels
        .get(SANDBOX_ID_LABEL)
        .map(|value| SandboxId::try_from(value.clone()))
        .transpose()
        .map_err(|error| invalid_output("container network list", error.to_string()))?;
    let ownership = classify(sandbox_id.as_ref(), &record.configuration.labels);
    let identity = ResourceIdentity::new(ResourceKind::Network, record.id)?;
    resources.push(RuntimeResource::discovered(
        identity,
        sandbox_id,
        ownership,
    ));
}
```

Run network inventory before entering the observation-cache reconciliation so
networks get the same process-local removal proof behavior as other resources.

- [ ] **Step 4: Run backend inventory tests and commit**

Run:

```bash
cargo test -p gascan-apple --test backend_fake_runner inventory_
```

Expected: all network-inventory tests pass. The pre-existing successful-create
contract remains intentionally red until Task 4 creates the now-required
managed network; do not weaken the core outcome requirement to make it pass.

Do not commit at this intermediate point. Continue directly to Task 4 so the
backend's now-required managed network is implemented before review.

---

### Task 4: Create, Reconcile, Roll Back, and Delete Apple Networks

**Files:**
- Modify: `crates/gascan-apple/src/backend.rs`
- Modify: `crates/gascan-apple/tests/backend_fake_runner.rs`

**Interfaces:**
- Consumes: `CreateRequest::network().managed_name()`.
- Produces: labeled `container network create` before volume creation.
- Produces: `container network delete` after container and volume deletion.
- Produces: verified network evidence in `CreateOutcome` and `CreateFailure`.

- [ ] **Step 1: Teach the fake runner the exact network mutation commands**

Support these literal shapes:

```rust
[
    "network", "create",
    "--label", manager,
    "--label", sandbox,
    name,
] => {
    if state.networks.contains_key(*name) {
        return conflict(name);
    }
    state.networks.insert(
        (*name).into(),
        (
            sandbox.split_once('=').unwrap().1.into(),
            manager.split_once('=').unwrap().1.into(),
        ),
    );
    json!(null)
}

["network", "delete", name] => {
    state.networks.remove(*name);
    json!(null)
}
```

Add network-list fault queues mirroring the existing volume and container
faults, including `CommandIo`, `InvalidJson`, `Absent`, `Foreign`, and
`Mismatched`.

- [ ] **Step 2: Write failing lifecycle, ordering, and ambiguity tests**

Add tests that assert:

```rust
#[tokio::test]
async fn networked_create_labels_network_before_volumes_and_attaches_container() {
    // Create a networked request.
    // Locate command indices for network create, first volume create, and run.
    // Assert network_index < volume_index < run_index.
    // Assert both ownership labels on network create.
    // Assert the run argv contains ["--network", request.network().managed_name().unwrap()].
    // Assert CreateOutcome contains Network, three Volume, then Container.
}

#[tokio::test]
async fn transient_network_create_io_reconciles_exact_owned_side_effect() {
    // Mutate the fake network map, return CommandIo, and allow re-inventory.
    // Assert the failure contains exactly one verified Network resource.
}

#[tokio::test]
async fn foreign_network_observation_is_never_returned_as_create_evidence() {
    // After create, expose the network with foreign labels.
    // Assert ownership_mismatch and empty created evidence.
}

#[tokio::test]
async fn remove_deletes_container_then_volumes_then_network() {
    // Create and remove the full outcome.
    // Assert command ordering and empty final inventory.
}

#[tokio::test]
async fn remove_refuses_a_network_changed_after_observation() {
    // Change the manager label before remove.
    // Assert ownership_mismatch and that no network delete command ran.
}
```

Update existing partial-create counts: a successful network is now the first
piece of evidence, successful container creation has five total resources, and
a container verification failure retains the network plus three volumes.

- [ ] **Step 3: Run the focused tests to verify they fail**

Run:

```bash
cargo test -p gascan-apple --test backend_fake_runner networked_create_labels_network_before_volumes_and_attaches_container
cargo test -p gascan-apple --test backend_fake_runner transient_network_create_io_reconciles_exact_owned_side_effect
cargo test -p gascan-apple --test backend_fake_runner remove_deletes_container_then_volumes_then_network
```

Expected: FAIL because the backend has not created or deleted networks.

- [ ] **Step 4: Implement network create and verified evidence**

Extend pre-create collision checking and `reconcile_created` so the request's
managed network name is an allowed exact resource.

Before the volume loop:

```rust
if let Some(name) = request.network().managed_name() {
    let manager = format!("{MANAGED_BY_LABEL}={MANAGED_BY}");
    let sandbox = format!("{SANDBOX_ID_LABEL}={}", request.id());
    let spec = CommandSpec::new(
        "container",
        [
            "network",
            "create",
            "--label",
            &manager,
            "--label",
            &sandbox,
            name,
        ],
    );
    if let Err(error) = self.runner.run(spec).await {
        if matches!(&error, RuntimeError::CommandIo { .. }) {
            created = self.reconcile_created(&request, &before, created).await;
        }
        return Err(create_failure(&request, created, error));
    }
    let identity = match ResourceIdentity::new(ResourceKind::Network, name) {
        Ok(identity) => identity,
        Err(error) => return Err(create_failure(&request, created, error)),
    };
    let resource = match self.current_for(&identity).await {
        Ok(Some(resource))
            if resource.ownership() == ResourceOwnership::GasCanOwned
                && resource.sandbox_id() == Some(request.id()) => resource,
        Ok(_) => {
            created = self.reconcile_created(&request, &before, created).await;
            return Err(create_failure(
                &request,
                created,
                RuntimeError::OwnershipMismatch { resource: name.to_owned() },
            ));
        }
        Err(error) => {
            created = self.reconcile_created(&request, &before, created).await;
            return Err(create_failure(&request, created, error));
        }
    };
    created.push(resource);
}
```

Use the same explicit error conversion style already present in the function
instead of `?` where `CreateFailure` requires accumulated evidence.

- [ ] **Step 5: Implement dependency-ordered network removal**

Change the kind order:

```rust
let ordered = [
    ResourceKind::Container,
    ResourceKind::Volume,
    ResourceKind::Network,
];
```

Extend the command match:

```rust
ResourceKind::Network => {
    CommandSpec::new("container", ["network", "delete", recorded.name()])
}
```

Retain the existing fresh `current_for`, exact `RuntimeResource` comparison,
ownership check, sandbox-ID check, command success requirement, and
observation-cache removal for every kind.

- [ ] **Step 6: Run Apple backend tests and commit**

Run:

```bash
cargo test -p gascan-apple --test backend_fake_runner
cargo test -p gascan-apple --test translate
cargo test -p gascan-apple --lib
```

Expected: all pass.

Commit:

```bash
git add crates/gascan-apple/src/backend.rs crates/gascan-apple/tests/backend_fake_runner.rs
git commit -m "feat: manage Apple sandbox network lifecycle"
```

---

### Task 5: Prove Daemon Rollback, Reconciliation, and Safe E2E Cleanup

**Files:**
- Modify: `crates/gascand/tests/lifecycle.rs`
- Modify: `crates/gascand/tests/reconcile.rs`
- Modify: `crates/gascan-e2e/tests/apple_common/mod.rs`
- Modify: `scripts/apple-e2e-cleanup.sh`
- Modify: `scripts/tests/apple_e2e_cleanup.rs`

**Interfaces:**
- Consumes: the five policy-derived expected resource identities.
- Consumes: `FakeRuntime::seed_network` and `network_exists`.
- Produces: network-aware test cleanup that inventories labels before deletion.
- Produces: `resource_presence(identity, sandbox_id)` with kind-specific inventory.

- [ ] **Step 1: Write daemon tests for network rollback, destroy, and reconciliation**

In `crates/gascand/tests/lifecycle.rs`, add a helper that writes a networked
manifest and tests:

```rust
#[tokio::test]
async fn failed_networked_up_rolls_back_the_managed_network() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    std::fs::write(root.join("gascan.toml"), "version = 1\nnetwork = 'networked'\n")?;
    let spec = SandboxSpec::from_root("network-rollback", root, Manifest::load(root)?)?;
    let network = PolicyCompiler::managed_network_name(spec.id());
    let runtime = FakeRuntime::failing_once(FailureBoundary::Start);
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );

    assert!(service.up(UpRequest::new(spec)).await.is_err());
    assert!(!runtime.network_exists(&network).await);
    Ok(())
}
```

Add a successful networked `up` → `destroy` test that asserts the managed
network exists after `up` and not after `destroy`.

In `crates/gascand/tests/reconcile.rs`, seed:

```rust
runtime.seed_network(
    &PolicyCompiler::managed_network_name(&unknown),
    Some(unknown.clone()),
    ResourceOwnership::GasCanOwned,
).await?;
```

Assert it becomes `ReconcileFinding::UnknownOwned` and remains present. Also
add a known sandbox fixture proving its exact network identity is not reported
as unknown.

- [ ] **Step 2: Run daemon tests to verify the new cases fail**

Run:

```bash
cargo test -p gascand --test lifecycle failed_networked_up_rolls_back_the_managed_network
cargo test -p gascand --test reconcile network
```

Expected: the lifecycle test fails until fake-runtime network evidence and
generic rollback are fully connected; reconciliation assertions expose any
expected-identity omissions.

- [ ] **Step 3: Make the Rust E2E harness kind-aware**

Change `resource_presence` to accept `&ResourceIdentity`. For containers and
volumes, keep the existing inspect commands. For a network, run
`container network list --format json`, require an array, select exactly one
record whose `id` and `configuration.name` equal the identity name, and
classify its two ownership labels.

Use an exhaustive mutation match:

```rust
match identity.kind() {
    ResourceKind::Container => {
        Command::new("container").args(["delete", name]).status()?
    }
    ResourceKind::Volume => {
        Command::new("container").args(["volume", "delete", name]).status()?
    }
    ResourceKind::Network => {
        Command::new("container").args(["network", "delete", name]).status()?
    }
}
```

Update `cleanup_resource_identities` so manifest index 0 is `Container`,
indices 1 through 3 are `Volume`, and index 4 is `Network`. Reject every other
resource count or order rather than guessing from names.

- [ ] **Step 4: Write cleanup-script tests before changing the shell script**

In `scripts/tests/apple_e2e_cleanup.rs`, add
`managed_resources(id: &str) -> serde_json::Value` returning:

```rust
serde_json::json!([
    id,
    format!("gascan-mise-{id}"),
    format!("gascan-cache-{id}"),
    format!("gascan-config-{id}"),
    format!("gascan-network-{id}"),
])
```

Replace repeated four-entry arrays with this helper. Extend the fake
`container` executable with labeled network list and network delete behavior.

Add tests proving:

- an exact owned network is deleted after the three volumes;
- a foreign or mismatched same-name network is retained and the manifest is
  retained;
- ambiguous duplicate network records abort cleanup;
- malformed network inventory aborts cleanup; and
- successful cleanup verifies that the network is absent.

- [ ] **Step 5: Run cleanup tests to verify they fail**

Run:

```bash
cargo test --manifest-path scripts/Cargo.toml --test apple_e2e_cleanup
```

Expected: failures because the script still requires a four-resource manifest
and never inventories or deletes networks.

- [ ] **Step 6: Extend the shell cleanup proof to networks**

In `scripts/apple-e2e-cleanup.sh`:

1. Require this exact manifest sequence:

```sh
expected=$(printf '%s\n%s\n%s\n%s\n%s\n' \
  "$id" \
  "gascan-mise-$id" \
  "gascan-cache-$id" \
  "gascan-config-$id" \
  "gascan-network-$id")
```

2. Inventory `container network list --format json` and validate every record
   is an object with string `.id`, string `.configuration.name`, and equal
   values.
3. After container and volume deletion, select exactly one or zero records for
   `gascan-network-$id`.
4. Delete only when both Gas Can labels exactly match; otherwise retain the
   manifest and exit nonzero.
5. Run `container network delete "gascan-network-$id"`.
6. Re-inventory networks during residue verification and leave the manifest in
   place if the exact network remains.

Do not use `container network delete --all`, substring matching, prefix-based
inventory selection, or name-only deletion authority.

- [ ] **Step 7: Run daemon and cleanup suites and commit**

Run:

```bash
cargo test -p gascand --test lifecycle
cargo test -p gascand --test reconcile
cargo test -p gascan-e2e --test apple_lifecycle
cargo test --manifest-path scripts/Cargo.toml --test apple_e2e_cleanup
```

Expected: all non-ignored tests pass.

Commit:

```bash
git add crates/gascand/tests/lifecycle.rs crates/gascand/tests/reconcile.rs crates/gascan-e2e/tests/apple_common/mod.rs scripts/apple-e2e-cleanup.sh scripts/tests/apple_e2e_cleanup.rs
git commit -m "test: cover managed network recovery and cleanup"
```

---

### Task 6: Add the Real Apple Connectivity and Isolation Proof

**Files:**
- Modify: `crates/gascan-e2e/tests/apple_common/mod.rs`
- Modify: `crates/gascan-e2e/tests/apple_lifecycle.rs`

**Interfaces:**
- Produces: `AppleE2e::new_networked(name: &str)`.
- Produces: `AppleE2e::assert_managed_network_attachment()`.
- Produces: live DNS and HTTPS egress proof through the managed NAT network.

- [ ] **Step 1: Add a networked harness constructor and structured attachment assertion**

Add an optional network-mode parameter to the existing private scoped
constructor. `AppleE2e::new` and existing scoped test helpers pass `None`;
`AppleE2e::new_networked` passes `Some("networked")`. Build the manifest as:

```rust
pub fn new_networked(name: &str) -> TestResult<Self> {
    let manifest = std::env::var_os("GASCAN_E2E_CLEANUP_MANIFEST")
        .map(std::path::PathBuf::from);
    let session_root = std::env::var_os("GASCAN_E2E_SESSION_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_session_root);
    Self::new_scoped_with_diagnostics(
        name,
        session_root,
        manifest,
        std::env::var_os("GASCAN_GATE4_DIAGNOSTICS").is_some(),
        Some("networked"),
    )
}
```

Inside the private constructor:

```rust
let network_line = match network_mode {
    Some(mode) => format!("network = {}\n", serde_json::to_string(mode)?),
    None => String::new(),
};
std::fs::write(
    root_path.join("gascan.toml"),
    format!(
        "version = 1\nname = {}\n{network_line}",
        serde_json::to_string(name)?
    ),
)?;
```

Keep `AppleE2e::new` behavior unchanged for tests that intentionally rewrite
the manifest.

Add:

```rust
pub fn assert_managed_network_attachment(&self) -> TestResult {
    let expected = format!("gascan-network-{}", self.id());
    let output = Command::new("container")
        .args(["inspect", self.id()])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "container inspect failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ).into());
    }
    let inspect: Value = serde_json::from_slice(&output.stdout)?;
    let networks = inspect[0]["configuration"]["networks"]
        .as_array()
        .ok_or("container inspect lacks network attachments")?;
    if networks.len() != 1 || networks[0]["network"] != expected {
        return Err(format!("unexpected network attachments: {networks:?}").into());
    }
    // Inventory the exact network and verify id/name plus both ownership labels.
    Ok(())
}
```

Then inventory `container network list --format json`, select exactly one
record with both `id` and `configuration.name` equal to `expected`, and verify
the two Gas Can ownership labels.

- [ ] **Step 2: Add live assertions to the ignored Apple lifecycle test**

Construct the lifecycle environment with `AppleE2e::new_networked`. Immediately
after the first successful `up`:

```rust
env.assert_managed_network_attachment()?;

let dns = env.success([
    "--sandbox", env.id(), "run", "--",
    "getent", "ahosts", "github.com",
])?;
if dns.stdout.is_empty() {
    return Err("managed network DNS lookup returned no addresses".into());
}

env.success([
    "--sandbox", env.id(), "run", "--",
    "curl", "--fail", "--silent", "--show-error",
    "--max-time", "20", "--output", "/dev/null", "https://github.com/",
])?;
```

After the existing destroy, retain `assert_no_owned_resources`; it now checks
the network as well as the container and volumes.

- [ ] **Step 3: Run the platform-neutral E2E tests**

Run:

```bash
cargo test -p gascan-e2e --test apple_lifecycle
cargo test -p gascan-e2e apple_common::tests
```

Expected: all platform-neutral tests pass and the real lifecycle remains
ignored in ordinary Cargo runs.

- [ ] **Step 4: Run formatting, lint, and all non-live suites**

Run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --manifest-path scripts/Cargo.toml
git diff --check
```

Expected: all pass. If the PTY test is denied by the execution sandbox, rerun
the exact workspace test command with host permissions; do not mark the suite
passing based only on a filtered run.

- [ ] **Step 5: Run the serial real Apple lifecycle**

Before running, confirm the user's pre-feature sandbox is already absent. Do
not destroy it. If it is still present, stop and ask the user to remove it.
Then run:

```bash
bash ./scripts/run-apple-e2e.sh apple_lifecycle
```

Expected:

- the managed network is created with exact labels;
- the container is attached only to that network, never `default`;
- `github.com` resolves inside the guest;
- HTTPS to `https://github.com/` succeeds;
- the complete lifecycle succeeds; and
- final cleanup leaves no exact managed container, volume, or network.

- [ ] **Step 6: Commit the live proof**

```bash
git add crates/gascan-e2e/tests/apple_common/mod.rs crates/gascan-e2e/tests/apple_lifecycle.rs
git commit -m "test: prove managed sandbox network egress"
```

- [ ] **Step 7: Perform final branch verification**

Run:

```bash
git status --short
git log --oneline --decorate -7
git diff --check main...HEAD
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --manifest-path scripts/Cargo.toml
```

Expected: the worktree is clean; the design, plan, and six implementation
commits are present; all checks pass; and no changes exist in the user's main
worktree.
