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
reports one concise result per capability — architecture, macOS version,
runtime service, storage, bind mounts, named volumes, TTY, signals, loopback
publishing, resource limits, offline isolation, and managed SSH:

```sh
gascan doctor
```

Use `gascan doctor --json | jq` when you need the complete machine-readable
report.

### Building from source

Building is for contributors; installing a release does not require it.
Packaging refuses to build from an untrusted source revision: the checkout must
be either a trusted signed commit or the exact signed release tag. Build from
the tag rather than from `main`, which moves ahead between releases:

```sh
git checkout v0.1.11
package=$(./packaging/macos/package.sh)
GASCAN_EXPECTED_SOURCE_REVISION=$(git rev-parse HEAD) \
GASCAN_EXPECTED_VERSION=0.1.11 \
  ./packaging/macos/install.sh "$package"
```

Skipping the checkout leaves `HEAD` on a commit the release tag does not
attest, and `package.sh` exits 65 with `release source HEAD needs a trusted
commit signature or exact signed v0.1.11 tag`.

Verification runs through Git's own trust policy, so the tag's signing key must
be one you have chosen to trust. Releases are signed with this SSH key:

```text
richard@liquescent.dev ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHyTKmfAwcJcdfKXmj2h3mwfgPaelE6gSMrquAcPmW09
```

Its fingerprint is `SHA256:3NWoJ1nmsLHxd8hAG/BnyriJJpIFXHaW3RtuPYANKc4`. Add it
to a Git allowed-signers file and point Git at it:

```sh
mkdir -p ~/.config/git
signer='richard@liquescent.dev'
key='ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHyTKmfAwcJcdfKXmj2h3mwfgPaelE6gSMrquAcPmW09'
printf '%s %s\n' "$signer" "$key" \
  >> ~/.config/git/allowed_signers
git config --global gpg.ssh.allowedSignersFile ~/.config/git/allowed_signers
git verify-tag v0.1.11
```

## Quickstart

Create `gascan.toml` in the project root. This practical starting point enables
network access for agent authentication and tool downloads, gives the sandbox
room for development, and installs the latest Claude Code into the persistent
tools volume:

```toml
version = 1
name = "my-project"
network = "networked"
user = "workspace"
gascamp = "bundled"

[resources]
cpus = 4
memory = "8GiB"

[storage]
tools = "10GiB"
cache = "10GiB"
config = "1GiB"

[tools]
node = "lts"
"npm:@anthropic-ai/claude-code" = "latest"
```

From the project root, check the host and create the sandbox:

```sh
gascan doctor
gascan up .
gascan shell
```

`gascan shell` opens interactive login Bash with colors and completion. It
starts at `/workspace`, backed by the project directory on the Mac, so it is
immediately ready for normal project commands:

```sh
git status
```

Claude Code and Herdr are also ready to launch inside it:

```sh
claude --version
claude

herdr --version
herdr
```

Claude prompts for sandbox-local authentication on first use. Herdr opens its
first-run onboarding; after that, running `herdr` starts or reattaches to the
background session. Press `Ctrl-B`, then `q` to detach while its panes keep
running. Agent credentials and configuration, plus Herdr configuration and
logs, remain inside the sandbox's managed volumes through `down`, `up`, and
workspace-image replacement. Stopping the sandbox stops its running processes;
starting Herdr again restores what the installed Herdr version can recover
from its persisted state. `gascan destroy --yes` deletes the managed volumes.

After editing `gascan.toml`, reconcile the running sandbox explicitly:

```sh
gascan apply
```

The essential host-side lifecycle is:

```sh
gascan status         # inspect the running sandbox and available updates
gascan shell          # return to an interactive shell at /workspace
gascan down           # stop the sandbox but retain its managed volumes
gascan up .           # start it again
gascan destroy --yes  # permanently delete the sandbox and managed volumes
```

See [Configuring `gascan.toml`](#configuring-gascantoml) for the full manifest,
and [SSH and VS Code Remote SSH](#ssh-and-vs-code-remote-ssh) for host-side
editor access.

Gascan shows live, in-place progress when stderr is an interactive terminal.
When output is redirected, the same meaningful milestones are printed as
stable plain text without animation or color. Set `NO_COLOR=1` to disable color
while keeping interactive progress. Use `--json` on supported commands for
machine-readable output.

Workspace image updates are reported by `gascan status`. Run `gascan apply` to
replace only the container while preserving the workspace and managed tools,
cache, and configuration volumes. Changes made directly to the container root
filesystem are not durable.

If replacement fails, Gascan reports the primary failure and restores the
previous workspace image. Fix the primary error and run `gascan apply` again.
If rollback also fails, preserve the reported primary and rollback diagnostics,
avoid changing or deleting the managed volumes, and retry `gascan apply` after
restoring access to both digest-qualified images. Use `gascan destroy --yes`
only when you intend to delete the sandbox and all of its managed volumes.

Commands other than `up` resolve the sandbox implicitly when exactly one
exists. With more than one, pass `--sandbox <id>`; `gascan list` prints the
ids. A sandbox id is the slugified `name` plus a short digest of the canonical
project root, so the same project always maps to the same sandbox.

### SSH and VS Code Remote SSH

A networked sandbox enables SSH by default. Apple Container publishes guest
port 22 on a host IPv4 loopback port, so SSH is reachable from the Mac but is
not exposed to the LAN. Inside the sandbox, `sshd` listens on its isolated
Gas Can network so Apple's native publisher can reach it; containers on the
Apple default network or another sandbox network cannot. Gas Can creates a
stable `gascan-<sandbox-id>` alias after strict host-key verification succeeds:

```sh
gascan status
gascan ssh
gascan ssh -- git status
```

Gas Can preserves each argument after `--` as a discrete local OpenSSH
argument and never invokes a local shell. Standard OpenSSH remote-command and
remote-shell semantics still apply inside the sandbox.

The host port is selected automatically unless `[ssh].host_port` requests a
specific port. An unavailable explicit port fails with
`ssh_port_unavailable`; Gas Can never silently substitutes another port.
Offline sandboxes have no SSH listener, identity authorization, or alias.
Explicitly enabling SSH while `network = "offline"` is rejected rather than
changing the sandbox's network policy.

On the first successful interactive `gascan up`, Gas Can offers to add its
managed SSH config to `~/.ssh/config`. Noninteractive and JSON commands never
prompt or modify that file. The same operation is available explicitly:

```sh
gascan ssh-config install
gascan ssh-config path
gascan ssh-config remove
```

After installing the include, connect from VS Code's **Remote - SSH: Connect
to Host...** command by selecting `gascan-<sandbox-id>`. Removing the include
does not remove Gas Can's managed aliases or prevent `gascan ssh`; it only
stops other OpenSSH clients from discovering them through `~/.ssh/config`.

Gas Can maintains one installation-wide Ed25519 client identity under
`~/.config/gascan/ssh`. Its private key remains on the host and survives
sandbox destruction. Each sandbox has a separate persistent host key in its
managed config volume. `down` temporarily removes the active alias; the next
`up` verifies the retained fingerprint before restoring it. `apply` preserves
both fingerprints and updates the alias if an automatically selected port
changes.

A host or client fingerprint mismatch fails closed with
`ssh_host_key_mismatch`: Gas Can does not publish or use an unverified alias.
Run `gascan doctor` and inspect the sandbox before retrying. Destroy and
recreate only when intentionally resetting trust, because
`gascan destroy --yes` removes the alias, active sandbox trust, sandbox host
key, and all managed volumes. Retired immutable known-host generations can
remain unreferenced so concurrent OpenSSH readers keep a consistent snapshot;
the current managed config does not load them. Destroy retains the
installation-wide client identity and does not remove the optional
`~/.ssh/config` include.

For SSH diagnostics, human output gives a compact summary; JSON includes the
exact fact details and remedies:

```sh
gascan doctor
gascan doctor --json | jq '.checks[] | select(.id | startswith("ssh."))'
```

The SSH facts are `ssh.client`, `ssh.identity`, `ssh.config`, and
`ssh.native_publish`. Also check `gascan status`: `Starting`, `Unhealthy`, or
`Unavailable` means `gascan ssh` will refuse the connection until a successful
`gascan up` verifies and publishes the alias.

### Commands

| Command | Purpose |
| --- | --- |
| `gascan up <project-root> [--json]` | Create and start a sandbox. |
| `gascan apply [project-root] [--json]` | Apply `gascan.toml` changes. |
| `gascan run -- <argv...>` | Run a single command in the sandbox. |
| `gascan shell [-- <argv...>]` | Open an interactive shell. |
| `gascan ssh [-- <argv...>]` | Open SSH or run a remote command. |
| `gascan ssh-config install` | Install the managed SSH include. |
| `gascan ssh-config remove` | Remove the managed SSH include. |
| `gascan ssh-config path` | Print the absolute generated OpenSSH config path. |
| `gascan status [--json]` | Show desired and actual state for one sandbox. |
| `gascan list [--json]` | List all sandboxes. |
| `gascan logs [--follow] [--since-millis <n>]` | Stream sandbox logs. |
| `gascan down [--json]` | Stop the sandbox without deleting state. |
| `gascan destroy --yes [--json]` | Delete the sandbox and volumes. |
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
See the [`gascan.toml` reference](docs/reference/manifest.md) for a compact
key-by-key specification.

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

[storage]                       # optional; managed-volume capacities
tools = "10GiB"
cache = "10GiB"
config = "1GiB"

[shell]
prompt = "standard"
# prompt = "starship"
# prompt = "starship-nerd-font"

[tools]                         # mise tool name = version
node = "lts"
python = "3.13"
"npm:@anthropic-ai/claude-code" = "latest"

[ports]                         # label = port, published on loopback only
web = 3000

[ssh]                           # optional; defaults from network mode
enabled = true
host_port = 2222                # optional; automatic when omitted
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
  publish ports or enable SSH.
- `networked` — outbound network access. Required for anything that downloads,
  including installing tool versions that are not already in the image. SSH
  is enabled by default.

### `[ssh]`

Controls native OpenSSH access:

| Key | Default | Notes |
| --- | --- | --- |
| `enabled` | From `network` | Explicit offline enablement is invalid. |
| `host_port` | automatic | Exact loopback port in `1024..=65535`. |

When enabled, Gas Can publishes exactly `127.0.0.1:<host-port>:22`. An
automatic port is selected for each creation and may change after
container-only image replacement; the `gascan-<sandbox-id>` alias remains
stable. An explicit port is used exactly, and a collision fails with
`ssh_port_unavailable`.

`host_port` is invalid when `enabled = false`, and it cannot collide with a
port declared in `[ports]`. Unknown SSH keys are rejected. Gas Can never
silently changes `network` to satisfy an SSH setting.

### `[shell]`

Controls the prompt for interactive Bash login sessions:

| Value | Behavior |
| --- | --- |
| `standard` | Native colored Bash prompt with Bash completion. |
| `starship` | Managed Starship preset that works with ordinary terminal fonts. |
| `starship-nerd-font` | Richer managed Starship preset with Nerd Font icons and separators. |

`standard` is the default, backward-compatible prompt.
It does not activate Starship.
Both Starship modes use Gas Can's pinned, offline-capable Starship binary.
`starship` requires no special font.
`starship-nerd-font` requires a Nerd Font installed and selected in the host
macOS terminal.
Gas Can does not install fonts on the host.

The same prompt choice applies to both `gascan shell` and SSH.
Run `gascan apply` after changing the prompt. The new selection takes effect
in the next interactive login session.

`gascan shell -- <argv>` preserves the explicit-command escape hatch: Gas Can
forwards the arguments unchanged and does not substitute its managed default
login Bash. The explicit command controls its own shell startup behavior.

Gas Can protects its pinned binary and root-managed prompt files from
workspace-user mutation.
Pre-existing same-user interactive shell customization is trusted caller
state; it is not a same-shell isolation boundary.

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
| `disk` | — | — | **Rejected**; use `[storage]`. |

Sizes must be a positive integer plus one of `KiB`, `MiB`, `GiB`, or `TiB`.
Decimal units (`GB`), bare numbers, and zero are all rejected. Unknown
process-limit requests are rejected as well. Apple cannot enforce a container
root-filesystem ceiling, so `disk` does not size managed volumes.

### `[storage]`

Each setting controls one independently sized, writable, Gas Can-managed
volume:

| Key | Default | Guest mount |
| --- | --- | --- |
| `tools` | `10GiB` | `/home/workspace/.local` |
| `cache` | `10GiB` | `/home/workspace/.cache` |
| `config` | `1GiB` | `/home/workspace/.config` |

Storage sizes use the same binary units as memory: a positive integer followed
by `KiB`, `MiB`, `GiB`, or `TiB`. Each volume has a maximum requested capacity
of `512GiB`; decimal units, bare numbers, zero, and larger values are rejected.
Omitted keys retain their defaults independently.

Gas Can stores user-installed executables, language toolchains, and application
data in the `tools` volume; download and build caches in `cache`; and
conventional XDG application configuration in `config`. A new sandbox receives
an approximately 1.5 GiB local copy of the bundled Rust toolchain in `tools`.
The copy uses no network access, but its capacity is charged to that volume.
Increase the three capacities independently when a workload needs more room:

```toml
[storage]
tools = "20GiB"
cache = "10GiB"
config = "2GiB"
```

The version-2 mount layout introduced in Gas Can 0.1.10 is not compatible with
volumes created by a pre-0.1.10 release. Back up anything you need, then perform
this one-time recreation from the project root:

```bash
gascan destroy --yes
gascan up .
```

Apple volumes cannot be resized in place. If any effective `[storage]` value
changes after a sandbox has been created, `gascan up` and `gascan apply` refuse
the change without modifying the existing volumes. Recreate explicitly:

```sh
gascan destroy --yes
gascan up /path/to/project
```

Destroying removes the sandbox and all three managed volumes, including their
contents. Back up anything you need before recreating. `[resources].disk` is
not an alternative capacity control; it remains rejected because the Apple
runtime cannot enforce a ceiling on the container root filesystem.

Inside a networked sandbox, conventional package-manager workflows write to
the managed volumes and place user-installed commands on `PATH`:

```bash
cargo run
rustup component add rust-src
npm install -g typescript
go install golang.org/x/tools/gopls@latest
python -m pip install --user ruff
gem install bundler
```

Declare project-specific dependency versions in the project's dependency
files or in `[tools]`; global installs are user-managed conveniences, not
dependency declarations made automatically by Gas Can.

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

### Default developer workstation

Every sandbox also includes a credential-free workstation baseline:

- Editors: Vim, Neovim, Emacs, and Pico (the reviewed Nano alternative).
- Coding agents: Claude Code, Codex, Pi, and Herdr.
- Forge and source tools: Git, GitHub CLI (`gh`), and GitLab CLI (`glab`).
- Network diagnostics: `ip`, `ss`, `ping`, `ifconfig`, `netstat`, `dig`,
  `nslookup`, `traceroute`, and `nc`.
- Terminal and inspection tools: `curl`, `wget`, `rsync`, `lsof`, `file`,
  `jq`, `ps`, `top`, `pstree`, `tree`, `less`, `rg`, `fd`, `fzf`, and `tmux`.

Discover the installed versions with each tool's normal command:

```sh
vim --version
nvim --version
emacs --version
pico --version
claude --version
codex --version
pi --version
herdr --version
go version
rustc --version
cargo --version
gh --version
glab --version
git --version
ip -Version
ss --version
ping -V
ifconfig --version
netstat --version
dig -v
traceroute --version
nc -h
rg --version
fd --version
fzf --version
tmux -V
```

The image gate compares locked tools with their exact locked versions and
checks documented output formats for snapshot-pinned Ubuntu tools. These
commands work in the default offline sandbox and do not download anything at
startup. Diagnostic packages do not grant the sandbox extra Linux
capabilities, devices, or host access.

Image-owned workstation files under `/opt/gascan/workstation` are immutable.
An explicit `[tools]` entry is installed below
`/home/workspace/.local/share/mise`, within the managed tools volume mounted at
`/home/workspace/.local`. Its mise shim is first in `PATH`, ahead of the
reviewed defaults in the immutable `/opt/gascan/mise` system data tree. The
requested version therefore overrides an image default without changing the
immutable workstation tree.

Native Claude Code, Codex, Pi, GitHub CLI, and GitLab CLI configuration is
sandbox-local below `/home/workspace/.config/gascan`, within the managed config
volume mounted at `/home/workspace/.config`. Mise caches and Pi session data
are kept separately in the managed cache volume mounted at
`/home/workspace/.cache`. Herdr is configured to read
`/home/workspace/.config/gascan/herdr/config.toml` and place its logs beside
that file, but Gas Can does not create a Herdr configuration or login. Gas Can
never imports the host home directory, SSH material, agent/forge tokens,
Docker socket, or macOS keychain into the sandbox. Native sandbox-local
configuration survives `gascan down`, `gascan up`, and container-only image
replacement; `gascan destroy --yes` deletes it with the config volume.

Requesting any other tool or version makes mise download it, which requires
`network = "networked"`. Installed tools persist in a per-sandbox volume, so
they survive `gascan down` and are removed by `gascan destroy`.

Gas Can hashes the desired tool set and reinstalls only when that hash
changes. As with `setup`, editing `[tools]` and running `up` on an existing
sandbox reports `apply_required` with reason `tools_changed`; run
`gascan apply` to reconcile.

#### Updating an image-provided tool

An explicit `[tools]` declaration overrides an immutable image default. Use
mise's normal tool name for language runtimes, or its package backend for
tools distributed through an ecosystem such as npm:

```toml
[tools]
go = "latest"
rust = "latest"
"npm:@anthropic-ai/claude-code" = "latest"
```

This is especially useful for Claude Code because new releases add support for
new Claude models more frequently than Gas Can publishes workspace images.
`latest` resolves the newest available release when the declaration is first
installed. For a controlled upgrade, pin an exact release and change the value
when ready:

```toml
[tools]
"npm:@anthropic-ai/claude-code" = "2.1.218"
```

After adding or changing an override, run:

```sh
gascan apply
```

The requested version is downloaded into the sandbox's persistent tools
volume and its mise shim takes precedence over the bundled fallback. The
sandbox must be `networked` unless that exact artifact is already cached.
Gas Can only reapplies tools when the `[tools]` declaration changes; an
unchanged `latest` entry is not a promise to check the registry on every
`gascan apply`.

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

See the [`gascan.toml` reference](docs/reference/manifest.md) for the complete
configuration contract and the
[macOS release checklist](docs/release/macos-checklist.md) for package
contents, signing/notarization inputs, the exact security contract, data
locations, clean-host verification, and conservative uninstall behavior.
