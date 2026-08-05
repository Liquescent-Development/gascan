# Arca internal IPv4/CIDR type — design

Date: 2026-08-05
Roadmap step: **P1.4** (`docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md`,
"P1.4 — the pin is not cold-buildable").
Repository touched: `Vas-Solutus/arca`. Gas Can changes are limited to
`engine/arca-pin.json`.

Every claim below is marked **VERIFIED** or **PLAN**. A PLAN is never promoted
without running something. Past-tense claims carry their anchor inline.

## 1. Why

Arca's `Package.resolved` at tag `gascan-engine-baseline` pins commits that no
longer exist upstream, so the pinned tree is not cold-buildable. Gas Can PR #44's
engine-pin CI gate is red for this reason. Full measurement is in the roadmap
section named above; it is not repeated here.

The fix is to delete the dependency rather than re-pin it. **Decision taken
2026-08-05 and not re-opened by this design:** a bump to `swift-ip` 0.3.10 would
turn the gate green (VERIFIED, exit 0 with isolated SwiftPM state) and was
rejected because it re-enters the same lottery with an ecosystem that rewrote
history and migrated org mid-`0.3.x`.

### 1.1 The dependency is pure baggage

**VERIFIED** — `~/code/arca/.build/checkouts/swift-ip/Package.swift:38` declares
`.target(name: "IP")` with **no dependency list**. The `IP` module Arca imports
has zero dependencies. All six pins P1.4 removes — `swift-ip`, `swift-bson`,
`swift-json`, `swift-grammar`, `swift-hash`, `swift-unixtime` — exist to serve
the `IP_BSON`, `IPinfo` and `Firewalls` targets, which Arca never references.

**VERIFIED** — an independent scratch package depending only on
`.product(name: "IP", package: "swift-ip")` fails to resolve with four
fingerprint-mismatch errors (`swift-bson`, `swift-grammar`, `swift-hash`,
`swift-json`), confirming the breakage is in swift-ip's own transitive graph and
not an artifact of Arca's `Package.resolved`.

### 1.2 Blast radius

**VERIFIED** by `grep -rn "^import IP" Sources/ Tests/` and
`grep -rno "IP\.V4\|IP\.Block\|IP\.Address\|IP\.V6" Sources/ Tests/` in
`~/code/arca`:

| File | `import IP` | Symbol references |
|---|---|---|
| `Sources/ContainerBridge/StateStore.swift` | line 5 | 4 |
| `Sources/ContainerBridge/WireGuardNetworkBackend.swift` | line 4 | 21 |

Nothing in `Tests/`, `Sources/DockerAPI/`, `Sources/ArcaDaemon/`,
`Sources/Arca/` or `Sources/ArcaTestHelper/` references the module.

## 2. The behavior being replaced

**VERIFIED** by compiling `swift-ip`'s `Sources/IP/*.swift` directly with
`swiftc -swift-version 6 -package-name ipx` against a characterization program,
exit code 0, at `swift-ip` revision `ba4efb6457f69f5f483094aa1230e8e76cc4999c`
(the revision `Package.resolved:207` pins for `exact: "0.3.3"`).

Raw observed output:

```
BLOCK 172.18.0.0/16 -> base=172.18.0.0 bits=16 lower=172.18.0.0 upper=172.18.255.255
BLOCK 172.18.0.5/16 -> base=172.18.0.0 bits=16 lower=172.18.0.0 upper=172.18.255.255
BLOCK 10.0.0.0/8    -> base=10.0.0.0   bits=8  lower=10.0.0.0   upper=10.255.255.255
BLOCK 192.168.1.0/24-> base=192.168.1.0 bits=24 lower=192.168.1.0 upper=192.168.1.255
BLOCK 1.2.3.4/32    -> base=1.2.3.4    bits=32 lower=1.2.3.4    upper=1.2.3.4
BLOCK 0.0.0.0/0     -> base=0.0.0.0    bits=0  lower=0.0.0.0    upper=255.255.255.255
BLOCK 172.18.0.0/33 -> nil
BLOCK 172.18.0.0    -> nil
BLOCK bogus/16      -> nil
ADDR "010.1.1.1"    -> 10.1.1.1
ADDR "+1.2.3.4"     -> 1.2.3.4
ADDR "1.2.3.-0"     -> 1.2.3.0
ADDR "1.2.3"        -> nil
ADDR "1.2.3.4.5"    -> nil
ADDR "1.2.3.256"    -> nil
ADDR " 1.2.3.4"     -> nil
ADDR "1.2.3.4 "     -> nil
ADDR ""             -> nil
```

Behaviors the replacement must reproduce:

- `Block.init?(String)` **masks its base**: `172.18.0.5/16` yields base
  `172.18.0.0` (`IP.Block.swift:77`, `self = base / bits`, where `/` is
  `zeroMasked` at `IP.Address.swift:92`).
- `Block.range` is `base ... base.onesMasked(to: bits)` (`IP.Block.swift:53`) —
  a `ClosedRange` whose upper bound is the **broadcast address**.
- `Block.contains(ip)` is `base == ip.zeroMasked(to: bits)`
  (`IP.Block.swift:50`).
- `Block.init?` rejects `bits > 32` (`IP.Block.swift:71`).
- `V4.init?(String)` requires exactly four `.`-separated components, each parsed
  by `UInt8.init(_:)`, which accepts a leading `+`/`-` and leading zeros and
  rejects surrounding whitespace (`IP.V4.swift:52-92`).
- `V4.value` is the logical host-order value, high byte first
  (`IP.Address.swift:44`); `V4.init(value:)` is its inverse
  (`IP.Address.swift:35`).

### 2.1 A defect this surfaced — not fixed here

**VERIFIED.** `Sources/ContainerBridge/WireGuardNetworkBackend.swift:1034` reads:

```swift
rangeEnd = block.range.upperBound  // This is broadcast - 1 already
```

The comment is wrong. `range.upperBound` is the broadcast address itself, as the
`172.18.0.0/16 -> upper=172.18.255.255` line above shows. The same value reaches
`stateStore.allocateAndReserveIP` via
`WireGuardNetworkBackend.swift:397`, and the allocator's loop is inclusive of
`rangeEnd`, so Arca can allocate a subnet's broadcast address to a container.

**Pre-existing, unrelated to swift-ip's removal, and deliberately not fixed in
P1.4.** It is filed as an Arca issue instead, following the precedent set by
`Vas-Solutus/arca#47` and `#48` during P0.4. Fixing it here would forfeit the
equivalence argument in §4 — a differential test cannot assert identity across a
deliberate behavior change. Practically it requires ~65,533 containers on a `/16`
to reach, but it is reachable immediately on a `/30`.

The replacement therefore **reproduces this behavior exactly**, and the committed
test asserting `upperBound == 172.18.255.255` carries a comment saying so, so
that a future reader does not mistake it for an endorsement.

## 3. Design

### 3.1 Placement — a new leaf target

`Sources/ArcaIP/`, a SwiftPM target with **zero dependencies**, added to
`Package.swift` and depended on by `ContainerBridge`.

Rejected: placing the type in `Sources/ContainerBridge/`. Testing it there drags
in Containerization, gRPC and SQLite, and P4.3's ContainerBridge split would have
to relocate it anyway. A dependency-free leaf target tests in isolation with no
VM and no entitlements, and it is exactly the shape P4.3 needs to produce.

The zero-dependency property is the target's principal asset. The name `ArcaIP`
is chosen over a broader `ArcaNetworking` because a broad name invites WireGuard
and port-map code, which would pull in Containerization and destroy that
property.

### 3.2 API

The module vends `public enum IP` as a namespace, so call sites continue to read
`IP.V4` and `IP.Block`.

```swift
public enum IP {}

extension IP {
  public struct V4: Hashable, Comparable, Sendable,
                    CustomStringConvertible, LosslessStringConvertible {
    public var value: UInt32                          // high byte = first octet
    public init(value: UInt32)
    public init(_ a: UInt8, _ b: UInt8, _ c: UInt8, _ d: UInt8)
    public init?(_ description: some StringProtocol)
    public var description: String                    // dotted decimal
  }

  public struct Block: Hashable, Sendable,
                       CustomStringConvertible, LosslessStringConvertible {
    public let base: V4                               // masked to `bits` on init
    public let bits: UInt8
    public init(base: V4, bits: UInt8)
    public init?(_ string: some StringProtocol)
    public var range: ClosedRange<V4>                 // base ... broadcast
    public func contains(_ address: V4) -> Bool
    public var description: String                    // "base/bits"
  }
}
```

Two departures from swift-ip's internals, neither a behavior change:

**Store `value: UInt32` in host order, not big-endian `storage`.** Arca never
reads `.storage` (VERIFIED — it appears zero times in `Sources/`). Dropping it
removes the byte swapping, the `unsafeBitCast` at `IP.V4.swift:28` and the
`withUnsafeBytes` at `IP.V4.swift:45`. `description` becomes shifts and masks of
the logical value, which is endian-independent; swift-ip's is endian-*dependent*
by its own doc comment at `IP.V4.swift:9-11`. Output is identical on every
platform Arca supports — `Package.swift:9` declares `.macOS("26.0")` and no
big-endian Apple platform exists.

**`IP.Block` is not generic.** swift-ip's `Block<Base: Address>` is instantiated
by Arca only ever at `IP.V4`. A protocol with one conformer is speculative
generality; the `enum IP` namespace is the extension seam if IPv6 arrives.

### 3.3 Call-site churn

8 lines total:

- 2 `import IP` → `import ArcaIP`.
- 6 occurrences of `IP.Block<IP.V4>(` → `IP.Block(`. **VERIFIED** at
  `WireGuardNetworkBackend.swift:382,392,1010,1027,1073,1115` — all six are in
  that one file; `StateStore.swift` constructs no blocks.

The 25 matches counted in §1.2 are grep hits, not distinct constructs: each
`IP.Block<IP.V4>` hits twice, once for `IP.Block` and once for `IP.V4`. So the
six block constructions account for 12 of the 25, and the remaining **13**
matches are standalone `IP.V4` uses — `IP.V4(String)`, `IP.V4(value:)`, and the
tuple annotation at `WireGuardNetworkBackend.swift:1025` — which are
source-compatible and change zero characters. Member access (`.value`, `.base`,
`.range.lowerBound`, `.range.upperBound`, `.contains`, `String(describing:)`,
`!=`) is unchanged throughout and is not counted by that grep at all.

## 4. Verification

### 4.1 Layer 1 — differential harness (development-time, not committed)

Because the `IP` target has no dependencies (§1.1), its sources compile directly
with `swiftc` even though SwiftPM cannot resolve the package. The harness
compiles swift-ip's `Sources/IP/*.swift` as module `SwiftIPRef` and the new
target as `ArcaIP`, imports both, and runs identical vectors through each,
disambiguating the two `enum IP` declarations by module prefix.

| Vectors | Count | Asserts identical |
|---|---|---|
| Random `UInt32`, seeded PRNG, fixed seed | 5,000,000 | `description`; reparse of that `description` |
| All 33 prefix lengths × random bases | 660,000 | `base`, `range.lowerBound`, `range.upperBound`, block `description` |
| 8 probe addresses per block | 5,280,000 | `contains` |
| Hand-written malformed and edge strings | ~60 | `nil` vs value, on both types |

Seeded rather than random so the run is reproducible. Any single mismatch fails
it. **PLAN** until run; its exit code and mismatch count are recorded in the
implementation report.

**This harness is not committed.** It depends on a swift-ip checkout that exists
only in a warm SwiftPM cache and that P1.4 deletes. The §4.2 goldens are the
durable artifact; the harness is the evidence that the goldens are correct.

### 4.2 Layer 2 — committed tests

`Tests/ArcaIPTests/`, swift-testing (`import Testing`, matching
`Tests/ArcaTests/`). Golden vectors whose expected values are taken from the
Layer 1 run, including the behaviors §2 locked in: `010.1.1.1` → `10.1.1.1`,
`+1.2.3.4` → `1.2.3.4`, `1.2.3.-0` → `1.2.3.0`, base masking, `/0` and `/32`
boundaries, `bits > 32` rejection, and `172.18.0.0/16`'s `range.upperBound`
being the broadcast address with the §2.1 comment attached.

These require no VM and no entitlements.

### 4.3 Layer 3 — cold build and functional pass

- **Cold build.** Fresh clone of the new tag, isolated `HOME` plus
  `--cache-path` and `--scratch-path`. A warm cache hides exactly the class of
  failure P1.4 exists to fix, so a local green build proves nothing here.
- **Exit codes captured directly** — redirected to a file and read, never through
  a pipe. `cmd | tail` returns `tail`'s status; this produced four false
  "exit code 0" reports across two prior sessions.
- **`swift package clean`** before trusting any build error count.
- **Existing suite.** `swift test`, with attention to
  `Tests/ArcaTests/NetworkIPAMTests.swift`.
- **Functional pass.** Boot a container and confirm addressing, gateway and
  subnet containment still behave. Explicit unique `--name` per
  `Vas-Solutus/arca#47`: `docker run --rm` does not remove, and generated names
  collide against a 36-name pool.

## 5. Sequence

1. Create `Sources/ArcaIP/`, `Package.swift` target and test target wiring.
2. Implement the type; write the Layer 2 tests.
3. Run the Layer 1 differential harness; record exit code and mismatch count.
4. Switch the two call sites; delete the `swift-ip` dependency from
   `Package.swift` and regenerate `Package.resolved`.
5. `swift package clean`, build, `swift test`, functional pass.
6. Cold-build verification from a fresh clone.
7. PR to Arca `main`. **Merge commit, never squash** — Gas Can pins Arca by
   commit and these documents cite its SHAs.

   **VERIFIED 2026-08-05** by `gh api repos/Vas-Solutus/arca/rulesets/10300321`,
   ruleset "main protection". The state differs from the handoff's record and is
   now stricter in the direction this work wants:

   | Parameter | Value | Consequence |
   |---|---|---|
   | `allowed_merge_methods` | `["merge"]` | Squash is no longer *possible*. The handoff records `["merge","squash"]` set on 2026-08-05; it is merge-only now. The trap is enforced by the ruleset rather than by discipline. |
   | `required_approving_review_count` | `0` | `--admin` should not be needed, unlike `#46`. |
   | `require_code_owner_review` | `true` | Inert — **VERIFIED** no `CODEOWNERS` file exists at `.github/`, repo root, or `docs/`. |
   | `require_last_push_approval` | `true` | Untested against a 0-count PR; if it blocks, that is the moment `--admin` becomes necessary. |
   | `required_review_thread_resolution` | `true` | Any review thread opened must be resolved before merge. |
   | `required_signatures` | present | The tag and commits must be signed. |
8. New signed annotated tag.
9. Bump `engine/arca-pin.json` in Gas Can on branch `arca-integration`.
10. PR #44's engine-pin gate goes green. **That green gate is the completion
    signal**, and it is the only one — a local build cannot substitute for it.

## 6. Out of scope

- Fixing the §2.1 broadcast defect. Filed as an Arca issue.
- Tightening the parser's leniency. Reproduced deliberately; existing SQLite
  rows were written by the lenient parser, so a stricter reader risks failing to
  load real installed state.
- Preserving the vanished `swift-grammar` / `swift-hash` objects. Local copies
  exist at `~/code/vendor-mirrors/`; an earlier session overstated this as urgent
  and corrected itself the same day. P1.4 removes both packages from the graph.
- P1's two open non-blocking items: the untested `--prune --prune-tags` path and
  exit 75's absence from the documented taxonomy.
