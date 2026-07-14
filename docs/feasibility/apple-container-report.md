# Apple container feasibility evidence

Status: **BLOCKED — offline live proof pending**

## Environment and established evidence

- Supported host previously observed: Apple silicon macOS 26+.
- Apple container application previously observed: 1.1.0.
- Guest image used by the live harness: `docker.io/library/alpine:3.20` (runtime previously reported Alpine 3.22.2 content; resolved digest must be captured by the controller).
- Lifecycle, exact canonical bind, named-volume persistence, configured/cgroup resource limits, loopback publishing, ownership-token cleanup, TTY, exact exits, binary streams, resize, and supported signal behavior have passing Task 4/5 evidence.

## Offline mechanism source evidence

Pinned Apple `container` tag 1.1.0 defines `NetworkClient.noNetworkName = "none"` as the reserved name for no network attachment. `Utility.makeContainerConfiguration` accepts `none` only as the sole network selection and sets `config.networks = []`; otherwise an omitted network attaches the built-in default. Network creation rejects the reserved name. Therefore the only Apple 1.1 hard no-attachment CLI form is:

```text
container run ... --network none ...
```

`container network create --internal` is only host-only networking and is not offline isolation. DNS suppression is explicitly rejected as evidence.

## Current capability decision

`AppleProbe::base_capabilities()` reports `NetworkIsolation::Unsupported` fail-closed. `offline_network_args` rejects before invoking mount construction unless passed `NetworkIsolation::Proven`; its only proven-form output is the literal pair `--network`, `none`.

Promotion to `Proven` is forbidden until the controller runs the ignored adversarial test and records that structured attachments are empty and all DNS, direct external IPv4, TEST-NET, and owned host-probe requests fail both before and after guest-root route/interface mutation.

The test first requires a normal owned networked container to reach the same `https://example.com`, direct `http://1.1.1.1`, and temporary host-server targets. This positive control prevents host-wide outages, image tool limitations, or an incorrectly bound host listener from producing a false offline pass.

## Controller commands required to resolve BLOCKED

```sh
./scripts/apple-test-preflight.sh
container image inspect docker.io/library/alpine:3.20
cargo test -p gascan-apple --test live -- --ignored --test-threads=1 offline_workspace_cannot_reach_external_or_host_networks
cargo test -p gascan-apple --test live -- --ignored --test-threads=1
```

`container image inspect` emits Apple 1.1 JSON by default; copy the immutable digest for the resolved Alpine image into the environment section. Afterward, confirm structured lists contain no current-prefix containers, volumes, or networks. Until all commands pass and evidence is appended, Gate 2 remains **BLOCKED**.
