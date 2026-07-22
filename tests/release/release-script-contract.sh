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

# The touched path lives under $fixture, not /tmp: two concurrent runs of
# this contract would otherwise race on the same shared path, and a stale
# file left behind by a killed run would make the next run's check a false
# positive regardless of whether this run's config was actually executed.
# The heredoc is unquoted so $fixture expands into the file content, but the
# command substitution itself is backslash-escaped so it is never run here.
config_executed=$fixture/gascan-config-executed
cat >"$fixture/hostile.env" <<EOF_HOSTILE
GASCAN_TAP_PATH=\$(touch $config_executed)
EOF_HOSTILE
# shellcheck disable=SC1007 # VAR= command clears the ambient env var for this call
observed=$(GASCAN_TAP_PATH= gascan_release_config \
  GASCAN_TAP_PATH '' "$fixture/hostile.env")
if [[ -e $config_executed ]]; then
  printf 'config file contents were executed\n' >&2
  exit 1
fi
[[ $observed == "\$(touch $config_executed)" ]] || {
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

# gascan_gate_github and the discrimination gascan_gate_no_release now does:
# a gh that cannot answer -- auth failure, rate limit, network outage -- must
# never be read as "no release exists". Exercised with a stub gh rather than a
# source grep, because the I1 bug this closes was exactly a source line that a
# keyword grep would have kept matching. The stub carries stderr as well as an
# exit code, because gh exits 1 for "release not found", for an expired
# token, and for an unreachable host alike -- an exit-code-only stub could
# only ever assert the property the exit code alone cannot prove.
github_stub=$fixture/github-stub
mkdir -p "$github_stub"
cat >"$github_stub/gh" <<'STUB_GH_AUTH'
#!/usr/bin/env bash
case "${1:-} ${2:-}" in
  'auth status') exit "${GASCAN_STUB_GH_AUTH_CODE:-0}" ;;
  'release view')
    if [[ " $* " == *' --json '* ]]; then
      printf '%s\n' "${GASCAN_STUB_GH_DRAFT:-false}"
      exit 0
    fi
    printf '%s' "${GASCAN_STUB_GH_VIEW_STDERR:-}" >&2
    exit "${GASCAN_STUB_GH_VIEW_CODE:-1}" ;;
esac
exit 0
STUB_GH_AUTH
chmod +x "$github_stub/gh"

set +e
unauth=$(PATH=$github_stub:$PATH GASCAN_STUB_GH_AUTH_CODE=1 gascan_gate_github 2>&1)
unauth_code=$?
set -e
[[ $unauth_code -ne 0 ]] || {
  printf 'an unauthenticated gh was accepted\n' >&2; exit 1; }
grep -Fq 'gh auth login' <<<"$unauth" || {
  printf 'unauthenticated message omits the remediation: %s\n' "$unauth" >&2; exit 1; }

PATH=$github_stub:$PATH GASCAN_STUB_GH_AUTH_CODE=0 gascan_gate_github >/dev/null || {
  printf 'an authenticated gh was rejected\n' >&2; exit 1; }

# "release not found" (exit 1) is the one acceptable "no release" answer.
PATH=$github_stub:$PATH GASCAN_STUB_GH_VIEW_CODE=1 \
  GASCAN_STUB_GH_VIEW_STDERR='release not found' \
  gascan_gate_no_release 0.1.4 >/dev/null || {
  printf 'gascan_gate_no_release rejected a genuine not-found answer\n' >&2; exit 1; }

# exit 1 with a 401 is the live bug: an exit-code-only discrimination reads
# this exactly like "release not found" and lets the run proceed to notarize
# against a repository the token cannot see.
set +e
unauthorized=$(PATH=$github_stub:$PATH GASCAN_STUB_GH_VIEW_CODE=1 \
  GASCAN_STUB_GH_VIEW_STDERR='non-200 OK status code: 401 Unauthorized (https://api.github.com/graphql) Bad credentials' \
  gascan_gate_no_release 0.1.4 2>&1 >/dev/null)
unauthorized_code=$?
set -e
[[ $unauthorized_code -ne 0 ]] || {
  printf 'gascan_gate_no_release accepted a 401 as "no release exists"\n' >&2; exit 1; }
grep -Fq 'could not ask GitHub' <<<"$unauthorized" || {
  printf 'the could-not-answer message is missing for a 401: %s\n' "$unauthorized" >&2
  exit 1; }

# exit 4 (no credentials at all) must still fail the gate directly, even
# though gascan_gate_github already rejects it one line earlier in the run.
set +e
noanswer=$(PATH=$github_stub:$PATH GASCAN_STUB_GH_VIEW_CODE=4 \
  GASCAN_STUB_GH_VIEW_STDERR='gh: not logged in' \
  gascan_gate_no_release 0.1.4 2>&1)
noanswer_code=$?
set -e
[[ $noanswer_code -ne 0 ]] || {
  printf 'gascan_gate_no_release passed although gh could not answer\n' >&2; exit 1; }
grep -Fq 'could not ask GitHub' <<<"$noanswer" || {
  printf 'the could-not-answer message is missing: %s\n' "$noanswer" >&2; exit 1; }

# exit 0: a release already exists. The gate must fail, and the message must
# carry the deletion recipe alongside the warning that stops an operator
# destroying their own signed tag with --cleanup-tag -- the one line this
# whole branch treats as a must-survive invariant.
set +e
existing=$(PATH=$github_stub:$PATH GASCAN_STUB_GH_VIEW_CODE=0 GASCAN_STUB_GH_DRAFT=true \
  gascan_gate_no_release 0.1.4 2>&1)
existing_code=$?
set -e
[[ $existing_code -ne 0 ]] || {
  printf 'gascan_gate_no_release accepted an existing release\n' >&2; exit 1; }
grep -Fq 'gh release delete' <<<"$existing" || {
  printf 'existing-release message omits the deletion command: %s\n' "$existing" >&2
  exit 1; }
grep -Fq 'do not add --cleanup-tag' <<<"$existing" || {
  printf 'existing-release message omits the --cleanup-tag warning: %s\n' "$existing" >&2
  exit 1; }
if grep -F 'gh release delete' <<<"$existing" | grep -Fq -- '--cleanup-tag'; then
  printf 'the printed delete command carries --cleanup-tag: %s\n' "$existing" >&2
  exit 1
fi

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

# The remote-tag check must prove the tag OBJECT, not merely its name: a
# remote v<version> pointing at a different object satisfies an emptiness
# test, and `gh release create --verify-tag --target` then binds the release
# to that existing remote tag rather than the one just built. Exercised
# against a private bare remote, never repo_root, so nothing is ever pushed
# to the real project repository.
tagdiff_origin=$fixture/tagdiff-origin.git
git init --quiet --bare --initial-branch=main "$tagdiff_origin"
git -C "$clone" remote set-url origin "$tagdiff_origin"
git -C "$clone" push --quiet origin "refs/tags/$tag"
gascan_gate_tag "$clone" "$probe_version" >/dev/null || {
  printf 'a tag whose remote object matches the local one was rejected\n' >&2; exit 1; }

# Re-sign the tag locally without pushing again: same target commit (still
# HEAD, so the peel-to-HEAD check still passes), but a different tag object
# and signature. The remote keeps the stale object -- proving a name is not
# proving an object.
git -C "$clone" tag -d "$tag" >/dev/null
git -C "$clone" tag -s "$tag" -m 're-signed locally; remote object is now stale'
set +e
diverged=$(gascan_gate_tag "$clone" "$probe_version" 2>&1 >/dev/null)
diverged_code=$?
set -e
[[ $diverged_code -ne 0 ]] || {
  printf 'a remote tag object different from the local one was accepted\n' >&2; exit 1; }
grep -Fq 'is a different object than the local one' <<<"$diverged" || {
  printf 'the different-object message is missing: %s\n' "$diverged" >&2; exit 1; }

# A network failure reaching the remote must read as exactly that, never as
# "the tag is not on the remote" -- the old code's suppressed stderr turned
# any ls-remote failure into a `git push` suggestion that would fail too.
git -C "$clone" remote set-url origin "$fixture/does-not-exist.git"
set +e
unreachable=$(gascan_gate_tag "$clone" "$probe_version" 2>&1 >/dev/null)
unreachable_code=$?
set -e
[[ $unreachable_code -ne 0 ]] || {
  printf 'an unreachable remote was treated as a passing tag check\n' >&2; exit 1; }
grep -Fq 'could not reach the remote' <<<"$unreachable" || {
  printf 'the unreachable-remote message is missing: %s\n' "$unreachable" >&2; exit 1; }
git -C "$clone" remote set-url origin "$tagdiff_origin"

# Tap gate rejects a path that is not a git work tree.
if gascan_gate_tap "$fixture" "$repo_root" >/dev/null 2>&1; then
  printf 'a non-repository tap path was accepted\n' >&2; exit 1
fi

# Tap gate rejects the product repository itself. During a real release the
# gascan repository satisfies every other condition this gate checks -- clean,
# on main, level with origin/main, pushable -- so a stale or mistyped
# GASCAN_TAP_PATH would otherwise pass all eight gates and the release path
# would then push a cask commit to the product repository's default branch.
if gascan_gate_tap "$repo_root" "$repo_root" >/dev/null 2>&1; then
  printf 'the product repository itself was accepted as the tap\n' >&2; exit 1
fi
set +e
same_repo=$(gascan_gate_tap "$repo_root" "$repo_root" 2>&1 >/dev/null)
set -e
grep -Fq 'tap path is the gascan repository itself' <<<"$same_repo" || {
  printf 'the same-repository message is missing: %s\n' "$same_repo" >&2; exit 1; }

# The push check, exercised against a local remote so the dry run has a real
# remote to authenticate and negotiate with. Without this the gate's central
# claim -- that it proves push access -- is asserted by nothing. The remote is
# named as a Homebrew tap conventionally is, since the gate now rejects an
# origin that does not look like one.
tap_origin=$fixture/homebrew-tap.git
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
gascan_gate_tap "$tap" "$repo_root" >/dev/null || {
  printf 'a clean, pushable tap on main was rejected\n' >&2; exit 1; }

# A tap origin that does not look like a Homebrew tap is rejected even when
# everything else about it is fine.
git -C "$tap" remote set-url origin "$fixture/not-a-tap.git"
set +e
badname=$(gascan_gate_tap "$tap" "$repo_root" 2>&1 >/dev/null)
badname_code=$?
set -e
[[ $badname_code -ne 0 ]] || {
  printf 'a tap origin not named like a Homebrew tap was accepted\n' >&2; exit 1; }
grep -Fq 'does not look like a Homebrew tap' <<<"$badname" || {
  printf 'the not-a-tap message is missing: %s\n' "$badname" >&2; exit 1; }
git -C "$tap" remote set-url origin "$tap_origin"

# `pushurl` is what makes this a *push* failure: fetch still resolves, so the
# gate reaches the dry run instead of stopping at the earlier fetch check.
git -C "$tap" config remote.origin.pushurl "$fixture/absent-remote.git"
set +e
unpushable=$(gascan_gate_tap "$tap" "$repo_root" 2>&1 >/dev/null)
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
ahead=$(gascan_gate_tap "$tap" "$repo_root" 2>&1 >/dev/null)
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
behind=$(gascan_gate_tap "$tap" "$repo_root" 2>&1 >/dev/null)
behind_code=$?
set -e
[[ $behind_code -ne 0 ]] || {
  printf 'a tap behind origin/main was accepted\n' >&2; exit 1; }
grep -Fq "git -C $tap pull --ff-only origin main" <<<"$behind" || {
  printf 'a behind tap was not told to pull: %s\n' "$behind" >&2; exit 1; }

# gascan_gate_tools, gascan_gate_identities, and gascan_gate_notary had no
# test at all before this: the Task 6 defect lived in one of the four gates
# this file never exercised. Each runs under a stub PATH inside its own
# subshell, so the stub cannot leak into any assertion outside it.

# A PATH missing one required command.
(
  toolsdir=$fixture/tools-bin
  mkdir -p "$toolsdir"
  for command in gh jq cargo pkgutil shasum ruby brew git codesign spctl xcrun \
    security cpio gzip; do
    [[ $command == jq ]] && continue
    tool_path=$(command -v "$command" 2>/dev/null) || continue
    ln -s "$tool_path" "$toolsdir/$command"
  done
  set +e
  missing_tool=$(PATH=$toolsdir gascan_gate_tools 2>&1)
  missing_tool_code=$?
  set -e
  [[ $missing_tool_code -eq 69 ]] || {
    printf 'gascan_gate_tools with jq missing exited %s, not 69: %s\n' \
      "$missing_tool_code" "$missing_tool" >&2
    exit 1; }
  grep -Fq 'jq' <<<"$missing_tool" || {
    printf 'gascan_gate_tools message omits the missing command: %s\n' "$missing_tool" >&2
    exit 1; }
)

# A stub `security` printing the real `  1) <hash> "<identity>"` shape.
(
  sec_bin=$fixture/security-bin
  mkdir -p "$sec_bin"
  cat >"$sec_bin/security" <<'STUB_SECURITY'
#!/usr/bin/env bash
cat <<'IDENTITIES'
  1) AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA "Developer ID Application: Example LLC (TEAMID1234)"
  2) BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB "Developer ID Installer: Example LLC (TEAMID1234)"
     2 valid identities found
IDENTITIES
STUB_SECURITY
  chmod +x "$sec_bin/security"

  PATH=$sec_bin:$PATH gascan_gate_identities \
    'Developer ID Application: Example LLC (TEAMID1234)' \
    'Developer ID Installer: Example LLC (TEAMID1234)' >/dev/null || {
    printf 'gascan_gate_identities rejected both identities present\n' >&2; exit 1; }

  set +e
  missing_installer=$(PATH=$sec_bin:$PATH gascan_gate_identities \
    'Developer ID Application: Example LLC (TEAMID1234)' \
    'Developer ID Installer: Missing LLC (TEAMID9999)' 2>&1 >/dev/null)
  missing_installer_code=$?
  set -e
  [[ $missing_installer_code -ne 0 ]] || {
    printf 'gascan_gate_identities accepted a missing installer identity\n' >&2; exit 1; }
  grep -Fq 'Developer ID Installer: Missing LLC (TEAMID9999)' <<<"$missing_installer" || {
    printf 'gascan_gate_identities message omits the missing identity: %s\n' \
      "$missing_installer" >&2
    exit 1; }

  # A truncated identity string is a substring of the real one, so a bare
  # substring match would accept it here and let it fail later inside
  # codesign instead. Matching the quoted form `security` prints closes that.
  set +e
  PATH=$sec_bin:$PATH gascan_gate_identities \
    'Developer ID Application: Example LLC' \
    'Developer ID Installer: Example LLC (TEAMID1234)' >/dev/null 2>&1
  truncated_code=$?
  set -e
  [[ $truncated_code -ne 0 ]] || {
    printf 'gascan_gate_identities accepted a truncated identity string\n' >&2; exit 1; }
)

# A stub `xcrun`: the success marker passes; a non-zero exit fails; a zero
# exit whose text lacks the success marker fails -- the gate's real
# assertion, and the reason it does not simply trust the exit code.
(
  xcrun_bin=$fixture/xcrun-bin
  mkdir -p "$xcrun_bin"
  cat >"$xcrun_bin/xcrun" <<'STUB_XCRUN'
#!/usr/bin/env bash
case "${GASCAN_STUB_XCRUN_MODE:-}" in
  success) printf 'Successfully received submission history\n'; exit 0 ;;
  fail) printf 'HTTP 403 Forbidden\n' >&2; exit 1 ;;
  bad-success) printf 'something else entirely\n'; exit 0 ;;
esac
exit 1
STUB_XCRUN
  chmod +x "$xcrun_bin/xcrun"

  PATH=$xcrun_bin:$PATH GASCAN_STUB_XCRUN_MODE=success \
    gascan_gate_notary example-profile >/dev/null || {
    printf 'gascan_gate_notary rejected a successful history\n' >&2; exit 1; }

  set +e
  PATH=$xcrun_bin:$PATH GASCAN_STUB_XCRUN_MODE=fail \
    gascan_gate_notary example-profile >/dev/null 2>&1
  notary_fail_code=$?
  set -e
  [[ $notary_fail_code -ne 0 ]] || {
    printf 'gascan_gate_notary accepted a non-zero notarytool exit\n' >&2; exit 1; }

  set +e
  notary_badtext=$(PATH=$xcrun_bin:$PATH GASCAN_STUB_XCRUN_MODE=bad-success \
    gascan_gate_notary example-profile 2>&1 >/dev/null)
  notary_badtext_code=$?
  set -e
  [[ $notary_badtext_code -ne 0 ]] || {
    printf 'gascan_gate_notary accepted a zero exit lacking the success marker\n' >&2
    exit 1; }
  grep -Fq 'did not authenticate' <<<"$notary_badtext" || {
    printf 'gascan_gate_notary message is not about the missing marker: %s\n' \
      "$notary_badtext" >&2
    exit 1; }
)

release=$repo_root/packaging/macos/release.sh
[[ -x $release ]] || { printf 'release.sh is not executable\n' >&2; exit 1; }
recovery=$repo_root/packaging/macos/release-recovery.sh

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

# A missing required config value stops the run. Each of the four names is
# unset independently, with the other three supplied, so the assertion cannot
# be satisfied entirely by one name: release.sh resolves them in order and
# aborts on the first miss, so unsetting all four at once proves only that
# GASCAN_CODESIGN_IDENTITY (resolved first) is not defaulted, and a default
# silently added to any of the other three would not fail that test.
for missing_name in GASCAN_CODESIGN_IDENTITY GASCAN_INSTALLER_SIGNING_IDENTITY \
  GASCAN_NOTARYTOOL_PROFILE GASCAN_TAP_PATH; do
  set +e
  unset_out=$(env GASCAN_CODESIGN_IDENTITY=x GASCAN_INSTALLER_SIGNING_IDENTITY=x \
    GASCAN_NOTARYTOOL_PROFILE=x GASCAN_TAP_PATH=x \
    env -u "$missing_name" \
    "$release" "$workspace_version" --check --config "$fixture/absent.env" 2>&1 >/dev/null)
  unset_code=$?
  set -e
  [[ $unset_code -ne 0 ]] || {
    printf '%s was defaulted\n' "$missing_name" >&2; exit 1; }
  grep -Fq "missing required release configuration: $missing_name" <<<"$unset_out" || {
    printf '%s was not the value reported missing: %s\n' "$missing_name" "$unset_out" >&2
    exit 1; }
done

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
# never match the very mutation it exists to catch. Shared with the direct
# gate-function scan below, so both logs are held to the same standard.
assert_no_logged_mutation() {
  local log=$1 logged
  while IFS= read -r logged; do
    # A dry-run push is read-only; the tap gate uses one to prove push access.
    # Scoped to that exact pair so `--dry-run` cannot excuse anything else.
    case " $logged " in
      *' push --dry-run '*) continue ;;
    esac
    case " $logged " in
      *' tag '*|*' push '*|*'--cleanup-tag'*|*' release delete '*)
        printf 'a mutation was logged: %s\n' "$logged" >&2
        return 1 ;;
    esac
  done <"$log"
}
assert_no_logged_mutation "$GASCAN_STUB_LOG" || exit 1

# --check must never leave the operator's ref. The checkout belongs to the
# release path, which runs only after --check has already exited.
while IFS= read -r logged; do
  case " $logged " in
    *' checkout '*)
      printf 'check mode moved the working tree: %s\n' "$logged" >&2
      exit 1 ;;
  esac
done <"$GASCAN_STUB_LOG"

# The run above stops at gascan_gate_tag -- this repository's real HEAD
# carries no tag for the current workspace version -- so gascan_gate_no_release,
# gascan_gate_identities, gascan_gate_notary, and gascan_gate_tap are never
# entered, the gh stub is never invoked once, and the push --dry-run exemption
# above is dead code. Coverage that depends on the operator's checkout is not a
# contract, so drive the gate functions directly under the stub PATH instead,
# where every one is reachable regardless of what this checkout looks like.
# $tap was left behind origin/main by the tap-gate tests above; resync it so
# gascan_gate_tap reaches its push --dry-run line rather than stopping early.
git -C "$tap" fetch --quiet origin main
git -C "$tap" reset --quiet --hard origin/main
: >"$GASCAN_STUB_LOG"
(
  # shellcheck disable=SC2030 # deliberately scoped to this subshell only; nothing outside reads it
  PATH=$stub_bin:$PATH
  gascan_gate_tools >/dev/null 2>&1 || true
  gascan_gate_version "$repo_root" 99.99.99 >/dev/null 2>&1 || true
  gascan_gate_tag "$clone" "$probe_version" >/dev/null 2>&1 || true
  gascan_gate_github >/dev/null 2>&1 || true
  gascan_gate_no_release 99.99.99 >/dev/null 2>&1 || true
  gascan_gate_identities x x >/dev/null 2>&1 || true
  gascan_gate_notary x >/dev/null 2>&1 || true
  gascan_gate_tap "$tap" "$repo_root" >/dev/null 2>&1 || true
)

# Name what must appear, so the scan can no longer pass by not getting there.
grep -Fq 'push --dry-run' "$GASCAN_STUB_LOG" || {
  printf 'the gate scan never reached the tap gate; it proves nothing\n' >&2
  exit 1; }
grep -Fq 'gh release view' "$GASCAN_STUB_LOG" || {
  printf 'the gate scan never reached the release gate; it proves nothing\n' >&2
  exit 1; }
assert_no_logged_mutation "$GASCAN_STUB_LOG" || exit 1

# `--cleanup-tag` must never be part of an executed deletion: it removes the
# signed tag from the remote. The gates deliberately *warn* about the flag in
# prose, which is the line that stops an operator destroying their own tag, so
# assert the dangerous construction rather than the string.
for f in "$release" "$gates" "$recovery"; do
  if grep -Eq 'gh release delete.*--cleanup-tag' "$f"; then
    printf '%s would delete a tag via --cleanup-tag\n' "$f" >&2; exit 1
  fi
done

# No hardcoded identity, team identifier, or profile.
for f in "$release" "$gates" "$config" "$recovery"; do
  if grep -Eq 'Developer ID (Application|Installer): [A-Za-z]|\([A-Z0-9]{10}\)' "$f"; then
    printf 'hardcoded signing identity or team identifier in %s\n' "$f" >&2
    exit 1
  fi
done

# `macos/package.sh`, not `package.sh`: the bare form is a substring of
# `verify-package.sh`, so the needle would be satisfied by a different line and
# could never fail.
for needle in verify-package.sh gascan_assert_distributable_package \
  render-cask.sh publish.sh macos/package.sh gascan_gate_github; do
  grep -Fq "$needle" "$release" || {
    printf 'release.sh never references %s\n' "$needle" >&2; exit 1; }
done

# gascan_gate_github is tested as a function above, but nothing yet proves
# release.sh actually calls it as a step of the run: the source-grep just
# above would survive the call being deleted just as easily as it catches it,
# and the mutation-scan --check run later in this file always stops at
# gascan_gate_tag, because this repository's real HEAD carries no tag for the
# current workspace version, so it never reaches gascan_gate_github either.
# Reach it with a disposable clone that DOES carry a valid signed tag, pushed
# to a throwaway bare remote so this never touches the real repository's refs.
authcheck_origin=$fixture/authcheck-origin.git
authcheck_clone=$fixture/authcheck-clone
git init --quiet --bare --initial-branch=main "$authcheck_origin"
git clone --quiet "$repo_root" "$authcheck_clone"
git -C "$authcheck_clone" remote set-url origin "$authcheck_origin"
ssh-keygen -q -t ed25519 -N '' -C release@example.invalid -f "$fixture/authcheck-key"
printf 'release@example.invalid %s\n' "$(cat "$fixture/authcheck-key.pub")" \
  >"$fixture/authcheck-allowed-signers"
git -C "$authcheck_clone" config user.name release
git -C "$authcheck_clone" config user.email release@example.invalid
git -C "$authcheck_clone" config gpg.format ssh
git -C "$authcheck_clone" config user.signingKey "$fixture/authcheck-key"
git -C "$authcheck_clone" config gpg.ssh.allowedSignersFile "$fixture/authcheck-allowed-signers"
# The workspace version may already carry a real released tag pointing
# elsewhere; retarget it in this disposable clone only, never in repo_root.
git -C "$authcheck_clone" tag -d "v$workspace_version" >/dev/null 2>&1 || true
git -C "$authcheck_clone" tag -s "v$workspace_version" -m 'gate_github integration fixture'
git -C "$authcheck_clone" push --quiet origin "refs/tags/v$workspace_version"

authcheck_bin=$fixture/authcheck-bin
mkdir -p "$authcheck_bin"
cat >"$authcheck_bin/gh" <<'STUB_GH_AUTHCHECK'
#!/usr/bin/env bash
case "${1:-} ${2:-}" in
  'auth status') exit "${GASCAN_STUB_GH_AUTH_CODE:-0}" ;;
  'release view') exit 1 ;;
esac
exit 0
STUB_GH_AUTHCHECK
chmod +x "$authcheck_bin/gh"

set +e
# shellcheck disable=SC2031 # this PATH override is its own command prefix, independent of the earlier subshell's
authcheck_out=$(PATH=$authcheck_bin:$PATH \
  GASCAN_CODESIGN_IDENTITY=x GASCAN_INSTALLER_SIGNING_IDENTITY=x \
  GASCAN_NOTARYTOOL_PROFILE=x GASCAN_TAP_PATH="$fixture" \
  GASCAN_STUB_GH_AUTH_CODE=1 \
  "$authcheck_clone/packaging/macos/release.sh" "$workspace_version" --check 2>&1 >/dev/null)
authcheck_code=$?
set -e
[[ $authcheck_code -ne 0 ]] || {
  printf 'release.sh --check passed although gh auth status failed\n' >&2; exit 1; }
grep -Fq 'gh auth login' <<<"$authcheck_out" || {
  printf 'release.sh --check did not name gh auth login when unauthenticated: %s\n' \
    "$authcheck_out" >&2
  exit 1; }
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
for f in "$release" "$recovery" "$gates" "$config"; do
  if grep -Eq '(^|[[:space:]])(local|declare|readonly|typeset)?[[:space:]]*status=' "$f"; then
    printf '%s assigns a variable named status\n' "$f" >&2
    exit 1
  fi
done

# The recovery narrator, exercised directly. It runs only when a release is
# already public, so without this it would be covered by a single grep -- and
# the defects found in it so far were exactly the kind a grep cannot see.
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
  if [[ $stage == staged || $stage == committed ]] && grep -Fq 'git -C /tmp/tap add' <<<"$advice"; then
    printf 'stage %s tells the operator to stage the cask again\n' "$stage" >&2
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

# When publish.sh printed nothing at all before it stopped -- the case Step 0c
# exists for, interrupted inside publish -- the recovery must say so instead
# of a blank line, and the placeholder in the render command must read
# correctly on its own rather than referring to output that was never
# produced.
no_output=$(gascan_report_live_release 1.2.3 /tmp/tap /tmp/repo none '' '' '')
grep -Fq 'publish.sh printed nothing before it stopped' <<<"$no_output" || {
  printf 'the empty-output case does not say nothing was printed: %s\n' "$no_output" >&2
  exit 1; }
grep -Fq 'shasum -a 256' <<<"$no_output" || {
  printf 'the empty-output case does not say where the checksum is: %s\n' "$no_output" >&2
  exit 1; }
grep -Fq 'render-cask.sh 1.2.3 <sha256> >' <<<"$no_output" || {
  printf 'the empty-output case renders a placeholder that does not read alone: %s\n' \
    "$no_output" >&2
  exit 1; }
if grep -Fq '<sha256-printed-above>' <<<"$no_output"; then
  printf 'the placeholder still refers to output that was never produced: %s\n' \
    "$no_output" >&2
  exit 1
fi

printf 'PASS: Gas Can release script contract\n'
