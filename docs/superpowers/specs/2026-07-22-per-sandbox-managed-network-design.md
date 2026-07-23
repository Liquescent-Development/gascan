# Per-Sandbox Managed Network Design

## Purpose

Gas Can must give every newly created `network = "networked"` sandbox working
outbound networking without attaching it to Apple Container's built-in
`default` network. Each networked sandbox receives its own Gas Can-managed NAT
network so it is isolated from unrelated Apple containers and from other Gas
Can sandboxes.

The host in which the failure was reproduced permits DNS only through
`10.10.10.53`. Apple Container's custom NAT gateways correctly relay guest DNS
through the host's permitted resolver path, while the built-in `default`
network resolves names but cannot establish outbound connections. Gas Can will
select a custom NAT network; it will not hard-code a DNS server, gateway,
subnet, or host-specific firewall rule.

## User-Visible Contract

The manifest contract remains unchanged:

```toml
network = "networked"
```

For a new networked sandbox with ID `<sandbox-id>`, `gascan up` creates and
uses:

```text
gascan-network-<sandbox-id>
```

The sandbox container is explicitly attached to that network. Apple Container
allocates the NAT subnet, gateway, and guest resolver. Published ports remain
bound to IPv4 loopback exactly as they are today.

An offline sandbox continues to use `--network none` and does not create a
managed network.

There is no migration behavior. Gas Can will not inspect, recreate, or repair
an existing sandbox that was created on the built-in network. Users must
destroy such a sandbox and create it again.

## Runtime Resource Model

`ResourceKind` gains a `Network` variant. The deterministic managed network
identity is derived from the sealed sandbox ID by the policy layer, alongside
the container and volume identities. This keeps resource naming centralized
and gives create, rollback, destroy, recovery, and reconciliation the same
expected identity.

A network is Gas Can-owned only when both labels are present and exact:

```text
dev.gascan.managed-by=gascan
dev.gascan.sandbox-id=<sandbox-id>
```

The network ID and configured name must also equal
`gascan-network-<sandbox-id>`. Missing labels, invalid sandbox IDs, inconsistent
names, and partial ownership metadata are classified using the existing
`Foreign` or `Mismatched` rules. Gas Can never adopts or deletes a network
based on its name alone.

The fake runtime models the same conditional resource: networked create
requests create one `Network` resource, while offline requests do not. Create
outcome and partial-create evidence validation permit the exact network
identity only for networked requests.

## Apple Backend Lifecycle

Before mutation, the Apple backend inventories containers, volumes, and
networks. Network inventory comes from:

```text
container network list --format json
```

The backend validates that each network record's ID and configured name agree,
parses its ownership labels, and converts it to a `RuntimeResource`. The same
observation cache and removal-proof behavior used for containers and volumes
applies to networks.

For a networked create request, the backend performs these mutations in order:

1. Confirm that none of the request's exact container, volume, or network names
   already exist.
2. Create `gascan-network-<sandbox-id>` with the two ownership labels by
   running `container network create`.
3. Re-inventory the exact network and verify its name, sandbox ID, and
   ownership before recording it as created.
4. Create and verify the managed volumes.
5. Run the container with
   `--network gascan-network-<sandbox-id>`, then verify the container.

The backend does not pass `--internal`, `--subnet`, `--subnet-v6`, `--plugin`,
`--option`, or a DNS override. The Apple Container default NAT plugin and its
automatic address allocation remain authoritative.

For an offline request, the network creation steps are skipped and translation
continues to emit `--network none`.

Destroy and create rollback delete resources in dependency order:

1. container;
2. volumes; and
3. network.

The network is therefore never deleted while its sandbox container is still
attached. Every deletion re-inventories the exact resource and compares the
current observation with the opaque removal proof immediately before invoking
`container network delete`.

## Failure Handling

A same-name network of any ownership is a resource conflict before creation.
If the name is foreign or its labels are malformed, Gas Can reports the
conflict and leaves it untouched.

If `container network create` reports a command I/O ambiguity, the backend
re-inventories resources. It includes the network in partial-create evidence
only when the resource is newly observed and its exact ownership labels match
the request. This gives the service enough proof to remove a network that was
created before the command result was lost.

Any later volume or container failure returns the verified network in the
partial-create evidence. Existing service rollback then deletes all proven
resources in dependency order. A failed ownership check never produces
deletion authority.

If network deletion fails, destroy remains failed and the database does not
claim complete absence. A subsequent destroy or pending-operation recovery can
retry after re-inventorying the still-owned network.

No automatic fallback to Apple's built-in `default` network is allowed. Such a
fallback would silently weaken both connectivity reliability and the requested
isolation policy.

## Reconciliation and Compatibility

The policy's expected resource identity set includes the deterministic network
name so destroy and recovery can find a proven managed network even though the
database does not persist runtime network details. Offline sandboxes simply
have no network resource to inventory.

Reconciliation treats a correctly labeled network belonging to an unknown
sandbox as an unknown owned resource, a network with unrelated ownership as
unowned, and inconsistent Gas Can metadata as an ownership mismatch. Existing
container-based sandbox presence checks remain unchanged.

This is a forward-only behavior change. There is no schema migration, database
change, legacy-network detector, `apply` recreation path, or compatibility
shim for existing sandboxes.

## Verification

Unit and contract coverage will establish:

- network resource identities are deterministic and ownership-checked;
- networked fake-runtime creates and rollback include the network;
- offline fake-runtime requests never create a network;
- Apple network inventory rejects inconsistent IDs and classifies ownership;
- Apple create invokes labeled network creation before volumes and attaches the
  container with the exact network name;
- offline translation remains `--network none`;
- ambiguous and later create failures retain only verified network evidence;
- destroy deletes container before network and refuses changed or foreign
  network observations; and
- reconciliation and recovery recognize the managed network identity.

The Apple live lifecycle matrix will additionally prove that a new networked
sandbox:

- is attached to its dedicated managed network rather than `default`;
- receives DNS through the custom NAT gateway;
- resolves a public hostname;
- establishes an outbound HTTPS connection; and
- leaves no managed network after destroy.

The full Rust workspace test suite and formatting/lint checks remain required.
