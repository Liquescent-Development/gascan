# Apple container feasibility evidence

Status: **PASS — hard offline isolation proven for Apple container 1.1.0**

## Environment and established evidence

- Test date: 2026-07-14 UTC.
- Controller preflight: **PASS** on macOS 26.5.1 (`arm64`).
- Apple `container` and `container-apiserver`: 1.1.0, commit `5973b9cc626a3e7a499bb316a958237ebe14e2ed`.
- Guest image: `docker.io/library/alpine:3.20`; index digest `sha256:d9e853e87e55526f6b2917df91a2115c36dd7c696a35be12163d44e6e2a4b6bc`; resolved arm64 variant digest `sha256:45e09956dc667c5eff3583c9d94830261fb1ca0be10a0a7db36266edf5de9e1d`.
- Final serialized live suite: **PASS, 9/9 in 50.31 seconds**.

## Capability matrix

| Runtime capability | Result | Exact passing test / evidence |
|---|---|---|
| Runtime version | Apple 1.1.0 supported | `probe::accepts_supported_major_and_rejects_future_major`; exact structured `container` entry |
| `bind_mounts` | verified | `storage::bind_mount_is_exact_and_named_volume_persists` |
| `named_volumes` | verified | `storage::bind_mount_is_exact_and_named_volume_persists` |
| `tty` | verified | `attach::attached_process_reports_resize_signal_and_exit` |
| `signals` | verified with limitation | TTY SIGINT passes in `attach::attached_process_reports_resize_signal_and_exit`; `attach::unsupported_signal_matrix_returns_promptly` proves other combinations fail promptly |
| `loopback_publish` | verified | `resources::published_port_is_reachable_only_through_loopback_binding` |
| `resource_limits` | verified | `resources::cpu_and_memory_limits_are_observable_in_guest` |
| `offline` | `NetworkIsolation::Proven` only on 1.1.0 | `network::offline_workspace_cannot_reach_external_or_host_networks`; exact `--network none` |

The live evidence verifies all rows. In the current conservative `base_capabilities` object, offline is promoted only for exact 1.1.0; unrelated boolean integrations remain separate implementation work and are not inferred merely from this report.

## Offline mechanism source evidence

Pinned Apple `container` tag 1.1.0 defines `NetworkClient.noNetworkName = "none"` as the reserved name for no network attachment. `Utility.makeContainerConfiguration` accepts `none` only as the sole network selection and sets `config.networks = []`; otherwise an omitted network attaches the built-in default. Network creation rejects the reserved name. Therefore the only Apple 1.1 hard no-attachment CLI form is:

```text
container run ... --network none ...
```

`container network create --internal` is only host-only networking and is not offline isolation. DNS suppression is explicitly rejected as evidence.

## Exact observed Apple 1.1 command forms

All forms below are literal argv; placeholders denote the unique owned names, token, canonical path, or reserved port observed per test:

```text
container system version --format json
container volume create --label dev.gascan.test=true --label dev.gascan.test.owner=<128-bit-hex> -s 104857600 <volume>
container volume list --format json
container volume delete <volume>
container run --name <container> --label dev.gascan.test=true --label dev.gascan.test.owner=<128-bit-hex> --mount type=bind,source=<canonical-workspace>,target=/workspace --volume <volume>:/opt/gascan --cpus 1 --memory 268435456 --init --detach [--publish 127.0.0.1:<host-port>:8080] [--network none] docker.io/library/alpine:3.20 sh -c <guest-script>
container inspect <container>
container list --all --format json
container exec <container> sh -c <command>
container stop --time 5 <container>
container start <container>
container delete <container>
sudo -n container system dns list --format json
sudo -n container system dns create --localhost 203.0.113.113 <gascan-128-bit-hex.test>
sudo -n container system dns delete <gascan-128-bit-hex.test>
```

The attach helper is executed directly as `${GASCAN_APPLE_ATTACH_HELPER}` or `target/gascan-apple-attach` with no shell and no argv. The controller sends a versioned start frame containing the exact container name, guest argv, and TTY boolean; subsequent framed messages carry binary stdin, resize, supported signal/control, close, stdout/stderr, typed errors, and exact exit status. Published ports use only `--publish 127.0.0.1:<host-port>:8080`. Resource requests use only `--cpus 1 --memory 268435456`. Hard offline construction adds only `--network none`.

## Verified offline evidence and capability decision

The controller ran `network::offline_workspace_cannot_reach_external_or_host_networks` alone with serialized ignored-test execution. It passed 1/1 in 36.19 seconds. The default-network container reached the owned DNS/PF host service, proving the ordinary VM-to-host path. Public `http://example.com` and direct `http://1.1.1.1` were both unreachable from that control on this host and were recorded as non-discriminating diagnostics. The offline container denied all four target roles—including the exact owned host endpoint proven reachable—with structured `configuration.networks=[]` both before and after guest-root route/interface mutation. The harness now prints the link-add and route-add exit codes plus the complete post-mutation `ip link show` and `ip route show` state, so the controller rerun can retain direct evidence of the attempted guest-root mutations.

`AppleProbe::base_capabilities()` now reports `NetworkIsolation::Proven` only for the exactly verified runtime version 1.1.0. Other parseable Apple 1.x versions remain `Unsupported` for offline isolation; absent, duplicate, malformed, or unsupported-major version evidence returns an error and cannot promote the capability. `offline_network_args` still rejects before invoking mount construction unless passed `NetworkIsolation::Proven`; its only proven-form output is the literal pair `--network`, `none`.

The test creates a cryptographically unique `gascan-<128-bit-lowercase-hex>.test` host mapping with the literal command `sudo -n container system dns create --localhost 203.0.113.113 <domain>`. This is a temporary **global host DNS/PF mutation**: Apple documents that creating a localhost domain disables iCloud Private Relay, and its PF redirect is removed on restart. Run the ignored test only on a dedicated controller where non-interactive administrative access and temporary Private Relay disruption are acceptable.

Ownership is fail-closed despite Apple 1.1 listing only domain names: the harness proves the generated domain is absent before creation and rejects collisions. It installs a pending guard before the privileged create, then reconciles exact structured presence after success, failure, or timeout. Ambiguous or present failure state retains the guard for explicit and bounded Drop cleanup. Delete success is not trusted: the harness re-lists and clears pending state only after structured absence. It never deletes an unfamiliar or ambiguous name. A normal owned networked container must reach the temporary host server through this domain, and the offline container reuses the identical URL before and after guest-root mutation.

The initial password, Alpine BusyBox HTTPS/TLS, Docker-only alias, and default-network gateway failures were fixture or setup failures and are not isolation evidence. Public DNS plus HTTP and direct external IPv4 negatives remain corroborating but non-discriminating on this host because its default-network Apple containers had no usable WAN path. The discriminating proof is structural empty attachment plus denial of the identical owned host path that normal attachment reached.

## Cleanup and limitations

```sh
sudo -v
cargo test -p gascan-apple --test live -- --ignored --nocapture --test-threads=1 offline_workspace_cannot_reach_external_or_host_networks
sudo -n container system dns list --format json
```

The final full live run passed 9/9 in 50.31 seconds. Controller cleanup verification returned the structured empty list `[]`, including `sudo -n container system dns list --format json` returning exactly `[]`; no test-owned DNS mapping remained.

This proof is limited to Apple container 1.1.0 on the environment and image digests recorded above, using the exact `--network none` mechanism. The test temporarily mutates global DNS/PF state, requires cached administrator authentication, can disable iCloud Private Relay, and must end with an empty structured DNS list. It does not promote later Apple versions by inference. Gate 2 is **PASS** for this exact verified capability.
