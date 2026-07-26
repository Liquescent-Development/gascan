# `gascan.toml` reference

Gas Can reads `gascan.toml` from the canonical project root. The schema is
closed: unknown keys and sections are rejected before the project is mounted.

If the file is absent, Gas Can uses version 1 defaults: offline networking,
the non-root `workspace` user, bundled Gascamp, default resources and managed
volume sizes, no setup script, tools, or application ports, and no SSH.

## Complete example

```toml
version = 1
name = "my-project"
network = "networked"
user = "workspace"
gascamp = "bundled"
setup = ".gascan/setup.sh"

[resources]
cpus = 4
memory = "8GiB"

[storage]
tools = "10GiB"
cache = "10GiB"
config = "1GiB"

[tools]
node = "lts"
python = "3.14.6"

[ports]
web = 3000

[ssh]
enabled = true
host_port = 2222
```

Every field except `version` is optional. This example shows explicit values;
omitting a field uses the defaults described below.

## Top-level keys

### `version`

Required integer. The only supported value is `1`.

### `name`

Optional string. Defaults to the project directory name. Gas Can slugifies the
name and combines it with a digest of the canonical project root to form the
sandbox ID. The same project and name therefore produce the same ID.

### `network`

Optional string:

- `"offline"` is the default. Gas Can requires the runtime to prove offline
  isolation and rejects application ports and SSH.
- `"networked"` permits outbound access, application ports, and native SSH.
  Use it when setup or mise must download content.

Gas Can never silently changes the selected network mode to satisfy another
setting.

### `user`

Optional string:

- `"workspace"` is the default non-root guest account. It has passwordless,
  guest-only `sudo`.
- `"root"` runs guest commands as root.

### `gascamp`

Optional string:

- `"bundled"` is the default pinned and tested copy from the workspace image.
- A path under `/workspace/gascamp` selects a project checkout for Gascamp
  development. Paths outside that directory and paths containing `..` are
  rejected.

### `setup`

Optional project-relative path to a setup script. Absolute paths, `..`, and
symbolic-link components are rejected. The target must be a regular, readable
file when it runs.

Gas Can records the script's SHA-256. A changed script causes `gascan up` to
report `apply_required`; run `gascan apply` to execute the changed setup.

## `[resources]`

Optional runtime resource policy:

| Key | Default | Maximum | Format |
| --- | --- | --- | --- |
| `cpus` | `4` | `16` | Positive integer |
| `memory` | `"8GiB"` | `"64GiB"` | Binary size string |

`disk` is parsed for a clear error but is rejected because Apple Container
cannot enforce a container root-filesystem ceiling. Use `[storage]` for the
writable managed volumes.

Binary sizes are a positive integer followed by `KiB`, `MiB`, `GiB`, or
`TiB`. Decimal units such as `GB`, bare numbers, and zero are invalid.

## `[storage]`

Optional independent capacities for Gas Can-managed volumes:

| Key | Default | Guest mount |
| --- | --- | --- |
| `tools` | `"10GiB"` | `/home/workspace/.local/share/mise` |
| `cache` | `"10GiB"` | `/home/workspace/.cache` |
| `config` | `"1GiB"` | `/home/workspace/.config/gascan` |

Each value uses the binary-size format above and may not exceed `512GiB`.
Omitted values retain their defaults independently.

Apple volumes cannot be resized in place. Changing an effective storage size
for an existing sandbox is rejected without modifying its volumes. Recreate
the sandbox explicitly:

```sh
gascan destroy --yes
gascan up /path/to/project
```

Destroying removes all managed-volume contents.

## `[tools]`

An optional map of mise tool names to versions:

```toml
[tools]
node = "lts"
python = "3.14.6"
```

Gas Can writes these entries to a managed mise configuration. Versions absent
from the workspace image require `network = "networked"` so mise can download
them. Installed tools persist in the managed tools volume.

Changing the desired tool map causes `gascan up` to report
`apply_required`; run `gascan apply` to reconcile it.

## `[ports]`

An optional map of labels to application port numbers:

```toml
[ports]
web = 3000
metrics = 9090
```

Each port is published as `127.0.0.1:<port>:<port>`. Gas Can does not support
host-to-guest remapping or non-loopback bindings.

- Port `0` and duplicate port numbers are rejected.
- Ports are rejected for offline sandboxes.
- A port may not collide with an explicit SSH host port.
- Undeclared guest ports are not reachable from the host.

## `[ssh]`

Optional native OpenSSH access:

```toml
[ssh]
enabled = true
host_port = 2222
```

### Defaults and validation

SSH defaults from the resolved network mode:

| Network | `[ssh]` absent | Result |
| --- | --- | --- |
| `networked` | Yes | SSH enabled with an automatic host port |
| `offline` | Yes | SSH disabled |

For a networked sandbox, `enabled = false` disables SSH. Explicitly setting
`enabled = true` while offline is rejected with guidance to either use
`network = "networked"` or disable SSH.

`host_port` is optional and valid only when SSH is enabled for a networked
sandbox. Its range is `1024..=65535`. An explicit port is used exactly; if it
is occupied, creation fails with `ssh_port_unavailable`. Gas Can never chooses
a different port in that case.

When `host_port` is omitted, Gas Can selects an available IPv4 loopback port
and performs bounded collision retries during native container creation.

### Exposure and access

Enabled SSH adds exactly one internal publication:

```text
127.0.0.1:<host-port>:22
```

It is not exposed on wildcard, LAN, or IPv6 addresses. Offline sandboxes
receive no SSH publication or authorized key.

The guest SSH service listens on the sandbox's IPv4 interfaces so Apple
Container can deliver the native publication. Each sandbox has a dedicated
Apple network; containers on the default network or another sandbox network
cannot reach that listener unless explicitly attached to the sandbox network.

After the guest host key and strict readiness check succeed, Gas Can publishes
the stable alias:

```text
gascan-<sandbox-id>
```

Connect directly or run a remote command:

```sh
gascan ssh
gascan ssh -- git status
```

Remote command arguments are passed directly to `/usr/bin/ssh` without shell
construction.

To make managed aliases visible to OpenSSH-based tools such as VS Code Remote
SSH:

```sh
gascan ssh-config install
gascan ssh-config path
gascan ssh-config remove
```

The first successful interactive human `gascan up` offers the same install.
JSON, CI, and other noninteractive runs do not prompt or modify
`~/.ssh/config`. Install and removal manage only Gas Can's exact include
block.

### Identity and lifecycle

Gas Can maintains one installation-wide Ed25519 client identity beneath
`~/.config/gascan/ssh`. Its private key stays on the host. Each sandbox keeps
its own persistent Ed25519 host key in the managed config volume.

- `down` removes the active alias before stopping the sandbox, while retaining
  the identities and recorded fingerprints.
- `up` verifies the retained host fingerprint through the native mapping
  before republishing the alias.
- `apply` preserves host and client fingerprints. An automatic host port may
  change, but the alias remains stable.
- `destroy` removes the alias, active sandbox trust, sandbox host key,
  sandbox-local credentials, and all managed volumes. Retired immutable
  known-host generations can remain unreferenced for concurrent-reader
  consistency; the current managed config does not load them. Destroy retains
  the installation client identity and leaves the optional user SSH include
  in place.

A fingerprint mismatch fails closed with `ssh_host_key_mismatch`. Gas Can does
not publish or use the unverified alias. Inspect the sandbox and run
`gascan doctor` before retrying; destroy and recreate only when intentionally
resetting both trust and sandbox state.

### Troubleshooting

`gascan status` reports whether SSH is disabled, starting, ready, unhealthy,
or unavailable. A ready sandbox includes its loopback endpoint and stable
alias.

`gascan doctor` summarizes managed SSH readiness. Use JSON for exact details
and remedies:

```sh
gascan doctor --json | jq '.checks[] | select(.id | startswith("ssh."))'
```

The SSH facts are:

- `ssh.client` — `/usr/bin/ssh` is a usable system OpenSSH client.
- `ssh.identity` — the managed client identity is absent or safely valid.
- `ssh.config` — the generated config is absent or safely accepted by
  OpenSSH.
- `ssh.native_publish` — Apple Container supports native IPv4 loopback
  publication.

## Deliberately unsupported settings

The manifest cannot request arbitrary bind mounts, host home or credential
imports, devices, OCI capabilities, secrets, host environment passthrough, or
raw runtime flags. The canonical project root is the only host bind mount and
is mounted at `/workspace`.
