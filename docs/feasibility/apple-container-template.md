# Apple container feasibility evidence

Status: `BLOCKED | PROVEN`

## Tested environment

- macOS: `<sw_vers -productVersion>`
- Architecture: `<uname -m>`
- Apple container application: `<container system version --format json>`
- Guest image: `docker.io/library/alpine:3.20`
- Resolved image digest: `<observed digest>`
- Test date: `<UTC timestamp>`

## Capability matrix

| Capability | Result | Passing test / exact evidence |
|---|---|---|
| Supported Apple 1.x version | `<result>` | `probe::accepts_supported_major_and_rejects_future_major` |
| Bind mounts | `<result>` | `storage::bind_mount_is_exact_and_named_volume_persists` |
| Named volumes | `<result>` | `storage::bind_mount_is_exact_and_named_volume_persists` |
| TTY | `<result>` | `<Task 5 live test>` |
| Signals | `<result>` | `<Task 5 live test and limitations>` |
| Loopback publish | `<result>` | `resources::published_port_is_reachable_only_through_loopback_binding` |
| CPU/memory limits | `<result>` | `resources::cpu_and_memory_limits_are_observable_in_guest` |
| Hard offline isolation | `<result>` | `network::offline_workspace_cannot_reach_external_or_host_networks` |

## Exact Apple 1.1 command forms

Record literal argv for create/run, inspect, stop/start/delete, volume lifecycle, attach helper, and offline networking. Offline must be `--network none`; DNS suppression is not evidence of isolation.

## Adversarial offline observations

Record structured empty attachment evidence and the result before and after guest-root route/interface mutation for:

- `http://example.com` (DNS plus external HTTP using BusyBox-supported behavior);
- `http://1.1.1.1` (direct external IPv4, independent of DNS);
- `http://192.0.2.1` (non-routable adversarial IPv4);
- the owned host probe reached through the unique temporary `gascan-<128-bit-hex>.test` mapping created by `sudo -n container system dns create --localhost 203.0.113.113 <domain>`.

The default-network control must reach the exact owned host endpoint used by the offline container. Record public DNS plus HTTP and direct external IPv4 reachability as diagnostics, not prerequisite positive controls: their offline failures corroborate isolation but are non-discriminating when the host's Apple containers have no usable WAN path.

## Cleanup

List every owned resource prefix/token and confirm structured container, volume, network, and `gascan-*.test` DNS lists contain none after cleanup. Record that this mapping is a global DNS/PF mutation, requires administrative access, can disable iCloud Private Relay, and has a PF redirect that Apple removes on restart.

## Decision

Do not select `PROVEN` until every mandatory row has observed passing live evidence. Any failure leaves the report `BLOCKED` and prevents Plans 3–4 integration.
