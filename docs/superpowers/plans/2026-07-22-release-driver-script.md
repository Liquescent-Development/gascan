# Release Driver Script Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One command, `./packaging/macos/release.sh <version>`, that drives an
already-tagged macOS release from build through published Homebrew cask, plus a
`--check` mode that verifies readiness without touching anything.

**Architecture:** A bash driver composing the existing `package.sh`,
`verify-package.sh`, `publish.sh`, and `render-cask.sh` rather than
reimplementing any of them. Four required configuration values resolve by
precedence (flag, environment, `~/.config/gascan/release.env`) with nothing
defaulted. Every gate runs before anything is built.

**Tech Stack:** bash, git, gh, jq, cargo, pkgutil, codesign, spctl, xcrun,
ruby, brew.

Design: `docs/superpowers/specs/2026-07-22-release-driver-script-design.md`

## Global Constraints

- Bash with `set -euo pipefail`, matching the existing `packaging/macos/*.sh`.
- **No hardcoded signing identity, team identifier, or notary profile string.**
  Contract-tested.
- No key, password, or API credential accepted as flag, environment value, or
  config entry. Names only.
- The config file is parsed as data, never `source`d or `eval`ed.
- The script never creates, moves, or deletes a git tag; never passes
  `--cleanup-tag`; never deletes a GitHub release.
- `--check` performs no mutation.
- Reuse a package only when `verify-package.sh` passes for that tag's revision
  and version *and* `gascan_assert_distributable_package` passes.
- Restore the operator's original git ref on every exit path.
- Never name a shell variable `status` — it is read-only in zsh and has
  previously made a successful release look like a failure.
- `shellcheck` clean on every shell file added or edited.
- `packaging/macos/` is a release input: this cannot drive the release that
  introduces it.

---

## File Structure

| File | Responsibility |
| --- | --- |
| `packaging/macos/release-config.sh` | Resolve one config value by precedence; parse the config file as data |
| `packaging/macos/release-gates.sh` | Every pre-flight assertion, one function each |
| `packaging/macos/release-recovery.sh` | What to tell an operator when a step fails after the release is public |
| `packaging/macos/release.sh` | Argument parsing, gate sequencing, execution |
| `tests/release/release-script-contract.sh` | Behavioral + source contract |
| `docs/release/macos-checklist.md` | Document the one-command path |

Gates live in their own file so each is independently testable and `release.sh`
stays a readable sequence.

---

### Task 1: Configuration resolution

**Files:**
- Create: `packaging/macos/release-config.sh`
- Create: `tests/release/release-script-contract.sh`

**Interfaces:**
- Produces: `gascan_release_config NAME FLAG_VALUE CONFIG_FILE` — echoes the
  resolved value, or returns 65 after naming the missing value and all three
  supply routes.

- [ ] **Step 1: Write the failing test**

Create `tests/release/release-script-contract.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-release-contract.XXXXXX")
trap 'rm -rf "$fixture"' EXIT

config=$repo_root/packaging/macos/release-config.sh
[[ -r $config ]] || { printf 'release-config.sh is not readable\n' >&2; exit 1; }
# shellcheck source=/dev/null
source "$config"

cat >"$fixture/release.env" <<'EOF_CONFIG'
GASCAN_NOTARYTOOL_PROFILE=from-file
GASCAN_TAP_PATH=/tmp/tap-from-file
EOF_CONFIG

observed=$(GASCAN_NOTARYTOOL_PROFILE= gascan_release_config \
  GASCAN_NOTARYTOOL_PROFILE '' "$fixture/release.env")
[[ $observed == from-file ]] || {
  printf 'config file value not used: %s\n' "$observed" >&2; exit 1; }

observed=$(GASCAN_NOTARYTOOL_PROFILE=from-env gascan_release_config \
  GASCAN_NOTARYTOOL_PROFILE '' "$fixture/release.env")
[[ $observed == from-env ]] || {
  printf 'environment did not beat config file: %s\n' "$observed" >&2; exit 1; }

observed=$(GASCAN_NOTARYTOOL_PROFILE=from-env gascan_release_config \
  GASCAN_NOTARYTOOL_PROFILE from-flag "$fixture/release.env")
[[ $observed == from-flag ]] || {
  printf 'flag did not beat environment: %s\n' "$observed" >&2; exit 1; }

set +e
missing=$(GASCAN_CODESIGN_IDENTITY= gascan_release_config \
  GASCAN_CODESIGN_IDENTITY '' "$fixture/release.env" 2>&1 >/dev/null)
missing_code=$?
set -e
[[ $missing_code -ne 0 ]] || {
  printf 'a missing required value was accepted\n' >&2; exit 1; }
for needle in GASCAN_CODESIGN_IDENTITY --codesign-identity release.env; do
  grep -Fq -- "$needle" <<<"$missing" || {
    printf 'missing-value message omits %s: %s\n' "$needle" "$missing" >&2
    exit 1; }
done

cat >"$fixture/spaces.env" <<'EOF_SPACES'
GASCAN_CODESIGN_IDENTITY=Developer ID Application: Example LLC (TEAMID1234)
EOF_SPACES
observed=$(GASCAN_CODESIGN_IDENTITY= gascan_release_config \
  GASCAN_CODESIGN_IDENTITY '' "$fixture/spaces.env")
[[ $observed == 'Developer ID Application: Example LLC (TEAMID1234)' ]] || {
  printf 'value with spaces was mangled: %s\n' "$observed" >&2; exit 1; }

cat >"$fixture/hostile.env" <<'EOF_HOSTILE'
GASCAN_TAP_PATH=$(touch /tmp/gascan-config-executed)
EOF_HOSTILE
rm -f /tmp/gascan-config-executed
observed=$(GASCAN_TAP_PATH= gascan_release_config \
  GASCAN_TAP_PATH '' "$fixture/hostile.env")
if [[ -e /tmp/gascan-config-executed ]]; then
  rm -f /tmp/gascan-config-executed
  printf 'config file contents were executed\n' >&2
  exit 1
fi
[[ $observed == '$(touch /tmp/gascan-config-executed)' ]] || {
  printf 'hostile value not preserved literally: %s\n' "$observed" >&2; exit 1; }

observed=$(GASCAN_TAP_PATH=from-env gascan_release_config \
  GASCAN_TAP_PATH '' "$fixture/absent.env")
[[ $observed == from-env ]] || {
  printf 'absent config file broke resolution: %s\n' "$observed" >&2; exit 1; }

printf 'PASS: Gas Can release script contract\n'
```

- [ ] **Step 2: Run to verify it fails**

Run: `bash tests/release/release-script-contract.sh; echo "exit: $?"`
Expected: exit 1, `release-config.sh is not readable`.

- [ ] **Step 3: Implement**

Create `packaging/macos/release-config.sh`:

```bash
#!/usr/bin/env bash
# Resolve one required release configuration value.
#
# Nothing is defaulted. Signing identities and a keychain profile name are
# organization- and machine-specific, so a repository script that carries one
# operator's setup breaks silently for everyone else.
#
# Precedence, first match wins: flag, environment, config file.
#
# The config file is read as data. It is never sourced and never evaluated, so
# shell syntax inside it is a value rather than a command.

gascan_release_config_flag() {
  case $1 in
    GASCAN_CODESIGN_IDENTITY) printf -- '--codesign-identity';;
    GASCAN_INSTALLER_SIGNING_IDENTITY) printf -- '--installer-identity';;
    GASCAN_NOTARYTOOL_PROFILE) printf -- '--notary-profile';;
    GASCAN_TAP_PATH) printf -- '--tap';;
    *) return 1;;
  esac
}

# Value is everything after the first '=', preserved verbatim.
gascan_release_config_file_value() {
  local file=$1 key=$2 line
  [[ -r $file ]] || return 1
  while IFS= read -r line || [[ -n $line ]]; do
    [[ $line == "$key="* ]] || continue
    printf '%s' "${line#"$key="}"
    return 0
  done <"$file"
  return 1
}

gascan_release_config() {
  local name=$1 flag_value=$2 file=$3 value flag
  if [[ -n $flag_value ]]; then
    printf '%s' "$flag_value"
    return 0
  fi
  value=${!name-}
  if [[ -n $value ]]; then
    printf '%s' "$value"
    return 0
  fi
  if value=$(gascan_release_config_file_value "$file" "$name") && [[ -n $value ]]; then
    printf '%s' "$value"
    return 0
  fi
  flag=$(gascan_release_config_flag "$name") || flag='(no flag)'
  printf 'missing required release configuration: %s\n' "$name" >&2
  printf 'supply it with %s, the %s environment variable, or a %s= line in %s\n' \
    "$flag" "$name" "$name" "$file" >&2
  return 65
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `bash tests/release/release-script-contract.sh; echo "exit: $?"`
Expected: `PASS: Gas Can release script contract`, exit 0.

- [ ] **Step 5: shellcheck**

Run: `shellcheck packaging/macos/release-config.sh tests/release/release-script-contract.sh; echo "exit: $?"`
Expected: exit 0. If `shellcheck` is unavailable, report that rather than
skipping silently.

- [ ] **Step 6: Commit**

```bash
git add packaging/macos/release-config.sh tests/release/release-script-contract.sh
git commit -m "feat: resolve release configuration without defaults

Signing identities and a keychain profile name are organization- and
machine-specific, so a repository script that defaults them carries one
operator's setup and breaks silently for anyone else. Resolve each value by
flag, then environment, then a config file outside the repository, and fail
naming the value and all three routes when it is absent from all of them.

The config file is read line by line as data rather than sourced, so shell
syntax inside it is preserved as a value instead of executed."
```

---

### Task 2: Pre-flight gate functions

**Files:**
- Create: `packaging/macos/release-gates.sh`
- Modify: `tests/release/release-script-contract.sh`

**Interfaces:**
- Consumes: `gascan_assert_release_inputs_clean` from `release-common.sh`.
- Produces, each returning 0 on success and non-zero after printing a specific
  remediation:
  - `gascan_gate_tools`
  - `gascan_gate_version REPO VERSION`
  - `gascan_gate_tag REPO VERSION`
  - `gascan_gate_no_release VERSION`
  - `gascan_gate_identities APP_IDENTITY INSTALLER_IDENTITY`
  - `gascan_gate_notary PROFILE`
  - `gascan_gate_tap TAP_PATH`

- [ ] **Step 1: Write the failing tests**

Insert into `tests/release/release-script-contract.sh` immediately before the
final `printf 'PASS: ...'` line:

```bash
gates=$repo_root/packaging/macos/release-gates.sh
[[ -r $gates ]] || { printf 'release-gates.sh is not readable\n' >&2; exit 1; }
# shellcheck source=/dev/null
source "$repo_root/packaging/macos/release-common.sh"
# shellcheck source=/dev/null
source "$gates"

# Version disagreement is rejected and the message names the workspace version.
workspace_version=$(cd "$repo_root" && cargo metadata --locked --no-deps \
  --format-version 1 | jq -er '.packages[] | select(.name == "gascan") | .version')
set +e
mismatch=$(gascan_gate_version "$repo_root" 99.99.99 2>&1 >/dev/null)
mismatch_code=$?
set -e
[[ $mismatch_code -ne 0 ]] || {
  printf 'version disagreement accepted\n' >&2; exit 1; }
grep -Fq "$workspace_version" <<<"$mismatch" || {
  printf 'mismatch message omits workspace version: %s\n' "$mismatch" >&2; exit 1; }
gascan_gate_version "$repo_root" "$workspace_version" >/dev/null || {
  printf 'the workspace version was rejected\n' >&2; exit 1; }

# Tag gates, exercised behaviorally in a disposable clone with an ephemeral
# signing key -- the technique publish-contract.sh uses, so the property holds
# for whatever version the workspace carries.
clone=$fixture/clone
git clone --quiet "$repo_root" "$clone"
ssh-keygen -q -t ed25519 -N '' -C release@example.invalid -f "$fixture/key"
printf 'release@example.invalid %s\n' "$(cat "$fixture/key.pub")" \
  >"$fixture/allowed-signers"
git -C "$clone" config user.name release
git -C "$clone" config user.email release@example.invalid
git -C "$clone" config gpg.format ssh
git -C "$clone" config user.signingKey "$fixture/key"
git -C "$clone" config gpg.ssh.allowedSignersFile "$fixture/allowed-signers"
# Use a version no real tag carries. The clone's origin IS this repository, so
# a real released version would already be present on the remote and the
# unpushed case could never be exercised.
probe_version=99.99.99
tag=v$probe_version

# absent tag
if gascan_gate_tag "$clone" "$probe_version" >/dev/null 2>&1; then
  printf 'an absent tag was accepted\n' >&2; exit 1
fi

# lightweight tag
git -C "$clone" tag "$tag"
if gascan_gate_tag "$clone" "$probe_version" >/dev/null 2>&1; then
  printf 'a lightweight tag was accepted\n' >&2; exit 1
fi
git -C "$clone" tag -d "$tag" >/dev/null

# annotated but unsigned
git -C "$clone" tag -a "$tag" -m unsigned
if gascan_gate_tag "$clone" "$probe_version" >/dev/null 2>&1; then
  printf 'an unsigned annotated tag was accepted\n' >&2; exit 1
fi
git -C "$clone" tag -d "$tag" >/dev/null

# signed but not pointing at HEAD
git -C "$clone" tag -s "$tag" -m 'not head' "$(git -C "$clone" rev-parse HEAD~1)"
if gascan_gate_tag "$clone" "$probe_version" >/dev/null 2>&1; then
  printf 'a tag that does not peel to HEAD was accepted\n' >&2; exit 1
fi
git -C "$clone" tag -d "$tag" >/dev/null

# signed, at HEAD, but absent from the remote
git -C "$clone" tag -s "$tag" -m 'at head'
set +e
unpushed=$(gascan_gate_tag "$clone" "$probe_version" 2>&1 >/dev/null)
unpushed_code=$?
set -e
[[ $unpushed_code -ne 0 ]] || {
  printf 'an unpushed tag was accepted\n' >&2; exit 1; }
grep -Fq 'git push origin' <<<"$unpushed" || {
  printf 'unpushed-tag message omits the push command: %s\n' "$unpushed" >&2
  exit 1; }

# Tap gate rejects a path that is not a git work tree.
if gascan_gate_tap "$fixture" >/dev/null 2>&1; then
  printf 'a non-repository tap path was accepted\n' >&2; exit 1
fi

# The push check, exercised against a local remote so the dry run has a real
# remote to authenticate and negotiate with. Without this the gate's central
# claim -- that it proves push access -- is asserted by nothing.
tap_origin=$fixture/tap-origin.git
tap=$fixture/tap
git init --quiet --bare --initial-branch=main "$tap_origin"
git init --quiet --initial-branch=main "$tap"
git -C "$tap" config user.name release
git -C "$tap" config user.email release@example.invalid
mkdir -p "$tap/Casks"
printf 'seed\n' >"$tap/README.md"
git -C "$tap" add README.md
git -C "$tap" commit --quiet -m seed
git -C "$tap" remote add origin "$tap_origin"
git -C "$tap" push --quiet origin main
gascan_gate_tap "$tap" >/dev/null || {
  printf 'a clean, pushable tap on main was rejected\n' >&2; exit 1; }

# `pushurl` is what makes this a *push* failure: fetch still resolves, so the
# gate reaches the dry run instead of stopping at the earlier fetch check.
git -C "$tap" config remote.origin.pushurl "$fixture/absent-remote.git"
set +e
unpushable=$(gascan_gate_tap "$tap" 2>&1 >/dev/null)
unpushable_code=$?
set -e
[[ $unpushable_code -ne 0 ]] || {
  printf 'a tap that cannot be pushed was accepted\n' >&2; exit 1; }
grep -Fq 'cannot push to origin/main' <<<"$unpushable" || {
  printf 'unpushable tap message is not about pushing: %s\n' "$unpushable" >&2
  exit 1; }
git -C "$tap" config --unset remote.origin.pushurl

# A tap holding a commit the remote does not have must not be told to pull.
printf 'ahead\n' >>"$tap/README.md"
git -C "$tap" commit --quiet -am ahead
set +e
ahead=$(gascan_gate_tap "$tap" 2>&1 >/dev/null)
ahead_code=$?
set -e
[[ $ahead_code -ne 0 ]] || {
  printf 'a tap ahead of origin/main was accepted\n' >&2; exit 1; }
grep -Fq "git -C $tap push origin main" <<<"$ahead" || {
  printf 'an ahead tap was not told to push: %s\n' "$ahead" >&2; exit 1; }
```

- [ ] **Step 2: Run to verify it fails**

Run: `bash tests/release/release-script-contract.sh; echo "exit: $?"`
Expected: exit 1, `release-gates.sh is not readable`.

- [ ] **Step 3: Implement**

Create `packaging/macos/release-gates.sh`:

```bash
#!/usr/bin/env bash
# Pre-flight assertions for a macOS release.
#
# Each gate returns non-zero after printing the specific command that fixes it.
# They run before anything is built, because the expensive failures surface late
# otherwise: a lapsed Apple agreement returns 403 only at notarization, and an
# unpushed tag aborts publish after the build.

gascan_gate_tools() {
  local command
  for command in gh jq cargo pkgutil shasum ruby brew git codesign spctl xcrun; do
    command -v "$command" >/dev/null || {
      printf 'required release command is unavailable: %s\n' "$command" >&2
      return 69
    }
  done
}

gascan_gate_version() {
  local repo=$1 version=$2 workspace
  workspace=$(cd "$repo" && cargo metadata --locked --no-deps --format-version 1 |
    jq -er '.packages[] | select(.name == "gascan") | .version') || return 65
  [[ $workspace == "$version" ]] || {
    printf 'workspace version is %s, not %s; bump the crates first\n' \
      "$workspace" "$version" >&2
    return 65
  }
}

gascan_gate_tag() {
  local repo=$1 version=$2 tag="v$2" object_type target head
  object_type=$(git -C "$repo" cat-file -t "refs/tags/$tag" 2>/dev/null) || object_type=
  [[ $object_type == tag ]] || {
    printf 'release tag %s is missing or not an annotated tag\n' "$tag" >&2
    printf "create it with: git tag -s %s -m 'Gas Can %s'\n" "$tag" "$version" >&2
    return 65
  }
  git -C "$repo" verify-tag "refs/tags/$tag" >/dev/null 2>&1 || {
    printf 'release tag %s does not carry a trusted signature\n' "$tag" >&2
    return 65
  }
  target=$(git -C "$repo" rev-parse --verify "refs/tags/$tag^{}") || return 65
  head=$(git -C "$repo" rev-parse --verify HEAD) || return 65
  [[ $target == "$head" ]] || {
    printf 'release tag %s does not point at HEAD (%s vs %s)\n' \
      "$tag" "${target:0:9}" "${head:0:9}" >&2
    return 65
  }
  [[ -n $(git -C "$repo" ls-remote --tags origin "$tag" 2>/dev/null) ]] || {
    printf 'release tag %s is not on the remote\n' "$tag" >&2
    printf 'push it with: git push origin %s\n' "$tag" >&2
    return 65
  }
}

gascan_gate_no_release() {
  local version=$1 tag="v$1" draft
  gh release view "$tag" >/dev/null 2>&1 || return 0
  draft=$(gh release view "$tag" --json isDraft --jq '.isDraft' 2>/dev/null) || draft=unknown
  printf 'a release for %s already exists (draft: %s)\n' "$tag" "$draft" >&2
  printf 'a published release is never overwritten; delete a stranded draft with:\n' >&2
  printf '  gh release delete %s --yes\n' "$tag" >&2
  printf 'do not add --cleanup-tag: it deletes the signed tag from the remote\n' >&2
  return 65
}

gascan_gate_identities() {
  local application=$1 installer=$2 identities
  identities=$(security find-identity -v 2>/dev/null) || {
    printf 'could not list keychain identities\n' >&2
    return 65
  }
  grep -Fq "$application" <<<"$identities" || {
    printf 'Developer ID Application identity is not in the keychain: %s\n' \
      "$application" >&2
    return 65
  }
  grep -Fq "$installer" <<<"$identities" || {
    printf 'Developer ID Installer identity is not in the keychain: %s\n' \
      "$installer" >&2
    return 65
  }
}

gascan_gate_notary() {
  local profile=$1 output
  # This gate exists so a lapsed Apple agreement costs two seconds rather than a
  # full build: notarization is the last step and the first to reject the account.
  output=$(xcrun notarytool history --keychain-profile "$profile" 2>&1) || {
    printf 'notarization profile %s cannot be used:\n%s\n' "$profile" "$output" >&2
    printf 'store one with: xcrun notarytool store-credentials %s ...\n' "$profile" >&2
    return 65
  }
  grep -Fq 'Successfully received submission history' <<<"$output" || {
    printf 'notarization profile %s did not authenticate:\n%s\n' "$profile" "$output" >&2
    return 65
  }
}

gascan_gate_tap() {
  local tap=$1 branch local_head remote_head
  [[ -d $tap ]] || {
    printf 'tap path does not exist: %s\n' "$tap" >&2
    return 65
  }
  git -C "$tap" rev-parse --is-inside-work-tree >/dev/null 2>&1 || {
    printf 'tap path is not a git work tree: %s\n' "$tap" >&2
    return 65
  }
  [[ -z $(git -C "$tap" status --porcelain) ]] || {
    printf 'tap has uncommitted changes: %s\n' "$tap" >&2
    return 65
  }
  branch=$(git -C "$tap" symbolic-ref --quiet --short HEAD) || branch=
  [[ $branch == main ]] || {
    printf 'tap is on %s, not main: %s\n' "${branch:-a detached HEAD}" "$tap" >&2
    return 65
  }
  git -C "$tap" fetch --quiet origin main || {
    printf 'could not fetch origin/main in the tap: %s\n' "$tap" >&2
    return 65
  }
  local_head=$(git -C "$tap" rev-parse HEAD) || return 65
  remote_head=$(git -C "$tap" rev-parse origin/main) || return 65
  if [[ $local_head != "$remote_head" ]]; then
    # A tap that is *ahead* is what a failed push after a successful commit
    # leaves behind, and `pull --ff-only` does not resolve that. Advising it
    # would contradict the recovery the driver itself prints.
    if git -C "$tap" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'tap has a commit that is not on origin/main: %s\n' "$tap" >&2
      printf 'run: git -C %s push origin main\n' "$tap" >&2
    elif git -C "$tap" merge-base --is-ancestor "$local_head" "$remote_head"; then
      printf 'tap is behind origin/main: %s\n' "$tap" >&2
      printf 'run: git -C %s pull --ff-only origin main\n' "$tap" >&2
    else
      # Neither is an ancestor of the other, so a fast-forward cannot resolve
      # it and advising one would send the operator in a circle.
      printf 'tap has diverged from origin/main: %s\n' "$tap" >&2
      printf 'reconcile it by hand before releasing\n' >&2
    fi
    return 65
  fi
  # Fetching proves read access; the release needs write access. Without this,
  # a missing or expired push credential surfaces only after the GitHub release
  # is already public -- exactly the late, expensive failure these gates exist
  # to move forward. A dry run authenticates and negotiates refs, then stops.
  git -C "$tap" push --dry-run --quiet origin main || {
    printf 'cannot push to origin/main in the tap: %s\n' "$tap" >&2
    printf 'check the credential for that remote before releasing\n' >&2
    return 65
  }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `bash tests/release/release-script-contract.sh; echo "exit: $?"`
Expected: `PASS`, exit 0.

- [ ] **Step 5: shellcheck**

Run: `shellcheck packaging/macos/release-gates.sh; echo "exit: $?"`
Expected: exit 0.

- [ ] **Step 6: Commit**

```bash
git add packaging/macos/release-gates.sh tests/release/release-script-contract.sh
git commit -m "feat: assert every release precondition before building

Releases fail late and expensively: a lapsed Apple agreement returns 403 only
at notarization, an unpushed tag aborts publish after the build, and a stranded
draft blocks the version with a documented recovery that deletes the signed tag
if used carelessly.

Each gate returns the command that fixes it. The stranded-draft gate prints a
recovery deliberately without --cleanup-tag, and the notarization gate turns the
403 into a two-second check."
```

---

### Task 3: Driver, argument parsing, and `--check`

**Files:**
- Create: `packaging/macos/release.sh` (`chmod +x`)
- Modify: `tests/release/release-script-contract.sh`

**Interfaces:**
- Consumes: Tasks 1 and 2.
- Produces: `release.sh VERSION [--check] [--codesign-identity V]
  [--installer-identity V] [--notary-profile V] [--tap PATH] [--config FILE]`.

- [ ] **Step 1: Write the failing tests**

Insert before the final `printf 'PASS: ...'`:

```bash
release=$repo_root/packaging/macos/release.sh
[[ -x $release ]] || { printf 'release.sh is not executable\n' >&2; exit 1; }

if "$release" >/dev/null 2>&1; then
  printf 'missing version accepted\n' >&2; exit 1
fi
[[ $("$release" 2>&1 >/dev/null | head -1) == usage:* ]] || {
  printf 'no usage line on a missing version\n' >&2; exit 1; }

if "$release" 1.2 --check >/dev/null 2>&1; then
  printf 'malformed version accepted\n' >&2; exit 1
fi

if "$release" 1.2.3 --nonsense >/dev/null 2>&1; then
  printf 'unknown flag accepted\n' >&2; exit 1
fi

# A value-taking flag without a value must say so. Left to `shift 2`, the
# script aborts under `set -e` with exit 1 and no output at all, which tells an
# operator who mistyped a flag nothing.
for incomplete in --codesign-identity --installer-identity --notary-profile \
  --tap --config; do
  set +e
  refused=$("$release" 1.2.3 "$incomplete" 2>&1 >/dev/null)
  refused_code=$?
  set -e
  [[ $refused_code -eq 64 ]] || {
    printf '%s without a value exited %s, not 64\n' "$incomplete" "$refused_code" >&2
    exit 1; }
  grep -Fq 'requires a value' <<<"$refused" || {
    printf '%s without a value said nothing useful: %s\n' "$incomplete" "$refused" >&2
    exit 1; }
  # An empty value is the same mistake wearing a disguise.
  set +e
  "$release" 1.2.3 "$incomplete" '' >/dev/null 2>&1
  empty_code=$?
  set -e
  [[ $empty_code -eq 64 ]] || {
    printf '%s with an empty value exited %s, not 64\n' "$incomplete" "$empty_code" >&2
    exit 1; }
  # Nor may a flag swallow the flag that follows it. `--config --check` taking
  # `--check` as a path drops the flag that makes the run read-only.
  set +e
  "$release" 1.2.3 "$incomplete" --check >/dev/null 2>&1
  swallow_code=$?
  set -e
  [[ $swallow_code -eq 64 ]] || {
    printf '%s swallowed the following flag, exiting %s not 64\n' \
      "$incomplete" "$swallow_code" >&2
    exit 1; }
done

# A missing required config value stops the run.
set +e
unconfigured=$(env -u GASCAN_CODESIGN_IDENTITY -u GASCAN_INSTALLER_SIGNING_IDENTITY \
  -u GASCAN_NOTARYTOOL_PROFILE -u GASCAN_TAP_PATH \
  "$release" "$workspace_version" --check --config "$fixture/absent.env" 2>&1 >/dev/null)
unconfigured_code=$?
set -e
[[ $unconfigured_code -ne 0 ]] || {
  printf 'a run with no configuration was accepted\n' >&2; exit 1; }
grep -Fq 'missing required release configuration' <<<"$unconfigured" || {
  printf 'no missing-configuration message: %s\n' "$unconfigured" >&2; exit 1; }

# Mutation is asserted behaviorally, not by source grep. Remediation text
# legitimately contains `git tag -s` and `gh release delete`, so grepping the
# source cannot distinguish a printed suggestion from an executed command.
# Stub git and gh, run --check, and require the log shows no mutation.
stub_bin=$fixture/bin
mkdir -p "$stub_bin"
export GASCAN_STUB_LOG=$fixture/commands.log
: >"$GASCAN_STUB_LOG"
cat >"$stub_bin/gh" <<'STUB_GH'
#!/usr/bin/env bash
printf 'gh %s\n' "$*" >>"${GASCAN_STUB_LOG:?}"
case "${1:-} ${2:-}" in
  'release view') exit 1 ;;
esac
exit 0
STUB_GH
cat >"$stub_bin/git" <<'STUB_GIT'
#!/usr/bin/env bash
printf 'git %s\n' "$*" >>"${GASCAN_STUB_LOG:?}"
# Refuse the mutation instead of passing it through. `origin` here is a live
# remote, so an unconditional exec would let a regression create or push a tag
# for real and only fail the assertion afterwards -- the log scan below runs
# after the whole run finishes. Read-only subcommands still reach real git,
# because the gates need its actual output, and `--dry-run` is read-only: the
# tap gate uses `push --dry-run` to prove the push credential works.
if [[ " $* " != *' push --dry-run '* ]]; then
  for word in "$@"; do
    case $word in
      tag|push|--cleanup-tag)
        printf 'stub git refused a mutating subcommand: %s\n' "$*" >&2
        exit 70 ;;
    esac
  done
fi
exec /usr/bin/git "$@"
STUB_GIT
chmod +x "$stub_bin/gh" "$stub_bin/git"

set +e
PATH=$stub_bin:$PATH GASCAN_CODESIGN_IDENTITY=x GASCAN_INSTALLER_SIGNING_IDENTITY=x \
  GASCAN_NOTARYTOOL_PROFILE=x GASCAN_TAP_PATH="$fixture" \
  "$release" "$workspace_version" --check >/dev/null 2>&1
set -e

# The log must not be empty: every gate that runs shells out to git or gh, so
# an empty log means the script died before reaching them and the scan below
# would pass without inspecting anything.
[[ -s $GASCAN_STUB_LOG ]] || {
  printf 'check mode ran no git or gh commands; the mutation scan proved nothing\n' >&2
  exit 1; }

# Match the subcommand anywhere in the line, not at the start: every call in
# this codebase is `git -C REPO ...`, so a prefix pattern like `git tag`* could
# never match the very mutation it exists to catch.
while IFS= read -r logged; do
  # A dry-run push is read-only; the tap gate uses one to prove push access.
  # Scoped to that exact pair so `--dry-run` cannot excuse anything else.
  case " $logged " in
    *' push --dry-run '*) continue ;;
  esac
  case " $logged " in
    *' tag '*|*' push '*|*'--cleanup-tag'*|*' release delete '*)
      printf 'check mode attempted a mutation: %s\n' "$logged" >&2
      exit 1 ;;
  esac
  # --check must never leave the operator's ref. The checkout belongs to the
  # release path, which runs only after --check has already exited.
  case " $logged " in
    *' checkout '*)
      printf 'check mode moved the working tree: %s\n' "$logged" >&2
      exit 1 ;;
  esac
done <"$GASCAN_STUB_LOG"

# `--cleanup-tag` must never be part of an executed deletion: it removes the
# signed tag from the remote. The gates deliberately *warn* about the flag in
# prose, which is the line that stops an operator destroying their own tag, so
# assert the dangerous construction rather than the string.
for f in "$release" "$gates"; do
  if grep -Eq 'gh release delete.*--cleanup-tag' "$f"; then
    printf '%s would delete a tag via --cleanup-tag\n' "$f" >&2; exit 1
  fi
done

# No hardcoded identity, team identifier, or profile.
for f in "$release" "$gates" "$config"; do
  if grep -Eq 'Developer ID (Application|Installer): [A-Za-z]|\([A-Z0-9]{10}\)' "$f"; then
    printf 'hardcoded signing identity or team identifier in %s\n' "$f" >&2
    exit 1
  fi
done
```

- [ ] **Step 2: Run to verify it fails**

Run: `bash tests/release/release-script-contract.sh; echo "exit: $?"`
Expected: exit 1, `release.sh is not executable`.

- [ ] **Step 3: Implement**

Create `packaging/macos/release.sh` and `chmod +x` it:

```bash
#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
# shellcheck source=release-common.sh
source "$repo_root/packaging/macos/release-common.sh"
# shellcheck source=release-config.sh
source "$repo_root/packaging/macos/release-config.sh"
# shellcheck source=release-gates.sh
source "$repo_root/packaging/macos/release-gates.sh"

usage() {
  cat >&2 <<'EOF_USAGE'
usage: release.sh VERSION [--check]
                  [--codesign-identity NAME] [--installer-identity NAME]
                  [--notary-profile NAME] [--tap PATH] [--config FILE]

Drives an already-tagged release: verifies every gate, then builds, signs,
notarizes, publishes, and updates the Homebrew cask.

  --check   run every gate and exit without building or publishing

Configuration resolves by flag, then environment, then the config file
(default: ${XDG_CONFIG_HOME:-$HOME/.config}/gascan/release.env). Nothing is
defaulted.

This never creates, moves, or deletes a tag. Create and push the signed tag
first:
    git tag -s vVERSION -m 'Gas Can VERSION' && git push origin vVERSION
EOF_USAGE
}

version=
check_only=false
flag_application=
flag_installer=
flag_profile=
flag_tap=
config_file="${XDG_CONFIG_HOME:-$HOME/.config}/gascan/release.env"

# Called as `require_value "$@"`, so $1 is the flag and $2 its value. Without
# this, a flag given as the last token leaves `shift 2` nothing to shift, and
# `set -e` aborts the script with exit 1 and not one word of explanation.
require_value() {
  [[ $# -ge 2 && -n $2 ]] || {
    printf '%s requires a value\n' "$1" >&2
    usage
    exit 64
  }
  # A following flag is not a value. `--config --check` would otherwise take
  # `--check` as the config path and silently drop the flag that makes this run
  # read-only, turning a rehearsal into a real release.
  [[ $2 != -* ]] || {
    printf '%s requires a value, but the next argument is a flag: %s\n' "$1" "$2" >&2
    usage
    exit 64
  }
}

while [[ $# -gt 0 ]]; do
  case $1 in
    --check) check_only=true; shift;;
    --codesign-identity) require_value "$@"; flag_application=$2; shift 2;;
    --installer-identity) require_value "$@"; flag_installer=$2; shift 2;;
    --notary-profile) require_value "$@"; flag_profile=$2; shift 2;;
    --tap) require_value "$@"; flag_tap=$2; shift 2;;
    --config) require_value "$@"; config_file=$2; shift 2;;
    -h|--help) usage; exit 0;;
    -*) printf 'unknown flag: %s\n' "$1" >&2; usage; exit 64;;
    *)
      [[ -z $version ]] || { printf 'unexpected argument: %s\n' "$1" >&2; usage; exit 64; }
      version=$1; shift;;
  esac
done

[[ -n $version ]] || { usage; exit 64; }
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  printf 'version must be MAJOR.MINOR.PATCH, got: %s\n' "$version" >&2
  usage
  exit 64
}

application_identity=$(gascan_release_config GASCAN_CODESIGN_IDENTITY "$flag_application" "$config_file")
installer_identity=$(gascan_release_config GASCAN_INSTALLER_SIGNING_IDENTITY "$flag_installer" "$config_file")
notary_profile=$(gascan_release_config GASCAN_NOTARYTOOL_PROFILE "$flag_profile" "$config_file")
tap_path=$(gascan_release_config GASCAN_TAP_PATH "$flag_tap" "$config_file")

cd "$repo_root"
printf 'checking release preconditions for %s\n' "$version" >&2
gascan_gate_tools
gascan_gate_version "$repo_root" "$version"
gascan_assert_release_inputs_clean "$repo_root" "release $version"
gascan_gate_tag "$repo_root" "$version"
gascan_gate_no_release "$version"
gascan_gate_identities "$application_identity" "$installer_identity"
gascan_gate_notary "$notary_profile"
gascan_gate_tap "$tap_path"
printf 'all release preconditions pass for %s\n' "$version" >&2

if [[ $check_only == true ]]; then
  printf 'check only: nothing was built, published, or changed\n' >&2
  exit 0
fi
```

- [ ] **Step 4: Run to verify it passes**

Run: `bash tests/release/release-script-contract.sh; echo "exit: $?"`
Expected: `PASS`, exit 0.

- [ ] **Step 5: shellcheck**

Run: `shellcheck packaging/macos/release.sh; echo "exit: $?"`
Expected: exit 0.

- [ ] **Step 6: Commit**

```bash
git add packaging/macos/release.sh tests/release/release-script-contract.sh
git commit -m "feat: add a gated release driver with a read-only check mode

One command replaces a dozen across two repositories. --check runs every gate
and exits, so readiness is verifiable in seconds instead of after a ten-minute
build. Configuration resolves by flag, environment, then a config file outside
the repository, with nothing defaulted."
```

---

### Task 4: Build, publish, and tap execution

**Files:**
- Create: `packaging/macos/release-recovery.sh`
- Modify: `packaging/macos/release.sh`
- Modify: `tests/release/release-script-contract.sh`

**Interfaces:**
- Consumes: Task 3.
- Produces: the full release path after gates pass, plus
  `gascan_print_release_values ASSET_URL CHECKSUM` and
  `gascan_report_live_release VERSION TAP_PATH REPO_ROOT STAGE ASSET_URL
  CHECKSUM PUBLISHED`, both writing to stdout so a caller chooses the stream.

The recovery narrator lives in its own file for one reason: it runs only when
a release is already public, so a driver-embedded version can be exercised by
nothing short of a live release. As a function taking its inputs, every stage
and the malformed-value case are assertable in milliseconds.

- [ ] **Step 1: Write the failing tests**

Insert before the final `printf 'PASS: ...'`:

```bash
# `macos/package.sh`, not `package.sh`: the bare form is a substring of
# `verify-package.sh`, so the needle would be satisfied by a different line and
# could never fail.
for needle in verify-package.sh gascan_assert_distributable_package \
  render-cask.sh publish.sh macos/package.sh; do
  grep -Fq "$needle" "$release" || {
    printf 'release.sh never references %s\n' "$needle" >&2; exit 1; }
done
grep -Eq 'trap .*EXIT' "$release" || {
  printf 'release.sh does not restore the original ref on exit\n' >&2; exit 1; }
grep -Eq 'trap .*INT TERM' "$release" || {
  printf 'release.sh does not name its interrupted exit status\n' >&2; exit 1; }
# A failure after publish must say the release is already live, because the
# recovery an operator reaches for otherwise deletes a published release. The
# warning lives in release-recovery.sh, so assert it there and assert
# release.sh actually reaches it: a warning nothing calls is no warning, and
# grepping release.sh for the phrase would be satisfied by any comment that
# happens to contain it.
grep -Fq 'release-recovery.sh' "$release" || {
  printf 'release.sh does not source the recovery\n' >&2; exit 1; }
grep -Fq 'gascan_report_live_release' "$release" || {
  printf 'release.sh never calls the recovery\n' >&2; exit 1; }
# `status` is read-only in zsh and has previously made a successful release
# look like a failure. `local status=` is how it would most likely reappear.
# Written as an `if` so an absent pattern -- the passing case -- does not trip
# `set -e`.
if grep -Eq '(^|[[:space:]])(local|declare|readonly|typeset)?[[:space:]]*status=' "$release"; then
  printf 'release.sh assigns a variable named status\n' >&2
  exit 1
fi

# The recovery narrator, exercised directly. It runs only when a release is
# already public, so without this it would be covered by a single grep -- and
# the defects found in it so far were exactly the kind a grep cannot see.
recovery=$repo_root/packaging/macos/release-recovery.sh
[[ -r $recovery ]] || { printf 'release-recovery.sh is not readable\n' >&2; exit 1; }
grep -Fq 'already published' "$recovery" || {
  printf 'the recovery never warns that the release is already published\n' >&2
  exit 1; }
# shellcheck source=/dev/null
source "$recovery"

good_url=https://github.com/example/gascan/releases/download/v1.2.3/gascan.pkg
good_sum=$(printf 'x' | shasum -a 256 | awk '{print $1}')

# Every stage names the commands that remain and none that are already done.
for stage in none rendered staged committed; do
  advice=$(gascan_report_live_release 1.2.3 /tmp/tap /tmp/repo "$stage" \
    "$good_url" "$good_sum" "$good_url"$'\n'"$good_sum")
  grep -Fq 'already published' <<<"$advice" || {
    printf 'stage %s does not say the release is published\n' "$stage" >&2; exit 1; }
  grep -Fq "$good_sum" <<<"$advice" || {
    printf 'stage %s omits the checksum\n' "$stage" >&2; exit 1; }
  grep -Fq 'git -C /tmp/tap push origin main' <<<"$advice" || {
    printf 'stage %s omits the explicit push\n' "$stage" >&2; exit 1; }
  case $stage in
    none)
      grep -Fq 'render-cask.sh' <<<"$advice" || {
        printf 'stage none omits the render step\n' >&2; exit 1; } ;;
    rendered|staged|committed)
      if grep -Fq 'render-cask.sh' <<<"$advice"; then
        printf 'stage %s tells the operator to re-render an existing cask\n' \
          "$stage" >&2
        exit 1
      fi ;;
  esac
  if [[ $stage == committed ]] && grep -Fq 'commit -m' <<<"$advice"; then
    printf 'stage committed tells the operator to commit again\n' >&2
    exit 1
  fi
done

# A value that failed validation must never be presented as authoritative, and
# must never be pasted into a render-cask.sh command that render-cask.sh
# rejects. Empty values are how the driver signals exactly that.
rejected=$(gascan_report_live_release 1.2.3 /tmp/tap /tmp/repo none '' '' \
  "chatter"$'\n'"$good_url")
grep -Fq 'chatter' <<<"$rejected" || {
  printf 'rejected publish output was not shown raw: %s\n' "$rejected" >&2; exit 1; }
if grep -Eq '^[[:space:]]*sha256:[[:space:]]*http' <<<"$rejected"; then
  printf 'a rejected value was labelled as the sha256: %s\n' "$rejected" >&2
  exit 1
fi
if grep -Eq 'render-cask\.sh [0-9.]+ http' <<<"$rejected"; then
  printf 'a rejected value was pasted into the render command: %s\n' "$rejected" >&2
  exit 1
fi
```

- [ ] **Step 2: Run to verify it fails**

Run: `bash tests/release/release-script-contract.sh; echo "exit: $?"`
Expected: exit 1, `release.sh never references verify-package.sh`.

- [ ] **Step 3: Implement**

Create `packaging/macos/release-recovery.sh`:

```bash
#!/usr/bin/env bash
# What to tell an operator when a release step fails after publish.sh has
# already flipped the release out of draft.
#
# This lives outside release.sh so it can be tested. It runs only when a
# release is public, which no test can arrange, and the defects found in it so
# far -- a recipe wrong for two of its three states, rejected values presented
# as authoritative -- are not the kind a source grep can see.
#
# Both functions write to stdout. The caller redirects.

gascan_print_release_values() {
  printf '  asset:  %s\n' "$1"
  printf '  sha256: %s\n' "$2"
}

# gascan_report_live_release VERSION TAP_PATH REPO_ROOT STAGE ASSET_URL
#                           CHECKSUM PUBLISHED
#
# ASSET_URL and CHECKSUM are empty when publish.sh's output did not validate.
# PUBLISHED is that raw output, shown instead so the operator sees what
# actually came back rather than a value already rejected.
gascan_report_live_release() {
  local version=$1 tap=$2 repo=$3 stage=$4 url=$5 sum=$6 raw=$7
  printf '\nthe GitHub release for v%s is already published; do not delete it\n' \
    "$version"
  if [[ -n $url && -n $sum ]]; then
    gascan_print_release_values "$url" "$sum"
  else
    printf 'publish.sh printed:\n%s\n' "$raw"
  fi
  printf 'finish the cask by hand:\n'
  if [[ $stage == none ]]; then
    # `none` also covers a failed `pull --ff-only`, which means origin/main
    # moved under the tap. Committing on the stale base would only get the
    # push rejected, so name the reconcile step before the rest.
    printf '  # if the tap could not fast-forward, reconcile it first:\n'
    printf '  #   git -C %s pull --ff-only origin main\n' "$tap"
    # render-cask.sh rejects a checksum that is not 64 hex characters, so name
    # the placeholder rather than emitting a command that pastes and fails.
    printf '  %s/packaging/macos/render-cask.sh %s %s > %s/Casks/gascan.rb\n' \
      "$repo" "$version" "${sum:-<sha256-printed-above>}" "$tap"
  fi
  if [[ $stage == rendered ]]; then
    printf '  # %s/Casks/gascan.rb is already rendered; check it before staging\n' \
      "$tap"
  fi
  if [[ $stage == none || $stage == rendered ]]; then
    printf '  git -C %s add Casks/gascan.rb\n' "$tap"
  fi
  if [[ $stage != committed ]]; then
    printf "  git -C %s commit -m 'gascan %s'\n" "$tap" "$version"
  fi
  # Always last, and always explicit: a tap without upstream tracking rejects
  # a bare push, which is the failure this recovery most often follows.
  printf '  git -C %s push origin main\n' "$tap"
}
```

Add its source line to `packaging/macos/release.sh`, beside the others:

```bash
# shellcheck source=release-recovery.sh
source "$repo_root/packaging/macos/release-recovery.sh"
```

Append to `packaging/macos/release.sh`:

```bash
original_ref=$(git symbolic-ref --quiet --short HEAD || git rev-parse HEAD)
# A failed restore leaves the operator on a detached HEAD at the tag. Say so:
# silently swallowing it means the next command they run happens somewhere
# they did not expect to be.
restore_ref() {
  git checkout --quiet "$original_ref" && return 0
  printf 'could not return to %s; you are on a detached HEAD\n' "$original_ref" >&2
  printf 'run: git checkout %s\n' "$original_ref" >&2
}

# Once publish.sh returns, the GitHub release is public and cannot be undone
# safely -- the documented recovery deletes a release, and an operator reaching
# for it lands one flag away from deleting the signed tag too. So every failure
# after that point has to say the release is already live and hand over the two
# values needed to finish the cask by hand, which the success path would
# otherwise be the only place to print.
release_is_live=false
published=
asset_url=
checksum=
# How far the tap work got. A single fixed recipe would be wrong for most of
# these: telling an operator to re-render after `brew style` rejected the cask
# reproduces the file that was just rejected, and telling them to commit again
# after the commit succeeded dead-ends in git's own "nothing to commit".
tap_stage=none

report_live_release() {
  [[ $release_is_live == true ]] || return 0
  gascan_report_live_release "$version" "$tap_path" "$repo_root" "$tap_stage" \
    "$asset_url" "$checksum" "$published" >&2
}

# The exit status reports the release, not the ref: a successful release whose
# ref restore failed still exits 0, because the release did happen and
# restore_ref has already printed the one command that fixes the checkout.
on_exit() {
  local exit_code=$?
  restore_ref
  [[ $exit_code -eq 0 ]] || report_live_release
  return $exit_code
}
trap on_exit EXIT
# Notarization runs for minutes with the operator parked on a detached HEAD.
# Matching release-smoke.sh, name the interrupted exit status rather than
# leaving it to differ between INT and TERM.
trap 'exit 130' INT TERM

# `--detach refs/tags/` names exactly the tag: a branch called v1.2.3 would
# otherwise win, and the release would be built from the wrong commit.
git checkout --quiet --detach "refs/tags/v$version"
revision=$(git rev-parse --verify HEAD)
# package.sh honors GASCAN_RELEASE_ARTIFACT_DIR. Looking somewhere else means
# reuse silently never triggers and every retry pays another notarization round
# trip -- the exact cost this path exists to avoid.
package="${GASCAN_RELEASE_ARTIFACT_DIR:-$repo_root/.artifacts/release}/gascan-$version-macos-arm64.pkg"

reusable=false
if [[ -f $package ]] &&
  "$repo_root/packaging/macos/verify-package.sh" "$package" "$revision" "$version" >/dev/null 2>&1 &&
  gascan_assert_distributable_package "$package" "$GASCAN_RELEASE_TEAM" >/dev/null 2>&1; then
  reusable=true
fi

if [[ $reusable == true ]]; then
  printf 'reusing the already notarized package for %s\n' "$revision" >&2
else
  printf 'building, signing, and notarizing; Apple notarization takes minutes\n' >&2
  package=$(
    GASCAN_CODESIGN_IDENTITY="$application_identity" \
    GASCAN_INSTALLER_SIGNING_IDENTITY="$installer_identity" \
    GASCAN_NOTARYTOOL_PROFILE="$notary_profile" \
      "$repo_root/packaging/macos/package.sh"
  )
  # One call, not a hand-rolled trio. `pkgutil --check-signature` alone exits 0
  # for a package signed by any certificate at all, so a trio built from it
  # would claim more than it proves; this pins the Developer ID Installer
  # certificate and the team, and asserts the exact payload. The reuse branch
  # does not repeat it -- reuse is conditional on this same call having just
  # passed for this same file, and it expands the package and verifies three
  # signatures each time it runs.
  gascan_assert_distributable_package "$package" "$GASCAN_RELEASE_TEAM" || exit 65
fi

published=$("$repo_root/packaging/macos/publish.sh" "$package")
release_is_live=true
# publish.sh's stdout is a two-line contract: asset URL, then SHA-256. Assert
# the shape rather than trusting positions. `gh release upload` inside it does
# not redirect its own stdout, so a future gh that chatters there would shift
# both lines, and a shifted URL ships a cask that breaks every install while
# the release itself looks fine.
published_lines=$(grep -c '' <<<"$published")
[[ $published_lines -eq 2 ]] || {
  printf 'publish.sh printed %s lines, expected the asset URL then the SHA-256:\n%s\n' \
    "$published_lines" "$published" >&2
  exit 65
}
# Validate before assigning. `asset_url` and `checksum` are what the recovery
# hands the operator to finish the cask with, so a rejected value must never
# reach them: it would be printed as authoritative and pasted into a
# render-cask.sh command that render-cask.sh then rejects.
candidate_url=$(sed -n '1p' <<<"$published")
candidate_sum=$(sed -n '2p' <<<"$published")
[[ $candidate_url == https://github.com/*/releases/download/*/* ]] || {
  printf 'publish did not report an asset URL:\n%s\n' "$published" >&2
  exit 65
}
[[ $candidate_sum =~ ^[0-9a-f]{64}$ ]] || {
  printf 'publish did not report a SHA-256:\n%s\n' "$published" >&2
  exit 65
}
asset_url=$candidate_url
checksum=$candidate_sum

# Name the remote and branch. A hand-assembled tap has no upstream tracking,
# and a bare `pull --ff-only` fails there with "no tracking information" -- at
# this point, minutes after the release went public. `gascan_gate_tap` proves
# the explicit form works, not this one.
git -C "$tap_path" pull --ff-only --quiet origin main
mkdir -p "$tap_path/Casks"
"$repo_root/packaging/macos/render-cask.sh" "$version" "$checksum" \
  >"$tap_path/Casks/gascan.rb"
tap_stage=rendered
ruby -c "$tap_path/Casks/gascan.rb" >/dev/null || {
  printf 'rendered cask is not valid Ruby: %s\n' "$tap_path/Casks/gascan.rb" >&2
  exit 65
}
# Let brew name the offenses, on stderr with every other diagnostic so the
# release summary owns stdout. Discarding them tells the operator only that
# something is wrong, at the one point where the release is already public.
brew style "$tap_path/Casks/gascan.rb" >&2 || {
  printf 'rendered cask fails brew style: %s\n' "$tap_path/Casks/gascan.rb" >&2
  exit 65
}
# `add` explicitly, not `commit -a`: the first release into a fresh tap writes
# Casks/gascan.rb as a new file, which `-a` never stages, so the commit would
# fail with "nothing to commit" after the release was already published.
git -C "$tap_path" add Casks/gascan.rb
tap_stage=staged
# An identical cask is not a failure. It happens when an operator wrote the
# cask by hand while recovering and then re-ran, and `git commit` with nothing
# staged would abort the run under `set -e` with only git's own wording.
# `diff --cached --quiet` exits 1 for differences and above 1 for a real
# error, so treating every non-zero as "there are changes" would commit on a
# failed inspection.
staged=0
git -C "$tap_path" diff --cached --quiet || staged=$?
case $staged in
  0)
    printf 'the cask already carries %s and this checksum; nothing to commit\n' \
      "$version" >&2 ;;
  1) git -C "$tap_path" commit --quiet -m "gascan $version" ;;
  *)
    printf 'could not inspect the staged cask in %s (git exited %s)\n' \
      "$tap_path" "$staged" >&2
    exit 65 ;;
esac
tap_stage=committed
# `origin main`, never a bare push, for the same reason the pull above names
# them: a hand-assembled tap has no upstream tracking, and git's default
# push.autoSetupRemote is false, so a bare push exits 128 with "no upstream
# branch" -- after the release is public, on the last mutation of the run.
# Unconditional, because with nothing committed this is a no-op that says
# "Everything up-to-date" rather than a step whose safety rests on an
# invariant established two hundred lines earlier.
git -C "$tap_path" push --quiet origin main

printf '\nreleased %s\n' "$version"
gascan_print_release_values "$asset_url" "$checksum"
printf '  cask:   %s\n' "$(git -C "$tap_path" rev-parse --short HEAD)"
printf '  verify: brew update && brew upgrade --cask gascan\n'
```

The tap `git push` targets `$tap_path`, a different repository. Task 3's
mutation assertion is behavioral and runs only under `--check`, which exits
before this code, so it stays correct without modification: `--check` must
mutate nothing, while a real run legitimately pushes the tap.

- [ ] **Step 4: Run to verify it passes**

Run: `bash tests/release/release-script-contract.sh; echo "exit: $?"`
Expected: `PASS`, exit 0.

- [ ] **Step 5: Verify `--check` mutates nothing**

Run:

```bash
git rev-parse HEAD >/tmp/before-head
git status --porcelain >/tmp/before-status
set +e
GASCAN_CODESIGN_IDENTITY=x GASCAN_INSTALLER_SIGNING_IDENTITY=x \
GASCAN_NOTARYTOOL_PROFILE=x GASCAN_TAP_PATH=/tmp \
  ./packaging/macos/release.sh \
  "$(cargo metadata --locked --no-deps --format-version 1 | jq -er '.packages[] | select(.name == "gascan") | .version')" --check
set -e
git rev-parse HEAD >/tmp/after-head
git status --porcelain >/tmp/after-status
diff /tmp/before-head /tmp/after-head && diff /tmp/before-status /tmp/after-status \
  && echo "check mutated nothing"
```

Expected: the run stops at a gate (during normal development there is no signed
tag at `HEAD`), and both diffs are empty. Record in your report which gate it
stopped at.

- [ ] **Step 6: All contracts and shellcheck**

```bash
shellcheck packaging/macos/release.sh packaging/macos/release-gates.sh \
  packaging/macos/release-config.sh packaging/macos/release-recovery.sh \
  tests/release/release-script-contract.sh
for c in tests/release/*-contract.sh; do bash "$c" >/dev/null 2>&1 || echo "FAIL $c"; done
echo done
```

Expected: shellcheck exit 0, no `FAIL` lines, 11 contracts.

- [ ] **Step 7: Commit**

```bash
git add packaging/macos/release.sh packaging/macos/release-recovery.sh \
  packaging/macos/release-gates.sh tests/release/release-script-contract.sh
git commit -m "feat: build, publish, and update the cask in one command

Compose package.sh, verify-package.sh, publish.sh and render-cask.sh rather
than reimplementing them. An already-notarized package is reused only when it
verifies against this tag's revision and version and is still distributable, so
a retry after a late failure costs seconds instead of another notarization
round trip.

The cask checksum comes from publish.sh's own output rather than being retyped:
a wrong sha256 breaks every user's install while the release itself looks
correct. An EXIT trap restores the operator's original ref."
```

---

### Task 5: Document the one-command path

**Files:**
- Modify: `docs/release/macos-checklist.md`

- [ ] **Step 1: Add the section**

In `docs/release/macos-checklist.md`, directly after the `## Publish` heading's
credential paragraph, insert:

```markdown
### One command

Once the signed tag is pushed, `release.sh` runs every gate, then builds, signs,
notarizes, publishes, and updates the cask:

```sh
./packaging/macos/release.sh <version> --check   # verify readiness, change nothing
./packaging/macos/release.sh <version>           # do it
```

Four values resolve by precedence — flag, environment, then
`~/.config/gascan/release.env` — and none is defaulted:

| Value | Flag | Environment |
| --- | --- | --- |
| Developer ID Application identity | `--codesign-identity` | `GASCAN_CODESIGN_IDENTITY` |
| Developer ID Installer identity | `--installer-identity` | `GASCAN_INSTALLER_SIGNING_IDENTITY` |
| Notarization keychain profile name | `--notary-profile` | `GASCAN_NOTARYTOOL_PROFILE` |
| Homebrew tap checkout | `--tap` | `GASCAN_TAP_PATH` |

`release.sh` never creates, moves, or deletes a tag, and never deletes a
release. Create and push the signed tag first. The manual steps below remain
correct and are what the script runs — read them when a gate fails.
```

- [ ] **Step 2: Verify contracts still pass**

Run: `for c in tests/release/*-contract.sh; do bash "$c" >/dev/null 2>&1 || echo "FAIL $c"; done; echo done`
Expected: no `FAIL` lines.

- [ ] **Step 3: Commit**

```bash
git add docs/release/macos-checklist.md
git commit -m "docs: document the one-command release path

The manual sequence stays as the reference for what the script does, since that
is what an operator needs when a gate fails."
```

---

## Manual verification

The happy path cannot run offline: it needs a real signed tag, live Apple
notarization, and two remotes. Confirm the read-only path instead:

```bash
./packaging/macos/release.sh "$(cargo metadata --locked --no-deps --format-version 1 \
  | jq -er '.packages[] | select(.name == "gascan") | .version')" --check
```

Expect it to stop at the tag gate during normal development, print the
`git tag -s` remediation, and leave `git status --porcelain` unchanged.

## Release note

`packaging/macos/` is a release input, so this changes the source revision. The
script cannot drive the release that introduces it; the first release it can
drive is the next one.
