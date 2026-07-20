# Gas Can macOS MVP release checklist

Gas Can 0.1 targets Apple-silicon Macs running macOS 26 or newer. It requires
Apple `container` 1.1.0 and its matching running service. The Gas Can package
does not redistribute `container`, `container-apiserver`, an Apple kernel, or
the workspace image. The daemon starts per user, on demand, when the CLI first
connects; the package installs no launch daemon or login item.

## Security and runtime contract

- The canonical selected project root is the only host directory mounted into
  a sandbox, at `/workspace`, read/write. Host home directories, credentials,
  SSH agents, Docker sockets, devices, and arbitrary mounts are not forwarded.
- The image's `workspace` user is the default. Passwordless `sudo` provides
  root only inside the guest and does not expand host access.
- `networked` uses Apple networking. `offline` uses Apple's no-network
  configuration and is a release-blocking promise. Published ports bind only
  to host loopback.
- CPU and memory limits are supported. Explicit disk limits fail closed with
  `disk_control_unsupported`; process-count input is rejected. Gas Can makes no
  disk- or process-ceiling claim on Apple `container` 1.1.0.
- The daemon API is versioned protobuf over a mode-0600, user-owned Unix
  socket. There is no daemon TCP listener.

The approved workspace image is the digest-qualified public GHCR reference in
`images/workspace/approved-image.txt`. End users consume that prebuilt image;
they do not build it during installation.

## Build and credentials

From a signed source revision on an Apple-silicon Mac:

```sh
./packaging/macos/package.sh
./tests/release/clean-host.sh --package-only
```

The package builder uses `cargo build --locked`, builds the native Swift attach
bridge, strips the three executables, and records their SHA-256 values and the
full source revision in `build-manifest.json`. It emits only the final package
path on stdout.

Release credentials are optional inputs and are never stored in the repository
or package:

- `GASCAN_CODESIGN_IDENTITY`: Developer ID Application identity for the three
  executables.
- `GASCAN_INSTALLER_SIGNING_IDENTITY`: Developer ID Installer identity passed
  to `pkgbuild`.
- `GASCAN_NOTARYTOOL_PROFILE`: existing Keychain profile passed to
  `xcrun notarytool`; a successful submission is stapled.

CI must inject those values from its secret store. Do not pass private keys,
passwords, API keys, or notarization credentials as command-line values.

## Install and verify

Install Apple `container` 1.1.0, start its service, then run:

```sh
./packaging/macos/install.sh .artifacts/release/gascan-0.1.0-macos-arm64.pkg
gascan doctor --json | jq
```

The product package contains:

- `/usr/local/bin/gascan`
- `/usr/local/bin/gascand`
- `/usr/local/bin/gascan-apple-attach`
- `/usr/local/share/gascan/LICENSE`
- `/usr/local/share/gascan/default-gascan.toml`
- `/usr/local/share/gascan/build-manifest.json`

Copy the default manifest into a project as `gascan.toml`, choose a unique
`name`, then use `gascan up PATH`, `gascan run`, `gascan shell`,
`gascan apply`, `gascan down`, and `gascan destroy --yes`. Setup changes run
only on initial creation or explicit `apply`.

Host controller state and the on-demand socket live under
`${XDG_RUNTIME_DIR}/gascan` when `XDG_RUNTIME_DIR` is set, otherwise under
`/tmp/gascan-UID/gascan`. Persistent tool, cache, and Gas Can configuration
live in Apple named volumes; the canonical project remains at its selected host
path. Apple owns its runtime/image storage locations.

## Clean-host Gate 5

On a clean supported Mac, with no pre-existing Gas Can installation and only
the approved image available, run:

```sh
sudo -v
GASCAN_RELEASE_CLEAN_HOST_CONFIRM=YES ./tests/release/clean-host.sh
```

The gate builds and inspects the package, installs it, requires `doctor` to
pass, checks every pinned language and both Gascamp sources, confirms setup
apply semantics, restarts the workspace and on-demand daemon, proves offline
DNS/public-IP denial, destroys its exact sandboxes, uninstalls, and requires
empty test-owned Apple container/volume inventories. Its required final line
is `PASS: Gas Can macOS MVP release gate`.

Do not claim Gate 5 from `--package-only`, unit tests, a dirty development
host, or partial output.

## Uninstall

```sh
./packaging/macos/uninstall.sh
```

The default removes only package-owned binaries, documentation, and the
package receipt. It deliberately preserves every sandbox, Apple volume, cache,
and controller-state file.

```sh
./packaging/macos/uninstall.sh --remove-data
```

The explicit flag first asks the installed CLI for its owned sandbox IDs and
destroys each through the authenticated daemon API before removing package
files. It never selects resources by a broad substring and never removes
foreign Apple resources.
