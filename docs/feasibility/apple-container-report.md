# Apple container feasibility evidence

Status: **PASS — hard offline isolation proven for Apple container 1.1.0**

## Environment and established evidence

- Controller preflight: **PASS** on macOS 26.5.1 (`arm64`).
- Apple `container` and `container-apiserver`: 1.1.0, commit `5973b9cc626a3e7a499bb316a958237ebe14e2ed`.
- Guest image: `docker.io/library/alpine:3.20`; index digest `sha256:d9e853e87e55526f6b2917df91a2115c36dd7c696a35be12163d44e6e2a4b6bc`; resolved arm64 variant digest `sha256:45e09956dc667c5eff3583c9d94830261fb1ca0be10a0a7db36266edf5de9e1d`.
- Lifecycle, exact canonical bind, named-volume persistence, configured/cgroup resource limits, loopback publishing, ownership-token cleanup, TTY, exact exits, binary streams, resize, and supported signal behavior have passing Task 4/5 evidence.

## Offline mechanism source evidence

Pinned Apple `container` tag 1.1.0 defines `NetworkClient.noNetworkName = "none"` as the reserved name for no network attachment. `Utility.makeContainerConfiguration` accepts `none` only as the sole network selection and sets `config.networks = []`; otherwise an omitted network attaches the built-in default. Network creation rejects the reserved name. Therefore the only Apple 1.1 hard no-attachment CLI form is:

```text
container run ... --network none ...
```

`container network create --internal` is only host-only networking and is not offline isolation. DNS suppression is explicitly rejected as evidence.

## Verified offline evidence and capability decision

The controller ran `network::offline_workspace_cannot_reach_external_or_host_networks` alone with serialized ignored-test execution. It passed 1/1 in 36.19 seconds. The default-network container reached the owned DNS/PF host service, proving the ordinary VM-to-host path. Public `http://example.com` and direct `http://1.1.1.1` were both unreachable from that control on this host and were recorded as non-discriminating diagnostics. The offline container denied all four target roles—including the exact owned host endpoint proven reachable—with structured `configuration.networks=[]` both before and after guest-root route/interface mutation. Cleanup verification `sudo -n container system dns list --format json` returned exactly `[]`.

`AppleProbe::base_capabilities()` now reports `NetworkIsolation::Proven` only for the exactly verified runtime version 1.1.0. Other parseable Apple 1.x versions remain `Unsupported` for offline isolation; absent, duplicate, malformed, or unsupported-major version evidence returns an error and cannot promote the capability. `offline_network_args` still rejects before invoking mount construction unless passed `NetworkIsolation::Proven`; its only proven-form output is the literal pair `--network`, `none`.

The test creates a cryptographically unique `gascan-<128-bit-lowercase-hex>.test` host mapping with the literal command `sudo -n container system dns create --localhost 203.0.113.113 <domain>`. This is a temporary **global host DNS/PF mutation**: Apple documents that creating a localhost domain disables iCloud Private Relay, and its PF redirect is removed on restart. Run the ignored test only on a dedicated controller where non-interactive administrative access and temporary Private Relay disruption are acceptable.

Ownership is fail-closed despite Apple 1.1 listing only domain names: the harness proves the generated domain is absent before creation, rejects collisions, proves exactly one identical domain exists afterward, and re-lists it immediately before an exact-name delete. It never deletes an unfamiliar or ambiguous name. On ordinary and error-path teardown, deletion uses literal argv without a shell; a bounded drop fallback applies the same exact-domain identity check. A normal owned networked container must reach the temporary host server through this domain, and the offline container reuses the identical URL before and after guest-root mutation. Any ownership mismatch is reported and deliberately retained for manual inspection.

The initial password, Alpine BusyBox HTTPS/TLS, Docker-only alias, and default-network gateway failures were fixture or setup failures and are not isolation evidence. Public DNS plus HTTP and direct external IPv4 negatives remain corroborating but non-discriminating on this host because its default-network Apple containers had no usable WAN path. The discriminating proof is structural empty attachment plus denial of the identical owned host path that normal attachment reached.

## Passing test mapping and limitations

```sh
sudo -v
cargo test -p gascan-apple --test live -- --ignored --nocapture --test-threads=1 offline_workspace_cannot_reach_external_or_host_networks
sudo -n container system dns list --format json
```

This proof is limited to Apple container 1.1.0 on the environment and image digests recorded above, using the exact `--network none` mechanism. The test temporarily mutates global DNS/PF state, requires cached administrator authentication, can disable iCloud Private Relay, and must end with an empty structured DNS list. It does not promote later Apple versions by inference. Gate 2 is **PASS** for this exact verified capability; the broader ignored live suite remains a separate controller action.
