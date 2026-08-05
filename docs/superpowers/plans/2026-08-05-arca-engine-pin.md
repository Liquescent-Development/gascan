# Arca Engine Pin (P1.1 / P1.2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pin Arca in Gas Can at `gascan-engine-baseline`, verify that pin's signature, build its Docker-free subgraph from Gas Can's pipeline, and record the pin in `build-manifest.json`.

**Architecture:** Gas Can holds a tracked pin file plus a tracked allowed-signers file. A build script fetches Arca into a gitignored cache, verifies the annotated tag's SSH signature against the tracked key, asserts the tag resolves to the pinned revision, then runs `swift build --target ContainerBridge`. `package.sh` calls that script before emitting the manifest, so the manifest never claims a pin the release did not compile. Nothing of Arca's is copied into Gas Can and nothing of Arca's is deleted.

**Tech Stack:** bash, `jq`, `git` (SSH signature verification), SwiftPM 6.3, GitHub Actions, `pkgbuild`.

**Spec:** `docs/superpowers/specs/2026-08-05-arca-engine-pin-design.md`

## Global Constraints

- Arca pin: URL `https://github.com/Vas-Solutus/arca.git`, tag `gascan-engine-baseline`, revision `b20be7c865978759026d233e2d012ec8dc393b27`.
- Signing key permitted to sign the pin: `richard@liquescent.dev ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHyTKmfAwcJcdfKXmj2h3mwfgPaelE6gSMrquAcPmW09` (fingerprint `SHA256:3NWoJ1nmsLHxd8hAG/BnyriJJpIFXHaW3RtuPYANKc4`).
- Exit codes, matching `packaging/macos/package.sh:12-17`: **64** malformed pin or usage, **65** provenance failure, **69** required tool missing.
- Fail fast. No fallbacks, no silencing, no "try the next thing" on failure.
- `swift build` targets `ContainerBridge` only — a **target**, never a product. There is no engine executable yet (spec §2.3).
- Capture exit codes directly. Never through a pipe: `cmd | tail` returns `tail`'s status. Redirect to a file and read `$?`.
- Manifest becomes `schema: 2`. `files[]` is unchanged — no engine binary ships.
- `tests/release/*` and `shellcheck` are hand-run; nothing in CI runs them (`docs/release/releasing.md:192`). The new CI workflow is Swift-only.
- Do not commit to `main` in any repository. Work lands on `arca-integration`.

## File Structure

| File | Responsibility |
|---|---|
| `engine/arca-pin.json` (create) | The pin: url, tag, revision. Single source of truth. |
| `engine/allowed-signers` (create) | Trust anchor for the pin's tag signature. |
| `scripts/build-arca-engine.sh` (create) | Validate pin → fetch → verify signature → checkout → build. |
| `tests/release/engine-pin-contract.sh` (create) | Hermetic proof the script rejects every bad pin. |
| `packaging/macos/release-common.sh` (modify :27-48) | Treat the pin and script as release inputs. |
| `tests/release/source-input-contract.sh` (modify :12, :20) | Seed the new inputs so the existing test keeps passing. |
| `packaging/macos/package.sh` (modify :49-50, :82-89) | Build the engine, then emit `schema: 2` with `engine`. |
| `packaging/macos/verify-package.sh` (modify :49-60) | Assert the new manifest shape. |
| `tests/release/installer-contract.sh` (modify :132) | Fixture manifest. |
| `tests/release/publish-contract.sh` (modify :205-218) | Fixture manifest. |
| `.github/workflows/engine-pin.yml` (create) | Compile the pin on every pin-bump PR. |

---

### Task 1: Pin, trust anchor, and build script

**Files:**
- Create: `engine/arca-pin.json`
- Create: `engine/allowed-signers`
- Create: `scripts/build-arca-engine.sh`
- Test: `tests/release/engine-pin-contract.sh`
- Modify: `packaging/macos/release-common.sh:27-48`
- Modify: `tests/release/source-input-contract.sh:12,20`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `scripts/build-arca-engine.sh`, which prints the absolute checkout path on stdout and exits 0 on success. Task 2 calls it. Honours three environment overrides, used only by tests — production always takes the defaults:

| Variable | Default |
|---|---|
| `GASCAN_ARCA_PIN_FILE` | `<repo>/engine/arca-pin.json` |
| `GASCAN_ARCA_ENGINE_CACHE` | `<repo>/.artifacts/arca-engine` |
| `GASCAN_ARCA_ALLOWED_SIGNERS` | `<repo>/engine/allowed-signers` |

- [ ] **Step 1: Confirm the pin file would not be silently ignored**

This is the `*.mod` failure class that already cost a session. Run it before writing anything.

```bash
cd ~/code/gascan
mkdir -p engine
git check-ignore -v engine/arca-pin.json; echo "EXIT=$? (expect 1 = not ignored)"
git check-ignore -v engine/allowed-signers; echo "EXIT=$? (expect 1 = not ignored)"
```

Expected: both `EXIT=1` with no output. If either is 0, a gitignore rule matches — stop and report it rather than working around it.

- [ ] **Step 2: Create the pin file**

`engine/arca-pin.json`:

```json
{
  "schema": 1,
  "name": "arca",
  "url": "https://github.com/Vas-Solutus/arca.git",
  "tag": "gascan-engine-baseline",
  "revision": "b20be7c865978759026d233e2d012ec8dc393b27"
}
```

- [ ] **Step 3: Create the trust anchor**

`engine/allowed-signers` — exactly one line, no trailing content:

```
richard@liquescent.dev ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHyTKmfAwcJcdfKXmj2h3mwfgPaelE6gSMrquAcPmW09
```

- [ ] **Step 4: Write the failing contract test**

`tests/release/engine-pin-contract.sh`. Style follows `tests/release/source-input-contract.sh`; the SSH-signing fixture follows `tests/release/publish-contract.sh:155-157`.

```bash
#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
script=$repo_root/scripts/build-arca-engine.sh
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-engine-pin-contract.XXXXXX")
trap 'rm -rf "$fixture"' EXIT

# A local signing identity, so the positive case needs no network and no real key.
ssh-keygen -q -t ed25519 -N '' -C engine@example.invalid -f "$fixture/key"
printf 'engine@example.invalid %s\n' "$(cat "$fixture/key.pub")" >"$fixture/allowed-signers"

# An upstream repository standing in for Arca. It carries a Package.swift with a
# target named ContainerBridge so the build step has something real to compile.
upstream=$fixture/upstream
mkdir -p "$upstream/Sources/ContainerBridge"
cat >"$upstream/Package.swift" <<'PACKAGE'
// swift-tools-version: 6.2
import PackageDescription
let package = Package(
    name: "Arca",
    targets: [.target(name: "ContainerBridge")]
)
PACKAGE
printf 'public let engineFixture = 1\n' >"$upstream/Sources/ContainerBridge/Fixture.swift"
git -C "$upstream" init -q
git -C "$upstream" config user.name fixture
git -C "$upstream" config user.email engine@example.invalid
git -C "$upstream" config gpg.format ssh
git -C "$upstream" config user.signingKey "$fixture/key"
git -C "$upstream" add -A
git -C "$upstream" -c commit.gpgsign=false commit -qm seed
pinned=$(git -C "$upstream" rev-parse --verify HEAD)
git -C "$upstream" tag -s -m 'engine baseline' engine-baseline "$pinned"

# A second commit, so "tag points somewhere else" is expressible.
printf 'public let drift = 2\n' >"$upstream/Sources/ContainerBridge/Drift.swift"
git -C "$upstream" add -A
git -C "$upstream" -c commit.gpgsign=false commit -qm drift
drifted=$(git -C "$upstream" rev-parse --verify HEAD)
git -C "$upstream" tag -s -m 'moved' moved-tag "$drifted"
git -C "$upstream" tag unsigned-tag "$pinned"

write_pin() {
  jq -n --arg url "$upstream" --arg tag "$2" --arg rev "$3" \
    '{schema: 1, name: "arca", url: $url, tag: $tag, revision: $rev}' >"$1"
}

run_case() {
  # `actual=0; ... || actual=$?` and not a bare `$?` on the next line: this file
  # runs under `set -e`, so a non-zero exit would abort the test before the
  # status could be read, and every negative case would vanish silently.
  local label=$1 pin=$2 expected=$3 actual=0
  GASCAN_ARCA_PIN_FILE=$pin \
  GASCAN_ARCA_ENGINE_CACHE=$fixture/cache-$label \
  GASCAN_ARCA_ALLOWED_SIGNERS=$fixture/allowed-signers \
    bash "$script" >"$fixture/$label.out" 2>&1 || actual=$?
  [[ $actual == "$expected" ]] || {
    printf 'case %s: expected exit %s, got %s\n' "$label" "$expected" "$actual" >&2
    cat "$fixture/$label.out" >&2
    exit 1
  }
}

# 64 — malformed pin
write_pin "$fixture/pin-short.json" engine-baseline deadbeef
run_case short-revision "$fixture/pin-short.json" 64

jq -n '{schema: 1, name: "arca", url: "x", tag: "y"}' >"$fixture/pin-nokey.json"
run_case missing-revision "$fixture/pin-nokey.json" 64

# 65 — tag resolves to a different commit than the pin
write_pin "$fixture/pin-moved.json" moved-tag "$pinned"
run_case moved-tag "$fixture/pin-moved.json" 65

# 65 — tag carries no signature
write_pin "$fixture/pin-unsigned.json" unsigned-tag "$pinned"
run_case unsigned-tag "$fixture/pin-unsigned.json" 65

# 65 — pinned revision absent from the repository
write_pin "$fixture/pin-absent.json" engine-baseline 0000000000000000000000000000000000000000
run_case absent-revision "$fixture/pin-absent.json" 65

# 0 — well-formed pin, signed tag, tag resolves to the pinned revision
write_pin "$fixture/pin-good.json" engine-baseline "$pinned"
run_case good "$fixture/pin-good.json" 0
grep -q 'cache-good' "$fixture/good.out" || {
  printf 'success case did not print the checkout path\n' >&2
  exit 1
}

printf 'PASS: Gas Can engine pin contract\n'
```

Note the third override, `GASCAN_ARCA_ALLOWED_SIGNERS`, used only so the test can substitute its throwaway key. Production always uses the tracked file.

- [ ] **Step 5: Run the test to verify it fails**

```bash
cd ~/code/gascan
chmod +x tests/release/engine-pin-contract.sh
bash tests/release/engine-pin-contract.sh > /tmp/engine-pin-1.log 2>&1; echo "EXIT=$?"
tail -5 /tmp/engine-pin-1.log
```

Expected: non-zero, complaining that `scripts/build-arca-engine.sh` does not exist.

- [ ] **Step 6: Write the build script**

`scripts/build-arca-engine.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd -P)
pin_file=${GASCAN_ARCA_PIN_FILE:-$repo_root/engine/arca-pin.json}
cache_root=${GASCAN_ARCA_ENGINE_CACHE:-$repo_root/.artifacts/arca-engine}
allowed_signers=${GASCAN_ARCA_ALLOWED_SIGNERS:-$repo_root/engine/allowed-signers}

for command in git jq swift; do
  command -v "$command" >/dev/null || {
    printf 'required command is unavailable: %s\n' "$command" >&2
    exit 69
  }
done

[[ -f $pin_file ]] || {
  printf 'engine pin file is missing: %s\n' "$pin_file" >&2
  exit 64
}
[[ -f $allowed_signers ]] || {
  printf 'engine allowed-signers file is missing: %s\n' "$allowed_signers" >&2
  exit 64
}
jq -e '
  (.schema == 1) and
  (.name | type == "string" and length > 0) and
  (.url | type == "string" and length > 0) and
  (.tag | type == "string" and length > 0) and
  (.revision | type == "string" and test("^[0-9a-f]{40}$"))
' "$pin_file" >/dev/null 2>&1 || {
  printf 'engine pin file is malformed: %s\n' "$pin_file" >&2
  exit 64
}

url=$(jq -er '.url' "$pin_file")
tag=$(jq -er '.tag' "$pin_file")
revision=$(jq -er '.revision' "$pin_file")

checkout=$cache_root/arca
mkdir -p "$cache_root"
[[ -d $checkout/.git ]] || git clone --quiet "$url" "$checkout"
git -C "$checkout" remote set-url origin "$url"
# --force accepts a moved tag deliberately. A moved tag is not silently trusted:
# it fails below on the tag-target assertion, which is the real gate and reports
# the actual mismatch instead of an opaque fetch rejection.
git -C "$checkout" fetch --quiet --tags --force origin

git -C "$checkout" cat-file -e "${revision}^{commit}" 2>/dev/null || {
  printf 'pinned revision is absent from %s after fetch: %s\n' "$url" "$revision" >&2
  exit 65
}
git -C "$checkout" -c "gpg.ssh.allowedSignersFile=$allowed_signers" \
  verify-tag "$tag" >/dev/null 2>&1 || {
  printf 'engine pin tag signature does not verify against %s: %s\n' \
    "$allowed_signers" "$tag" >&2
  exit 65
}
tag_target=$(git -C "$checkout" rev-parse --verify "refs/tags/${tag}^{}") || {
  printf 'engine pin tag is absent: %s\n' "$tag" >&2
  exit 65
}
[[ $tag_target == "$revision" ]] || {
  printf 'engine pin tag %s resolves to %s, not the pinned revision %s\n' \
    "$tag" "$tag_target" "$revision" >&2
  exit 65
}

git -C "$checkout" checkout --quiet --detach "$revision"
git -C "$checkout" submodule update --init --recursive --quiet

swift build --package-path "$checkout" --configuration release --target ContainerBridge >&2

printf '%s\n' "$checkout"
```

- [ ] **Step 7: Run the test to verify it passes**

```bash
cd ~/code/gascan
chmod +x scripts/build-arca-engine.sh
bash tests/release/engine-pin-contract.sh > /tmp/engine-pin-2.log 2>&1; echo "EXIT=$?"
tail -5 /tmp/engine-pin-2.log
```

Expected: `EXIT=0` and `PASS: Gas Can engine pin contract`.

- [ ] **Step 8: Verify the script against the real pin**

The contract test proves rejection logic against a fixture. This proves the real pin. It takes several minutes on first run and is the P1.2 exit evidence — do not skip it, and do not report P1.2 done without it.

```bash
cd ~/code/gascan
./scripts/build-arca-engine.sh > /tmp/engine-real.log 2>&1; echo "EXIT=$?"
tail -3 /tmp/engine-real.log
```

Expected: `EXIT=0`, last line of stdout is the checkout path, and the log ends with `Build of target: 'ContainerBridge' complete!`. Record the wall time — it is the U3 datapoint.

- [ ] **Step 9: Make the pin a release input**

In `packaging/macos/release-common.sh`, the `inputs` array at :27-31 becomes:

```bash
  local -a inputs=(
    Cargo.toml Cargo.lock rust-toolchain.toml crates helpers proto engine
    scripts/build-apple-attach-helper.sh scripts/workspace-image-source-digest.sh
    scripts/build-arca-engine.sh packaging/macos LICENSE images/workspace
  )
```

The tracked-file loop at :36-37 becomes:

```bash
  for path in Cargo.toml Cargo.lock rust-toolchain.toml scripts/build-apple-attach-helper.sh \
    scripts/workspace-image-source-digest.sh scripts/build-arca-engine.sh \
    engine/arca-pin.json engine/allowed-signers LICENSE images/workspace; do
```

The ignored-source scan at :43-48 becomes:

```bash
  ignored_source=$(
    git -C "$repo" ls-files --others --ignored --exclude-standard -- \
      crates helpers proto engine packaging/macos scripts/build-apple-attach-helper.sh \
      scripts/build-arca-engine.sh images/workspace ':(exclude)helpers/apple-attach/.build/**' |
      awk '/^images\/workspace\// || /\.(rs|swift|toml|proto|sh|json)$/ || /(^|\/)Package\.swift$/ { print; exit }'
  )
```

Note `json` added to the awk extension list, so an ignored stray pin file is caught.

- [ ] **Step 10: Seed the new inputs in the existing source-input test**

Without this, `source-input-contract.sh` fails — its fixture repos would lack the newly required tracked files. In `tests/release/source-input-contract.sh`, `seed_repo` at :11 gains `engine` to the `mkdir -p` list, and the `seed_path` list at :12 gains two entries:

```bash
  mkdir -p "$root/crates" "$root/helpers" "$root/scripts" "$root/packaging/macos" "$root/proto" "$root/images/workspace" "$root/engine"
  for seed_path in Cargo.toml Cargo.lock crates/lib.rs helpers/helper.swift scripts/build-apple-attach-helper.sh scripts/workspace-image-source-digest.sh scripts/build-arca-engine.sh engine/arca-pin.json engine/allowed-signers packaging/macos/package.sh LICENSE rust-toolchain.toml proto/gascan.proto images/workspace/Dockerfile images/workspace/approved-image.txt images/workspace/approved-source.sha256 images/workspace/versions.lock; do
```

And `classes` at :20 gains the pin, proving it is guarded rather than merely listed:

```bash
classes=(rust-toolchain.toml proto/gascan.proto scripts/workspace-image-source-digest.sh engine/arca-pin.json images/workspace/Dockerfile images/workspace/approved-image.txt images/workspace/approved-source.sha256 images/workspace/versions.lock)
```

- [ ] **Step 11: Run both contract tests**

```bash
cd ~/code/gascan
bash tests/release/source-input-contract.sh > /tmp/source-input.log 2>&1; echo "SOURCE_INPUT_EXIT=$?"
bash tests/release/engine-pin-contract.sh > /tmp/engine-pin-3.log 2>&1; echo "ENGINE_PIN_EXIT=$?"
tail -2 /tmp/source-input.log /tmp/engine-pin-3.log
```

Expected: both `EXIT=0`, both printing their `PASS:` line.

- [ ] **Step 12: Shellcheck the new script**

```bash
cd ~/code/gascan
shellcheck scripts/build-arca-engine.sh tests/release/engine-pin-contract.sh > /tmp/shellcheck.log 2>&1; echo "EXIT=$?"
cat /tmp/shellcheck.log
```

Expected: `EXIT=0`, empty output. Fix any finding rather than adding a suppression.

- [ ] **Step 13: Commit**

```bash
cd ~/code/gascan
git add engine/arca-pin.json engine/allowed-signers scripts/build-arca-engine.sh \
  tests/release/engine-pin-contract.sh packaging/macos/release-common.sh \
  tests/release/source-input-contract.sh
git commit -m "feat: pin Arca at gascan-engine-baseline and build it from the pin

The pin carries both tag and revision and the build script asserts they agree.
Neither is sufficient alone: b20be7c is GitHub's merge commit and carries the
web-flow key, so the only maintainer signature at the pin is the annotated tag,
while the revision is what is actually built.

engine/allowed-signers tracks the trust anchor rather than inheriting ambient
git config. Verified that verify-tag exits 1 with 'No principal matched' when
the allowed-signers file lacks the signing key, so CI would otherwise fail for
a reason unrelated to the pin."
```

---

### Task 2: Manifest schema 2

**Files:**
- Modify: `packaging/macos/package.sh:49-50,82-89`
- Modify: `packaging/macos/verify-package.sh:49-60`
- Modify: `tests/release/installer-contract.sh:132`
- Modify: `tests/release/publish-contract.sh:205-218`

**Interfaces:**
- Consumes: `scripts/build-arca-engine.sh` from Task 1 — invoked for its exit status; its stdout is not used here.
- Produces: `build-manifest.json` at `schema: 2` with a top-level `engine` object `{name, url, tag, revision}`. `files[]` is unchanged at exactly three entries.

- [ ] **Step 1: Update the package verifier first**

This is the assertion the rest of the task has to satisfy, so it goes first and fails loudly until `package.sh` catches up. In `packaging/macos/verify-package.sh`, replace lines 49-60 with:

```bash
manifest=$work/root/usr/local/share/gascan/build-manifest.json
jq -e --arg revision "$expected_revision" --arg version "$expected_version" '
  . == {
    architecture: "arm64",
    engine: .engine,
    files: .files,
    product: "Gas Can",
    schema: 2,
    source_revision: $revision,
    version: $version
  } and
  (.engine | keys == ["name", "revision", "tag", "url"]) and
  (.engine.name | length > 0) and
  (.engine.url | startswith("https://")) and
  (.engine.tag | length > 0) and
  (.engine.revision | test("^[0-9a-f]{40}$")) and
  (.files | map(.path) == ["usr/local/bin/gascan", "usr/local/bin/gascan-apple-attach", "usr/local/bin/gascand"]) and
  all(.files[]; (.sha256 | test("^[0-9a-f]{64}$")))
' "$manifest" >/dev/null || { printf 'build manifest is invalid\n' >&2; exit 65; }
```

`jq`'s `keys` sorts, hence the alphabetical order in the `.engine | keys` assertion.

- [ ] **Step 2: Build the engine before emitting the manifest**

In `packaging/macos/package.sh`, after line 50's attach-helper call, add:

```bash
"$repo_root/scripts/build-arca-engine.sh" >&2
```

Ordering is the point: `set -euo pipefail` at `package.sh:2` makes a failed engine build fail the package, so the manifest can never claim a pin this release did not compile.

- [ ] **Step 3: Emit the engine object**

In `packaging/macos/package.sh`, replace the `jq -nS` block at :82-88 with:

```bash
engine_json=$(jq -cS '{name, url, tag, revision}' "$repo_root/engine/arca-pin.json")
jq -nS \
  --arg architecture arm64 \
  --arg source_revision "$revision" \
  --arg version "$version" \
  --argjson engine "$engine_json" \
  --argjson files "$files_json" \
  '{schema: 2, product: "Gas Can", version: $version, architecture: $architecture, source_revision: $source_revision, engine: $engine, files: $files}' \
  >"$root/usr/local/share/gascan/build-manifest.json"
```

The pin file's own `schema` key is dropped by the `{name, url, tag, revision}` projection, so two unrelated schema numbers never appear in one object.

- [ ] **Step 4: Update the installer-contract fixture**

In `tests/release/installer-contract.sh`, line 132's fixture manifest becomes (single line, as it is today):

```bash
printf "%s\\n" "{\"architecture\":\"arm64\",\"engine\":{\"name\":\"arca\",\"revision\":\"b20be7c865978759026d233e2d012ec8dc393b27\",\"tag\":\"gascan-engine-baseline\",\"url\":\"https://github.com/Vas-Solutus/arca.git\"},\"files\":[{\"path\":\"usr/local/bin/gascan\",\"sha256\":\"$FIXTURE_MANIFEST_HASH\"},{\"path\":\"usr/local/bin/gascan-apple-attach\",\"sha256\":\"$FIXTURE_MANIFEST_HASH\"},{\"path\":\"usr/local/bin/gascand\",\"sha256\":\"$FIXTURE_MANIFEST_HASH\"}],\"product\":\"Gas Can\",\"schema\":2,\"source_revision\":\"$FIXTURE_REVISION\",\"version\":\"0.1.0\"}" >usr/local/share/gascan/build-manifest.json'
```

- [ ] **Step 5: Update the publish-contract fixture**

In `tests/release/publish-contract.sh`, the `jq -n` block at :205-218 becomes:

```bash
jq -n --arg rev "$revision" --arg ver "$version" \
  --arg s1 "$sha_gascan" --arg s2 "$sha_attach" --arg s3 "$sha_gascand" '
{
  architecture: "arm64",
  engine: {
    name: "arca",
    revision: "b20be7c865978759026d233e2d012ec8dc393b27",
    tag: "gascan-engine-baseline",
    url: "https://github.com/Vas-Solutus/arca.git"
  },
  files: [
    {path: "usr/local/bin/gascan", sha256: $s1},
    {path: "usr/local/bin/gascan-apple-attach", sha256: $s2},
    {path: "usr/local/bin/gascand", sha256: $s3}
  ],
  product: "Gas Can",
  schema: 2,
  source_revision: $rev,
  version: $ver
}' >"$fixture_root/usr/local/share/gascan/build-manifest.json"
```

- [ ] **Step 6: Run the affected contract tests**

```bash
cd ~/code/gascan
for c in installer-contract publish-contract distributable-package-contract cask-contract; do
  bash "tests/release/$c.sh" > "/tmp/$c.log" 2>&1
  printf '%-40s EXIT=%s\n' "$c" "$?"
done
```

Expected: every line `EXIT=0`. On any failure read that log — do not proceed.

- [ ] **Step 7: Run the whole release contract suite**

The schema bump touches shared machinery, so the sweep is the real gate.

```bash
cd ~/code/gascan
for c in tests/release/*-contract.sh; do
  bash "$c" > "/tmp/$(basename "$c").log" 2>&1
  printf '%-50s EXIT=%s\n' "$(basename "$c")" "$?"
done
```

Expected: every line `EXIT=0`.

- [ ] **Step 8: Commit**

```bash
cd ~/code/gascan
git add packaging/macos/package.sh packaging/macos/verify-package.sh \
  tests/release/installer-contract.sh tests/release/publish-contract.sh
git commit -m "feat: record the Arca engine pin in build-manifest.json at schema 2

verify-package.sh asserts the manifest by exact object equality, so adding a key
is a breaking change whether or not it is labelled one; the schema bump says so.

files[] is deliberately unchanged. No engine binary ships because none exists
yet, so the pin is attested and the binary is not. package.sh builds the pin
before emitting the manifest, so the manifest cannot claim a pin the release
did not compile."
```

---

### Task 3: Pin-bump CI gate

**Files:**
- Create: `.github/workflows/engine-pin.yml`

**Interfaces:**
- Consumes: `scripts/build-arca-engine.sh` from Task 1.
- Produces: nothing consumed by later tasks.

- [ ] **Step 1: Establish which macOS runners this repository can actually use**

The spec marks this **PLAN** and unverified. Every existing job uses `ubuntu-24.04-arm`; Arca needs macOS 26 and Swift 6.3. Determine the truth before writing a label in.

```bash
cd ~/code/gascan
gh api repos/:owner/:repo/actions/runners --jq '.runners[] | {name, os: .os, labels: [.labels[].name]}' 2>&1 | head
gh api /repos/:owner/:repo/actions/runner-groups 2>&1 | head -5
```

Then check GitHub's current hosted-runner labels for macOS 26 arm64 in the Actions documentation. Record what you find — this is the U3-adjacent unknown the spec flagged.

- [ ] **Step 2: Write the workflow**

`.github/workflows/engine-pin.yml`. Replace `macos-26` in `runs-on` only if Step 1 established a different correct label.

```yaml
name: engine-pin

on:
  pull_request:
    paths:
      - engine/arca-pin.json
      - engine/allowed-signers
      - scripts/build-arca-engine.sh
      - .github/workflows/engine-pin.yml

permissions:
  contents: read

concurrency:
  group: engine-pin-${{ github.ref }}
  cancel-in-progress: true

jobs:
  build-engine:
    runs-on: macos-26
    timeout-minutes: 45
    steps:
      - uses: actions/checkout@v4

      - name: Report toolchain
        run: |
          swift --version
          sw_vers

      - name: Build the pinned Arca engine
        run: ./scripts/build-arca-engine.sh
```

No `submodules:` on the checkout — Gas Can has none, and the script initialises Arca's own.

- [ ] **Step 3: Verify the workflow parses**

```bash
cd ~/code/gascan
python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/engine-pin.yml')); print('YAML OK')"
```

Expected: `YAML OK`.

- [ ] **Step 4: Commit and push the branch**

```bash
cd ~/code/gascan
git add .github/workflows/engine-pin.yml
git commit -m "ci: compile the Arca engine pin on every pin-bump PR

Without this the gate fires only at release. 'Breakage presents as Gas Can's
build failing at pin-bump time' is the argument that chose a target split over
a build flag, and a release-only gate does not deliver it."
git push -u origin arca-integration
```

- [ ] **Step 5: Prove the workflow actually runs**

A workflow that has never run is a PLAN, not a VERIFIED gate. Open the PR and watch this job specifically.

```bash
cd ~/code/gascan
gh pr create --fill --base main --head arca-integration
gh run list --workflow=engine-pin.yml --limit 3
```

Then, once it completes:

```bash
gh run view --workflow=engine-pin.yml --log-failed > /tmp/engine-pin-ci.log 2>&1; echo "EXIT=$?"
tail -20 /tmp/engine-pin-ci.log
```

Expected: the run concludes `success`. If the runner label is unavailable or the image lacks Swift 6.3, **stop and report it** — self-hosted versus deferring the job to P2.1 is a decision for the maintainer, not something to work around silently.

- [ ] **Step 6: Record the outcome in the spec**

Update `docs/superpowers/specs/2026-08-05-arca-engine-pin-design.md` §4.5, replacing the **PLAN** marker with **VERIFIED** and the actual evidence — runner label, Swift version from the "Report toolchain" step, run URL, and wall time. If the runner turned out unavailable, record that instead, struck through in place per the document conventions. Do not delete the PLAN text.

```bash
cd ~/code/gascan
git add docs/superpowers/specs/2026-08-05-arca-engine-pin-design.md
git commit -m "docs: record the engine-pin CI runner outcome"
```

---

## Completion Criteria

P1.1 and P1.2 are done when all of the following have been **run**, not reasoned about:

- [ ] `./scripts/build-arca-engine.sh` exits 0 against the real pin, ending in `Build of target: 'ContainerBridge' complete!`
- [ ] `bash tests/release/engine-pin-contract.sh` exits 0
- [ ] Every `tests/release/*-contract.sh` exits 0
- [ ] `shellcheck` is clean on both new scripts
- [ ] The `engine-pin` workflow has a concluded run, success or an explicitly reported blocker
- [ ] The spec's §4.5 PLAN marker is resolved either way

**Explicitly not in scope** (spec §8): no engine binary in `files[]`, no entitlements signing, no Docker-semantics removal, and no change to Arca. Do not start P6, and do not attempt U5 or U6.
