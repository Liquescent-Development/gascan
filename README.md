# Gas Can

Gas Can is a secure, local sandbox for agentic coding on Apple-silicon Macs.
It runs each selected project inside a long-lived Linux container backed by
Apple's `container` runtime and the pinned Gas Can polyglot workspace image.

The macOS MVP requires macOS 26 or newer and Apple `container` 1.1.0. Install
and start Apple's runtime first; Gas Can does not bundle it. Build and install
the current package with:

The checkout must be a trusted signed commit or the exact signed release tag
(`v0.1.0` for this version); packaging rejects unsigned source revisions.

```sh
package=$(./packaging/macos/package.sh)
GASCAN_EXPECTED_SOURCE_REVISION=$(git rev-parse HEAD) \
GASCAN_EXPECTED_VERSION=0.1.0 \
  ./packaging/macos/install.sh "$package"
gascan doctor --json | jq
```

Copy [`packaging/macos/default-gascan.toml`](packaging/macos/default-gascan.toml)
to `gascan.toml` in a project, then:

```sh
gascan up /path/to/project
gascan run -- node --version
gascan shell
gascan apply /path/to/project
gascan down
gascan destroy --yes
```

Only the canonical project root is mounted from the host. The guest defaults
to the non-root `workspace` user and offers passwordless guest-only `sudo`.
Network modes are `networked` and fail-closed `offline`; published ports are
loopback-only. CPU and memory limits are supported. Disk-limit requests and
unknown process-limit requests are rejected on the supported Apple runtime.

See the [macOS release checklist](docs/release/macos-checklist.md) for package
contents, signing/notarization inputs, the exact security contract, data
locations, clean-host verification, and conservative uninstall behavior.
