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
  [[ $local_head == "$remote_head" ]] || {
    printf 'tap is not up to date with origin/main: %s\n' "$tap" >&2
    printf 'run: git -C %s pull --ff-only\n' "$tap" >&2
    return 65
  }
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
