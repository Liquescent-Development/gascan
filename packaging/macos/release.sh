#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
# shellcheck source=release-common.sh
source "$repo_root/packaging/macos/release-common.sh"
# shellcheck source=release-config.sh
source "$repo_root/packaging/macos/release-config.sh"
# shellcheck source=release-gates.sh
source "$repo_root/packaging/macos/release-gates.sh"
# shellcheck source=release-recovery.sh
source "$repo_root/packaging/macos/release-recovery.sh"

usage() {
  # Unquoted so $config_file resolves to the path actually read, rather than
  # printing the literal ${XDG_CONFIG_HOME:-$HOME/.config} expansion an
  # operator cannot act on. config_file is assigned before every call site.
  cat >&2 <<EOF_USAGE
usage: release.sh VERSION [--check]
                  [--codesign-identity NAME] [--installer-identity NAME]
                  [--notary-profile NAME] [--tap PATH] [--config FILE]

Drives an already-tagged release: verifies every gate, then builds, signs,
notarizes, publishes, and updates the Homebrew cask.

  --check   run every gate and exit without building or publishing

Configuration resolves by flag, then environment, then the config file
(default: $config_file). Nothing is defaulted.

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
gascan_gate_github
gascan_gate_no_release "$version"
gascan_gate_identities "$application_identity" "$installer_identity"
gascan_gate_notary "$notary_profile"
gascan_gate_tap "$tap_path" "$repo_root"
printf 'all release preconditions pass for %s\n' "$version" >&2

if [[ $check_only == true ]]; then
  printf 'check only: nothing was built, published, or changed\n' >&2
  exit 0
fi

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
# after that point has to say the release is already live and hand over the
# checksum the cask is built from, which the success path would otherwise be
# the only place to print. The asset URL goes with it as confirmation of what
# was published; render-cask.sh derives its own URL from the version.
release_is_live=false
publish_attempted=false
published=
asset_url=
checksum=
# How far the tap work got. A single fixed recipe would be wrong for most of
# these: telling an operator to re-render after `brew style` rejected the cask
# reproduces the file that was just rejected, and telling them to commit again
# after the commit succeeded dead-ends in git's own "nothing to commit".
tap_stage=none

report_live_release() {
  if [[ $release_is_live != true && $publish_attempted == true ]]; then
    # publish died without saying how far it got. The marker is written the
    # instant the release becomes public, local and immediate -- asking
    # GitHub instead would be an unbounded network call from inside the EXIT
    # trap, at the one moment an interrupted run most needs to let go.
    [[ -f $published_marker ]] && release_is_live=true
  fi
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
fi
# No distributability check here. publish.sh runs
# `gascan_assert_distributable_package` as its first action, before it touches
# gh at all, so a second call buys no earlier failure and pays another package
# expansion and three signature verifications on every release. The reuse
# branch above still calls it, because there it is the reuse predicate rather
# than a repeat.

published_marker="$(dirname "$package")/v$version.published"
rm -f "$published_marker"
publish_attempted=true
published=$("$repo_root/packaging/macos/publish.sh" "$package")
release_is_live=true
# publish.sh's stdout is a two-line contract: asset URL, then SHA-256. Assert
# the shape rather than trusting positions. `gh release upload` inside it does
# not redirect its own stdout, so a future gh that chatters there would shift
# both lines, putting the URL where the checksum belongs. The checksum is what
# the cask is built from -- render-cask.sh derives the URL itself -- so the
# shape check is what stands between chatter and a wrong digest.
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
cask_revision=$(git -C "$tap_path" rev-parse --short HEAD)
printf '  cask:   %s\n' "$cask_revision"
printf '  verify: brew update && brew upgrade --cask gascan\n'
