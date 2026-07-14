# Apple container feasibility evidence

Status: **BLOCKED — administrator authentication and offline live proof pending**

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

## Current capability decision

`AppleProbe::base_capabilities()` reports `NetworkIsolation::Unsupported` fail-closed. `offline_network_args` rejects before invoking mount construction unless passed `NetworkIsolation::Proven`; its only proven-form output is the literal pair `--network`, `none`.

Promotion to `Proven` is forbidden until the controller runs the ignored adversarial test and records that structured attachments are empty and all DNS plus external HTTP, direct external IPv4, TEST-NET, and owned host-probe requests fail both before and after guest-root route/interface mutation.

The test creates a cryptographically unique `gascan-<128-bit-lowercase-hex>.test` host mapping with the literal command `sudo -n container system dns create --localhost 203.0.113.113 <domain>`. This is a temporary **global host DNS/PF mutation**: Apple documents that creating a localhost domain disables iCloud Private Relay, and its PF redirect is removed on restart. Run the ignored test only on a dedicated controller where non-interactive administrative access and temporary Private Relay disruption are acceptable.

Ownership is fail-closed despite Apple 1.1 listing only domain names: the harness proves the generated domain is absent before creation, rejects collisions, proves exactly one identical domain exists afterward, and re-lists it immediately before an exact-name delete. It never deletes an unfamiliar or ambiguous name. On ordinary and error-path teardown, deletion uses literal argv without a shell; a bounded drop fallback applies the same exact-domain identity check. A normal owned networked container must reach the temporary host server through this domain, and the offline container reuses the identical URL before and after guest-root mutation. Any ownership mismatch is reported and deliberately retained for manual inspection.

The first controller attempt stopped safely before mutation because `sudo -n` required a password. After authentication, mapping creation and control-container startup succeeded, but the first positive control failed at `https://example.com`: Alpine BusyBox `wget` TLS/CA support is not a valid generic connectivity oracle. The fixture now uses `http://example.com` for the combined DNS and external-HTTP control while retaining direct external IPv4, TEST-NET, and the owned host mapping. Each positive-control failure identifies its mechanism. Earlier failures involving the Docker-only alias and default-network gateway were also test-fixture failures; those approaches have been removed. None of these failures is evidence against Apple offline isolation. The remaining blocker is a passing focused adversarial test. Network isolation remains `Unsupported`, not `Proven`.

## Controller commands required to resolve BLOCKED

```sh
sudo -v
cargo test -p gascan-apple --test live -- --ignored --test-threads=1 offline_workspace_cannot_reach_external_or_host_networks
sudo -n container system dns list --format json
```

Run `sudo -v` interactively immediately before the focused test so its literal `sudo -n container system dns list/create/delete` commands can use the cached credential. Afterward, the final command must return a structured list containing no test-owned `gascan-*.test` mapping. Until the focused test passes and its observations and cleanup are recorded, Gate 2 remains **BLOCKED**.
