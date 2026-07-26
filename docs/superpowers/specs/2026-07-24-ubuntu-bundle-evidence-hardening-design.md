# Ubuntu Bundle Evidence Hardening Design

## Goal

Make the reviewed Ubuntu package bundle prove four properties behaviorally:
the complete root input is exact, signed package records cannot conflict,
shared cache bytes are never trusted before validation, and the configured
offline closure supplies every reviewed command without added privileges.

## Architecture

The producer and independent verifier remain separate implementations. The
producer fetches and verifies all signed release/index evidence before it
examines a shared cache, stages only validated cache candidates into a private
APT directory, validates all downloaded payloads, and atomically publishes
validated payloads. The independent verifier separately normalizes signed
metadata, recomputes dependency and command evidence, and rejects any mismatch.

The CI validation job extracts the bundle and launches the exact pinned Ubuntu
ARM64 image with `--network none`. Inside that disposable container it installs
and configures every exact manifest package from the mounted local repository,
runs `dpkg --audit`, and invokes the independent verifier to compare canonical
command-provider/path evidence. A bounded timeout and automatic container
cleanup make failure closed and prevent leaked validation state.

## Signed Metadata Semantics

Signed records are grouped by `(Package, Version, Architecture)`.

- A second record for the same group inside one signed index is always
  ambiguous, even if Filename, hashes, or other fields differ.
- A record republished in another signed index is accepted only when its full
  parsed stanza is equal, including folded/unknown fields and all
  selection/install fields such as Filename, Size, SHA256, Depends,
  Pre-Depends, Provides, Multi-Arch, Conflicts, Breaks, and Replaces.
- Any cross-index difference is rejected before package selection or cache use.

The producer and independent verifier implement this rule separately and are
covered by the same mutation matrix.

## Cache Boundary

`UBUNTU_PACKAGE_CACHE` remains scoped by snapshot, architecture, and reviewed
root-input digest. It is never used as APT's archive directory.

After signed metadata verification, a producer-only helper examines each cache
candidate in the shared directory. A candidate enters the private run staging
directory only when its decoded filename, exact `dpkg-deb` package tuple, byte
size, and SHA-256 digest match the unique normalized signed record. Poisoned or
unrecognized entries are not staged or modified.

After APT completes its private download, the helper validates every private
payload before any publication begins. Each new cache entry is copied to a
same-directory temporary file, flushed, and atomically renamed. Failure or
injected interruption removes temporary files, does not alter prior valid
entries, and does not publish an invalid destination.

## Command Evidence

The canonical evidence file is `command-providers.tsv` with three tab-separated
columns:

```text
command	package	absolute-command-path
```

The required mappings are:

```text
dig bind9-dnsutils
nslookup bind9-dnsutils
ip iproute2
ss iproute2
ping iputils-ping
ifconfig net-tools
netstat net-tools
ps procps
top procps
pstree psmisc
nano nano
pico nano
```

For every command, runtime recomputation requires `command -v` to return an
absolute executable path. The path, or its fully resolved alternative target,
must be owned by the expected exact manifest package according to
`dpkg-query`. All manifest versions and architectures must match the installed
database. `pico` must exist after package configuration, resolve through the
alternatives link to Nano's executable, and be attributed to the exact selected
Nano package.

The producer writes the file only after an offline exact install and successful
`dpkg --audit`. The independent verifier recomputes it in a separate clean
pinned container and requires byte-for-byte canonical equality.

## Exact Root Contract

The package-contract test stores the entire trusted sorted root file and
compares it byte-for-byte with `tests/image/system-tools.txt`. Adding, removing,
or substituting any root therefore fails even if the digest/config records are
updated together.

## Error Handling

The producer exits before output publication for:

- invalid or conflicting signed metadata;
- a cache candidate that cannot be validated for staging;
- an invalid private download;
- incomplete or interrupted atomic cache publication;
- offline install or package configuration failure;
- an audited package database error;
- a missing command, non-executable path, wrong provider/path, or absent Pico
  alternative.

The CI validator fails on timeout, container failure, audit failure, or command
evidence mismatch. It has no network and receives only read-only source/evidence
mounts plus disposable container state.

## Tests

Rust fixture mutations cover:

- same-index duplicate PVA with changed filename/hash;
- cross-index changes to Depends, Provides, Multi-Arch, and an unknown field;
- missing command evidence, wrong provider/path, and missing Pico alternative;
- poisoned cache input, failed-run non-publication, valid reuse, and interrupted
  atomic publication;
- byte-exact full root input;
- pinned ARM64 `--network none` workflow installation, `dpkg --audit`, timeout,
  cleanup, and independent command comparison.

The final acceptance run regenerates the real archive using the validated
content cache, installs it in a fresh pinned offline ARM64 container,
independently verifies all signed/dependency/command evidence, and runs the
focused and complete scripts suites plus lint, syntax, formatting, and diff
checks.
