# Offline Workspace Image Build Evidence

Status: **PENDING — NOT GATE EVIDENCE**

The immutable workspace bundles in `images/workspace/versions.lock` currently have
`publication = "pending"`. Consequently, no cold prefetch, Apple offline build,
image platform/digest, warm rebuild, corruption check, or live smoke result is
claimed here.

There is also no reviewed builder-VM network-isolation implementation. Apple
Containerization 1.1 provides `--network none` for container run/create, but no
equivalent supported control for the separate BuildKit builder VM. The gate
therefore fails closed before building. A host `sandbox-exec` wrapper is not an
acceptable substitute because it confines the CLI process, not builder-VM
egress.

Once all three records are published, the Gate code must first be updated and
re-reviewed with a concrete builder-VM isolation design, pinned identity and
installer contract, and live validation. Only after that code change may an
operator run:

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
