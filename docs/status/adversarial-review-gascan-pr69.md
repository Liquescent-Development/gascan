# Adversarial review — Gas Can PR #69 (`docs/p5-1-engine-design`, 9665107..39be145)

> **Status: C1 and I1–I5 are fixed on this branch; the six Minors are not.** M1 and M2
> were taken as well, because they were load-bearing for the rest — M2's EXIT trap
> collapsed every documented exit code to 1, and the new pin-contract cases assert exact
> codes. M4's overclaiming comment was corrected in place rather than the check widened;
> the finding itself stands. This file is left as written: it records what was observed at
> `39be145`, and rewriting it would destroy the evidence. What changed, and what it was
> checked against, is in `docs/status/START-HERE.md`.

Reviewer stance: find what is wrong. Prior "this is good" reviews treated as claims to attack.

**Counts: 1 Critical, 5 Important, 6 Minor.** Nine claims/properties attacked and held — listed at
the end with what I ran, so "checked" is distinguishable from "assumed".

The working tree, index, HEAD and branch state of both repositories are unchanged (`git status
--porcelain` empty, scratch `git worktree` removed and pruned). `/Users/kiener/code/arca` was
read only.

---

## CRITICAL

### C1. `scripts/build-arca-engine.sh:64-78` — the signature gate can verify a different object than the one it compiles

The script's provenance chain is three gates:

```
64  git ... verify-tag "$tag"                              # <- unqualified name
70  tag_target=$(git ... rev-parse --verify "refs/tags/${tag}^{}")
74  [[ $tag_target == "$revision" ]]
85  git ... checkout --quiet --detach --force "$revision"
```

Line 64 names the tag **unqualified**. Git's rev resolution tries `$GIT_DIR/<name>`, then
`refs/<name>`, and only then `refs/tags/<name>`. Line 70 names it **fully qualified**. So the
object whose signature is verified and the object whose identity is checked are resolved by two
different rules and need not be the same object.

**Concrete failure scenario, executed end to end.** Upstream carries two tags:

- `refs/tags/foo` — an annotated tag signed by the key in `engine/allowed-signers`, on a good commit.
- `refs/tags/tags/foo` — an *unsigned lightweight* tag on an attacker's commit.

`git fetch --prune --prune-tags --tags --force origin` (line 58) fetches **both**, because the
`--tags` refspec is `refs/tags/*:refs/tags/*`. The pin then names `"tag": "tags/foo"` with
`"revision"` set to the attacker's commit. Result (my run, scratch repo, reproduced verbatim):

```
gate1 cat-file: PASS
warning: refname 'tags/foo' is ambiguous.
gate2 verify-tag 'tags/foo': PASS (verified object: 1f01f3e...)   <- this is refs/tags/foo
gate3 refs/tags/tags/foo^{} = 2afa173...  ; pinned revision = 2afa173...
>>> ALL THREE GATES PASS, script would compile EVIL commit 2afa173 (good=9afce87)
```

The signature that was checked belongs to an object with no relationship to the bytes handed to
`swift build`. **No local write access is required** — only a pushed tag pair upstream and a pin
naming a slash-containing tag name.

A second, weaker-precondition variant needs no slash: any `refs/<tag>` planted in the warm cache
(`.artifacts/arca-engine/arca/.git`) shadows `refs/tags/<tag>` for line 64 the same way. I
confirmed that too (`verify-tag mytag: PASSES (resolved refs/mytag)` while `refs/tags/mytag`
pointed elsewhere).

**Why it matters.** This script's own comments call it the thing "standing between a signed tag
and a shipped binary", and it goes to real lengths elsewhere (clean checkout, `-x`, submodule
scrub) to make the compiled bytes provably the tag's tree. The tree is proven; *which tag was
signed* is not.

**Fix.** One word: `verify-tag "refs/tags/${tag}"` — I confirmed git accepts the fully-qualified
form. Stronger: verify, then require `git rev-parse "refs/tags/${tag}"` and the verified object to
be the same object, and peel that object to `$revision`. Also constrain `.tag` in the pin schema
(line 24-30) to `^[A-Za-z0-9._-]+$`, which independently kills the slash variant.

Note: `scripts/sync-arca-proto.sh:82` has the same unqualified `verify-tag "$tag"` (not in this
diff). It happens to be immune because it does `git init` + `fetch origin tag <tag>`, so only the
one ref exists — but it is immune by accident, not by construction.

---

## IMPORTANT

### I1. `tests/release/engine-targets-check.sh:86` — the forbidden names are never asserted to exist, so a rename makes the check silently vacuous

The script guards the *roots* precisely because "a renamed root would make every assertion below
vacuously true" (line 45), and guards the *manifest shape* with `MANIFEST_SHAPE_CHANGED`. It does
not guard the forbidden names. `def forbidden($name): $name == "DockerAPI" or $name ==
"ArcaDaemon";` is a literal string comparison against names that are never required to be present.

**Concrete failure scenario, executed.** Scratch SwiftPM package, `arca-engine → ArcaEngine →
DockerHTTPAPI` (a rename of `DockerAPI`, nothing else changed):

```
$ ./tests/release/engine-targets-check.sh <pkg>
PASS: neither arca-engine nor ArcaEngine reaches DockerAPI or ArcaDaemon
exit=0
```

The engine reaches the Docker surface and the release gate is green. This is the "a check that
cannot fail while claiming it can" class after any upstream rename — and Arca is a separate
repository whose renames Gas Can does not review.

**Fix.** Reuse the `missing` guard already in the file, over `["DockerAPI","ArcaDaemon"]`, and exit
65 with "the forbidden targets were renamed; this check is not measuring anything." Cost: one
`jq` expression, identical in shape to lines 46-56.

### I2. `scripts/ci-classify-paths.sh:41-44 vs :58` — the live tier is gated on its test files but not on its subject

`crates/gascan-arca/tests/live/*` fires `engine` so a change to the live tests runs the live
tests. But `crates/*` (line 58) fires `rust` only. So:

**Concrete failure scenario.** Edit `crates/gascan-arca/src/channel.rs` — the file that owns the
placeholder authority `http://[::]:50051`, the Unix connector, and `source_chain()`. Classification
yields `rust=true contracts=false engine=false`. The `engine` job is skipped, `gate` accepts
`skipped`, and the change merges without the live tier ever executing against a real engine. The
properties verified **only** by the live tier — that a real server accepts the placeholder
authority, that the real engine's `unsupported_capability` reaches the backend as an outcome — are
unguarded for changes to the code that implements them. Same for `Cargo.toml`/`Cargo.lock` (a tonic
bump) and for `crates/gascan-engine-proto/*`.

The classifier's own comment at line 38-40 states the hole it is closing ("a change to the live
tests would never run the live tests") and stops one step short of it.

**Fix.** Add `crates/gascan-arca/*|crates/gascan-engine-proto/*` → `engine=true; rust=true`, or
accept the cost knowingly and say so in the comment instead of leaving it implied-covered.

### I3. `crates/gascan-arca/tests/live/read_rpcs.rs:58-78` — the comment claims eight methods, the test exercises one

```rust
/// The eight unimplemented methods must ANSWER. A gRPC status would reach the
/// consumer as an unreachable engine ...
async fn an_unimplemented_method_reports_unsupported_capability_not_a_transport_fault() {
    ... backend.start(&id).await.expect_err(...)
```

Only `Start` is called. PR claim 2 ("Unimplemented methods answer inside the gRPC response `oneof`
as `unsupported_capability`, never as a gRPC status") is asserted for 1 of 8.

I tested the claim myself against the pinned engine in a scratch worktree and **it holds for all
eight** — including the two streaming methods, where the arm is a different message
(`ExecServerFrame.frame.error`, `LogsChunk.outcome.error`) that nothing in the tier touches:

```
EIGHT start:            oneof_code=Some("unsupported_capability") grpc_status=None
EIGHT stop:             oneof_code=Some("unsupported_capability") grpc_status=None
EIGHT remove:           oneof_code=Some("unsupported_capability") grpc_status=None
EIGHT create:           oneof_code=Some("unsupported_capability") grpc_status=None
EIGHT create_container: oneof_code=Some("unsupported_capability") grpc_status=None
EIGHT prepare_image:    oneof_code=Some("unsupported_capability") grpc_status=None
EIGHT logs:  first frame = LogsChunk { outcome: Error(EngineError { code: "unsupported_capability", ...
EIGHT exec:  first frame = ExecServerFrame { frame: Error(EngineError { code: "unsupported_capability", ...
```

So the *claim* is true and the *coverage* is not. A regression in Arca that made `Logs` or `Exec`
answer with a gRPC status would pass this tier. The finding is the durable comment asserting
coverage the suite does not have — the exact failure class this branch's other work is careful
about.

**Fix.** Loop the six unary methods and add the two streaming first-frames; the probe above is
~40 lines and runs in 0.07s.

### I4. `docs/release/releasing.md:99` — the release checklist uses the exact masked-pipeline trap ci.yml documents and avoids

```sh
./tests/release/engine-targets-check.sh "$(./scripts/build-arca-engine.sh | head -1)"
```

The surrounding block is a plain `sh` snippet with no `set -o pipefail`, so the substitution's
status is `head`'s and a failed engine build is discarded. All of `build-arca-engine.sh`'s
diagnostics go to stderr, so the substitution yields the empty string and the releaser gets
`swift package describe --package-path ""` failing on a missing manifest — a misleading error, at
the release gate, for a build that never succeeded.

`.github/workflows/ci.yml:104-107` calls out this precise trap ("The pipeline would otherwise mask
a build failure behind head's exit status") and writes stdout to a file first. Both were added in
this PR.

**Fix.** Mirror the workflow: `out=$(mktemp); ./scripts/build-arca-engine.sh >"$out"` then
`./tests/release/engine-targets-check.sh "$(head -1 "$out")"`.

### I5. `scripts/ci-classify-paths.sh:35` — false count in a durable artifact

> `# The live tier. Four of its six tests are #[ignore]d, so ...`

Measured (`env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --test live`):

```
running 8 tests
...
test result: ok. 2 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out
```

Eight tests, six ignored. Both numbers in the comment are wrong; it was written in the branch's
final commit (`39be145`, "fix two records"). The consequence is small, but the comment is the
justification for a CI routing rule, and its arithmetic is the reason a reader would trust the
rule. `.github/workflows/ci.yml:137-139` states the same fact correctly ("six #[ignore]
attributes ... 2 in connect.rs, 4 in read_rpcs.rs"), so the two records now disagree.

---

## MINOR

### M1. `.github/workflows/ci.yml:152-154` — a comment describing a job that was deleted, now attached to `gate`

```yaml
  # Temporary. Settles whether a hosted runner carries an Apple container runtime,
  # replacing a PLAN claim in the design spec with a measurement. Deliberately
  # outside gate's needs, so it can never block a merge. Removed once recorded.
  gate:
```

This belonged to the `runtime-probe` job, removed in `37e3ec0 ci: delete the runtime-probe job`
(`git log -S "runtime-probe" -- .github/workflows/ci.yml`). Every sentence is false of `gate`,
which is permanent, is the required check, and exists to block merges. Pre-existing, but this PR
edits this file and this is the block a reader reaches for when asking what gates the engine job.

### M2. `scripts/build-arca-engine.sh:50` — the EXIT trap can overwrite the script's exit status and strand the lock

`trap 'rmdir "$lock"' EXIT`. Measured on this machine:

```
$ bash -c 'set -euo pipefail; trap "false" EXIT; exit 65'; echo $?
1
$ bash -c 'set -euo pipefail; d=$(mktemp -d); trap "rmdir \"$d\"" EXIT; touch "$d/x"; exit 65'; echo $?
rmdir: .../tmp.eUGEWWO87D: Directory not empty
1
```

Any stray entry in `$cache_root/.lock` (a Finder `.DS_Store`, an NFS `.nfs*` sillyrename) turns
every documented exit code — 64, 65, 69, 70, 75 — into 1 **and** leaves the lock in place. The
lock has no documented recovery path (`tests/release/engine-pin-contract.sh` asserts exact codes
and would start failing for a reason unrelated to what it tests). Fix:
`trap 'rc=$?; rmdir "$lock" || true; exit "$rc"' EXIT`.

### M3. `crates/gascan-arca/tests/live/common/mod.rs:60-77` — an assert that cannot fire, and a leak class the design chose rather than removed

The socket root is `/tmp/gascan-arca-live-<pid>-<seq>`, a hardcoded constant plus two integers —
observed at 45 bytes in my run (`/tmp/gascan-arca-live-30193-5/engine.sock`). The `sun_path`
assert at line 73 therefore cannot fire, while the `SocketRoot` doc at lines 12-14 cites it as a
live panic path this guard exists to unwind through ("`start()` can still panic on the `sun_path`
assert"). It is defensible as a regression guard on a future edit; the doc overstates it.

Separately: `create_dir` (not `create_dir_all`) is a deliberate choice to fail on a leftover
directory, and a SIGKILL or Ctrl-C'd run leaks both the directory and the `arca-engine` child
(Rust's default SIGINT disposition terminates without unwinding, so neither `Drop` nor
`kill_on_drop` runs). macOS recycles pids freely and does not sweep `/tmp` outside of reboot, so
one interrupted run poisons that pid on that machine indefinitely — a false red in the tier whose
whole value is being believable. `tempfile::Builder::new().prefix("gascan-arca-live-")
.tempdir_in("/tmp")` gives a short path, a collision-free name, and the same RAII, and removes the
class entirely — the stated reason for avoiding `TempDir` was path length, which `tempdir_in`
already solves.

### M4. `tests/release/engine-targets-check.sh:101-104` — the product check is narrower than its comment claims

> `Checking them costs one line and closes the day someone moves DockerAPI into its own SwiftPM package`

It closes that day only if the new **product** is also named `DockerAPI`. A product named
`ArcaDockerKit` (or a package-level `.product(name: "ArcaKit")` re-exporting it) passes. Same root
cause as I1: the instrument is a literal name list with no liveness check.

### M5. `.github/workflows/ci.yml:141-150` — the live-tier step cannot fail on zero selected tests

Its own comment concedes it: "A smaller number means tests were dropped; cargo would report it and
not fail on its own." `cargo test ... -- --ignored` exits 0 on `running 0 tests`. This is the same
vacuous-gate class that `build-arca-engine.sh:161-169` goes to 20 lines of comment to close for
`swift test --filter`, left open one file away.

It *is* closed indirectly: `scripts/ci-check-ignored-tests.sh` diffs against
`tests/ci/expected-ignored-tests.txt`, and any edit under `crates/gascan-arca/tests/live/*` fires
`rust` as well as `engine`, so the guard runs. So this is defence-in-depth, not a live hole — but
the step asserts a number in prose that it does not check.

### M6. `crates/gascan-arca/src/channel.rs:31-45` / `tests/live/common/mod.rs:98-144` — the readiness probe opens and discards a connection

`await_socket` establishes a full `ChannelTransport` and drops it, then `transport()` dials again.
Correct (I verified `connect_with_connector` is eager — see H1 below — so the probe genuinely
proves the listener accepts), but it costs a connection per engine and means the connection the
tests use is never the one readiness was proven on. Cosmetic; noting it because a lazy-channel
regression in `connect` would turn `await_socket` into a pure `socket.exists()` check and
reintroduce the bind race the comment at lines 99-101 says it exists to close. The two non-ignored
`connect` tests are what actually prevents that, which is worth stating in the comment.

---

## Attacked and could not break

Each of these is a PR claim or a suspected weakness I actively tried to falsify, with what I ran.

**H1. Claim 1, the exact error renderings.** Not just asserted-substring — I printed them from a
scratch worktree (`git worktree add --detach`, removed afterwards):

```
MISSING   >>>connect: engine transport failure: /var/folders/.../absent.sock: transport error: No such file or directory (os error 2)<<<
NONSOCKET >>>connect: engine transport failure: /var/folders/.../f: transport error: Socket operation on non-socket (os error 38)<<<
```

Both name the dialed path and carry the exact errno text the PR body claims. This also proves
`Endpoint::connect_with_connector` is eager here — a lazy channel would have returned `Ok`.

**H2. Claim 2, for all eight unimplemented methods.** Probed directly against
`.artifacts/arca-engine/arca/.build/release/arca-engine` — see I3 for the full output. All eight
answer in the `oneof`; none produced a gRPC status. The claim is true; only its test coverage is
thin.

**H3. `a_call_against_a_killed_engine_fails_rather_than_hanging` passes for the wrong reason.**
This was my strongest suspicion: `kill(mut self)` consumes `self`, so `SocketRoot::drop` deletes
the socket at the same instant the engine dies, and I expected the test to be measuring socket
removal. I built the counterfactual — engine **alive**, socket directory removed, same open
transport — and:

```
ALIVE-ENGINE-SOCKET-REMOVED: is_err=false -> None
```

The call succeeds. tonic reuses the established connection, so the kill is what makes the test
fail. **The test isolates its property.** (It would still read better if `kill()` did not also
remove the socket, so a future regression's diagnosis is unambiguous.)

**H4. The three read-RPC tests pass vacuously against an empty response.** They do not.
`crates/gascan-arca/src/backend.rs:118-124, 137-143, 385-393` map an unset `outcome` to
`translate::missing_outcome(...)`, a hard error, and
`crates/gascan-arca/src/translate.rs:300-314` rejects both an absent `engine_version` and
`Isolation::Unspecified` (the proto zero value). So
`capabilities_report_only_what_this_engine_build_implements` requires the engine to have sent a
populated version and a non-zero isolation enum; it is not satisfiable by `CapabilitiesResponse{}`.

**H5. Claim 4, "capable of failing".** Constructed a scratch package with
`ArcaEngine → DockerAPI` behind `.when(platforms: [.linux])`, to test whether `swift package
describe` hides host-inapplicable edges on macOS. It does not — the edge is reported and the check
fails correctly:

```
the engine reaches a forbidden target:
arca-engine -> ArcaEngine -> DockerAPI
ArcaEngine -> DockerAPI
exit=1
```

The real run against `~/code/arca` passes on the merits: `arca-engine → ArcaEngine →
{SandboxEngineProto, ContainerBridge → ArcaIP}`, with `DockerAPI` and `ArcaDaemon` present in the
manifest and reachable only from `Arca`/`ArcaTests`. I also checked the `$seen`-dedup BFS for a
path where a reachable forbidden node is skipped — it cannot happen; dedup affects which *path* is
reported, not whether a node is visited.

**H6. Claim 5's arithmetic.** I did not run the workspace suite (instructed not to), so I checked
the census instead: `cargo test --workspace -- --list` yields **1463** `: test` entries at HEAD,
and `tests/ci/expected-ignored-tests.txt` is **28** lines. 1435 + 28 = 1463. The claimed totals are
consistent with the test set that exists at `39be145`, and no commit after the measurement adds or
removes Rust tests.

**H7. `--prune-tags` really is a revocation channel** (`build-arca-engine.sh:53-58` makes a strong
claim about it). Cloned a fixture, fetched, deleted the tag upstream, re-fetched with the script's
exact flags: `after upstream delete: tags=[]`. Claim holds.

**H8. The pin resolves and the engine job can actually run it.**
`git ls-remote https://github.com/Vas-Solutus/arca.git` → `refs/tags/gascan-engine-m1^{} =
f5fde96224937e4617b8dac9ae5eeea837089420`, matching `engine/arca-pin.json` exactly. The tag
verifies against `engine/allowed-signers` (`Good "git" signature for richard@liquescent.dev`). Both
`Vas-Solutus/arca` and the `git@github.com:` submodule `Vas-Solutus/arca-containerization` are
reachable **anonymously** (`env -i ... GIT_SSH_COMMAND=/usr/bin/false git ls-remote`), so a hosted
runner with no credentials can clone both and the `insteadOf` rewrite at line 91 does what it
claims. START-HERE's caveat that the first CI run is the first end-to-end run remains accurate,
but the two failure modes I could check ahead of time (tag absent from remote, submodule
unreachable without SSH) are both clear.

**H9. Harness hygiene on the happy path.** Ran the full ignored tier
(`GASCAN_ARCA_ENGINE_BIN=... cargo test -p gascan-arca --test live --no-fail-fast -- --ignored`):
6 passed in 1.10s, six engines on six distinct socket roots, and `/tmp/gascan-arca-live-*` was
empty both before and after. No orphaned children. The `SOCKET_SEQUENCE` + pid scheme is
collision-free within and across concurrent `cargo test` processes; the only leak path is the
non-unwinding one in M3.

**H10. Hygiene gates.** `shellcheck` clean across all eight scripts touched;
`cargo fmt --all --check` clean; `cargo clippy -p gascan-arca --all-targets -- -D warnings` clean;
`tests/ci/classify-paths-contract.sh` — all checks passed, and its 24 cases genuinely pin the
routing (including the space-in-path and unmapped-path cases).

**Not attacked** (out of my reach under the stated limits): `scripts/build-arca-engine.sh` and
`tests/release/engine-pin-contract.sh` were not executed — I was told not to run them. C1 and M2
are therefore reasoned from the script text plus isolated git/bash experiments, not from a run of
the script itself; C1's three gates were reproduced individually against a fixture repository
rather than through the script. The pin contract's negative cases are well constructed on
inspection, but note that **none of them would catch C1** — the fixture never creates a shadowing
ref, so the contract's `good` case passes for a correct reason and its failure cases never reach
the ambiguity.
