# ArcaIP Internal IPv4/CIDR Type Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `swift-ip` with a zero-dependency internal IPv4/CIDR type in Arca, so the pinned engine commit becomes cold-buildable and Gas Can PR #44's engine-pin gate goes green.

**Architecture:** A new SwiftPM leaf target `ArcaIP` with no dependencies vends `public enum IP` containing `IP.V4` and a non-generic `IP.Block`. `ContainerBridge` depends on it. The type is a bit-for-bit behavioral clone of `swift-ip` 0.3.3, which is what makes a differential test against the real library possible. Removing it drops 6 of Arca's 38 pins.

**Tech Stack:** Swift 6.3.3 (`arm64-apple-macosx26.0`), SwiftPM, swift-testing (`import Testing`), `swiftc` for the differential harness.

**Spec:** `docs/superpowers/specs/2026-08-05-arca-internal-ip-type-design.md` (Gas Can, `arca-integration`, commit `cdd85b5`).

## Global Constraints

- **Repository for Tasks 1–6 is `~/code/arca`, not Gas Can.** Only Task 7 touches Gas Can.
- **Never commit to `main` in any repository.** Code reaches `main` via PR.
- **Never squash-merge Arca.** VERIFIED 2026-08-05: ruleset `10300321` sets `allowed_merge_methods: ["merge"]`, so squash is not possible — but the reason matters and must survive: Gas Can pins Arca by commit and these documents cite its SHAs.
- **`required_approving_review_count` is `0`** (VERIFIED same command), so `--admin` should not be needed. `require_last_push_approval: true` is untested against a 0-count PR; if merge blocks, report it rather than reaching for `--admin` silently.
- **Signing.** `git config user.signingkey` is `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHyTKmfAwcJcdfKXmj2h3mwfgPaelE6gSMrquAcPmW09`, `gpg.format=ssh`, `tag.gpgsign=true`. This key matches `gascan/engine/allowed-signers` for `richard@liquescent.dev`. The gate verifies the tag against that file; a tag signed by any other key fails with exit 65.
- **Capture exit codes directly, never through a pipe.** `cmd | tail` returns `tail`'s status. Redirect to a file and read `$?`, or use `|| rc=$?`. Four false "exit code 0" reports came from this across two prior sessions.
- **`swift package clean` before trusting any build error count.**
- **A warm SwiftPM cache hides cold-build failures.** This is the entire reason P1.4 exists. Any cold claim must isolate `HOME` plus `--cache-path`/`--scratch-path`.
- **Behavioral fidelity is the acceptance criterion.** Do not "improve" parsing strictness or the broadcast range. Both are reproduced deliberately; see spec §2.1 and §6.
- **Mark every claim VERIFIED or PLAN.** Never promote a PLAN without running something. Past-tense claims carry their anchor inline (command, SHA, file:line, exit code).

---

### Task 1: `IP.V4` — the address type

**Files:**
- Create: `Sources/ArcaIP/IP.swift`
- Create: `Sources/ArcaIP/IP.V4.swift`
- Create: `Tests/ArcaIPTests/V4Tests.swift`
- Modify: `Package.swift` (add `ArcaIP` target and `ArcaIPTests` test target)

**Interfaces:**
- Consumes: nothing.
- Produces: `public enum IP` namespace. `IP.V4` with `public var value: UInt32`, `public init(value: UInt32)`, `public init(_ a: UInt8, _ b: UInt8, _ c: UInt8, _ d: UInt8)`, `public init?(_ description: some StringProtocol)`, `public var description: String`. Conforms to `Hashable`, `Comparable`, `Sendable`, `CustomStringConvertible`, `LosslessStringConvertible`.

- [ ] **Step 1: Add the targets to `Package.swift`**

Add to the `targets:` array, after the `ContainerBridge` target:

```swift
        // Internal IPv4/CIDR types. Zero dependencies by design: this target
        // replaced swift-ip, whose transitive graph pinned commits that no
        // longer exist upstream and made the tree impossible to build cold.
        // Adding a dependency here would forfeit that property.
        .target(name: "ArcaIP"),

        .testTarget(
            name: "ArcaIPTests",
            dependencies: ["ArcaIP"]
        ),
```

Do **not** yet remove the `swift-ip` dependency or change `ContainerBridge` — that is Task 4, and keeping the old library resolvable is what makes Task 3's differential harness possible.

- [ ] **Step 2: Write the failing tests**

Create `Tests/ArcaIPTests/V4Tests.swift`:

```swift
import Testing

@testable import ArcaIP

@Suite("IP.V4")
struct V4Tests {
    @Test("parses dotted-decimal notation")
    func parsesDottedDecimal() {
        #expect(IP.V4("172.18.0.1")?.value == 0xAC12_0001)
        #expect(IP.V4("0.0.0.0")?.value == 0x0000_0000)
        #expect(IP.V4("255.255.255.255")?.value == 0xFFFF_FFFF)
        #expect(IP.V4("127.0.0.1")?.value == 0x7F00_0001)
    }

    @Test("rejects malformed input")
    func rejectsMalformed() {
        #expect(IP.V4("1.2.3") == nil)
        #expect(IP.V4("1.2.3.4.5") == nil)
        #expect(IP.V4("1.2.3.256") == nil)
        #expect(IP.V4(" 1.2.3.4") == nil)
        #expect(IP.V4("1.2.3.4 ") == nil)
        #expect(IP.V4("") == nil)
        #expect(IP.V4("bogus") == nil)
    }

    // These three are inherited leniency, not oversights. swift-ip 0.3.3 parsed
    // each octet with UInt8.init(_:), which accepts a leading sign and leading
    // zeros. Arca's SQLite rows were written by that parser, so tightening this
    // risks failing to load real installed state. See spec section 6.
    @Test("reproduces swift-ip's parser leniency deliberately")
    func reproducesLeniency() {
        #expect(IP.V4("010.1.1.1")?.value == 0x0A01_0101)
        #expect(IP.V4("+1.2.3.4")?.value == 0x0102_0304)
        #expect(IP.V4("1.2.3.-0")?.value == 0x0102_0300)
    }

    @Test("formats dotted-decimal notation")
    func formatsDottedDecimal() {
        #expect(String(describing: IP.V4(value: 0xAC12_0001)) == "172.18.0.1")
        #expect(String(describing: IP.V4(value: 0x0000_0000)) == "0.0.0.0")
        #expect(String(describing: IP.V4(value: 0xFFFF_FFFF)) == "255.255.255.255")
        #expect(String(describing: IP.V4(value: 0x7F00_0001)) == "127.0.0.1")
    }

    @Test("octet initialiser puts the first octet in the high byte")
    func octetInitialiser() {
        #expect(IP.V4(172, 18, 0, 1).value == 0xAC12_0001)
        #expect(String(describing: IP.V4(192, 168, 1, 254)) == "192.168.1.254")
    }

    @Test("round-trips value through description")
    func roundTrips() {
        for value: UInt32 in [0, 1, 0x0102_0304, 0xAC12_0001, 0x7F00_0001, .max] {
            let address = IP.V4(value: value)
            #expect(IP.V4(String(describing: address))?.value == value)
        }
    }

    @Test("orders by logical value")
    func orders() {
        #expect(IP.V4("10.0.0.1")! < IP.V4("10.0.0.2")!)
        #expect(IP.V4("9.255.255.255")! < IP.V4("10.0.0.0")!)
        #expect(IP.V4("1.2.3.4")! == IP.V4("1.2.3.4")!)
        #expect(IP.V4("1.2.3.4")! != IP.V4("1.2.3.5")!)
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cd ~/code/arca && swift test --filter ArcaIPTests > /tmp/t1.log 2>&1; echo "rc=$?"`

Expected: non-zero `rc`, with "cannot find 'IP' in scope" or "no such module 'ArcaIP'". Read `/tmp/t1.log` to confirm the failure is the missing type and not a `Package.swift` syntax error.

- [ ] **Step 4: Write the namespace**

Create `Sources/ArcaIP/IP.swift`:

```swift
/// Namespace for IP addressing types.
///
/// This target replaced `swift-ip` 0.3.3, which Arca used for exactly two
/// types. Its own `IP` target had no dependencies, but the package carried
/// five more that pinned commits which no longer exist upstream, so the
/// pinned Arca tree could not be built from a cold cache.
///
/// Behaviour is reproduced exactly rather than improved. The design and the
/// evidence live in the Gas Can repository at
/// `docs/superpowers/specs/2026-08-05-arca-internal-ip-type-design.md`.
public enum IP {}
```

- [ ] **Step 5: Write the address type**

Create `Sources/ArcaIP/IP.V4.swift`:

```swift
extension IP {
    /// An IPv4 address, which is 32 bits wide.
    public struct V4: Hashable, Sendable {
        /// The logical value of the address: the high byte is the first octet.
        ///
        /// `swift-ip` stored big-endian raw bytes and computed this property.
        /// Arca never read that raw storage, so this type stores the logical
        /// value directly. One consequence is that `description` here is
        /// endian-independent, where `swift-ip`'s was endian-*dependent* by its
        /// own documentation. Arca is macOS-only and macOS is little-endian
        /// everywhere, so the output is identical on every supported platform.
        public var value: UInt32

        /// Creates an address from its logical value.
        public init(value: UInt32) {
            self.value = value
        }

        /// Creates an address from its four octets, first octet first.
        public init(_ a: UInt8, _ b: UInt8, _ c: UInt8, _ d: UInt8) {
            self.value =
                UInt32(a) << 24 | UInt32(b) << 16 | UInt32(c) << 8 | UInt32(d)
        }
    }
}

extension IP.V4: Comparable {
    /// Compares two addresses by their logical value.
    public static func < (a: Self, b: Self) -> Bool { a.value < b.value }
}

extension IP.V4: CustomStringConvertible {
    /// Formats the address in dotted-decimal notation.
    public var description: String {
        """
        \(self.value >> 24)\
        .\(self.value >> 16 & 0xFF)\
        .\(self.value >> 8 & 0xFF)\
        .\(self.value & 0xFF)
        """
    }
}

extension IP.V4: LosslessStringConvertible {
    /// Parses an address in dotted-decimal notation.
    ///
    /// Requires exactly four `.`-separated fields, each parsed by
    /// `UInt8.init(_:)`. That parser accepts a leading `+` or `-` and leading
    /// zeros, so `010.1.1.1`, `+1.2.3.4` and `1.2.3.-0` all parse. This
    /// leniency is inherited from `swift-ip` 0.3.3 on purpose: rows already
    /// written to Arca's SQLite state came through that parser, so a stricter
    /// reader could fail to load real installed state.
    public init?(_ description: some StringProtocol) {
        guard
            let firstDot = description.firstIndex(of: "."),
            let a = UInt8(description[..<firstDot])
        else { return nil }

        let afterFirst = description.index(after: firstDot)
        guard
            let secondDot = description[afterFirst...].firstIndex(of: "."),
            let b = UInt8(description[afterFirst ..< secondDot])
        else { return nil }

        let afterSecond = description.index(after: secondDot)
        guard
            let thirdDot = description[afterSecond...].firstIndex(of: "."),
            let c = UInt8(description[afterSecond ..< thirdDot])
        else { return nil }

        let afterThird = description.index(after: thirdDot)
        guard let d = UInt8(description[afterThird...]) else { return nil }

        self.init(a, b, c, d)
    }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cd ~/code/arca && swift test --filter ArcaIPTests > /tmp/t1.log 2>&1; echo "rc=$?"`

Expected: `rc=0`. If the build fails elsewhere in the package, note that `swift test` builds every target; use `swift build --target ArcaIP` first to isolate.

- [ ] **Step 7: Commit**

```bash
cd ~/code/arca
git checkout -b fix/replace-swift-ip-with-internal-type
git add Package.swift Sources/ArcaIP/IP.swift Sources/ArcaIP/IP.V4.swift Tests/ArcaIPTests/V4Tests.swift
git commit -m "feat: add ArcaIP.IP.V4, a dependency-free IPv4 address type

First half of replacing swift-ip, whose transitive graph pins commits
that no longer exist upstream and makes a cold build impossible.

Stores the logical value rather than swift-ip's big-endian raw storage,
which Arca never read. Parser leniency is reproduced deliberately and
the test says so: existing SQLite rows were written by it."
```

---

### Task 2: `IP.Block` — the CIDR type

**Files:**
- Create: `Sources/ArcaIP/IP.Block.swift`
- Create: `Tests/ArcaIPTests/BlockTests.swift`

**Interfaces:**
- Consumes: `IP.V4` from Task 1 — `init(value: UInt32)`, `var value: UInt32`, `Comparable`.
- Produces: `IP.Block` with `public let base: IP.V4`, `public let bits: UInt8`, `public init(base: IP.V4, bits: UInt8)`, `public init?(_ string: some StringProtocol)`, `public var range: ClosedRange<IP.V4>`, `public func contains(_ address: IP.V4) -> Bool`, `public var description: String`. Note it is **not generic** — call sites write `IP.Block(...)`, not `IP.Block<IP.V4>(...)`.

- [ ] **Step 1: Write the failing tests**

Create `Tests/ArcaIPTests/BlockTests.swift`:

```swift
import Testing

@testable import ArcaIP

@Suite("IP.Block")
struct BlockTests {
    @Test("parses CIDR notation")
    func parsesCIDR() {
        let block = IP.Block("172.18.0.0/16")
        #expect(block?.base.value == 0xAC12_0000)
        #expect(block?.bits == 16)
        #expect(String(describing: block!) == "172.18.0.0/16")
    }

    @Test("masks the base address on construction")
    func masksBase() {
        // swift-ip's `/` operator zero-masks, so a non-canonical base is
        // silently canonicalised rather than rejected.
        #expect(IP.Block("172.18.0.5/16")?.base.value == 0xAC12_0000)
        #expect(String(describing: IP.Block("192.168.1.77/24")!) == "192.168.1.0/24")
        #expect(IP.Block(base: IP.V4("10.9.8.7")!, bits: 8).base.value == 0x0A00_0000)
    }

    @Test("rejects malformed input")
    func rejectsMalformed() {
        #expect(IP.Block("172.18.0.0/33") == nil)
        #expect(IP.Block("172.18.0.0") == nil)
        #expect(IP.Block("bogus/16") == nil)
        #expect(IP.Block("172.18.0.0/") == nil)
        #expect(IP.Block("/16") == nil)
        #expect(IP.Block("") == nil)
    }

    @Test("range lower bound is the network address")
    func rangeLowerBound() {
        #expect(IP.Block("172.18.0.0/16")!.range.lowerBound.value == 0xAC12_0000)
        #expect(IP.Block("10.0.0.0/8")!.range.lowerBound.value == 0x0A00_0000)
    }

    // The upper bound is the BROADCAST address, not broadcast - 1. This
    // reproduces swift-ip 0.3.3 exactly and is asserted here so the behaviour
    // cannot drift silently.
    //
    // It is not an endorsement. WireGuardNetworkBackend.swift comments that
    // this bound is "broadcast - 1 already"; that comment is wrong, and because
    // Arca's allocator treats the bound as inclusive it can hand a container
    // its subnet's broadcast address. Tracked as a separate Arca issue, and
    // deliberately not fixed here so this replacement stays behaviour-identical.
    @Test("range upper bound is the broadcast address, reproducing swift-ip")
    func rangeUpperBoundIsBroadcast() {
        #expect(IP.Block("172.18.0.0/16")!.range.upperBound.value == 0xAC12_FFFF)
        #expect(IP.Block("10.0.0.0/8")!.range.upperBound.value == 0x0AFF_FFFF)
        #expect(IP.Block("192.168.1.0/24")!.range.upperBound.value == 0xC0A8_01FF)
    }

    @Test("handles the /0 and /32 boundaries")
    func boundaries() {
        let all = IP.Block("0.0.0.0/0")!
        #expect(all.range.lowerBound.value == 0x0000_0000)
        #expect(all.range.upperBound.value == 0xFFFF_FFFF)
        #expect(all.contains(IP.V4("8.8.8.8")!))

        let host = IP.Block("1.2.3.4/32")!
        #expect(host.range.lowerBound.value == 0x0102_0304)
        #expect(host.range.upperBound.value == 0x0102_0304)
        #expect(host.contains(IP.V4("1.2.3.4")!))
        #expect(!host.contains(IP.V4("1.2.3.5")!))
    }

    @Test("containment checks the masked prefix")
    func containment() {
        let block = IP.Block("172.18.0.0/16")!
        #expect(block.contains(IP.V4("172.18.0.0")!))
        #expect(block.contains(IP.V4("172.18.5.9")!))
        #expect(block.contains(IP.V4("172.18.255.255")!))
        #expect(!block.contains(IP.V4("172.19.5.9")!))
        #expect(!block.contains(IP.V4("172.17.255.255")!))
    }

    @Test("is hashable by base and prefix length")
    func hashable() {
        #expect(IP.Block("10.0.0.0/8") == IP.Block("10.0.0.0/8"))
        #expect(IP.Block("10.0.0.0/8") != IP.Block("10.0.0.0/16"))
        #expect(Set([IP.Block("10.0.0.0/8"), IP.Block("10.0.0.0/8")]).count == 1)
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd ~/code/arca && swift test --filter ArcaIPTests > /tmp/t2.log 2>&1; echo "rc=$?"`

Expected: non-zero `rc`, "cannot find 'Block' in scope" or similar. Task 1's tests should still be present in the same run.

- [ ] **Step 3: Write the CIDR type**

Create `Sources/ArcaIP/IP.Block.swift`:

```swift
extension IP {
    /// A CIDR block of IPv4 addresses.
    ///
    /// `swift-ip` made this generic over an `Address` protocol. Arca only ever
    /// instantiated it at `IP.V4`, so the generic parameter is dropped here; a
    /// protocol with a single conformer buys nothing. The `IP` namespace is the
    /// extension point if IPv6 is ever needed.
    public struct Block: Hashable, Sendable {
        /// The network address, already masked to `bits`.
        public let base: V4

        /// The prefix length, in the range `0...32`.
        public let bits: UInt8

        /// Creates a block, masking `base` down to `bits`.
        ///
        /// - Precondition: `bits` is at most 32.
        public init(base: V4, bits: UInt8) {
            precondition(bits <= 32, "IPv4 prefix length out of range: \(bits)")
            self.bits = bits
            self.base = V4(value: base.value & Self.mask(ones: bits))
        }

        /// A mask whose logical high `bits` are 1 and whose remainder is 0.
        static func mask(ones bits: UInt8) -> UInt32 {
            bits == 0 ? 0 : ~UInt32.zero << (32 - UInt32(bits))
        }
    }
}

extension IP.Block {
    /// The closed range of addresses the block covers.
    ///
    /// The upper bound is the block's **broadcast address**, reproducing
    /// `swift-ip` 0.3.3 exactly. Arca's IP allocator treats this bound as
    /// inclusive, which means it can allocate a subnet's broadcast address.
    /// That defect predates this type, is tracked separately, and is
    /// reproduced rather than fixed so the replacement stays
    /// behaviour-identical to what it replaced.
    public var range: ClosedRange<IP.V4> {
        self.base ... IP.V4(value: self.base.value | ~Self.mask(ones: self.bits))
    }

    /// Returns whether `address` falls inside the block.
    public func contains(_ address: IP.V4) -> Bool {
        address.value & Self.mask(ones: self.bits) == self.base.value
    }
}

extension IP.Block: CustomStringConvertible {
    /// Formats the block in CIDR notation.
    public var description: String { "\(self.base)/\(self.bits)" }
}

extension IP.Block: LosslessStringConvertible {
    /// Parses a block in CIDR notation, masking the base address.
    public init?(_ string: some StringProtocol) {
        guard
            let slash = string.lastIndex(of: "/"),
            let base = IP.V4(string[..<slash]),
            let bits = UInt8(string[string.index(after: slash)...]),
            bits <= 32
        else { return nil }

        self.init(base: base, bits: bits)
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd ~/code/arca && swift test --filter ArcaIPTests > /tmp/t2.log 2>&1; echo "rc=$?"`

Expected: `rc=0`, with both `V4Tests` and `BlockTests` suites passing.

- [ ] **Step 5: Commit**

```bash
cd ~/code/arca
git add Sources/ArcaIP/IP.Block.swift Tests/ArcaIPTests/BlockTests.swift
git commit -m "feat: add ArcaIP.IP.Block, a non-generic IPv4 CIDR type

Drops swift-ip's Address protocol and generic parameter, which Arca only
ever instantiated at IP.V4.

range.upperBound is the broadcast address, reproducing swift-ip exactly.
The test asserts it and records that this is deliberate: Arca's allocator
treats the bound as inclusive, so it can allocate a subnet's broadcast
address. That defect predates this change and is tracked separately."
```

---

### Task 3: Differential harness against real `swift-ip`

**Files:**
- Create: `/private/tmp/claude-501/-Users-kiener-code-gascan/c4e3d605-ffed-4a84-934a-8d315ff0fd9f/scratchpad/diffharness/harness.swift` (scratchpad — **not committed**)
- Create: `/private/tmp/claude-501/-Users-kiener-code-gascan/c4e3d605-ffed-4a84-934a-8d315ff0fd9f/scratchpad/diffharness/run.sh`

**Interfaces:**
- Consumes: `ArcaIP` sources from Tasks 1–2; `swift-ip` sources at `~/code/arca/.build/checkouts/swift-ip/Sources/IP/*.swift`.
- Produces: an exit code and a mismatch count, recorded in the Task 6 PR body and the implementation report. No source artifact.

**Why it is not committed:** it depends on a `swift-ip` checkout that exists only in a warm SwiftPM cache and that Task 4 deletes from the graph. The Task 1–2 golden tests are the durable artifact; this harness is the evidence that those goldens are right.

**Prerequisite check:** the checkout must still exist. VERIFIED present 2026-08-05 at revision `ba4efb6457f69f5f483094aa1230e8e76cc4999c`. If it is gone, do not proceed and do not silently skip — report it, because the goldens then have no independent backing.

- [ ] **Step 1: Write the harness**

Create `harness.swift`:

```swift
import ArcaIP
import Foundation
import SwiftIPRef

/// SplitMix64. Seeded so the run reproduces exactly; an unseeded run would
/// make "no mismatches" an unrepeatable claim.
struct SplitMix64 {
    var state: UInt64

    mutating func next() -> UInt64 {
        self.state &+= 0x9E37_79B9_7F4A_7C15
        var z = self.state
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }

    mutating func next32() -> UInt32 { UInt32(truncatingIfNeeded: self.next()) }
}

var rng = SplitMix64(state: 0x5EED_1234_5678_9ABC)
var checks = 0
var mismatches = 0

func check(_ equal: Bool, _ label: @autoclosure () -> String) {
    checks += 1
    if !equal {
        mismatches += 1
        if mismatches <= 20 { print("MISMATCH: \(label())") }
    }
}

// 1. Address formatting and reparsing.
for _ in 0 ..< 5_000_000 {
    let value = rng.next32()
    let reference = SwiftIPRef.IP.V4(value: value)
    let replacement = ArcaIP.IP.V4(value: value)

    let referenceText = String(describing: reference)
    let replacementText = String(describing: replacement)
    check(referenceText == replacementText,
          "description \(value): \(referenceText) vs \(replacementText)")

    check(SwiftIPRef.IP.V4(referenceText)?.value
            == ArcaIP.IP.V4(replacementText)?.value,
          "reparse \(referenceText)")
}

// 2. Blocks at every prefix length, and containment around their boundaries.
for bits in UInt8(0) ... 32 {
    for _ in 0 ..< 20_000 {
        let value = rng.next32()
        let text = "\(ArcaIP.IP.V4(value: value))/\(bits)"

        guard
            let reference = SwiftIPRef.IP.Block<SwiftIPRef.IP.V4>(text),
            let replacement = ArcaIP.IP.Block(text)
        else {
            check(false, "block parse disagreement or failure: \(text)")
            continue
        }

        check(reference.base.value == replacement.base.value, "base \(text)")
        check(reference.bits == replacement.bits, "bits \(text)")
        check(reference.range.lowerBound.value == replacement.range.lowerBound.value,
              "lower \(text)")
        check(reference.range.upperBound.value == replacement.range.upperBound.value,
              "upper \(text)")
        check(String(describing: reference) == String(describing: replacement),
              "block description \(text)")

        // Probes chosen around the block's edges, where masking bugs live.
        // Purely random probes almost never land inside a long prefix.
        let low = replacement.range.lowerBound.value
        let high = replacement.range.upperBound.value
        let span = high &- low &+ 1
        let probes: [UInt32] = [
            low, high,
            low &- 1, low &+ 1,
            high &- 1, high &+ 1,
            low &+ (span == 0 ? rng.next32() : rng.next32() % span),
            rng.next32(),
        ]
        for probe in probes {
            check(reference.contains(.init(value: probe))
                    == replacement.contains(.init(value: probe)),
                  "contains \(text) ~ \(probe)")
        }
    }
}

// 3. Hand-written edge and malformed input, on both parsers.
let addressVectors = [
    "172.18.0.1", "0.0.0.0", "255.255.255.255", "127.0.0.1",
    "010.1.1.1", "0010.010.010.010", "+1.2.3.4", "1.2.3.-0", "-0.-0.-0.-0",
    "1.2.3", "1.2.3.4.5", "1.2.3.256", "256.1.1.1", "1.2.3.4 ", " 1.2.3.4",
    "", ".", "...", "1.2.3.", ".2.3.4", "1..3.4", "bogus", "1.2.3.4a",
    "0x1.2.3.4", "1.2.3.+4", "1 .2.3.4", "\t1.2.3.4", "1.2.3.4\n",
    "999.999.999.999", "00.00.00.00", "1.02.003.0004",
]
for text in addressVectors {
    check(SwiftIPRef.IP.V4(text)?.value == ArcaIP.IP.V4(text)?.value,
          "address vector \(String(reflecting: text))")
}

let blockVectors = [
    "172.18.0.0/16", "172.18.0.5/16", "10.0.0.0/8", "192.168.1.0/24",
    "1.2.3.4/32", "0.0.0.0/0", "172.18.0.0/33", "172.18.0.0/255",
    "172.18.0.0", "bogus/16", "172.18.0.0/", "/16", "", "/",
    "172.18.0.0/16/24", "172.18.0.0/+16", "172.18.0.0/-1", "172.18.0.0/016",
    "1.2.3.4/0", "255.255.255.255/32", "010.1.1.1/8",
]
for text in blockVectors {
    let reference = SwiftIPRef.IP.Block<SwiftIPRef.IP.V4>(text)
    let replacement = ArcaIP.IP.Block(text)
    check((reference == nil) == (replacement == nil),
          "block vector nil-ness \(String(reflecting: text))")
    if let reference, let replacement {
        check(reference.base.value == replacement.base.value
                && reference.bits == replacement.bits,
              "block vector value \(String(reflecting: text))")
    }
}

print("checks=\(checks) mismatches=\(mismatches)")
exit(mismatches == 0 ? 0 : 1)
```

Note `"172.18.0.0/16/24"` is in the vectors because both parsers use `lastIndex(of: "/")`, which makes that string parse as base `172.18.0.0/16` — and therefore fail, since `IP.V4` rejects it. It is included precisely to pin that shared quirk.

- [ ] **Step 2: Write the build-and-run script**

Create `run.sh`. Note each exit code is captured directly, never through a pipe:

```bash
#!/usr/bin/env bash
set -uo pipefail

here=$(cd "$(dirname "$0")" && pwd -P)
arca=$HOME/code/arca
refsrc=$arca/.build/checkouts/swift-ip/Sources/IP
build=$here/build

if [[ ! -d $refsrc ]]; then
    printf 'swift-ip reference checkout is absent: %s\n' "$refsrc" >&2
    printf 'The goldens have no independent backing. Do not skip this.\n' >&2
    exit 2
fi

printf 'reference revision: '
git -C "$arca/.build/checkouts/swift-ip" rev-parse HEAD

rm -rf "$build" && mkdir -p "$build"

# swift-ip's IP target uses `package` access control, so it needs -package-name
# even when compiled standalone.
swiftc -swift-version 6 -O -package-name ipx \
    -module-name SwiftIPRef \
    -emit-module -emit-module-path "$build/SwiftIPRef.swiftmodule" \
    -emit-library -static -o "$build/libSwiftIPRef.a" \
    "$refsrc"/*.swift > "$build/ref.log" 2>&1
rc=$?
if [[ $rc -ne 0 ]]; then
    printf 'reference module failed to compile: rc=%d\n' "$rc" >&2
    cat "$build/ref.log" >&2
    exit 1
fi

swiftc -swift-version 6 -O \
    -module-name ArcaIP \
    -emit-module -emit-module-path "$build/ArcaIP.swiftmodule" \
    -emit-library -static -o "$build/libArcaIP.a" \
    "$arca"/Sources/ArcaIP/*.swift > "$build/new.log" 2>&1
rc=$?
if [[ $rc -ne 0 ]]; then
    printf 'ArcaIP module failed to compile: rc=%d\n' "$rc" >&2
    cat "$build/new.log" >&2
    exit 1
fi

swiftc -swift-version 6 -O -I "$build" \
    "$here/harness.swift" "$build/libSwiftIPRef.a" "$build/libArcaIP.a" \
    -o "$build/diff" > "$build/link.log" 2>&1
rc=$?
if [[ $rc -ne 0 ]]; then
    printf 'harness failed to compile: rc=%d\n' "$rc" >&2
    cat "$build/link.log" >&2
    exit 1
fi

"$build/diff" > "$build/diff.out" 2>&1
rc=$?
cat "$build/diff.out"
printf 'HARNESS_RC=%d\n' "$rc"
exit $rc
```

- [ ] **Step 3: Run the harness**

Run: `chmod +x run.sh && ./run.sh`

Expected: `checks=` a number above 10,000,000, `mismatches=0`, `HARNESS_RC=0`.

If `mismatches` is non-zero, the printed `MISMATCH:` lines name the exact failing vector. **Do not adjust the harness to make it pass.** A mismatch means the replacement is not behaviour-identical, which is the whole acceptance criterion. Fix `ArcaIP`, or — if the reference itself is what is wrong for Arca's purposes — stop and escalate, because that reverses a decision the user already took.

If the harness fails to compile because of the two `enum IP` declarations colliding, the fix is fully-qualified `SwiftIPRef.IP` / `ArcaIP.IP` spellings, which the harness already uses; report the actual error rather than working around it.

- [ ] **Step 4: Record the evidence**

Write the exact `checks=`, `mismatches=` and `HARNESS_RC=` values down. They go in the Task 6 PR body and the final report, marked VERIFIED with the reference revision. Nothing is committed in this task.

---

### Task 4: Switch the call sites and remove `swift-ip`

**Files:**
- Modify: `Package.swift` (remove the `swift-ip` dependency and its product reference)
- Modify: `Package.resolved` (regenerated)
- Modify: `Sources/ContainerBridge/StateStore.swift:5`
- Modify: `Sources/ContainerBridge/WireGuardNetworkBackend.swift:4,382,392,1010,1027,1073,1115`

**Interfaces:**
- Consumes: `IP.V4` and `IP.Block` from Tasks 1–2.
- Produces: a package whose dependency graph contains no `tayloraswift` package.

- [ ] **Step 1: Switch the two imports**

In `Sources/ContainerBridge/StateStore.swift` line 5 and `Sources/ContainerBridge/WireGuardNetworkBackend.swift` line 4, replace `import IP` with `import ArcaIP`.

- [ ] **Step 2: Drop the generic parameter at the six block sites**

In `Sources/ContainerBridge/WireGuardNetworkBackend.swift`, replace `IP.Block<IP.V4>(` with `IP.Block(` at lines 382, 392, 1010, 1027, 1073, 1115. All six are in that one file; `StateStore.swift` constructs no blocks.

```bash
cd ~/code/arca
sed -i '' 's/IP\.Block<IP\.V4>(/IP.Block(/g' Sources/ContainerBridge/WireGuardNetworkBackend.swift
```

Then verify no occurrence survives anywhere:

```bash
grep -rn "IP\.Block<" Sources/ ; echo "rc=$?"
```

Expected: no output, `rc=1` (grep found nothing).

- [ ] **Step 3: Correct the one comment that is now provably wrong**

`Sources/ContainerBridge/WireGuardNetworkBackend.swift:1034` currently reads:

```swift
            rangeEnd = block.range.upperBound  // This is broadcast - 1 already
```

Replace the comment — the behaviour stays, only the false statement goes:

```swift
            // NOTE: this bound is the broadcast address itself, not broadcast-1
            // as this comment previously claimed, and the loop below is
            // inclusive of it. Tracked as a separate defect; behaviour is
            // deliberately unchanged here.
            rangeEnd = block.range.upperBound
```

This is a comment-only edit. Do not change the expression.

- [ ] **Step 4: Remove the dependency from `Package.swift`**

Delete the dependency line:

```swift
        .package(url: "https://github.com/tayloraswift/swift-ip.git", exact: "0.3.3"),
```

and, from the `ContainerBridge` target's `dependencies`, delete:

```swift
                .product(name: "IP", package: "swift-ip"),
```

then add in its place:

```swift
                "ArcaIP",
```

- [ ] **Step 5: Re-resolve and inspect what moved**

```bash
cd ~/code/arca
swift package resolve > /tmp/resolve.log 2>&1; echo "rc=$?"
```

Expected: `rc=0`. If resolution fails because SwiftPM is validating the now-stale `tayloraswift` entries still present in `Package.resolved`, delete just those six objects from the JSON by hand and re-run. Do **not** delete the whole `Package.resolved` — that would re-resolve every remaining dependency and could silently bump swift-nio, gRPC or SQLite.swift, turning a supply-chain removal into an unreviewable version bump.

Then inspect the diff:

```bash
git diff --stat Package.resolved
git diff Package.resolved | grep '^[-+].*identity' | sort
```

Expected: exactly six removals — `swift-ip`, `swift-bson`, `swift-json`, `swift-grammar`, `swift-hash`, `swift-unixtime` — and **zero additions**. Any other change means a version moved and must be investigated before proceeding.

- [ ] **Step 6: Confirm the graph is clean**

```bash
cd ~/code/arca
grep -c tayloraswift Package.resolved Package.swift; echo "rc=$?"
```

Expected: `0` for both files.

- [ ] **Step 7: Build clean and run the full suite**

```bash
cd ~/code/arca
swift package clean
swift build --configuration release --target ContainerBridge > /tmp/build.log 2>&1; echo "BUILD_RC=$?"
swift build --build-tests > /tmp/buildtests.log 2>&1; echo "BUILDTESTS_RC=$?"
swift test > /tmp/test.log 2>&1; echo "TEST_RC=$?"
```

Expected: `BUILD_RC=0`, `BUILDTESTS_RC=0`. `--target ContainerBridge --configuration release` is exactly what the Gas Can gate runs, so it is the load-bearing one.

For `TEST_RC`: read `/tmp/test.log`. Some of `Tests/ArcaTests/` needs a VM and entitlements and may not pass in this environment. **Establish whether any failure is pre-existing by stashing and re-running on the unmodified tree — do not assume.** Report the comparison rather than a bare pass/fail. `Tests/ArcaTests/NetworkIPAMTests.swift` is the suite most likely to catch a regression from this change and deserves specific attention.

- [ ] **Step 8: Commit**

```bash
cd ~/code/arca
git add Package.swift Package.resolved Sources/ContainerBridge/StateStore.swift Sources/ContainerBridge/WireGuardNetworkBackend.swift
git commit -m "fix: replace swift-ip with the internal ArcaIP type

swift-ip 0.3.3's transitive graph pins commits that no longer exist
upstream: the tayloraswift family rewrote history and migrated org
mid-0.3.x. A fresh clone cannot resolve them, so the pinned Arca tree
could not be built from a cold cache. Every local build hid this behind
a warm SwiftPM cache; Gas Can's engine-pin CI gate caught it on its
first run.

Drops 6 of 38 pins: swift-ip, swift-bson, swift-json, swift-grammar,
swift-hash, swift-unixtime. Arca imported one dependency-free module
from that package and used two types from it.

Re-pinning to 0.3.10 would also have worked and was rejected: it
re-enters the same lottery and does nothing about the decay.

Also corrects a comment that claimed range.upperBound is broadcast-1.
It is the broadcast address. Behaviour is unchanged and the defect is
tracked separately."
```

---

### Task 5: Cold build and functional pass

**Files:** none modified. This task produces evidence only.

**Interfaces:**
- Consumes: the branch from Task 4.
- Produces: a VERIFIED cold-build exit code and a functional-pass record.

- [ ] **Step 1: Push the branch and open the PR**

```bash
cd ~/code/arca
git push -u origin fix/replace-swift-ip-with-internal-type
```

Open the PR against `main` with `gh pr create`. Body must carry the Task 3 harness numbers and the Task 4 build exit codes, each marked VERIFIED with its anchor. Do not merge yet.

- [ ] **Step 2: Cold-build the branch head in an isolated environment**

A warm cache hides exactly the failure class this task exists to prove is gone, so `HOME`, the SwiftPM cache and the scratch path are all isolated. Exit code captured directly.

```bash
tmp=$(mktemp -d /tmp/arca-cold.XXXXXX)
mkdir -p "$tmp/home"
head=$(git -C ~/code/arca rev-parse HEAD)

env HOME="$tmp/home" git clone --quiet \
    https://github.com/Vas-Solutus/arca.git "$tmp/arca" > "$tmp/clone.log" 2>&1
echo "CLONE_RC=$?"

env HOME="$tmp/home" git -C "$tmp/arca" checkout --quiet --detach "$head" \
    > "$tmp/checkout.log" 2>&1
echo "CHECKOUT_RC=$?"

# Arca pins containerization to an SSH remote, which the gate rewrites to https.
env HOME="$tmp/home" git -C "$tmp/arca" \
    -c 'url.https://github.com/.insteadOf=git@github.com:' \
    submodule update --init --recursive --quiet > "$tmp/sub.log" 2>&1
echo "SUBMODULE_RC=$?"

env HOME="$tmp/home" swift build \
    --package-path "$tmp/arca" \
    --cache-path "$tmp/spm-cache" \
    --scratch-path "$tmp/spm-scratch" \
    --configuration release \
    --target ContainerBridge > "$tmp/build.log" 2>&1
echo "COLD_BUILD_RC=$?"

echo "log: $tmp/build.log"
grep -ci tayloraswift "$tmp/build.log"
```

Expected: every `_RC=0`, and zero `tayloraswift` mentions in the build log. **`COLD_BUILD_RC=0` is the finding this whole plan exists to produce** — record it verbatim with the commit SHA.

If it fails, use superpowers:systematic-debugging rather than guessing; the failure is by definition something a warm cache was hiding.

- [ ] **Step 3: Functional pass**

Build and start the daemon, then exercise the paths that actually use the replaced type: subnet parsing, gateway calculation, IP allocation and subnet containment.

```bash
cd ~/code/arca
swift build --configuration release > /tmp/fbuild.log 2>&1; echo "rc=$?"
```

Then, with the daemon running, using an **explicit unique name on every container** — `docker run --rm` does not remove and generated names collide against a 36-name pool (`Vas-Solutus/arca#47`):

```bash
suffix=$(date +%s)
docker network create --subnet 172.31.0.0/16 "p14net-$suffix"
docker network inspect "p14net-$suffix"        # gateway must be 172.31.0.1
docker run -d --name "p14a-$suffix" --network "p14net-$suffix" alpine sleep 300
docker run -d --name "p14b-$suffix" --network "p14net-$suffix" alpine sleep 300
docker inspect "p14a-$suffix" "p14b-$suffix"   # distinct addresses inside the subnet
docker run -d --name "p14c-$suffix" --network "p14net-$suffix" --ip 172.31.9.9 alpine sleep 300
docker run --name "p14d-$suffix" --network "p14net-$suffix" --ip 10.9.9.9 alpine true
```

Expected: gateway `172.31.0.1`; the two auto-allocated addresses distinct and inside `172.31.0.0/16`; `--ip 172.31.9.9` accepted; `--ip 10.9.9.9` **rejected** with an out-of-subnet error, which is the `isIPInSubnet` path and therefore `IP.Block.contains`.

Clean up by explicit name afterwards. Record what actually happened, including anything that failed.

---

### Task 6: Merge, tag, and file the broadcast defect

**Files:** none in the working tree.

**Interfaces:**
- Consumes: the reviewed PR from Task 5.
- Produces: a new signed annotated tag on Arca `main`, and its 40-character commit SHA, which Task 7 consumes.

- [ ] **Step 1: File the broadcast defect as an Arca issue**

Do this before merging, so the commit and the issue can reference each other. Follow the `#47`/`#48` precedent.

```bash
gh issue create -R Vas-Solutus/arca \
  --title "IP allocator can hand a container its subnet's broadcast address" \
  --body "$(cat <<'EOF'
`WireGuardNetworkBackend.swift` sets the allocation range end from
`block.range.upperBound`, which is the subnet's **broadcast address**, and the
allocation loop is inclusive of that bound. A container can therefore be
allocated the broadcast address of its network.

The comment on that line claimed the bound was already `broadcast - 1`. It was
not. The comment was corrected when `swift-ip` was replaced; the behaviour was
deliberately left unchanged so that replacement stayed behaviour-identical and
could be verified by differential testing.

VERIFIED against swift-ip 0.3.3 (revision `ba4efb6`) and against the
replacement: `IP.Block("172.18.0.0/16").range.upperBound` is `172.18.255.255`.

Reachability: needs ~65,533 containers on a `/16`, but is immediate on a `/30`.

Fix is to end the range one below the broadcast address, with a guard for
prefixes where no host range exists (`/31`, `/32`).
EOF
)"
```

- [ ] **Step 2: Merge the PR with a merge commit**

```bash
gh pr merge <number> -R Vas-Solutus/arca --merge
```

**`--merge`, never `--squash`.** Gas Can pins Arca by commit and these documents cite its SHAs. If the merge is refused by `require_last_push_approval`, report the refusal and its message — do not silently add `--admin`.

- [ ] **Step 3: Capture the merge commit**

```bash
cd ~/code/arca
git checkout main && git pull --ff-only
git rev-parse HEAD
git log --oneline -3
```

Record the 40-character SHA. Task 7 needs it exactly.

- [ ] **Step 4: Create and push the signed annotated tag**

The gate verifies the tag's signature against `gascan/engine/allowed-signers`, which holds only `richard@liquescent.dev`'s ed25519 key. `tag.gpgsign=true` and `gpg.format=ssh` are already configured.

```bash
cd ~/code/arca
git tag -a -s gascan-engine-ip-internal -m "Engine baseline: cold-buildable

Replaces swift-ip with the internal ArcaIP target, dropping 6 of 38 pins
and removing every dependency on commits that no longer exist upstream.

This is the first Arca tag that resolves and builds from a cold SwiftPM
cache. VERIFIED by an isolated build with HOME, --cache-path and
--scratch-path all redirected to a fresh temporary directory."
git push origin gascan-engine-ip-internal
```

- [ ] **Step 5: Verify the tag the way the gate will**

This is the exact assertion `scripts/build-arca-engine.sh` makes. Run it now rather than discovering a mismatch in CI.

```bash
cd ~/code/arca
git -c "gpg.ssh.allowedSignersFile=$HOME/code/gascan/engine/allowed-signers" \
    verify-tag gascan-engine-ip-internal > /tmp/verify.log 2>&1; echo "VERIFY_RC=$?"
git rev-parse --verify "refs/tags/gascan-engine-ip-internal^{}"
git rev-parse --verify main
```

Expected: `VERIFY_RC=0`, and the tag's dereferenced target identical to `main`'s SHA. If they differ, the gate fails with exit 65.

---

### Task 7: Bump the Gas Can pin and turn the gate green

**Files:**
- Modify: `~/code/gascan/engine/arca-pin.json`

**Interfaces:**
- Consumes: the tag name and 40-character SHA from Task 6.
- Produces: a green `engine-pin` check on PR #44.

- [ ] **Step 1: Update the pin**

In `~/code/gascan/engine/arca-pin.json`, set `tag` and `revision` to the Task 6 values. Keep `schema: 1` — `scripts/build-arca-engine.sh` asserts `.schema == 1` and exits 64 otherwise. `revision` must match `^[0-9a-f]{40}$`.

```json
{
  "schema": 1,
  "name": "arca",
  "url": "https://github.com/Vas-Solutus/arca.git",
  "tag": "gascan-engine-ip-internal",
  "revision": "<40-char SHA from Task 6 Step 3>"
}
```

- [ ] **Step 2: Run the gate's own script locally against a cold cache**

The script is what CI runs. Pointing its cache at a fresh directory and isolating `HOME` reproduces the runner's cold state.

```bash
cd ~/code/gascan
tmp=$(mktemp -d /tmp/gate.XXXXXX)
mkdir -p "$tmp/home"
env HOME="$tmp/home" GASCAN_ARCA_ENGINE_CACHE="$tmp/cache" \
    ./scripts/build-arca-engine.sh > "$tmp/gate.log" 2>&1
echo "GATE_RC=$?"
tail -30 "$tmp/gate.log"
```

Expected: `GATE_RC=0`. The taxonomy for failures: 64 malformed pin, 65 provenance failure, 69 missing tool, 75 cache lock held.

- [ ] **Step 3: Run the release contract tests**

The pin is covered by the release contract suite that landed with P1.1/P1.2; all 14 passed at that time.

```bash
cd ~/code/gascan
cargo test --test '*' > /tmp/contract.log 2>&1; echo "rc=$?"
```

Read the log and report which suite ran and its counts. If the correct invocation differs, find it rather than guessing — do not report a pass for a suite that did not run.

- [ ] **Step 4: Commit and push**

```bash
cd ~/code/gascan
git add engine/arca-pin.json
git commit -m "chore: bump the Arca engine pin to the cold-buildable tag

P1.4 complete. The previous pin, gascan-engine-baseline, could not be
resolved from a cold SwiftPM cache: swift-ip 0.3.3's transitive graph
pinned commits the tayloraswift family deleted when it rewrote history
and migrated org mid-0.3.x. The engine-pin gate caught it on its first
run and has been red since.

The new tag replaces swift-ip with an internal type and drops 6 of
Arca's 38 pins."
git push
```

- [ ] **Step 5: Watch the gate**

```bash
cd ~/code/gascan
gh pr checks 44 --watch
```

**A green `engine-pin` check on PR #44 is the completion signal for P1.4, and it is the only one.** A local green build does not substitute for it — that is precisely the substitution that hid this defect for months. Do not report P1.4 complete on any other evidence.

If it goes red, read the run log and use superpowers:systematic-debugging. Exit 65 means provenance (tag signature or tag-target mismatch); exit 64 means the pin JSON; a SwiftPM resolution error means something is still reaching a vanished commit.

- [ ] **Step 6: Update the handoff and roadmap**

Per the conventions these documents are built on:

- `docs/status/arca-integration-handoff.md` — add a P1.4 completion section with the harness numbers, the cold-build exit code, the new tag and SHA, all marked VERIFIED with anchors.
- `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md` — mark P1.4 done. **Strike through superseded text in place with a pointer; do not quietly edit it away.** P1.4 "Blocks P2" becomes satisfied, so note that P2 is unblocked.
- Note the new Arca issue number for the broadcast defect.

Commit these separately from the pin bump.

---

## Self-Review

**Spec coverage.** §1 motivation → Task 4 commit message and Task 7. §1.1 zero-dependency claim → Task 1 Step 1 target with no dependencies. §1.2 blast radius → Task 4 Steps 1–2. §2 behaviour to reproduce → Tasks 1–2 tests, Task 3 harness. §2.1 broadcast defect → Task 2 test, Task 4 Step 3 comment fix, Task 6 Step 1 issue. §3.1 placement → Task 1 Step 1. §3.2 API → Tasks 1–2. §3.3 churn → Task 4 Steps 1–2. §4.1 harness → Task 3. §4.2 committed tests → Tasks 1–2. §4.3 cold build and functional pass → Task 5. §5 sequence → Tasks 1–7 in order. §6 out of scope → carried as explicit "do not" instructions in Global Constraints and Task 4 Step 3.

**Placeholder scan.** The only bracketed placeholders are `<number>` (PR number, unknowable until Task 5) and `<40-char SHA from Task 6 Step 3>` (unknowable until Task 6), each with its source named. No TBD, no "add error handling", no "similar to Task N".

**Type consistency.** `IP.V4(value:)`, `.value`, `IP.V4(_:_:_:_:)`, `IP.Block(base:bits:)`, `.base`, `.bits`, `.range`, `.contains(_:)`, `Block.mask(ones:)` are spelled identically in Tasks 1, 2, 3 and 4. `IP.Block` is non-generic in every appearance after Task 2, and Task 4 Step 2's `grep -rn "IP\.Block<"` enforces it. The harness is the one place a generic spelling survives, correctly, because it also references the real `swift-ip` type as `SwiftIPRef.IP.Block<SwiftIPRef.IP.V4>`.
