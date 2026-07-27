# Writable Runtime Homes and Managed Storage Design

## Summary

Gas Can currently gives the `workspace` user writable managed volumes only at
three narrow leaf paths:

- `/home/workspace/.local/share/mise`
- `/home/workspace/.cache`
- `/home/workspace/.config/gascan`

The connected workspace image also retains its image-build
`CARGO_HOME=/opt/gascan/mise/cargo` and
`RUSTUP_HOME=/opt/gascan/mise/rustup` settings at runtime. The bundled Rust
tree is root-owned, so ordinary commands such as `cargo run` fail as soon as
Cargo tries to update its registry cache.

A live audit found the same structural problem in other defaults. npm and
RubyGems select immutable installation prefixes, Go installs executables into
a non-persistent directory that is absent from `PATH`, Python user installs
cannot create the conventional `~/.local/bin`, and conventional XDG
configuration cannot be created below the root-owned `~/.config` parent.

Gas Can will broaden the existing managed volumes to conventional user roots,
define one explicit writable-home policy for the bundled development tools,
seed the bundled Rust toolchain into writable per-sandbox storage, and reject
old volume layouts with clear recreation guidance. Bundled programs remain
immutable fallbacks under `/opt/gascan`; user-created state and overrides
remain writable and persistent.

## Goals

- Make `cargo run`, dependency fetching, `cargo install`, rustup toolchain
  management, target installation, and component installation work as the
  `workspace` user.
- Prevent every bundled default from selecting an immutable `/opt/gascan`
  location for normal user writes.
- Persist user-installed tools, caches, configuration, credentials, plugins,
  and agent state in the independently sized `tools`, `cache`, and `config`
  volumes.
- Preserve immutable, reviewed workstation binaries as safe fallbacks.
- Put user-installed executables before immutable defaults in `PATH`.
- Detect the incompatible old volume layout before a container can mount old
  volume contents at the new roots.
- Exercise real package-manager writes in automated and connected-image tests.
- Release the fix as Gas Can `0.1.10`.

## Non-goals

- No compatibility shim for existing sandbox volumes. Existing sandboxes must
  be destroyed and recreated.
- No single persistent volume for the entire home directory.
- No persistence guarantee for arbitrary files directly under
  `/home/workspace`, such as shell history or legacy dotfiles.
- No host credential forwarding or relaxation of sandbox isolation.
- No automatic network upgrade of bundled default tools during `gascan up`.
- No change to the independent `tools`, `cache`, and `config` capacity
  settings in `gascan.toml`.

## Managed Volume Layout

The three existing logical volumes retain their names, capacities, ownership
metadata, and independent lifecycle, but their mount targets become:

| Volume | New target | Purpose |
| --- | --- | --- |
| `tools` | `/home/workspace/.local` | mise installs, language toolchains, package-manager installs, and user binaries |
| `cache` | `/home/workspace/.cache` | disposable but persistent download, registry, module, and build caches |
| `config` | `/home/workspace/.config` | Gas Can state, agent credentials/configuration, CLI configuration, and XDG application configuration |

The daemon initializes the tools and cache roots as
`workspace:workspace` mode `0700`. It initializes the config root as
`root:workspace` mode `1770`: the workspace group can create conventional XDG
configuration, while the sticky bit prevents the workspace user from
renaming or deleting Gas Can's root-owned SSH entries. Image assembly creates
the same root shape so direct image checks and production containers agree.

The image `VOLUME` declaration, policy compiler, storage-capacity extraction,
runtime inspection expectations, workstation contract, and documentation all
use these exact targets. No code may continue treating a language-specific
leaf as the volume boundary.

## Runtime Home Policy

Gas Can supplies the same explicit environment to normal commands,
provisioning commands, SSH sessions, and image contract tests.

| Concern | Runtime location |
| --- | --- |
| `XDG_DATA_HOME` | `/home/workspace/.local/share` |
| `XDG_CACHE_HOME` | `/home/workspace/.cache` |
| `XDG_CONFIG_HOME` | `/home/workspace/.config` |
| `MISE_DATA_DIR` | `/home/workspace/.local/share/mise` |
| `MISE_CACHE_DIR` | `/home/workspace/.cache/mise` |
| `MISE_GLOBAL_CONFIG_FILE` | `/home/workspace/.config/gascan/mise.toml` |
| `MISE_SYSTEM_CONFIG_FILE` | `/etc/mise/config.toml` |
| `MISE_STATE_DIR` | `/home/workspace/.config/gascan/mise-state` |
| `CARGO_HOME`, `MISE_CARGO_HOME` | `/home/workspace/.local/share/cargo` |
| `RUSTUP_HOME`, `MISE_RUSTUP_HOME` | `/home/workspace/.local/share/rustup` |
| `NPM_CONFIG_PREFIX` | `/home/workspace/.local` |
| `NPM_CONFIG_CACHE` | `/home/workspace/.cache/npm` |
| `GOPATH` | `/home/workspace/.local/share/go` |
| `GOCACHE` | `/home/workspace/.cache/go-build` |
| `GOMODCACHE` | `/home/workspace/.cache/go-mod` |
| `PYTHONUSERBASE` | `/home/workspace/.local` |
| `GEM_HOME` | `/home/workspace/.local/share/gem` |
| `MIX_HOME` | `/home/workspace/.local/share/mix` |
| `HEX_HOME` | `/home/workspace/.local/share/hex` |
| `REBAR_CACHE_DIR` | `/home/workspace/.cache/rebar3` |

The existing Claude, Codex, Pi, Herdr, GitHub CLI, and GitLab CLI variables
remain below the expanded config or cache roots. Git and XDG-aware editors
can use conventional paths below `/home/workspace/.config`.

The generated global mise config has higher precedence than the immutable
system config, so manifest tools override bundled defaults while undeclared
bundled tools remain available as reviewed fallbacks. Provisioning uses the
same environment as normal runtime commands. Its resolution parser retains
requested records sourced from the generated global config and permits
additional records only when they are singleton, active, installed records
sourced from the exact immutable `/etc/mise/config.toml` path.

`PATH` begins with user-controlled executable locations, followed by
user-mise shims, immutable system-mise shims, reviewed workstation commands,
and operating-system directories. Its exact ordered entries are:

1. `/home/workspace/.local/bin`
2. `/home/workspace/.local/share/cargo/bin`
3. `/home/workspace/.local/share/go/bin`
4. `/home/workspace/.local/share/gem/bin`
5. `/home/workspace/.local/share/mise/shims`
6. `/opt/gascan/mise/shims`
7. `/usr/local/sbin`
8. `/usr/local/bin`
9. `/opt/gascan/workstation/bin`
10. `/usr/sbin`
11. `/usr/bin`
12. `/sbin`
13. `/bin`

The exact Ruby version-specific user directory remains discoverable through
RubyGems itself; Gas Can does not hard-code a Ruby ABI version into policy.
The generic writable `GEM_HOME/bin` is the supported executable destination.

The Dockerfile may use `/opt/gascan/mise/{cargo,rustup}` while constructing
the immutable image. The final runtime stage must override those build-only
values with the writable paths above. Tests distinguish build-time homes from
the effective runtime environment instead of requiring the build paths to
remain effective.

## Writable Rust Bootstrap

mise's Rust backend is implemented through rustup. Merely changing
`RUSTUP_HOME` disconnects the bundled mise shim from its installed toolchain;
a live probe confirms that an empty user Rust home makes `cargo` an invalid
shim.

During first provisioning, Gas Can copies the bundled toolchain from
`/opt/gascan/mise/rustup` into
`/home/workspace/.local/share/rustup`. It also seeds the reviewed rustup
command layout from `/opt/gascan/mise/cargo/bin` into the writable
`/home/workspace/.local/share/cargo/bin`; otherwise mise selects the copied
toolchain but its shims cannot find `rustc`, `cargo`, or `rustup`. The copy:

- runs only after the tools volume is mounted and owned by `workspace`;
- copies as `workspace` without preserving root ownership;
- stages each missing toolchain before atomic publication;
- never replaces an existing user toolchain directory;
- copies only the metadata needed for rustup to recognize bundled
  toolchains;
- records the bundled toolchain identity in a Gas Can-owned marker;
- reconciles a newly bundled toolchain after an image update while preserving
  user-installed toolchains, targets, components, and settings;
- cleans incomplete staging directories after interruption.

The command seed accepts only the locked image layout: a regular executable
`rustup` and the static allowlist of proxy symlinks whose raw target is exactly
`rustup`. Missing, unexpected, alternate-target, and alternate-type entries
fail closed. Gas Can publishes a user-owned regular `rustup` and recreates the
reviewed proxy symlinks through restrictive staging and atomic no-clobber
renames. Existing executable user commands and already-correct proxy symlinks
are preserved; unsafe destination collisions are never overwritten.

The immutable, strictly size-bounded rustup `settings.toml` is the sole source
of truth for the bundled default. Gas Can validates the entire actual rustup
subset without shell evaluation: exact canonical `version`,
`default_toolchain`, and `profile` records in order, followed only by the
optional exact empty `[overrides]` table. Unknown keys, malformed or duplicate
records, nested tables, and nonblank override entries fail closed. The one safe
default must identify a validated bundled toolchain with executable Cargo and
Rust commands. When the user has no rustup settings, Gas Can generates a
minimal mode-0600 rustup-compatible settings file through restrictive staging
and atomic no-clobber publication. Existing regular user settings are
preserved unchanged; symlink and non-regular collisions fail closed. This lets
direct `rustc`, `cargo`, and `rustup` calls use the bundled version from a
neutral directory without downloads.

On retry after a crash or SIGKILL, the bootstrap reclaims only its exact
reserved staging prefixes in the Rust-home and Cargo-bin directories. Each
prefix has a fixed allowed artifact type; symlinks, unsafe basenames, and type
mismatches fail closed. Removing a valid staging directory never follows a
nested symlink. Final paths, similarly named files outside the exact prefixes,
and unrelated user dotfiles are untouched.

The current bundled Rust home is approximately 1.5 GiB. This per-sandbox cost
is intentional and is charged to the configurable `tools` volume, whose
default is 10 GiB. The bootstrap is local and performs no network access.

After bootstrap, the bundled Rust version still works without downloads, and
the user can run `rustup component add`, `rustup target add`, or declare a
different Rust version and components in `gascan.toml`. Cargo registry and Git
state remains in the persistent Cargo home. Project `target` directories
remain inside the workspace unless the project configures otherwise.

## Volume Layout Migration

The persisted sandbox record gains an explicit managed-volume layout version.
The new layout is version 2.

For a sandbox record created without version 2:

- `status`, `up`, and `apply` report that the managed storage layout changed;
- Gas Can instructs the user to run `gascan destroy --yes` followed by
  `gascan up`;
- Gas Can does not recreate or remount the container automatically;
- `destroy` continues recognizing and deleting the existing named volumes.

This guard is required because the old tools volume contains mise data at its
root. Reusing that volume at `~/.local` would place the data at the wrong
relative path. The same issue applies to the old config volume. Silent reuse
would look successful while corrupting the runtime layout.

New sandboxes record layout version 2 when creation succeeds. Capacity-change
errors retain their existing behavior and wording, with layout incompatibility
reported independently.

## Default-Tool Audit

The workstation contract validates every bundled tool that has a conventional
user-write location. For each path it checks:

- the resolved destination is below `~/.local`, `~/.cache`, or `~/.config`;
- the destination or its nearest managed parent is writable by UID 1000;
- the relevant managed volume is the backing mount;
- no normal user-write destination resolves below `/opt/gascan`;
- installed executable destinations appear on `PATH` before immutable
  fallbacks.

The audit covers Rust/Cargo, mise, npm/Node, Go, Python user installs,
RubyGems, Mix/Hex/rebar, Claude, Codex, Pi, Herdr, `gh`, `glab`, Git, and the
XDG configuration used by Neovim and other compliant applications. Java and
operating-system debugging tools have no package-manager home imposed by Gas
Can; they must remain executable without attempting an image-tree write.

## Error Handling and Recovery

- Unsafe mount ownership or mode fails provisioning before package-manager
  work begins.
- A Rust bootstrap failure reports a dedicated provisioning step and retains
  the immutable source unchanged.
- Interrupted Rust copies leave only a recognizable staging directory, which
  the next provisioning attempt removes before retrying.
- A destination collision that is a symlink, non-directory, or unmarked
  Gas Can staging path fails closed rather than being replaced.
- Package-manager failures retain bounded, sanitized output through the
  existing provisioning error path.
- Existing user directories and settings within a valid version-2 volume are
  never recursively chowned or deleted.

## Testing and Verification

Unit and integration tests will cover:

- exact version-2 mount targets, capacities, ownership, and environment;
- exact `PATH` ordering and agreement between interactive, SSH, and
  provisioning environments;
- rejection of old/absent layout versions with destroy-and-recreate guidance;
- continued deletion of version-1 named volumes;
- Rust bootstrap first-run, idempotence, interrupted staging cleanup,
  preservation of user state, and addition of a newly bundled toolchain;
- image contracts that allow `/opt/gascan` Rust homes only before image
  installation and require writable homes in the final runtime stage;
- package-manager destinations and writable-root containment;
- correction of the stale SSH warning test that still assumes mode `0755` is
  unsafe.

The connected-image gate will run as `workspace` and demonstrate:

- a Cargo project with a crates.io dependency can fetch, build, and run;
- `cargo install --path` publishes a command that is immediately on `PATH`;
- rustup can add and inspect a component or target in the writable toolchain;
- npm can install a local package globally and resolve its executable;
- `go install` publishes a local command on `PATH`;
- RubyGems can install a local gem;
- Python can perform a user install and resolve its executable;
- mise can install a user override without writing below `/opt/gascan`;
- a file can be created below a conventional XDG application directory.

Local verification includes formatting, Clippy with warnings denied, the
complete Rust workspace test suite, script tests, image contract tests, and
the release preflight. The final release smoke test installs the published
artifact, creates a fresh version-2 sandbox, reruns representative write
checks, and destroys that sandbox.

## Documentation and Release

The README will document:

- the new volume roots and what belongs in each;
- the one-time destroy/recreate requirement for pre-`0.1.10` sandboxes;
- the 1.5 GiB writable Rust seed charged to the tools volume;
- examples for Cargo, rustup components, npm globals, Go tools, Python user
  installs, RubyGems, and mise tool overrides;
- how to increase `[storage].tools`, `[storage].cache`, and
  `[storage].config` independently.

Implementation will be committed on an isolated branch, reviewed through a
pull request, and merged only after all required checks pass. The repository's
release driver will perform the patch-version update to `0.1.10`, create and
push the signed tag, publish the GitHub release, and verify the installed
release. The temporary worktree and merged local branch will then be removed
without touching the user's existing dirty checkout.
