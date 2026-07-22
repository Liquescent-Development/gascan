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

# shellcheck disable=SC1007 # VAR= command clears the ambient env var for this call
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
# shellcheck disable=SC1007 # VAR= command clears the ambient env var for this call
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
# shellcheck disable=SC1007 # VAR= command clears the ambient env var for this call
observed=$(GASCAN_CODESIGN_IDENTITY= gascan_release_config \
  GASCAN_CODESIGN_IDENTITY '' "$fixture/spaces.env")
[[ $observed == 'Developer ID Application: Example LLC (TEAMID1234)' ]] || {
  printf 'value with spaces was mangled: %s\n' "$observed" >&2; exit 1; }

cat >"$fixture/hostile.env" <<'EOF_HOSTILE'
GASCAN_TAP_PATH=$(touch /tmp/gascan-config-executed)
EOF_HOSTILE
rm -f /tmp/gascan-config-executed
# shellcheck disable=SC1007 # VAR= command clears the ambient env var for this call
observed=$(GASCAN_TAP_PATH= gascan_release_config \
  GASCAN_TAP_PATH '' "$fixture/hostile.env")
if [[ -e /tmp/gascan-config-executed ]]; then
  rm -f /tmp/gascan-config-executed
  printf 'config file contents were executed\n' >&2
  exit 1
fi
# shellcheck disable=SC2016 # single quotes are deliberate: comparing a literal, not expanding it
[[ $observed == '$(touch /tmp/gascan-config-executed)' ]] || {
  printf 'hostile value not preserved literally: %s\n' "$observed" >&2; exit 1; }

observed=$(GASCAN_TAP_PATH=from-env gascan_release_config \
  GASCAN_TAP_PATH '' "$fixture/absent.env")
[[ $observed == from-env ]] || {
  printf 'absent config file broke resolution: %s\n' "$observed" >&2; exit 1; }

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

# A tap missing a commit that origin/main carries must be told to pull, not
# reconcile by hand or push.
git -C "$tap" push --quiet origin main
git -C "$tap" reset --quiet --hard HEAD~1
set +e
behind=$(gascan_gate_tap "$tap" 2>&1 >/dev/null)
behind_code=$?
set -e
[[ $behind_code -ne 0 ]] || {
  printf 'a tap behind origin/main was accepted\n' >&2; exit 1; }
grep -Fq "git -C $tap pull --ff-only origin main" <<<"$behind" || {
  printf 'a behind tap was not told to pull: %s\n' "$behind" >&2; exit 1; }

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
# recovery an operator reaches for otherwise deletes a published release.
grep -Fq 'already published' "$release" || {
  printf 'release.sh never warns that the release is already published\n' >&2
  exit 1; }
# `status` is read-only in zsh and has previously made a successful release
# look like a failure. `local status=` is how it would most likely reappear.
# Written as an `if` so an absent pattern -- the passing case -- does not trip
# `set -e`.
if grep -Eq '(^|[[:space:]])(local|declare|readonly|typeset)?[[:space:]]*status=' "$release"; then
  printf 'release.sh assigns a variable named status\n' >&2
  exit 1
fi

printf 'PASS: Gas Can release script contract\n'
