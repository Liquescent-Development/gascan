# Default SSH and Developer Workstation Design

## Status

Approved in collaborative design review on 2026-07-23.

## Summary

Gas Can sandboxes are currently too sparse for immediate interactive
development. Publishing guest port 22 would not solve the problem because the
workspace image does not install or run an SSH server, does not configure
authentication, and does not manage SSH across sandbox lifecycle events.

This change establishes two related product contracts:

1. Every sandbox has secure, loopback-only SSH access by default, including
   sandboxes whose outbound network mode is `offline`.
2. Every approved workspace image contains a locked, reviewed developer
   workstation baseline with editors, coding agents, language runtimes, forge
   clients, build tools, and non-privileged diagnostics.

SSH is a managed Gas Can subsystem. It is not implemented as a setup-script
convention or as a user-declared runtime port. Default tools are immutable image
content. They are never downloaded during sandbox startup.

## Decision Record

The selected approach is an integrated developer-workstation image plus a
first-class SSH control plane.

Two alternatives were rejected:

- Setup-script SSH cannot reliably own restart, readiness, key persistence,
  port collision, or structured failure behavior.
- First-run developer-profile installation makes initial use slow and
  registry-dependent and cannot satisfy the offline-ready contract.

## Goals

- Make a new sandbox immediately usable from a terminal or VS Code Remote SSH.
- Preserve offline isolation while providing host-to-guest SSH access.
- Require only public-key authentication and fail closed on host-key changes.
- Avoid fixed-port collisions and port-allocation races.
- Keep host credentials out of sandboxes.
- Persist sandbox-local SSH identity and agent/forge login state across
  container replacement.
- Make default tool versions exact, reproducible, reviewable, and available
  without network access.
- Provide a safe container-only image-upgrade path that preserves managed
  volumes.
- Preserve polished human output and stable structured JSON errors.

## Non-goals

- Exposing SSH on a non-loopback host address.
- Enabling password, keyboard-interactive, or root SSH login.
- Importing host SSH, coding-agent, GitHub, or GitLab credentials.
- Giving diagnostics additional Linux capabilities.
- Providing arbitrary SSH daemon configuration in `gascan.toml`.
- Making mutable container-root changes durable.
- Launching VS Code or authenticating real third-party accounts in CI.
- Implementing an SSH protocol server in Gas Can.

## Architecture

### Host control plane

The Gas Can daemon owns:

- One Ed25519 SSH client identity per Gas Can installation.
- One loopback TCP listener for each running, SSH-ready sandbox.
- A byte-safe bridge from each accepted TCP connection to a non-TTY Apple exec
  session in the guest.
- SSH readiness and host-key verification.
- The generated OpenSSH configuration and known-hosts file.
- Reconstruction and cleanup of active listeners after daemon or sandbox
  lifecycle changes.

The Gas Can CLI owns:

- Interactive first-use consent before editing `~/.ssh/config`.
- `gascan ssh` process execution and exit-status propagation.
- Explicit host SSH-config install, removal, and path commands.
- Human and JSON presentation.

### Guest image

The approved workspace image installs OpenSSH server and client components.
Its default entrypoint initializes the SSH state and runs `sshd` under `tini`.
The runtime still selects the locked workspace image user; the entrypoint uses
the image's existing passwordless, guest-only `sudo` boundary to perform the
minimal privileged SSH initialization and launch.

The default command keeps `sshd` in the foreground. Explicit image commands
continue to execute directly so image build and live-test fixtures retain their
existing command behavior.

The guest stores these files beneath the existing sandbox-owned config volume:

- Persistent SSH host private and public keys.
- The single authorized Gas Can client public key.
- Generated `sshd_config`.
- Coding-agent and forge-client authentication/configuration state.

Sensitive guest files are owned by `workspace` or root as required and use
minimum permissions. Gas Can adds no independent encryption layer; at-rest
protection inherits the host filesystem and FileVault.

### Offline-safe transport

SSH does not use Apple runtime port publication. The daemon binds either:

- `127.0.0.1:0`, allowing macOS to atomically assign a free high port; or
- An explicitly requested loopback port from the manifest.

For each accepted connection, the daemon opens a cancellable, non-TTY Apple
exec session that connects inside the guest to `127.0.0.1:22`. It copies bytes
bidirectionally without text conversion. Standard error is retained only as a
bounded diagnostic stream and is never mixed into SSH protocol bytes.

This control-plane bridge does not add a guest route, network interface, or
runtime-published port. The existing `offline` network policy remains
unchanged, and user-declared ports remain forbidden for offline sandboxes.

Listeners and concurrent bridge sessions are bounded. Connections have bounded
startup/readiness timeouts. Down, destroy, daemon shutdown, and cancellation
close listeners and terminate bridge exec sessions promptly.

## Manifest Contract

SSH is enabled by default. The optional schema is:

```toml
[ssh]
enabled = true
host_port = 2222
```

Rules:

- `enabled` defaults to `true`.
- `host_port` is optional.
- An explicit port must be in `1024..=65535`.
- The daemon binds explicit ports only on `127.0.0.1`.
- If an explicit port is unavailable, `up` fails with
  `ssh_port_unavailable`; Gas Can never silently substitutes another port.
- An automatic port is selected by binding port zero, eliminating check/use
  races.
- SSH configuration is independent of `network = "offline"` or
  `network = "networked"`.
- Unknown fields are rejected.

The SSH bridge is an internal control endpoint, not a `RuntimePort`, and it is
not included in the user `[ports]` map.

## SSH Security Contract

The image generates and validates an explicit OpenSSH server configuration with
these effective properties:

```text
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin no
AllowUsers workspace
PubkeyAuthentication yes
AuthenticationMethods publickey
PermitUserEnvironment no
AllowAgentForwarding no
X11Forwarding no
PermitTunnel no
GatewayPorts no
```

SFTP and noninteractive exec are enabled. Local and dynamic forwarding required
by VS Code Remote SSH are enabled, while remote forwarding is disabled.
Forward destinations are restricted to guest loopback so the client can reach
the VS Code server without turning the SSH session into a route to other guest
networks.

SSH sessions receive the same locked `PATH`, mise data/config/cache variables,
locale, home, user, and workspace contract as Gas Can exec sessions. This must
also hold for noninteractive commands that do not source shell profiles.

The generated client entry uses:

```sshconfig
Host gascan-<sandbox-id>
    HostName 127.0.0.1
    Port <active-daemon-port>
    User workspace
    IdentityFile ~/.config/gascan/ssh/id_ed25519
    IdentitiesOnly yes
    HostKeyAlias gascan-<sandbox-id>
    UserKnownHostsFile ~/.config/gascan/ssh/known_hosts
    StrictHostKeyChecking yes
```

Gas Can never disables host-key checking. A different key for an existing
sandbox fails with `ssh_host_key_mismatch`.

The client private key never enters a sandbox. Only its public key is supplied
to the guest. A compromised sandbox can read that public key but cannot use it
to access another sandbox.

## Identity, State, and Persistence

### Host installation state

Gas Can's private application-state directory contains:

- `ssh/id_ed25519`, mode `0600`.
- `ssh/id_ed25519.pub`, mode `0644`.
- `ssh/config`, generated atomically.
- `ssh/known_hosts`, generated atomically.
- The remembered first-use SSH-config prompt decision.

Directories use mode `0700`. The daemon and CLI reject symlinks, non-regular
files, unexpected ownership, and unsafe permissions at sensitive paths.

The host client identity remains when one sandbox is destroyed because other
sandboxes use it.

### Sandbox durable state

The store schema advances from version 3 and records:

- The exact approved workspace image digest in use.
- SSH enabled/disabled resolution.
- The expected sandbox SSH host-key fingerprint.
- The host-install client public-key fingerprint authorized in the sandbox.

The active loopback port is ephemeral daemon state. It is intentionally not a
durable identity: after daemon or sandbox restart the daemon may receive a new
automatic port and atomically regenerate the managed config. The SSH alias and
host key remain stable.

Legacy records are migrated without inventing unproven image or SSH values.
The daemon backfills only values it can verify from structured runtime and
guest evidence. Otherwise status reports an apply-required upgrade rather than
guessing.

### Sandbox-local third-party credentials

Claude Code, Codex, Pi, Herdr, `gh`, and `glab` authenticate independently
inside each sandbox. Gas Can does not import corresponding host files,
environment variables, sockets, or keychains.

Supported configuration-directory environment variables are preferred. Where
a tool requires a fixed home-relative path, the image owns a symlink into the
managed Gas Can config volume. The acceptance contract is behavioral:

- Login state survives down/up and container-only image replacement.
- Cache and logs use the managed cache volume rather than credential storage.
- `gascan destroy` removes sandbox-local login state with the config volume.
- No credential material appears in the image, manifest, runtime inspection,
  operation events, or diagnostics.

## SSH Lifecycle

### First creation or start

1. Ensure and validate the host installation client key.
2. Compile the sandbox request and inject only the client public key plus
   non-secret SSH initialization metadata.
3. Create or start the sandbox.
4. The entrypoint creates missing sandbox host keys, atomically writes the
   authorized key and locked server configuration, and starts `sshd`.
5. Bind the requested or automatic loopback listener.
6. Bridge a readiness connection and verify the expected host public key.
7. Execute a real command as `workspace` and verify the SSH environment.
8. Run normal provisioning, setup, and health gates.
9. Persist the verified image and SSH resolution.
10. Atomically add the active alias and known-host entry to the managed files.
11. Only then report `up` or `apply` success.

An operation that fails before the final commit does not advertise a usable SSH
alias. Existing rollback rules preserve the primary failure and any rollback
failure.

### Down

`down`:

- Stops accepting new connections.
- Cancels active bridge sessions.
- Removes the sandbox's active entry from the generated SSH config.
- Stops the sandbox.
- Retains the host client key, sandbox host keys, credentials, and expected
  fingerprint.

### Up after down

`up` starts the sandbox, verifies the persistent host key, allocates a new
automatic port if necessary, and restores the stable alias atomically.

### Daemon restart

The daemon reconciles structured runtime and store state. For every owned,
running sandbox with a valid SSH resolution it:

- Creates a new listener.
- Verifies SSH readiness and the stored host-key fingerprint.
- Publishes the active alias.

Unverified or unhealthy sandboxes are omitted from the generated config and
reported through status/doctor evidence.

### Destroy

`destroy` removes:

- Active listeners and bridge sessions.
- The generated alias and known-host entry.
- The container, network, and all Gas Can-owned volumes.
- Sandbox host keys and third-party login state contained in the config
  volume.

The installation client key and the one-time host-config preference remain.

## Host SSH-Config Integration

On the first successful interactive, non-JSON `gascan up`, if the managed
include is absent and no decision has been remembered, the CLI asks:

```text
Enable Gas Can SSH aliases in ~/.ssh/config?
This will add: Include ~/.config/gascan/ssh/config
[Y/n]
```

On approval, Gas Can:

- Creates `~/.ssh` with mode `0700` if absent.
- Creates or updates a regular `~/.ssh/config` with mode `0600`.
- Inserts one exact include near the beginning so an earlier `Host *` block
  cannot override generated host-specific values.
- Preserves all existing bytes apart from the necessary insertion.
- Writes atomically and never creates a duplicate.
- Refuses symlink, ownership, type, or permission hazards with an actionable
  error.

Declining is remembered and suppresses future prompts. Noninteractive and JSON
commands never prompt or edit `~/.ssh/config`; they report the explicit setup
command instead.

`gascan ssh-config remove` removes only the exact Gas Can-owned include and
leaves the generated managed file intact. It does not rewrite unrelated SSH
configuration.

## CLI and API Contract

New commands:

```text
gascan ssh [--sandbox <id>] [-- <command>...]
gascan ssh-config install
gascan ssh-config remove
gascan ssh-config path
```

`gascan ssh` without a command opens an interactive workspace shell through
the system OpenSSH client. With a command, it preserves the remote command's
exit code and signal semantics. It never shells a joined argument string.

Human `status` reports:

- SSH state: disabled, starting, ready, unhealthy, or unavailable.
- Stable alias.
- Active loopback endpoint.
- Host-key and client-key fingerprints.
- Generated config path.
- Apply-required image or SSH upgrade reasons.

JSON output retains those as separate structured fields.

Stable public error codes include:

- `ssh_disabled`
- `ssh_not_ready`
- `ssh_port_unavailable`
- `ssh_host_key_mismatch`
- `ssh_bridge_failed`
- `ssh_config_unsafe`
- `ssh_config_update_failed`
- `ssh_client_unavailable`
- `image_upgrade_required`
- `image_replacement_failed`

Failures include the phase, sandbox identity, bounded diagnostics, retryability,
and exact recovery action where applicable. Raw private keys, tokens, full
third-party config files, and unbounded SSH streams never enter errors or
operation events.

## Default Workstation Image Contract

The approved image guarantees these commands before project provisioning or
network access.

### Editors

- `vim`
- `nvim`
- `emacs` from the non-GUI Emacs build
- `pico`, provided by the reviewed Nano package/compatibility command

### Coding agents

- `claude`
- `codex`
- `pi`
- `herdr`

### Core development and forges

- `go`
- `rustc`
- `cargo`
- `git`
- `gh`
- `glab`

The existing locked Node, Python, Ruby, Java, Erlang, and Elixir runtimes
remain.

### Diagnostics and terminal utilities

- Networking: `ip`, `ss`, `ping`, `ifconfig`, `netstat`, `dig`, `nslookup`,
  `traceroute`, and `nc`.
- Transfer and inspection: `curl`, `wget`, `rsync`, `lsof`, `file`, and `jq`.
- Process and filesystem: `ps`, `top`, `pstree`, `tree`, and `less`.
- Interactive development: `rg`, `fd`, `fzf`, and `tmux`.
- Existing compilers, headers, archive utilities, Chromium, and Gascamp.

Commands such as `tcpdump` that imply unavailable capture capabilities are not
part of the guarantee. Installing diagnostics grants no additional runtime
capabilities.

### Version and provenance policy

- `versions.toml` contains update intent.
- `versions.lock` contains every exact resolved version, source, digest, and
  platform required to reproduce the image.
- Docker builds and sandbox startup never resolve `latest`.
- Ubuntu packages come from the existing dated snapshot and reviewed bundle.
- Tools unavailable or unsuitable in Ubuntu come from an official upstream
  artifact or official package registry with exact integrity evidence.
- The image gate invokes important commands and compares exact reported
  versions with the lock.
- No unverified installer is piped directly into a shell.
- The approved image remains a digest-qualified GHCR reference.

Default tools live in the immutable image filesystem. Locked mise-managed
defaults live beneath `/opt/gascan`; reviewed Ubuntu packages live in their
normal system paths. The mutable per-sandbox mise shims remain earlier in
`PATH`, allowing explicit `[tools]` entries to override a default version
without mutating the image.

## Image Upgrade Contract

Sandbox records persist the exact image digest. When the current approved image
differs:

- `status` reports `apply_required` with reason `image_changed`.
- `up` does not mutate the sandbox and directs the user to `gascan apply`.
- `apply` pulls and verifies the new digest before stopping the old container.
- `apply` replaces only the container and preserves the workspace bind mount,
  managed tools/cache/config volumes, SSH host identity, sandbox-local
  credentials, sandbox ID, and SSH alias.
- Setup, SSH readiness, and health run against the replacement.
- The new image resolution is committed only after all gates pass.

Mutable container-root changes are intentionally discarded. The workspace,
manifest, setup script, and managed volumes are the durable contract.

Replacement is ownership checked and failure atomic where the runtime permits:

1. Retain the exact previous image resolution.
2. Pull and validate the replacement before mutation.
3. Stop and remove only the owned old container, retaining volumes and network.
4. Create the replacement with the same durable resources.
5. On failure, clean exact partial replacement evidence.
6. Attempt to recreate the previous locked image with the same resources.
7. Report the primary and rollback outcomes separately.

No container, temporary alias, listener, network, or volume may leak after a
successful rollback. If rollback also fails, durable evidence supports explicit
recovery without guessing or deleting unproven resources.

This container-only replacement is the normal way existing sandboxes receive
future pinned editor and coding-agent updates.

## Error Handling

- SSH initialization and readiness are required health gates.
- `up` and `apply` never report success while SSH is unhealthy when SSH is
  enabled.
- Guest initialization, bridge, and handshake output uses bounded stdout and
  stderr handling with terminal exit/signal evidence.
- Host configuration writes are prepare/validate/atomic-rename operations.
- Listener/config publication occurs only after the sandbox is fully healthy.
- Cleanup operates only on exact owned resources and exact generated config
  records.
- A host-key mismatch never auto-accepts or overwrites trusted state.
- Explicit-port collision never falls back silently.
- Image replacement preserves the primary failure even when rollback fails.

## Verification Strategy

### Unit and component tests

- Manifest defaults, opt-out, high-port boundaries, unknown fields, and offline
  compatibility.
- Policy separation between internal SSH control and user runtime ports.
- Store migration and exact round trips for image and SSH resolution.
- Host key/config path ownership, permissions, symlink rejection, idempotent
  include insertion/removal, and atomic writes.
- Generated `sshd_config` security properties and environment contract.
- Binary bridge integrity, simultaneous directions, backpressure, half-close,
  cancellation, bounded diagnostics, and concurrent sessions.
- Listener reconstruction and atomic config regeneration.
- Stable API/client/human/JSON success and failure contracts.
- Image replacement state machine and failure injection at every mutation and
  rollback boundary.

### Image gates

- Every guaranteed command exists.
- Every locked tool reports the exact expected version.
- `sshd -T` matches the security contract.
- Agent/forge configuration and cache paths target the correct managed volume.
- No credential or private host input exists in image layers.
- Package set, source provenance, connected/offline builder isolation, and
  digest approval remain exact.

### Live Apple tests

- SSH works for both offline and networked sandboxes.
- A real OpenSSH client performs interactive shell, noninteractive exec, SFTP,
  and VS Code-style local/dynamic forwarding.
- Offline isolation remains proven before, during, and after SSH sessions.
- The host listener is reachable only through IPv4 loopback.
- Multiple sandboxes and concurrent sessions remain isolated.
- Daemon restart, down/up, explicit port collision, host-key mismatch, and
  destroy have exact state and cleanup.
- Container-only replacement preserves volumes, SSH identity, credentials, and
  alias while updating the image.
- Injected replacement failures restore the previous image or retain exact
  actionable recovery evidence.
- Every coding agent and guaranteed tool passes a credential-free version/help
  smoke test.

CI never authenticates a real third-party account or launches an interactive
coding-agent session.

## Documentation and Release

README and release documentation cover:

- The guaranteed workstation command set and exact-version discovery.
- SSH defaults, opt-out, explicit host port, aliases, and VS Code setup.
- First-use SSH config consent and explicit install/removal commands.
- Sandbox-local authentication persistence and destroy semantics.
- Image upgrade and rollback behavior.
- Security and troubleshooting.

`gascan doctor` adds facts for:

- System OpenSSH client/key-generation availability.
- Host SSH state directory safety.
- Daemon bridge capability.
- Guest SSH/workstation image contract.
- Exact running image resolution.

Release ordering is mandatory:

1. Resolve and lock all image inputs.
2. Build and run image gates.
3. Run live SSH, isolation, upgrade, and cleanup acceptance.
4. Publish the approved digest-qualified image.
5. Commit the approved image reference and release evidence.
6. Version and release the matching CLI/daemon.

Release evidence reports image-size and build-time changes without imposing an
arbitrary size ceiling for this usability-focused workstation baseline.

## Acceptance Criteria

- A default offline sandbox reaches `up` success with SSH ready and no outbound
  network path.
- `gascan ssh` and a generated OpenSSH alias work without per-sandbox manual
  key setup.
- VS Code Remote SSH prerequisites pass through the Gas Can bridge.
- The host config include is offered once, explicit, idempotent, and safe.
- All guaranteed tools are available at exact locked versions without startup
  downloads.
- Host credentials are never imported.
- Sandbox-local logins survive down/up and image replacement and disappear on
  destroy.
- Image changes produce `apply_required`, and apply performs a volume-preserving
  container replacement.
- Host-key changes, port conflicts, SSH failures, and replacement failures
  fail closed with stable structured errors and actionable human output.
- Live failure injection leaves no unowned or test-owned resources, listeners,
  aliases, or processes.
