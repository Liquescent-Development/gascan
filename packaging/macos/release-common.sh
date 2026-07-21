#!/usr/bin/env bash

gascan_user_runtime_root() {
  printf '/private/tmp/gascan-%s\n' "$(id -u)"
}

gascan_verify_release_source() {
  local repo=$1 revision=$2 version=$3 tag object_type target
  git -C "$repo" verify-commit "$revision" >/dev/null 2>&1 && return 0
  [[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || return 1
  tag="v$version"
  object_type=$(git -C "$repo" cat-file -t "refs/tags/$tag" 2>/dev/null) || return 1
  [[ $object_type == tag ]] || return 1
  git -C "$repo" verify-tag "refs/tags/$tag" >/dev/null 2>&1 || return 1
  target=$(git -C "$repo" rev-parse --verify "refs/tags/$tag^{}") || return 1
  [[ $target == "$revision" ]]
}

gascan_assert_release_inputs_clean() {
  local repo=$1 label=$2 path ignored_source
  local -a inputs=(
    Cargo.toml Cargo.lock rust-toolchain.toml crates helpers proto
    scripts/build-apple-attach-helper.sh packaging/macos LICENSE
    images/workspace/approved-image.txt images/workspace/versions.lock
  )
  if [[ -n $(git -C "$repo" status --porcelain --untracked-files=all -- "${inputs[@]}") ]]; then
    printf 'release source inputs are not clean (%s)\n' "$label" >&2
    return 65
  fi
  for path in Cargo.toml Cargo.lock rust-toolchain.toml scripts/build-apple-attach-helper.sh LICENSE \
    images/workspace/approved-image.txt images/workspace/versions.lock; do
    git -C "$repo" ls-files --error-unmatch -- "$path" >/dev/null 2>&1 || {
      printf 'release source input is not tracked (%s): %s\n' "$label" "$path" >&2
      return 65
    }
  done
  ignored_source=$(
    git -C "$repo" ls-files --others --ignored --exclude-standard -- \
      crates helpers proto packaging/macos scripts/build-apple-attach-helper.sh \
      ':(exclude)helpers/apple-attach/.build/**' |
      awk '/\.(rs|swift|toml|proto|sh)$/ || /(^|\/)Package\.swift$/ { print; exit }'
  )
  if [[ -n $ignored_source ]]; then
    printf 'ignored release source input exists (%s): %s\n' "$label" "$ignored_source" >&2
    return 65
  fi
}

gascan_release_test_signal() {
  [[ ${GASCAN_RELEASE_TESTING:-} == YES ]] || return 0
  case ${GASCAN_RELEASE_TEST_SIGNAL_AFTER_TRAPS:-} in
    INT) kill -INT "$$";;
    TERM) kill -TERM "$$";;
    '') ;;
    *) printf 'invalid release-test signal\n' >&2; return 64;;
  esac
}

gascan_lock_section_json() {
  local lock=$1 section=$2
  awk -v section="[$section]" '
    $0 == section { active=1; next }
    active && /^\[/ { exit }
    active && /^[a-z][a-z0-9_]*[[:space:]]*=/ { print }
  ' "$lock" | jq -Rn '
    [inputs | capture("^(?<key>[a-z][a-z0-9_]*)[[:space:]]*=[[:space:]]*\\\"(?<value>[^\\\"]+)\\\"$") | {key:.key,value:.value}] | from_entries
  '
}

gascan_assert_destroyed_controller_record() {
  local inventory=$1 expected_id=$2
  [[ -n $expected_id ]] || return 1
  jq -e --arg id "$expected_id" '
    type == "array" and
    ([.[] | select(type == "object" and .sandbox_id == $id)] |
      length == 1 and .[0].actual_state == "absent")
  ' <<<"$inventory" >/dev/null
}

gascan_exact_apple_prerequisites() {
  local version status commit=5973b9cc626a3e7a499bb316a958237ebe14e2ed
  version=$(container system version --format json) || return 1
  status=$(container system status --format json) || return 1
  jq -e --arg commit "$commit" '
    type == "array" and
    ([.[] | select(.appName == "container")] | length) == 1 and
    ([.[] | select(.appName == "container-apiserver")] | length) == 1 and
    all(.[] | select(.appName == "container" or .appName == "container-apiserver");
      .buildType == "release" and .commit == $commit) and
    ([.[] | select(.appName == "container")][0].version == "1.1.0") and
    ([.[] | select(.appName == "container-apiserver")][0].version ==
      "container-apiserver version 1.1.0 (build: release, commit: 5973b9c)")
  ' <<<"$version" >/dev/null || return 1
  jq -e --arg commit "$commit" '
    type == "object" and .status == "running" and
    .apiServerAppName == "container-apiserver" and
    .apiServerBuild == "release" and .apiServerCommit == $commit and
    .apiServerVersion == "container-apiserver version 1.1.0 (build: release, commit: 5973b9c)"
  ' <<<"$status" >/dev/null
}

gascan_stop_attested_daemon() {
  local gascan_bin=$1 expected=$2 attestation pid executable start token
  attestation=$($gascan_bin daemon-attest 2>/dev/null) || return 0
  pid=$(jq -er '.pid | select(type == "number" and . > 1 and . < 4294967296)' <<<"$attestation") || return 1
  executable=$(jq -er '.executable | select(type == "string")' <<<"$attestation") || return 1
  start=$(jq -er '.start_identity | select(type == "string" and length > 0)' <<<"$attestation") || return 1
  token=$(jq -er '.instance_token | select(type == "string" and length > 0)' <<<"$attestation") || return 1
  expected=$(realpath "$expected") || return 1
  [[ $(realpath "$executable") == "$expected" ]] || return 1
  local observed_command observed_executable observed_start second
  observed_command=$(ps -p "$pid" -o command= 2>/dev/null) || return 1
  observed_executable=$(realpath "${observed_command%% *}") || return 1
  observed_start=$(ps -p "$pid" -o lstart= 2>/dev/null | sed 's/^ *//;s/ *$//') || return 1
  [[ $observed_executable == "$expected" && $observed_start == "$start" ]] || return 1
  second=$($gascan_bin daemon-attest 2>/dev/null) || return 1
  jq -e --argjson pid "$pid" --arg exe "$executable" --arg start "$start" --arg token "$token" '
    .pid == $pid and .executable == $exe and .start_identity == $start and .instance_token == $token
  ' <<<"$second" >/dev/null || return 1
  env kill -TERM "$pid"
  for _ in {1..100}; do
    observed_start=$(ps -p "$pid" -o lstart= 2>/dev/null | sed 's/^ *//;s/ *$//' || true)
    [[ $observed_start == "$start" ]] || return 0
    sleep 0.05
  done
  printf 'installed Gas Can daemon did not stop promptly\n' >&2
  return 1
}

gascan_audit_clean_host() {
  local label=$1 runtime_root=$2 install_root=$3 failed=false containers volumes dns path
  if pkgutil --pkg-info dev.gascan.pkg >/dev/null 2>&1; then
    printf '%s: package receipt remains\n' "$label" >&2; failed=true
  fi
  for path in usr/local/bin/gascan usr/local/bin/gascand usr/local/bin/gascan-apple-attach usr/local/share/gascan; do
    if [[ -e $install_root/$path || -L $install_root/$path ]]; then
      printf '%s: package path remains: /%s\n' "$label" "$path" >&2; failed=true
    fi
  done
  if [[ -e $runtime_root || -L $runtime_root ]]; then
    printf '%s: controller/socket state remains: %s\n' "$label" "$runtime_root" >&2; failed=true
  fi
  containers=$(container list --all --format json) || return 1
  volumes=$(container volume list --format json) || return 1
  dns=$(container system dns list --format json) || return 1
  jq -e 'type == "array" and ([.[] | select(.configuration.labels."dev.gascan.managed-by" == "gascan")] | length) == 0' <<<"$containers" >/dev/null || { printf '%s: Gas Can-owned Apple container remains\n' "$label" >&2; failed=true; }
  jq -e 'type == "array" and ([.[] | select(.configuration.labels."dev.gascan.managed-by" == "gascan")] | length) == 0' <<<"$volumes" >/dev/null || { printf '%s: Gas Can-owned Apple volume remains\n' "$label" >&2; failed=true; }
  jq -e 'type == "array" and ([.[] | select(test("^gascan-[0-9a-f]{32}\\.test$"))] | length) == 0' <<<"$dns" >/dev/null || { printf '%s: Gas Can test DNS route remains\n' "$label" >&2; failed=true; }
  [[ $failed == false ]]
}

# The exact Apple Developer team that signs Gas Can releases.
# shellcheck disable=SC2034 # consumed by publish.sh, which sources this file
GASCAN_RELEASE_TEAM=Z548WR4TF8

# The exact asset set a published release must carry, as a sorted,
# comma-joined list. Both this and the observed value from
# `gh release view --json assets --jq` are built with the same
# `sort | join(",")` pipeline so neither side depends on a hand-written
# ordering: codepoint sort puts build-manifest.json first, ahead of the
# package.
#
# Raw output is part of the contract. `gh --jq` prints raw text, so encoding
# this side as JSON would wrap it in quotes and the two could never compare
# equal -- the check would reject every release rather than the incomplete ones
# it exists to catch.
gascan_expected_release_assets() {
  local base=$1
  jq -rn --arg a "$base" --arg b "$base.sha256" --arg c build-manifest.json \
    '[$a, $b, $c] | sort | join(",")'
}

# Every path pkgutil reports for a Gas Can package payload. macOS 26's pkgbuild
# serializes its protected com.apple.provenance xattr as paired AppleDouble
# records, so those are part of the expected set. This is the single definition
# of "the payload is exactly what we ship": both the build-time verifier and the
# release gate compare against it.
gascan_expected_payload_files() {
  cat <<'EOF_PAYLOAD'
.
./._usr
./usr
./usr/._local
./usr/local
./usr/local/._bin
./usr/local/._share
./usr/local/bin
./usr/local/bin/._gascan
./usr/local/bin/._gascan-apple-attach
./usr/local/bin/._gascand
./usr/local/bin/gascan
./usr/local/bin/gascan-apple-attach
./usr/local/bin/gascand
./usr/local/share
./usr/local/share/._gascan
./usr/local/share/gascan
./usr/local/share/gascan/._LICENSE
./usr/local/share/gascan/._build-manifest.json
./usr/local/share/gascan/._default-gascan.toml
./usr/local/share/gascan/LICENSE
./usr/local/share/gascan/build-manifest.json
./usr/local/share/gascan/default-gascan.toml
EOF_PAYLOAD
}

# Compare the package's entire payload listing against that allowlist. Listing
# rather than walking an extracted root is what makes symlinks, AppleDouble
# records, nested directories and hostile filenames all visible.
gascan_assert_exact_payload() {
  local package=$1 work
  work=$(mktemp -d "${TMPDIR:-/tmp}/gascan-payload.XXXXXX") || return 70
  if ! pkgutil --payload-files "$package" >"$work/listing"; then
    rm -rf "$work"
    printf 'package payload could not be listed\n' >&2
    return 65
  fi
  LC_ALL=C sort -o "$work/actual-payload" "$work/listing"
  gascan_expected_payload_files | LC_ALL=C sort >"$work/expected-payload"
  if ! cmp -s "$work/actual-payload" "$work/expected-payload"; then
    printf 'package payload differs from the exact allowlist\n' >&2
    diff -u "$work/expected-payload" "$work/actual-payload" >&2 || true
    rm -rf "$work"
    return 65
  fi
  rm -rf "$work"
}

# Apple's canonical Developer ID requirement: the team owns the leaf, the
# intermediate carries the Developer ID marker, and the leaf carries the
# Developer ID Application marker. This is requirement-language text, which is
# what `csreq` compiles; `codesign -R` needs a leading `=` to read it as literal
# text rather than a file path, so that prefix belongs at the call site.
gascan_developer_id_requirement() {
  local team=$1
  printf 'anchor apple generic and certificate leaf[subject.OU] = %s' "$team"
  printf ' and certificate 1[field.1.2.840.113635.100.6.2.6] exists'
  printf ' and certificate leaf[field.1.2.840.113635.100.6.1.13] exists\n'
}

gascan_assert_distributable_package() {
  local package=$1 team=$2 signature work
  [[ $team =~ ^[A-Z0-9]{10}$ ]] || {
    printf 'team identifier must be ten uppercase alphanumeric characters\n' >&2
    return 64
  }
  [[ -f $package ]] || {
    printf 'package does not exist: %s\n' "$package" >&2
    return 66
  }
  signature=$(pkgutil --check-signature "$package" 2>&1) || {
    printf 'package is not signed\n' >&2
    return 65
  }
  grep -Fq 'Developer ID Installer' <<<"$signature" || {
    printf 'package is not signed by a Developer ID Installer certificate\n' >&2
    return 65
  }
  grep -Fq "($team)" <<<"$signature" || {
    printf 'package signature does not belong to team %s\n' "$team" >&2
    return 65
  }
  spctl --assess --type install "$package" >/dev/null 2>&1 || {
    printf 'Gatekeeper rejects the package as an install candidate\n' >&2
    return 65
  }
  xcrun stapler validate "$package" >/dev/null 2>&1 || {
    printf 'package has no stapled notarization ticket\n' >&2
    return 65
  }
  gascan_assert_exact_payload "$package" || return $?
  work=$(mktemp -d "${TMPDIR:-/tmp}/gascan-distributable.XXXXXX") || return 70
  if ! pkgutil --expand "$package" "$work/pkg" >/dev/null 2>&1; then
    rm -rf "$work"
    printf 'package could not be expanded\n' >&2
    return 65
  fi
  mkdir "$work/root"
  if ! (cd "$work/root" && gzip -dc "$work/pkg/Payload" | cpio -idm --quiet); then
    rm -rf "$work"
    printf 'package payload could not be extracted\n' >&2
    return 65
  fi
  if [[ -e $work/pkg/Scripts ]]; then
    rm -rf "$work"
    printf 'package carries installer scripts\n' >&2
    return 65
  fi
  local -a executables=(gascan gascan-apple-attach gascand)
  local requirement entry
  requirement="=$(gascan_developer_id_requirement "$team")"
  for entry in "${executables[@]}"; do
    if [[ ! -f $work/root/usr/local/bin/$entry ]]; then
      rm -rf "$work"
      printf 'package payload is missing an executable: usr/local/bin/%s\n' "$entry" >&2
      return 65
    fi
    if ! codesign --verify --strict -R "$requirement" \
      "$work/root/usr/local/bin/$entry" >/dev/null 2>&1; then
      rm -rf "$work"
      printf 'executable is not Developer ID signed by team %s: %s\n' \
        "$team" "usr/local/bin/$entry" >&2
      return 65
    fi
  done
  rm -rf "$work"
}
