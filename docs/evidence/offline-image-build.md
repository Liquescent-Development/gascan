# Offline Workspace Image Build Evidence

Status: **PENDING — NOT GATE EVIDENCE**

The immutable workspace bundles in `images/workspace/versions.lock` currently have
`publication = "pending"`. Consequently, no cold prefetch, Apple offline build,
image platform/digest, warm rebuild, corruption check, or live smoke result is
claimed here.

This offline gate is deferred and is not a macOS MVP prerequisite. A
2026-07-15 diagnostic proved that Apple builder public connectivity works after
correcting a local firewall. The MVP uses the connected, locked build described
in `docs/superpowers/specs/2026-07-15-connected-mvp-build-design.md`.

If offline-build hardening resumes, publish all three records and update the
gate through normal review. Deliberate builder-VM network isolation is not a
requirement; the offline gate instead proves that a prepared local context can
build without fetching. An operator may then run:

```sh
sudo -v
./scripts/run-offline-image-gate.sh cold
./scripts/run-offline-image-gate.sh warm
for bundle in ubuntu_packages mise_runtimes gascamp_source_vendor; do
  ./scripts/run-offline-image-gate.sh corrupt "$bundle" && exit 1
done
```

Record only sanitized harness output. It contains the mode, exact digest-qualified
workspace image reference, and basename-keyed artifact hashes. Before replacing
this template, attach the Apple version, macOS version and architecture, exact
`versions.lock` digest, cold and warm command exit statuses, all smoke statuses,
and confirmation that no current-run ownership-token container remains. Do not
record credentials, release tokens, absolute home paths, or unrelated container
inventory.
