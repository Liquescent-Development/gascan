#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd -P)
lock="$root/images/workspace/versions.lock"
artifacts="$root/.artifacts"
context="$artifacts/connected-workspace-context"
base_image='ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab'
reviewed_revision='f6b248c5926240856dbea83d1d2c5c90ea1c1456'
die() { printf 'connected workspace image build: %s\n' "$*" >&2; exit 1; }
run_tool() { cargo run --quiet --locked --offline --manifest-path "$root/scripts/Cargo.toml" --bin "$1" -- "${@:2}"; }
top_value() { awk -F ' = ' -v key="$1" '$1 == key { gsub(/^"|"$/, "", $2); print $2; exit }' "$lock"; }

for name in $(compgen -e); do
  case "$name" in
    GASCAMP_*TOKEN*|GITHUB_TOKEN|GH_TOKEN|GITLAB_TOKEN|DOCKER_AUTH_CONFIG|HTTP_AUTHORIZATION|AUTHORIZATION|AWS_ACCESS_KEY_ID|AWS_SECRET_ACCESS_KEY|AWS_SESSION_TOKEN|BUILD_TOKEN|BUILD_*_TOKEN|*_BUILD_TOKEN|*_BUILD_*_TOKEN|BUILD_CREDENTIAL|BUILD_*_CREDENTIAL|*_BUILD_CREDENTIAL|*_BUILD_*_CREDENTIAL|BUILD_PASSWORD|BUILD_*_PASSWORD|*_BUILD_PASSWORD|*_BUILD_*_PASSWORD|BUILD_SECRET|BUILD_*_SECRET|*_BUILD_SECRET|*_BUILD_*_SECRET)
      test -z "${!name:-}" || die "authentication input is forbidden: $name"
      ;;
  esac
done
for argument in "$@"; do
  case "$argument" in --secret|--secret=*|*Authorization:*|*authorization:*) die 'secret-bearing build option is forbidden' ;; esac
done
test "$#" -eq 0 || die 'unexpected build argument'

operation_timeout=${GASCAN_CONNECTED_TIMEOUT_SECONDS:-300}
case "$operation_timeout" in ''|*[!0-9]*) die 'timeout must be a positive integer' ;; esac
test "$operation_timeout" -gt 0 || die 'timeout must be a positive integer'
run_bounded() {
  timeout_seconds=$1; shift
  set -m
  "$@" & command_pid=$!
  ( sleep "$timeout_seconds"; if kill -0 "$command_pid" 2>/dev/null; then kill -TERM -- "-$command_pid" 2>/dev/null || true; sleep 1; kill -KILL -- "-$command_pid" 2>/dev/null || true; fi ) & watchdog_pid=$!
  set +m
  if wait "$command_pid"; then result=0; else result=$?; fi
  kill -TERM -- "-$watchdog_pid" 2>/dev/null || true
  wait "$watchdog_pid" 2>/dev/null || true
  return "$result"
}

test -f "$lock" || die 'missing image lock'
test "$(top_value workspace_build_mode)" = connected || die 'connected entrypoint requires exact connected lock'
test -d "$context" || die 'missing connected workspace context'
test "$(top_value base_image)" = "$base_image" || die 'base image differs from reviewed digest'
gascamp_revision=$(awk -F ' = ' '$1 == "revision" { gsub(/^"|"$/, "", $2); print $2; exit }' "$lock")
test "$gascamp_revision" = "$reviewed_revision" || die 'Gascamp revision differs from reviewed revision'
tag=$(top_value workspace_tag)
[[ "$tag" =~ ^gascan-workspace:[a-z0-9._-]+$ ]] || die 'workspace tag is not exact'

snapshot_helper='/Library/PrivilegedHelperTools/dev.gascan.snapshot-workspace-context'
snapshot_receipt=''
build_diagnostic=''
diagnostic_sensitive=''
helper_sha256=''; helper_device=''; helper_inode=''
cleanup() {
  status=${1:-$?}
  test -z "$build_diagnostic" || rm -f "$build_diagnostic" || status=1
  test -z "$diagnostic_sensitive" || rm -f "$diagnostic_sensitive" || status=1
  test -z "$snapshot_receipt" || run_bounded "$operation_timeout" sudo -n "$snapshot_helper" --self "$helper_sha256" "$helper_device" "$helper_inode" finish "$snapshot_receipt" >/dev/null || status=1
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
started_at=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
context_manifest=$(run_tool prepare-workspace-context --verify-connected "$root" "$lock" "$artifacts" "$context")
[[ "$context_manifest" =~ ^[0-9a-f]{64}$ ]] || die 'context verifier returned an invalid digest'
helper_identity=$(run_tool snapshot-helper-identity "$snapshot_helper") || die 'snapshot helper identity is unsafe'
IFS=$'\t' read -r helper_sha256 helper_device helper_inode <<<"$helper_identity"
snapshot_receipt=$(run_bounded "$operation_timeout" sudo -n "$snapshot_helper" --self "$helper_sha256" "$helper_device" "$helper_inode" create "$context" "$context_manifest") || die 'snapshot creation failed'
snapshot=$(run_bounded "$operation_timeout" sudo -n "$snapshot_helper" --self "$helper_sha256" "$helper_device" "$helper_inode" path "$snapshot_receipt") || die 'snapshot validation failed'
test -d "$snapshot" || die 'sealed public snapshot is unavailable'
snapshot_manifest=$(shasum -a 256 "$snapshot/context-manifest.tsv" | cut -d' ' -f1)
test "$snapshot_manifest" = "$context_manifest" || die 'sealed public manifest differs before build'

base_inspect=$(container image inspect "$base_image")
test "$(printf '%s' "$base_inspect" | run_tool validate-image-inspect)" = "${base_image#ubuntu@}" || die 'exact local base is unavailable'
umask 077
build_diagnostic=$(mktemp "$artifacts/.connected-build-diagnostic.XXXXXX") || die 'cannot create private build diagnostic'
diagnostic_sensitive=$(mktemp "$artifacts/.connected-build-sensitive.XXXXXX") || die 'cannot create private diagnostic marker'
chmod 0600 "$build_diagnostic" || die 'cannot protect build diagnostic'
chmod 0600 "$diagnostic_sensitive" || die 'cannot protect diagnostic marker'
diagnostic_limit=131072
set +e
container build --arch arm64 \
  --build-arg "BASE_IMAGE=$base_image" \
  --build-arg "GASCAMP_REVISION=$gascamp_revision" \
  --tag "$tag" --file "$snapshot/Dockerfile" "$snapshot" 2>&1 \
  | LC_ALL=C awk -v limit="$((diagnostic_limit + 1))" -v marker="$diagnostic_sensitive" '
      BEGIN { pattern = "(^|[^[:alnum:]_])(authorization|bearer|token|secret|password|credential)([^[:alnum:]_]|$)" }
      { if (tolower($0) ~ pattern) print "sensitive" >marker; chunk = $0 ORS; remaining = limit - written; if (remaining > 0) { printf "%s", substr(chunk, 1, remaining); written += length(chunk) } }
    ' >"$build_diagnostic"
build_status=${PIPESTATUS[0]}
set -e
if test "$build_status" -ne 0; then
  if test -s "$diagnostic_sensitive"; then
    printf 'connected workspace image build: diagnostic rejected as potentially sensitive\n' >&2
  else
    printf 'connected workspace image build: container build failed (status %s); bounded diagnostic follows:\n' "$build_status" >&2
    head -c "$diagnostic_limit" "$build_diagnostic" >&2
    test "$(wc -c <"$build_diagnostic" | tr -d ' ')" -le "$diagnostic_limit" || printf '\nconnected workspace image build: diagnostic truncated\n' >&2
  fi
  exit "$build_status"
fi
rm -f "$build_diagnostic"
build_diagnostic=''
rm -f "$diagnostic_sensitive"
diagnostic_sensitive=''

test "$(shasum -a 256 "$snapshot/context-manifest.tsv" | cut -d' ' -f1)" = "$context_manifest" || die 'sealed public manifest changed during build'
test "$(run_tool prepare-workspace-context --verify-connected "$root" "$lock" "$artifacts" "$context")" = "$context_manifest" || die 'workspace context changed during build'
image_inspect=$(container image inspect "$tag")
image_digest=$(printf '%s' "$image_inspect" | run_tool validate-connected-build "$tag") || die 'built image inspect is invalid'
reference="$tag@$image_digest"
[[ "$reference" =~ ^gascan-workspace:[a-z0-9._-]+@sha256:[0-9a-f]{64}$ ]] || die 'final image reference is invalid'

mkdir -p "$artifacts"
ref_tmp=$(mktemp "$artifacts/.workspace-image-ref.XXXXXX")
json_tmp=$(mktemp "$artifacts/.workspace-image-build.XXXXXX")
cleanup_publication() { status=$?; rm -f "$ref_tmp" "$json_tmp" || status=1; cleanup "$status"; }
trap cleanup_publication EXIT
printf '%s\n' "$reference" >"$ref_tmp"
lock_digest=$(shasum -a 256 "$lock" | cut -d' ' -f1)
printf '{"reference":"%s","tag":"%s","platform":"linux/arm64","lock_digest":"%s","context_digest":"%s","image_digest":"%s","apple_version":"%s","started_at":"%s","finished_at":"%s","status":"succeeded"}\n' \
  "$reference" "$tag" "$lock_digest" "$context_manifest" "$image_digest" "$(sw_vers -productVersion)" "$started_at" "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" >"$json_tmp"
run_tool validate-connected-build validate-receipt "$ref_tmp" "$json_tmp" "$lock_digest" "$context_manifest" || die 'build receipt pair is invalid'
mv -f "$json_tmp" "$artifacts/workspace-image-build.json"
mv -f "$ref_tmp" "$artifacts/workspace-image-ref"
trap cleanup EXIT
printf '%s\n' "$reference"
