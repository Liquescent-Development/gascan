#!/usr/bin/env bash
set -euo pipefail

die() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

active_command_pid=''
active_watchdog_pid=''

handle_signal() {
  signal_status="$1"
  trap - INT TERM HUP
  if test -n "$active_command_pid"; then
    kill -TERM -- "-$active_command_pid" 2>/dev/null || true
    sleep 1
    kill -KILL -- "-$active_command_pid" 2>/dev/null || true
    wait "$active_command_pid" 2>/dev/null || true
  fi
  if test -n "$active_watchdog_pid"; then
    kill -TERM -- "-$active_watchdog_pid" 2>/dev/null || true
    wait "$active_watchdog_pid" 2>/dev/null || true
  fi
  exit "$signal_status"
}

run_bounded() {
  timeout_seconds="$1"
  shift
  set -m
  "$@" &
  command_pid=$!
  set +m
  set -m
  (
    sleep "$timeout_seconds"
    if kill -0 "$command_pid" 2>/dev/null; then
      kill -TERM -- "-$command_pid" 2>/dev/null || true
      sleep 1
      kill -KILL -- "-$command_pid" 2>/dev/null || true
    fi
  ) &
  watchdog_pid=$!
  active_command_pid="$command_pid"
  active_watchdog_pid="$watchdog_pid"
  set +m
  if wait "$command_pid"; then result=0; else result=$?; fi
  kill -TERM -- "-$watchdog_pid" 2>/dev/null || true
  wait "$watchdog_pid" 2>/dev/null || true
  active_command_pid=''
  active_watchdog_pid=''
  return "$result"
}

operation_timeout="${GASCAN_PROBE_TIMEOUT_SECONDS:-300}"
case "$operation_timeout" in ''|*[!0-9]*) die 'GASCAN_PROBE_TIMEOUT_SECONDS must be a positive integer' ;; esac
test "$operation_timeout" -gt 0 || die 'GASCAN_PROBE_TIMEOUT_SECONDS must be a positive integer'

ownership_label='com.gascan.build-secret-probe'

container_has_ownership() {
  json_file="$1"
  jq -e --arg name "$container_name" --arg label "$ownership_label" --arg marker "$ownership_marker" '
    type == "array" and length == 1 and
    .[0].id == $name and .[0].configuration.id == $name and
    .[0].configuration.labels[$label] == $marker
  ' "$json_file" >/dev/null
}

image_has_ownership() {
  json_file="$1"
  jq -e --arg reference "$tag" --arg id "$image_id" --arg label "$ownership_label" --arg marker "$ownership_marker" '
    type == "array" and length == 1 and
    .[0].id == $id and .[0].configuration.name == $reference and
    ([.[0].variants[].config.config.Labels[$label]?] | any(. == $marker))
  ' "$json_file" >/dev/null
}

test -n "${GASCAN_TEST_SECRET_FILE:-}" || die 'GASCAN_TEST_SECRET_FILE is required'
case "$GASCAN_TEST_SECRET_FILE" in /*) ;; *) die 'secret path must be absolute' ;; esac
test ! -L "$GASCAN_TEST_SECRET_FILE" || die 'secret must not be a symbolic link'
secret="$(realpath "$GASCAN_TEST_SECRET_FILE")" || die 'cannot canonicalize secret path'
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
case "$secret" in "$repo"|"$repo"/*) die 'secret file must be outside the repository' ;; esac

uid="$(id -u)"
test -f "$secret" || die 'secret must be a regular file'
test ! -L "$secret" || die 'secret must not be a symbolic link'
test "$uid" = "$(stat -f %u "$secret")" || die 'secret must be owned by the current UID'
test "600" = "$(stat -f %Lp "$secret")" || die 'secret mode must be 0600'
test "$(awk 'NF { count++ } END { print count + 0 }' "$secret")" = 1 || die 'secret must contain one nonempty line'
test "$(wc -l <"$secret" | tr -d ' ')" = 1 || die 'secret must contain exactly one line'

token="$(openssl rand -hex 16)"
tag="gascan-build-secret-probe:$token"
container_name="gascan-build-secret-probe-$token"
tmp_base="${TMPDIR:-/tmp}"
tmp_base="${tmp_base%/}"
private_root="$(mktemp -d "$tmp_base/gascan-build-secret-probe.$token.XXXXXX")"
chmod 0700 "$private_root"
context="$private_root/context"
mkdir "$context"
chmod 0700 "$context"
staged_secret="$context/.build-secrets/gascamp_read_token"
transmitted="$private_root/transmitted-context.tar"
transcript="$private_root/build.transcript"
inspect="$private_root/image-inspect.json"
exported="$private_root/container.tar"
built=false
created=false
image_id=''
ownership_marker="$container_name"
cleanup_container_inspect="$private_root/cleanup-container-inspect.json"
cleanup_image_inspect="$private_root/cleanup-image-inspect.json"

capture_bounded() {
  capture_file="$1"
  shift
  capture_pipe="$capture_file.pipe"
  mkfifo "$capture_pipe"
  cat "$capture_pipe" >"$capture_file" &
  capture_pid=$!
  if run_bounded "$operation_timeout" "$@" >"$capture_pipe" 2>&1; then capture_status=0; else capture_status=$?; fi
  wait "$capture_pid" 2>/dev/null || capture_status=1
  rm -f "$capture_pipe"
  return "$capture_status"
}

container_owned() {
  capture_bounded "$cleanup_container_inspect" container inspect "$container_name" &&
    container_has_ownership "$cleanup_container_inspect"
}

image_owned() {
  capture_bounded "$cleanup_image_inspect" container image inspect "$tag" &&
    image_has_ownership "$cleanup_image_inspect"
}

cleanup() {
  status=$?
  trap - EXIT INT TERM HUP
  if test "$created" = true; then
    if container_owned; then
      run_bounded "$operation_timeout" container stop "$container_name" >/dev/null 2>&1 || true
      if container_owned; then
        run_bounded "$operation_timeout" container delete "$container_name" >/dev/null 2>&1 || status=1
      else
        status=1
      fi
    else
      status=1
    fi
  fi
  if test "$built" = true; then
    if image_owned; then
      run_bounded "$operation_timeout" container image delete "$tag" >/dev/null 2>&1 || status=1
    else
      status=1
    fi
  fi
  rm -f "$staged_secret"
  rm -rf "$private_root"
  exit "$status"
}
trap cleanup EXIT
trap 'handle_signal 130' INT
trap 'handle_signal 143' TERM
trap 'handle_signal 129' HUP

mkdir "$context/.build-secrets"
chmod 0700 "$context/.build-secrets"
install -m 0600 "$secret" "$staged_secret"
test -f "$staged_secret" || die 'staged secret must be a regular file'
test ! -L "$staged_secret" || die 'staged secret must not be a symbolic link'
test "$uid" = "$(stat -f %u "$staged_secret")" || die 'staged secret must be owned by the current UID'
test "600" = "$(stat -f %Lp "$staged_secret")" || die 'staged secret mode must be 0600'
printf '%s\n' '.build-secrets' >"$context/.dockerignore"

cat >"$context/Dockerfile" <<'EOF'
FROM ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab
ARG EXPECTED_SECRET_SHA256
RUN --mount=type=secret,id=gascamp_read_token,required=true \
    test "$(sha256sum /run/secrets/gascamp_read_token | cut -d' ' -f1)" = "$EXPECTED_SECRET_SHA256"
RUN test ! -e /run/secrets/gascamp_read_token
EOF

tar -cf "$transmitted" --exclude='./.build-secrets' -C "$context" .
if tar -tf "$transmitted" | grep -q '^\./\.build-secrets'; then
  die 'staged secret entered transmitted context representation'
fi
if grep -a -F -q -f "$secret" "$transmitted"; then
  die 'synthetic secret entered transmitted context representation'
fi

expected_sha256="$(shasum -a 256 "$secret" | cut -d' ' -f1)"
if ! capture_bounded "$transcript" container build --secret "id=gascamp_read_token,src=$staged_secret" \
  --build-arg "EXPECTED_SECRET_SHA256=$expected_sha256" \
  --label "$ownership_label=$ownership_marker" --tag "$tag" "$context"; then
  if grep -a -F -q -f "$secret" "$transcript"; then
    die 'build failed; transcript withheld because it contains the synthetic secret'
  fi
  printf 'build failed; sanitized transcript follows\n' >&2
  sed "s|$secret|<secret-path>|g" "$transcript" >&2
  exit 1
fi
built=true

inspect_help="$private_root/image-inspect-help"
if capture_bounded "$inspect_help" container image inspect --help && grep -q -- '--format' "$inspect_help"; then
  capture_bounded "$inspect" container image inspect --format json "$tag"
else
  capture_bounded "$inspect" container image inspect "$tag"
fi
jq -e 'type == "object" or type == "array"' "$inspect" >/dev/null || die 'image inspect was not structured JSON'
image_id="$(jq -er 'select(type == "array" and length == 1) | .[0].id' "$inspect")" || die 'built image ID missing'
image_has_ownership "$inspect" || die 'built image ownership mismatch'

run_bounded "$operation_timeout" container create --name "$container_name" \
  --label "$ownership_label=$ownership_marker" "$tag" /bin/sh -c 'sleep 30' >/dev/null
created=true
container_owned || die 'created container ownership mismatch'
run_bounded "$operation_timeout" container start "$container_name" >/dev/null
container_owned || die 'container ownership mismatch before stop'
run_bounded "$operation_timeout" container stop "$container_name" >/dev/null
run_bounded "$operation_timeout" container export "$container_name" --output "$exported" >/dev/null

for artifact in "$context/Dockerfile" "$transmitted" "$transcript" "$inspect" "$exported"; do
  if grep -a -F -q -f "$secret" "$artifact"; then
    die "synthetic secret retained in $(basename "$artifact")"
  fi
done

digest="$(jq -r 'if type == "array" then .[0] else . end | .digest // .id // .ID // empty' "$inspect")"
test -n "$digest" || die 'image inspect omitted digest/id'
printf 'PASS image=%s cleanup=scheduled\n' "$digest"
