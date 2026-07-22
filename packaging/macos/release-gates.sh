#!/usr/bin/env bash
# Pre-flight assertions for a macOS release.
#
# Each gate returns non-zero after printing the specific command that fixes it.
# They run before anything is built, because the expensive failures surface late
# otherwise: a lapsed Apple agreement returns 403 only at notarization, and an
# unpushed tag aborts publish after the build.

gascan_gate_tools() {
  local command
  for command in gh jq cargo pkgutil shasum ruby brew git codesign spctl xcrun \
    security cpio gzip; do
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
  local remote_tag local_tag ls_remote_output
  ls_remote_output=$(git -C "$repo" ls-remote --tags origin "refs/tags/$tag") || {
    printf 'could not reach the remote to check for tag %s\n' "$tag" >&2
    return 65
  }
  remote_tag=$(awk 'NR==1{print $1}' <<<"$ls_remote_output")
  [[ -n $remote_tag ]] || {
    printf 'release tag %s is not on the remote\n' "$tag" >&2
    printf 'push it with: git push origin %s\n' "$tag" >&2
    return 65
  }
  local_tag=$(git -C "$repo" rev-parse --verify "refs/tags/$tag") || return 65
  [[ $remote_tag == "$local_tag" ]] || {
    printf 'remote tag %s is a different object than the local one (%s vs %s)\n' \
      "$tag" "${remote_tag:0:9}" "${local_tag:0:9}" >&2
    printf 'reconcile the tag by hand before releasing\n' >&2
    return 65
  }
}

gascan_gate_github() {
  gh auth status >/dev/null 2>&1 || {
    printf 'GitHub CLI is not authenticated for this repository\n' >&2
    printf 'run: gh auth login\n' >&2
    return 65
  }
}

gascan_gate_no_release() {
  local tag="v$1" draft view_code=0 view_err
  view_err=$(gh release view "$tag" 2>&1 >/dev/null) || view_code=$?
  # gh exits 1 for "release not found", for HTTP 401, and for an unreachable
  # host alike, so the exit code alone cannot tell absence from inability.
  # Only the not-found message means "no release exists".
  if [[ $view_code -ne 0 ]]; then
    [[ $view_code -eq 1 && $view_err == *'release not found'* ]] && return 0
    printf 'could not ask GitHub whether %s already exists (gh exited %s): %s\n' \
      "$tag" "$view_code" "$view_err" >&2
    return 65
  fi
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
  # `security find-identity -v` quotes each identity -- `  1) <hash>
  # "<identity>"` -- so match the quoted form: a bare substring match would let
  # a truncated identity string pass here and fail later inside codesign.
  grep -Fq "\"$application\"" <<<"$identities" || {
    printf 'Developer ID Application identity is not in the keychain: %s\n' \
      "$application" >&2
    return 65
  }
  grep -Fq "\"$installer\"" <<<"$identities" || {
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
  local tap=$1 repo=$2 branch local_head remote_head origin_url
  [[ -d $tap ]] || {
    printf 'tap path does not exist: %s\n' "$tap" >&2
    return 65
  }
  [[ $(cd "$tap" && pwd -P) != "$repo" ]] || {
    printf 'tap path is the gascan repository itself: %s\n' "$tap" >&2
    printf 'point --tap or GASCAN_TAP_PATH at the Homebrew tap checkout\n' >&2
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
  origin_url=$(git -C "$tap" remote get-url origin 2>/dev/null) || origin_url=
  case $origin_url in
    *homebrew-*|*/tap|*/tap.git) ;;
    *)
      printf 'tap origin does not look like a Homebrew tap: %s\n' \
        "${origin_url:-none}" >&2
      printf 'a tap repository is conventionally named homebrew-<name>\n' >&2
      return 65 ;;
  esac
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
