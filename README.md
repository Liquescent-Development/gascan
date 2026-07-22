# Gas Can

Gas Can is a secure, local sandbox for agentic coding on Apple-silicon Macs.
It runs each selected project inside a long-lived Linux container backed by
Apple's `container` runtime and the pinned Gas Can polyglot workspace image.

Only the canonical project root is mounted from the host. The guest defaults
to the non-root `workspace` user with passwordless guest-only `sudo`, and the
sandbox is fail-closed offline unless the project opts into networking.

## Requirements

- Apple-silicon Mac running macOS 26 or newer.
- Apple `container` 1.1.0, installed and started first. Gas Can does not
  bundle it.

## Install

Gas Can is distributed as a signed, notarized macOS package. Install it with
Homebrew:

```sh
brew tap liquescent-development/tap
brew trust liquescent-development/tap
brew install --cask gascan
```

Homebrew 6 refuses to load casks from a third-party tap until you trust it, so
the `brew trust` step is required, not advisory. Without it `brew install`
stops with `Refusing to load cask ... from untrusted tap`. Trust is recorded
per user in `~/.config/homebrew/trust.json`; nothing the tap publishes can
waive it. To trust only this cask rather than the whole tap:

```sh
brew trust --cask liquescent-development/tap/gascan
```

Or download `gascan-<version>-macos-arm64.pkg` from the
[latest release](https://github.com/Liquescent-Development/gascan/releases/latest)
and open it. Each release also publishes a `.sha256` checksum and the
`build-manifest.json`, which records the source revision and a SHA-256 for
every installed executable.

Then confirm the host and runtime satisfy the security contract. `doctor`
reports one fact per capability — architecture, macOS version, runtime service,
storage, bind mounts, named volumes, TTY, signals, loopback publishing,
resource limits, and offline isolation:

```sh
gascan doctor --json | jq
```

### Building from source

Building is for contributors; installing a release does not require it.
Packaging refuses to build from an untrusted source revision: the checkout must
be either a trusted signed commit or the exact signed release tag. Build from
the tag rather than from `main`, which moves ahead between releases:

```sh
git checkout v0.1.4
package=$(./packaging/macos/package.sh)
GASCAN_EXPECTED_SOURCE_REVISION=$(git rev-parse HEAD) \
GASCAN_EXPECTED_VERSION=0.1.4 \
  ./packaging/macos/install.sh "$package"
```

Skipping the checkout leaves `HEAD` on a commit the release tag does not
attest, and `package.sh` exits 65 with `release source HEAD needs a trusted
commit signature or exact signed v0.1.4 tag`.

Verification runs through Git's own trust policy, so the tag's signing key must
be one you have chosen to trust. Releases are signed with this SSH key:

```
richard@liquescent.dev ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHyTKmfAwcJcdfKXmj2h3mwfgPaelE6gSMrquAcPmW09
```

Its fingerprint is `SHA256:3NWoJ1nmsLHxd8hAG/BnyriJJpIFXHaW3RtuPYANKc4`. Add it
to a Git allowed-signers file and point Git at it:

```sh
mkdir -p ~/.config/git
printf 'richard@liquescent.dev ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHyTKmfAwcJcdfKXmj2h3mwfgPaelE6gSMrquAcPmW09\n' \
  >> ~/.config/git/allowed_signers
git config --global gpg.ssh.allowedSignersFile ~/.config/git/allowed_signers
git verify-tag v0.1.4
```

## Quickstart

Copy [`packaging/macos/default-gascan.toml`](packaging/macos/default-gascan.toml)
to `gascan.toml` in the project root and set `name` to your project:

```toml
version = 1
name = "my-project"
network = "networked"
user = "workspace"
gascamp = "bundled"

[resources]
cpus = 2
memory = "4GiB"

[tools]
node = "24.18.0"
python = "3.14.6"
```

Create the sandbox, use it, then stop it:

```sh
gascan up /path/to/project     # create and start; mounts the project at /workspace
gascan run -- node --version   # run one command in the sandbox
gascan shell                   # interactive shell at /workspace
gascan apply /path/to/project  # re-apply gascan.toml after editing it
gascan down                    # stop the sandbox, keep its state
gascan destroy --yes           # remove the sandbox and its managed volumes
```

Commands other than `up` resolve the sandbox implicitly when exactly one
exists. With more than one, pass `--sandbox <id>`; `gascan list` prints the
ids. A sandbox id is the slugified `name` plus a short digest of the canonical
project root, so the same project always maps to the same sandbox.

### Commands

| Command | Purpose |
| --- | --- |
| `gascan up <project-root> [--json]` | Create and start the sandbox for a project root. |
| `gascan apply [project-root] [--json]` | Reconcile the running sandbox with the current `gascan.toml`. |
| `gascan run -- <argv...>` | Run a single command in the sandbox. |
| `gascan shell [-- <argv...>]` | Open an interactive shell. |
| `gascan status [--json]` | Show desired and actual state for one sandbox. |
| `gascan list [--json]` | List all sandboxes. |
| `gascan logs [--follow] [--since-millis <n>]` | Stream sandbox logs. |
| `gascan down [--json]` | Stop the sandbox without deleting state. |
| `gascan destroy --yes [--json]` | Delete the sandbox and its Gas Can-owned volumes. |
| `gascan doctor [--json]` | Report host, runtime, and capability facts. |

`--sandbox <id>` is accepted on every command.

## Configuring `gascan.toml`

`gascan.toml` lives in the project root and is read from the canonical root
only. If the file is absent, the project gets the built-in defaults: offline
networking, the `workspace` user, bundled Gascamp, no extra tools, no
published ports, and default resources.

The schema is deliberately small. **Unknown keys are rejected**, so a
misspelled security setting fails loudly instead of being silently ignored.
Invalid manifests fail before the workspace is ever mounted.

### Full schema

```toml
version = 1                     # required; must be 1
name = "code"                   # optional; defaults to the project directory name
network = "networked"           # "networked" | "offline" (default: "offline")
user = "workspace"              # "workspace" | "root" (default: "workspace")
gascamp = "bundled"             # "bundled" | a path under /workspace/gascamp
setup = ".gascan/setup.sh"      # optional; path relative to the project root

[resources]
cpus = 6                        # optional; default 4, maximum 16
memory = "12GiB"                # optional; default 8GiB, maximum 64GiB

[tools]                         # mise tool name = version
node = "lts"
python = "3.13"

[ports]                         # label = port, published on loopback only
web = 3000
```

### `version`

Must be `1`. Any other value is rejected as an unsupported manifest version.

### `name`

Names the sandbox. Defaults to the project directory's name. It is slugified
and combined with a digest of the canonical project root to form the sandbox
id, so renaming a project changes its sandbox id.

### `network`

- `offline` (default) — fail-closed isolation. Gas Can refuses to start unless
  the runtime can *prove* offline isolation, and an offline sandbox may not
  publish ports.
- `networked` — outbound network access. Required for anything that downloads,
  including installing tool versions that are not already in the image.

### `user`

- `workspace` (default) — non-root guest user with passwordless, guest-only
  `sudo`.
- `root` — runs as root in the guest. Prefer `workspace`; `sudo` already covers
  guest-side privilege needs.

### `gascamp`

- `bundled` (default) — the pinned, tested Gascamp shipped in the image. This is
  the only source Gas Can treats as trusted.
- A path beneath `/workspace/gascamp` — uses a checkout inside the mounted
  project, for dogfooding Gascamp itself. Status and diagnostics label this as
  untrusted workspace code. Paths outside `/workspace/gascamp`, and paths
  containing `..`, are rejected.

### `setup`

An optional project-relative path to a setup script that runs after initial
creation and on explicit `gascan apply`.

Constraints, all enforced before execution:

- Must stay beneath the project root. Absolute paths, `..`, and root
  components are rejected.
- No component may be a symbolic link.
- Must be a regular, readable file.

Gas Can records the script's SHA-256 and re-runs the script only when that
digest changes. A changed setup script never runs silently: `up` on an
existing sandbox reports `apply_required` with reason `setup_changed` and
leaves the sandbox as-is until you run `gascan apply`. The digest is
re-verified inside the guest immediately before execution, so a script edited
mid-operation fails rather than running.

### `[resources]`

| Key | Default | Maximum | Notes |
| --- | --- | --- | --- |
| `cpus` | 4 | 16 | Integer; must be greater than zero. |
| `memory` | `8GiB` | `64GiB` | String with binary units. |
| `disk` | — | — | Parsed, but **rejected** on the supported Apple runtime, which cannot enforce a sandbox disk ceiling. |

Sizes must be a positive integer plus one of `KiB`, `MiB`, `GiB`, or `TiB`.
Decimal units (`GB`), bare numbers, and zero are all rejected. Unknown
process-limit requests are rejected as well.

### `[tools]`

A map of mise tool name to version, applied by mise inside the guest. The
declaration is written to a Gas Can-owned mise config; repository-provided mise
configuration containing executable environment directives, templates, or hooks
is not automatically trusted.

The image preinstalls these versions, which resolve without any download:

| Tool | Version |
| --- | --- |
| `elixir` | 1.20.2-otp-29 |
| `erlang` | 29.0.3 |
| `go` | 1.26.5 |
| `java` | 25.0.2 |
| `node` | 24.18.0 |
| `python` | 3.14.6 |
| `ruby` | 3.4.10 |
| `rust` | 1.97.0 |

Requesting any other tool or version makes mise download it, which requires
`network = "networked"`. Installed tools persist in a per-sandbox volume, so
they survive `gascan down` and are removed by `gascan destroy`.

Gas Can hashes the desired tool set and reinstalls only when that hash
changes. As with `setup`, editing `[tools]` and running `up` on an existing
sandbox reports `apply_required` with reason `tools_changed`; run
`gascan apply` to reconcile.

### `[ports]`

A map of label to port number. Each declared port is published on
`127.0.0.1` only, with the same host and guest port number — there is no
host-to-guest port remapping and no non-loopback binding.

- Port `0` is rejected.
- The same port number declared twice is rejected.
- Any published port under `network = "offline"` is rejected.
- Undeclared ports are never reachable from the host.

### What the manifest deliberately cannot do

The schema does not accept arbitrary bind mounts, devices, secrets, OCI
capabilities, host environment passthrough, or raw backend flags. Only the
canonical project root is mounted, at `/workspace`. The guest environment is
constructed by Gas Can; only `TERM`, `COLORTERM`, `LANG`, and `LC_*` are
carried over from the host.

## Further reading

See the [macOS release checklist](docs/release/macos-checklist.md) for package
contents, signing/notarization inputs, the exact security contract, data
locations, clean-host verification, and conservative uninstall behavior.
