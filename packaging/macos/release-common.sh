#!/usr/bin/env bash

gascan_user_runtime_root() {
  printf '/private/tmp/gascan-%s\n' "$(id -u)"
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
