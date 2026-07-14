# Gas Can macOS Sandbox Design

## Summary

Gas Can is a secure, CLI-first sandbox for agentic coding and orchestration. The macOS MVP runs one long-lived OCI workspace container per selected host code root. Apple's Containerization stack places that container inside its own lightweight Linux VM, which is the hard security boundary.

Gas Can does not implement a container runtime or VM platform. It owns developer experience, lifecycle, policy, and reproducibility while delegating images, VM creation, mounts, networking, and volumes to Apple's `container` CLI through a narrow backend adapter. Attached process execution uses a bundled Swift helper linked to Apple's public `ContainerAPIClient`, because the CLI subprocess surface does not expose the guest process identity required for exact exit, resize, and supported terminal interrupt control.

The first release requires Apple silicon and macOS 26 or newer. A later Linux backend may implement the same runtime contract with Firecracker.

## Goals

- Protect the macOS host outside a user-selected code root from untrusted repositories, dependencies, and agent-executed commands.
- Provide a fast CLI workflow: `up`, `apply`, `shell`, `run`, `down`, `destroy`, `list`, `status`, `logs`, and `doctor`.
- Make a broad polyglot development environment and Gascamp available immediately.
- Keep workspace state reproducible and replaceable while persisting expensive caches and tool installations.
- Provide an on-demand local service and versioned API that a future GUI can use unchanged.
- Preserve a clean backend boundary for future Firecracker support on Linux.

## Non-goals

- Reimplementing Docker, OCI image management, networking, or a general VM platform.
- Supporting Intel Macs or macOS releases older than 26 in the MVP.
- Supporting arbitrary host mounts, host credential forwarding, or runtime socket forwarding in the MVP.
- Protecting files within the selected read/write code root from sandboxed commands.
- Supporting multiple workspace containers per code root in the MVP.
- Providing full Dev Container compatibility in the MVP.

## Product Decomposition

The product consists of five bounded systems:

1. The CLI and future GUI clients.
2. An on-demand local control service, `gascand`.
3. Platform runtime adapters, beginning with Apple `container`.
4. The versioned polyglot workspace image.
5. Manifest, mount, network, resource, and Gascamp policy.

This spec covers a complete macOS thin slice across all five. Linux/Firecracker support and a GUI receive separate future specs.

## Architecture

### Clients

The `gascan` CLI is a thin client of `gascand`. It renders progress and errors for humans, attaches terminals, and forwards signals, but does not contain VM lifecycle logic. A future GUI uses the same local API and event stream.

### On-demand daemon

The CLI starts `gascand` when needed. The daemon listens only on a user-owned Unix socket, serves a versioned local API, coordinates concurrent operations, owns desired-state metadata, and exits after a configurable idle period when no operations or sessions are active.

The daemon owns:

- Manifest loading and validation.
- Sandbox identity and desired state.
- Security, mount, network, port, and resource policy.
- Lifecycle orchestration and rollback.
- Structured events, progress, and diagnostic logs.
- Compatibility checks and runtime reconciliation.

### Runtime backend

The macOS `RuntimeBackend` invokes Apple's `container` executable with argument arrays, never shell-constructed commands. It uses structured output for inspection and discovery and hides Apple-specific identifiers and schemas from clients.

For attached processes only, the adapter starts a bundled `gascan-apple-attach` Swift helper linked exactly to Apple `ContainerAPIClient` 1.1.0. The helper creates the guest process and forwards framed stdin/stdout/stderr, resize, close, error, and exact-exit events over private pipes. TTY SIGINT is delivered as terminal byte `0x03`; non-TTY SIGINT and every other signal are rejected promptly as unsupported because the public 1.1.0 signal call has a verified wire mismatch and can hang. It exposes no socket, registry, image, mount, network, or lifecycle operation. The Rust adapter validates every outbound request and treats a helper/protocol version mismatch as a compatibility error.

The backend contract covers:

- Capability and version probing.
- Image resolution and pulling.
- Container creation, start, stop, inspection, and deletion.
- Command execution with TTY, resize, TTY SIGINT, typed rejection of unsupported signals, environment, working directory, and exact exit status for every process that starts.
- Host bind mounts and named volumes.
- CPU, memory, disk, process, network, and published-port policy.
- Logs and runtime events.

The supported Apple CLI versions form an explicit tested range. Unsupported versions produce an actionable compatibility error. Fixtures of structured output protect the adapter from schema drift.

### Sandbox topology

`gascan up ~/code` creates one named sandbox for the canonical code-root path. It consists of one long-lived OCI workspace container in the dedicated lightweight VM provided by Apple's runtime. `/workspace` maps read/write to the selected code root. All repositories beneath that root share the container.

The sandbox ID derives from a user-facing name plus a collision-resistant digest of the canonical path. Canonicalization occurs before policy checks so symlinks cannot expand mount access unexpectedly.

## Lifecycle and Commands

### `gascan up <code-root>`

`up` starts the daemon, canonicalizes the root, loads `<code-root>/gascan.toml` when present, validates policy, derives the sandbox ID, checks runtime capabilities, resolves the image digest, creates persistent volumes, starts the workspace, provisions declared tools, runs setup, and performs a health check.

The operation is idempotent. Repeating it reconciles the existing sandbox rather than creating another one.

### `gascan shell [name]`

Opens an interactive login shell in `/workspace`. It starts an existing stopped sandbox but does not create an absent sandbox.

### `gascan apply [name]`

Applies changed tool declarations or a changed setup script to an existing sandbox, then records the resolved versions and setup digest. `up` performs this provisioning during initial creation. Later `up` calls report unapplied changes but do not silently execute newly checked-in workspace code.

### `gascan run [name] -- <command>`

Executes a command in `/workspace`, preserving terminal behavior, the pinned Apple 1.1.0 signal support matrix, and the exact exit code for every process that starts. A missing executable is a typed Apple start error, not a synthetic exit 127. It starts an existing stopped sandbox but does not create an absent sandbox. Only terminal and locale variables (`TERM`, `COLORTERM`, `LANG`, and `LC_*`) cross from the host by default. The guest supplies its own `PATH`, home, and tool environment; arbitrary host environment inheritance is forbidden.

### `gascan down [name]`

Gracefully stops the workspace while retaining metadata, its writable container state, and named volumes.

### `gascan destroy [name]`

Removes the workspace, metadata, and persistent volumes after interactive confirmation. A noninteractive caller must pass an explicit confirmation flag.

### Inspection commands

`list`, `status`, and `logs` expose lifecycle state and diagnostics. `doctor` checks platform requirements, Apple service health, supported CLI version, kernel availability, disk capacity, mount accessibility, and network-isolation capabilities.

## State and Reconciliation

Each sandbox follows this state machine:

`absent -> creating -> running <-> stopped -> destroying -> absent`

Lifecycle operations write a pending operation before mutating runtime state. `up` creates resources, starts the workspace, runs setup, checks health, and only then marks it running. Failure removes resources created solely by that attempt while preserving pre-existing volumes and the host workspace.

`gascand` metadata is authoritative for desired state; Apple runtime inspection is authoritative for actual state. After a daemon or host crash, the daemon reconciles the two. It reports unknown runtime resources but never deletes them automatically.

Commands encountering a transitional state wait with progress when safe or return a typed operation-in-progress response. Client disconnection does not abandon lifecycle operations. A daemon loss during an attached session terminates that session with a clear error.

Setup failures leave a stopped, inspectable sandbox. Logs explain the failing step, and a repair/retry operation repeats provisioning without deleting persistent data.

## Security Model

### Precise guarantee

Gas Can protects the host outside the selected code root from code running in the workspace. The selected root is deliberately read/write and is inside the sandbox's trust boundary. Sandboxed commands may read, change, or delete any file beneath it.

The lightweight VM is the hard isolation boundary. Container restrictions provide defense in depth and reproducibility, not the primary host-security claim.

### Host exposure

The MVP exposes only the selected code root. It does not forward the host home directory, dotfiles, SSH agent, credential stores, environment secrets, Apple runtime socket, Docker socket, arbitrary devices, or arbitrary host paths.

The control API uses a user-owned Unix socket with restrictive filesystem permissions and peer-identity validation. It does not listen on TCP.

### Guest privilege

Commands start as a normal workspace user with passwordless `sudo` to full guest root. A manifest option may make root the default user. Guest root is not macOS root; the dedicated VM remains the security boundary.

Root can compromise guest state and all files in `/workspace`, so persistent volumes never hold host secrets and remain disposable. The runtime uses sensible default OCI capability restrictions, but Gas Can does not remove capabilities required by normal package installation, debugging, local services, or development tooling merely to claim container-level isolation.

### Network modes

Every sandbox selects one of two immutable-at-runtime modes:

- `networked`: outbound traffic is allowed. Unsolicited inbound host access is blocked unless a port is explicitly declared and published on loopback.
- `offline`: the backend must prove that the workspace lacks external network connectivity.

Network policy cannot be loosened from inside the workspace. If the installed Apple runtime cannot enforce offline mode, `up` fails before mounting the code root. Verifying this capability is an early implementation spike and release gate; Gas Can does not approximate offline mode with an unverified convention.

### Resource policy

CPU, memory, disk, and process ceilings protect host availability. Safe defaults apply when omitted and can be raised explicitly. Published ports bind to host loopback by default.

## Workspace Image

Gas Can ships a pinned ARM64 OCI image based on a mainstream Linux distribution. It contains:

- Shells, Git, GitHub CLI, editors, compilers, common build tools, and debugging/network utilities.
- Common system libraries needed by supported runtime installers.
- Browser-automation dependencies.
- A pinned `mise` executable.
- A pinned, tested Gascamp release.
- A lightweight init process for correct signal forwarding and child reaping.

Image tags resolve to digests. Sandbox metadata records the Gas Can version, image digest, Gascamp version/source, mise version, and resolved tool versions.

### Privilege and persistence layers

State separates into:

1. A replaceable base image containing the OS, common tools, and bundled Gascamp.
2. Persistent named volumes containing mise installations, language/package caches, and non-secret user configuration.
3. The read/write host code-root mount at `/workspace`.

The writable container state survives stop/start but is not guaranteed across image refresh or rebuild. Interactive system-package changes are therefore convenient but not reproducible.

### Tool management with mise

Mise manages language runtimes and developer CLIs; the OS package manager remains responsible for system libraries. `[tools]` in `gascan.toml` becomes a Gas Can-owned mise configuration containing only plain tool/version declarations. Mise installations and download caches live on named volumes.

Repository-provided mise configuration is not automatically trusted when it contains executable environment directives, templates, or hooks. Plain safe tool declarations may be consumed according to mise's trust model. `gascan run` executes with mise's resolved environment without copying host tool configuration into the guest.

### Gascamp selection

The bundled, pinned Gascamp is the default. `gascamp = "/workspace/gascamp"` selects a local checkout beneath the mounted code root for dogfooding. Gas Can labels that override as untrusted workspace code in status and diagnostics.

## Manifest

The root manifest is intentionally smaller than general OCI or Dev Container configuration:

```toml
version = 1
name = "code"
network = "networked"
user = "workspace"
gascamp = "bundled"
setup = "./.gascan/setup.sh"

[resources]
cpus = 6
memory = "12GiB"
disk = "80GiB"

[tools]
node = "lts"
python = "3.13"
go = "stable"
rust = "stable"

[ports]
web = 3000
```

The setup path must resolve beneath the selected code root. It runs after initial creation and through explicit `gascan apply`, with a content digest recorded in metadata. A changed setup script does not execute silently during unrelated `up`, `run`, or `shell` commands; `up` reports the change and directs the user to `gascan apply`.

The MVP schema does not accept arbitrary mounts, devices, secrets, OCI capabilities, or raw backend flags. Unknown keys are errors so misspelled security settings cannot be ignored.

## Local API

The daemon API is versioned from the first release. It models stable Gas Can concepts rather than Apple command shapes:

- Sandbox desired and actual state.
- Lifecycle operations with operation IDs.
- Attach sessions for shell and command execution.
- Structured progress, logs, warnings, and typed errors.
- Backend capabilities and compatibility diagnostics.

Long-running requests return operation IDs and stream events, allowing CLI disconnect/reconnect and future GUI progress views. API compatibility is negotiated during connection setup.

## Error Handling

- Invalid manifests and unsupported security capabilities fail before mounting the workspace.
- Runtime errors retain the invoked operation and sanitized diagnostics but never log secrets or the full host environment.
- Image pull and setup failures identify a retryable phase.
- Destructive cleanup operates only on resources carrying Gas Can ownership metadata and the expected sandbox identity.
- Disk exhaustion, signal interruption, daemon restart, and partially created resources are explicit test cases.
- Human CLI messages include a concise remedy; structured clients receive stable error codes and fields.

## Testing

### Unit tests

Cover manifest parsing, path canonicalization, sandbox naming, state transitions, command construction, version negotiation, rollback decisions, environment filtering, and policy rejection.

### Backend contract tests

Every backend passes the same lifecycle contract suite against a fake runtime and then its real platform. The contract defines capability probing, idempotency, attachment behavior, cleanup ownership, and error mapping.

### macOS integration tests

On Apple-silicon macOS 26 or newer, use temporary workspaces to exercise create, attach, run, stop, restart, refresh, destroy, daemon crash recovery, exact exit status, signals, TTY resize, and concurrent clients.

### Security acceptance tests

Attempt to:

- Read host paths outside the selected mount.
- Reach common host credential and socket locations.
- Add undeclared mounts or published ports.
- Reach external and host networks in offline mode.
- Accept inbound traffic without an explicitly declared port.
- Exceed resource ceilings.

These tests validate the public security guarantee rather than only implementation mechanisms.

### Image smoke matrix

Exercise representative Node, Python, Go, Rust, Java, Ruby, and Elixir projects; native compilation; Git and GitHub tooling; browser automation; passwordless sudo; mise version selection; persistent caches; and both bundled and local Gascamp startup.

### Release gate

On a clean supported Mac, install Gas Can and Apple `container`, create a sandbox from scratch, run a small multi-language repository, prove offline isolation, stop and restart it, then destroy it without leaving owned runtime resources.

## Implementation Sequence

This is an architectural sequence, not the detailed implementation plan:

1. Prove Apple CLI lifecycle, bind mounts, named volumes, structured output, TTY attachment, and enforceable offline networking.
2. Define the backend contract and implement a fake backend.
3. Build the daemon state machine, metadata store, Unix-socket API, and reconciliation.
4. Implement `up`, `apply`, `run`, `shell`, `down`, inspection, and destroy through the Apple adapter.
5. Build and validate the base workspace image, mise persistence, Gascamp selection, and setup flow.
6. Add security acceptance tests, crash recovery, compatibility fixtures, packaging, and the clean-host release gate.

## Deferred Work

- Linux/Firecracker backend.
- GUI client.
- Multiple containers or repository-specific environments beneath one root.
- Dev Container compatibility.
- Opt-in credential or SSH-agent brokering.
- Arbitrary host mounts and advanced device access.
- Remote control APIs or multi-user daemon operation.
- Intel/amd64 execution through Rosetta.

## References

- [Apple Containerization](https://github.com/apple/containerization)
- [Apple container CLI](https://github.com/apple/container)
- [Apple container how-to](https://github.com/apple/container/blob/main/docs/how-to.md)
- [mise development tools](https://mise.jdx.dev/dev-tools/)
- [mise trust model](https://mise.jdx.dev/cli/trust.html)
