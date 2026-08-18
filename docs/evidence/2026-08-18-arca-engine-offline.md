# Arca engine offline isolation — REFUTED, 2026-08-18

**The offline proof was run against the pinned engine and it failed. An
`offline` sandbox on this engine has full network egress.**

`CERTIFIED_ENGINE_REVISION` (`crates/gascan-arca/src/translate.rs`) therefore
stays `None`, and `Sources/ArcaEngine/SandboxEngineService.swift`'s
`capabilities.offline` stays `.unverified`. There is no engine build to certify.
This document is the recorded observation Task 15 required; what it licenses is
the refusal, not the claim.

## What was under test

| | |
|---|---|
| Arca revision | `c545612b056e028d5885968a7b9f586d694f994c`, the revision `engine/arca-pin.json` names under tag `gascan-engine-m4` |
| Engine binary | `.artifacts/arca-engine/arca/.build/release/arca-engine`, built by `scripts/build-arca-engine.sh` from that pin |
| Kernel | `~/Library/Application Support/dev.gascan/engine/vmlinux`, installed by `gascan engine fetch` |
| vminit | `~/Library/Application Support/dev.gascan/engine/vminit`, likewise |
| Base image | `/tmp/alpine-oci` (`alpine:3.20`, `linux/arm64`), plus a layer adding a `workspace` account |
| Host | macOS 26.6.1, arm64 |
| Gas Can revision | the working tree on `feat/milestone-4-product-wiring` at parent `cd3e2a5904600bf034e904316366717870434b7d`; the commit that adds this document is the one that adds the test |

## The command

```
GASCAN_ARCA_ENGINE_BIN=.../arca-engine \
GASCAN_ARCA_KERNEL_PATH="$HOME/Library/Application Support/dev.gascan/engine/vmlinux" \
GASCAN_ARCA_VMINIT_LAYOUT="$HOME/Library/Application Support/dev.gascan/engine/vminit" \
GASCAN_ARCA_BASE_OCI_LAYOUT=/tmp/alpine-oci \
cargo test -p gascan-arca --test live -- --ignored --test-threads=1 --nocapture network::
```

Test: `network::an_offline_sandbox_has_no_egress_at_either_privilege_level`.
Result: **FAILED**, `test result: FAILED. 0 passed; 1 failed`, in 9.68s.

## The positive control passed

Every probe was first run against a `network = 'networked'` sandbox built from
the same image, and every one succeeded, at both privilege levels. Six failures
from a guest whose tools are broken read exactly like six failures from a guest
with no network; the control is what separates them. The control's guest carried
`eth0 172.18.0.2/32` — the engine's per-sandbox WireGuard bridge.

## The confounder that was closed first

`Sandbox::boot` asserts that the compiled `CreateRequest` carries
`RuntimeNetwork::Offline` before anything is observed. The manifest is three
translations away from the wire, and a request that had silently compiled to
`Networked` would produce exactly the reachability reported below. That
assertion passed.

## What the offline guest had

```
1: lo    inet 127.0.0.1/8 scope host lo
1: lo    inet6 ::1/128 scope host
2: eth0  inet 192.168.71.2/24 scope global eth0
2: eth0  inet6 fdd7:5351:9cb6:8b4f::2/64 scope global flags 02
2: eth0  inet6 fe80::140b:28ff:fe29:43c5/64 scope link tentative
```

```
default via 192.168.71.1 dev eth0
192.168.71.0/24 dev eth0 scope link  src 192.168.71.2
```

```
nameserver 192.168.71.1
```

A vmnet interface, a default route through it, and a resolver on its gateway.
Parent design §2.1 defines offline as **no network attachment at all — no
vmnet, no WireGuard**. The subnet is auto-allocated per run and was
`192.168.69.0/24`, `192.168.70.0/24` and `192.168.71.0/24` across three runs;
the finding is the same each time.

## The thirteen violations

```
an offline sandbox carried non-loopback interfaces ["eth0", "teql0", "tunl0", "sit0", "ip6tnl0"]
an offline sandbox reached a test-owned host endpoint as guest root
an offline sandbox reached a test-owned host endpoint as the sandbox user
an offline sandbox reached a public IP as guest root
an offline sandbox reached a public IP as the sandbox user
an offline sandbox reached public DNS as guest root
an offline sandbox reached public DNS as the sandbox user
after a guest-root mutation, an offline sandbox reached a test-owned host endpoint as guest root
after a guest-root mutation, an offline sandbox reached a test-owned host endpoint as the sandbox user
after a guest-root mutation, an offline sandbox reached a public IP as guest root
after a guest-root mutation, an offline sandbox reached a public IP as the sandbox user
after a guest-root mutation, an offline sandbox reached public DNS as guest root
after a guest-root mutation, an offline sandbox reached public DNS as the sandbox user
```

The three mechanisms are the ones `packaging/macos/release-smoke.sh:1015-1037`
asserts, each as the sandbox user and again as guest root — the shape Task 15
named. `nslookup example.com` resolved through `192.168.71.1` to real addresses
(`104.20.23.154`, `172.66.147.243`, and two `2606:4700:10::` v6 addresses), and
`wget http://1.1.1.1/` completed. This is not a host-only network; it is
internet egress.

## What this does not say

- **Nothing about egress policy, peer channels or packet filtering.** Those are
  P6's, and `Capabilities` fields 10-19 stay reserved.
- **Nothing about a leak reaching a user through the product.** It has not:
  `PolicyCompiler::validate_capabilities` (`crates/gascan-core/src/policy.rs`)
  refuses to compile an offline sandbox unless `capabilities.offline` is
  `Proven`, and the revision gate has kept every engine at `Unverified` since
  Task 10. MEASURED end to end — `gascan up` on an offline manifest, against a
  real `gascand` on a real engine, is refused, and
  `gascan-e2e`'s `an_offline_manifest_is_refused_because_no_engine_build_is_certified`
  is the standing instrument for it. **The fail-closed default is what this
  evidence vindicates.**
- **Nothing about whether the engine could implement offline.** Whether the
  attachment is deliberate, a default-network fallback, or an omission is Arca's
  to determine.

## What follows

1. `CERTIFIED_ENGINE_REVISION` stays `None`. Setting it would be the
   "claim with no instrument" defect inverted into something worse: a claim
   against an instrument that refuted it.
2. `capabilities.offline` stays `.unverified` in the engine. `.proven` would be
   a false statement, and Gas Can's gate would still refuse it — but the engine
   would be lying about itself, which is the shape design §2.3 forbids.
3. **No new signed tag, no new release and no pin bump are needed for this
   milestone.** Reaching `Proven` requires an engine that does not attach vmnet
   to an offline sandbox; that is Arca work, not a re-tag of the same tree.
4. `an_offline_sandbox_has_no_egress_at_either_privilege_level` is left in the
   live tier asserting the property, so it **fails today by design** and turns
   green on the build that earns the constant. It is the acceptance instrument
   for whoever does that work.
