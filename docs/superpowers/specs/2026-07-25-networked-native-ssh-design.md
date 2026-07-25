# Networked Native SSH Design

## Status

Approved in collaborative design review on 2026-07-25.

This document supersedes the SSH transport, offline-SSH, lifecycle, CLI, and
acceptance portions of
`docs/superpowers/specs/2026-07-23-default-ssh-workstation-design.md` and the
corresponding unfinished work in
`docs/superpowers/plans/2026-07-23-managed-ssh-access.md`.

The completed developer-workstation image contract remains unchanged.

## Goal

Provide polished, secure SSH and VS Code Remote SSH access to networked Gas Can
sandboxes without implementing a custom TCP relay. Apple Container owns the
host-to-guest connection through a native loopback-only published port.

SSH is intentionally unavailable to offline sandboxes in this release.

## Manifest Contract

The existing optional manifest section remains:

```toml
[ssh]
enabled = true
host_port = 2222
```

Resolution rules:

- A networked sandbox enables SSH by default when `[ssh]` is absent.
- An offline sandbox disables SSH when `[ssh]` is absent.
- An offline sandbox that explicitly sets `ssh.enabled = true` is rejected
  with an actionable validation error.
- `host_port` is optional and valid only when SSH is enabled for a networked
  sandbox.
- An explicit port must be in `1024..=65535`.
- `ssh.enabled = false` with `host_port` is invalid.
- Unknown fields are rejected.

The rejection message for explicit offline SSH directs the user to either set
`network = "networked"` or disable SSH. Gas Can never silently changes the
network mode.

## Native Transport

For an SSH-enabled networked sandbox, Gas Can adds one internal runtime port:

```text
127.0.0.1:<host-port>:22
```

The host address is always IPv4 loopback. The SSH port is not a user
application port and is never exposed on a wildcard, LAN, or IPv6 address.

An explicit `host_port` is used exactly. If unavailable, creation fails with
`ssh_port_unavailable`; Gas Can never substitutes another port.

When no port is specified, Gas Can asks the operating system for a loopback
port, releases the temporary reservation immediately before container
creation, and retries native creation on a detected collision. Retries are
bounded. A successful native mapping becomes the sandbox's active SSH
endpoint.

No `SshBridge`, exec-to-`nc` session, byte copying, listener registry, or
daemon-owned connection task exists.

## Guest Contract

The approved workspace image provides the locked-down OpenSSH service already
specified and implemented:

- `sshd` listens only on guest `127.0.0.1:22`.
- Only the generated Gas Can Ed25519 public key is accepted.
- Password, keyboard-interactive, root, agent, remote, tunnel, X11, user
  environment, and gateway forwarding are disabled.
- Local and dynamic forwarding are allowed only to guest loopback.
- Host keys, authorized key, and generated daemon configuration live beneath
  the managed config volume.
- A valid persistent Ed25519 host key is never regenerated.
- Explicit image commands and the SSH-disabled keepalive path remain intact.

Offline sandboxes receive `GASCAN_SSH_ENABLED=0` and no SSH authorized key or
native port publication.

## Host Identity and OpenSSH Files

Gas Can keeps one install-wide Ed25519 client identity in its private
application configuration directory. The private key remains host-only and
uses mode `0600`. Only the public key and fingerprint enter sandbox policy or
status.

Generated OpenSSH state provides stable aliases:

```text
gascan-<sandbox-id>
```

Each active stanza fixes:

- `HostName 127.0.0.1`
- the active native host port
- `User workspace`
- the absolute managed identity path
- `IdentitiesOnly yes`
- the stable host-key alias
- the managed known-hosts generation
- `StrictHostKeyChecking yes`
- `ForwardAgent no`

Reusable stanzas permit VS Code local and dynamic forwarding. Readiness adds
`BatchMode=yes` and `ClearAllForwardings=yes`.

Host identity and generated-file handling remains fail-closed against unsafe
ownership, permissions, symlinks, hard links, non-regular files, malformed
keys, OpenSSH path expansion, and interrupted publication. Known-host data is
written as an immutable generation; the config rename is the publication
commit point, so readers observe either the complete previous generation or
the complete new generation.

The CLI safely offers one-time installation of the exact managed include in
`~/.ssh/config`. JSON and noninteractive execution never prompt or mutate the
user's SSH configuration.

## Lifecycle

### Up and first creation

1. Resolve the manifest and reject explicit offline SSH.
2. Ensure and validate the host client identity.
3. Select the explicit or automatic native loopback port.
4. Create the sandbox with the public key, SSH enablement metadata, and native
   loopback publication.
5. Start the guest.
6. Read and validate its persistent Ed25519 host public key through fixed
   runtime-exec arguments.
7. Prepare the immutable known-host generation without publishing its alias.
8. Run a strict OpenSSH readiness command through the native mapping using
   explicit discrete endpoint, user, identity, host-key alias, known-hosts,
   and strict-checking arguments.
9. Persist the verified host/client fingerprints and atomically publish the
   alias by committing the generated config.
10. Report operation success.

No alias is advertised before readiness succeeds. Existing lifecycle rollback
rules preserve the primary failure and clean partial native resources.

### Down and up

Down removes the active alias before stopping the container. It retains the
client identity, sandbox host key, trust fingerprints, and sandbox-local
credentials.

Up reads the container's native mapping, verifies the persistent host key,
performs readiness, and republishes the stable alias.

### Apply

Container-only image replacement preserves managed volumes and SSH host
identity. An automatic native host port may change. Apply verifies the
existing host fingerprint through the new mapping before publishing the
updated alias.

### Destroy

Destroy removes the active alias and sandbox trust generation, then deletes
the container and owned volumes. This removes the sandbox host key and
sandbox-local credentials. The installation client identity remains.

### Daemon restart

Reconciliation inspects owned running containers and their native loopback
port mappings. It validates stored fingerprints, performs readiness, and
regenerates active aliases. Stopped, disabled, offline, unverified, or
unhealthy sandboxes remain unpublished. One broken sandbox cannot prevent the
daemon or other aliases from starting.

The daemon reconstructs no listeners because Apple Container owns them.

## CLI and Status

The public experience retains:

```text
gascan ssh [--sandbox <id>] [-- <command>...]
gascan ssh-config install
gascan ssh-config remove
gascan ssh-config path
```

`gascan ssh` invokes system OpenSSH with discrete arguments and inherited
stdio. Remote command arguments are appended unchanged and never joined
through a shell. OpenSSH exit and signal status is propagated.

Human and JSON status report whether SSH is disabled, starting, ready,
unhealthy, or unavailable, plus the active loopback endpoint, stable alias,
fingerprints, and generated config path. Offline status reports SSH disabled
by network policy.

Stable errors include:

- `ssh_requires_network`
- `ssh_disabled`
- `ssh_not_ready`
- `ssh_port_unavailable`
- `ssh_host_key_mismatch`
- `ssh_config_unsafe`
- `ssh_config_update_failed`
- `ssh_client_unavailable`

The obsolete `ssh_bridge_failed` error is removed.

## Verification

Static and component verification covers:

- network-dependent SSH defaults and explicit offline rejection;
- native runtime translation fixed to IPv4 loopback and guest port 22;
- bounded automatic-port retry and exact explicit-port failure;
- guest OpenSSH policy, persistence, and config-volume protections;
- host identity and generated-file filesystem attacks;
- generation-consistent config and known-host publication;
- lifecycle behavior for create/up/down/apply/destroy/restart and rollback;
- stable status, CLI arguments, exit propagation, aliases, and include
  management.

One release-blocking live Apple acceptance proves:

1. A networked sandbox publishes SSH only on host IPv4 loopback.
2. Public-key login and exact remote command arguments work.
3. VS Code-style local forwarding reaches a guest-loopback service.
4. Remote and agent forwarding fail.
5. Host and client fingerprints survive down/up and image replacement.
6. Daemon restart regenerates a working alias from native runtime state.
7. Explicit-port collisions are actionable.
8. Destroy removes sandbox SSH state and leaves no owned resources.
9. Offline defaults publish no SSH port, and explicit offline SSH is rejected.

The custom byte-stream, backpressure, half-close, connection-cancellation,
offline-SSH, bridge-listener, and bridge-isolation test matrices are deleted.

## Remaining Delivery Work

Implementation is reorganized into five bounded packages:

1. Revise manifest and policy for networked-only native SSH.
2. Complete host identity and generation-consistent OpenSSH file handling.
3. Integrate native SSH ports with lifecycle and reconciliation.
4. Add status, CLI, aliases, and safe include management.
5. Rebuild and approve the image, run live acceptance, merge, version, tag,
   and release.

Release ordering remains image first, then compatible daemon/CLI code, then
the normal signed product release.
