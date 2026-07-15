#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd -P)
lock="$root/images/workspace/versions.lock"
artifacts="$root/.artifacts"
context="$artifacts/connected-workspace-context"
private_context='/private/context'
base_image='ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab'
reviewed_revision='f6b248c5926240856dbea83d1d2c5c90ea1c1456'
die() { printf 'connected workspace image build: %s\n' "$*" >&2; exit 1; }
run_tool() { cargo run --quiet --locked --offline --manifest-path "$root/scripts/Cargo.toml" --bin "$1" -- "${@:2}"; }
top_value() { awk -F ' = ' -v key="$1" '$1 == key { gsub(/^"|"$/, "", $2); print $2; exit }' "$lock"; }

test -f "$lock" || die 'missing image lock'
test "$(top_value workspace_build_mode)" = connected || die 'connected entrypoint requires exact connected lock'
test -d "$context" || die 'missing connected workspace context'
test "$(top_value base_image)" = "$base_image" || die 'base image differs from reviewed digest'
gascamp_revision=$(awk -F ' = ' '$1 == "revision" { gsub(/^"|"$/, "", $2); print $2; exit }' "$lock")
test "$gascamp_revision" = "$reviewed_revision" || die 'Gascamp revision differs from reviewed revision'
tag=$(top_value workspace_tag)
[[ "$tag" =~ ^gascan-workspace:[a-z0-9._-]+$ ]] || die 'workspace tag is not exact'

test -n "${GASCAMP_READ_TOKEN_FILE:-}" || die 'GASCAMP_READ_TOKEN_FILE is required'
case "$GASCAMP_READ_TOKEN_FILE" in /*) ;; *) die 'secret path must be absolute' ;; esac
test ! -L "$GASCAMP_READ_TOKEN_FILE" || die 'secret must not be a symbolic link'
secret=$(realpath "$GASCAMP_READ_TOKEN_FILE") || die 'cannot canonicalize secret path'
case "$secret" in "$root"|"$root"/*) die 'secret file must be outside the repository' ;; esac
test -f "$secret" || die 'secret must be a regular file'
test ! -L "$secret" || die 'secret must not be a symbolic link'
uid=$(id -u)
owner=$(stat -f %u "$secret" 2>/dev/null || stat -c %u "$secret")
mode=$(stat -f %Lp "$secret" 2>/dev/null || stat -c %a "$secret")
test "$owner" = "$uid" || die 'secret must be owned by current UID'
test "$mode" = 600 || die 'secret mode must be 0600'
test -s "$secret" || die 'secret must not be empty'
test "$(awk 'NF { count++ } END { print count + 0 }' "$secret")" = 1 || die 'secret must contain one nonempty line'
test "$(wc -l <"$secret" | tr -d ' ')" = 1 || die 'secret must contain exactly one line'

context_manifest=$(run_tool prepare-workspace-context --verify "$root" "$lock" "$artifacts" "$context")
[[ "$context_manifest" =~ ^[0-9a-f]{64}$ ]] || die 'context verifier returned an invalid digest'
snapshot_helper='/Library/PrivilegedHelperTools/dev.gascan.snapshot-workspace-context'
helper_identity=$(run_tool snapshot-helper-identity "$snapshot_helper") || die 'snapshot helper identity is unsafe'
IFS=$'\t' read -r helper_sha256 helper_device helper_inode <<<"$helper_identity"
snapshot_receipt=''
cleanup() {
  status=$?
  rm -f "$private_context/.build-secrets/gascamp_read_token" 2>/dev/null || status=1
  test -z "$snapshot_receipt" || sudo -n "$snapshot_helper" --self "$helper_sha256" "$helper_device" "$helper_inode" finish "$snapshot_receipt" >/dev/null || status=1
  exit "$status"
}
trap cleanup EXIT INT TERM
snapshot_receipt=$(sudo -n "$snapshot_helper" --self "$helper_sha256" "$helper_device" "$helper_inode" create "$context" "$context_manifest") || die 'snapshot creation failed'
snapshot=$(sudo -n "$snapshot_helper" --self "$helper_sha256" "$helper_device" "$helper_inode" path "$snapshot_receipt") || die 'snapshot validation failed'
test "$snapshot" = "$private_context" || die 'reviewed snapshot is not mounted at /private/context'
test -d "$private_context" || die 'private context is unavailable'
mkdir "$private_context/.build-secrets"
chmod 0700 "$private_context/.build-secrets"
install -m 0600 "$secret" "$private_context/.build-secrets/gascamp_read_token"
printf '%s\n' '.build-secrets' >"$private_context/.dockerignore"
test "$(stat -f %Lp "$private_context/.build-secrets/gascamp_read_token" 2>/dev/null || stat -c %a "$private_context/.build-secrets/gascamp_read_token")" = 600 || die 'staged secret mode is unsafe'
test "$(cat "$private_context/.dockerignore")" = .build-secrets || die 'secret exclusion is invalid'
if tar -cf - --exclude='./.build-secrets' -C "$private_context" . | tar -tf - | grep -q '^\./\.build-secrets'; then
  die 'secret entered transmitted context'
fi

base_inspect=$(container image inspect --format json "$base_image")
test "$(printf '%s' "$base_inspect" | run_tool validate-image-inspect)" = "${base_image#ubuntu@}" || die 'exact local base is unavailable'
container build --arch arm64 \
  --secret "id=gascamp_read_token,src=/private/context/.build-secrets/gascamp_read_token" \
  --build-arg "BASE_IMAGE=$base_image" \
  --build-arg "GASCAMP_REVISION=$gascamp_revision" \
  --tag "$tag" --file "$private_context/Dockerfile" "$private_context" >/dev/null 2>&1

test "$(run_tool prepare-workspace-context --verify "$root" "$lock" "$artifacts" "$context")" = "$context_manifest" || die 'workspace context changed during build'
test "$(cat "$private_context/.dockerignore")" = .build-secrets || die 'secret exclusion changed during build'
test -f "$private_context/.build-secrets/gascamp_read_token" || die 'staged secret changed during build'
image_inspect=$(container image inspect --format json "$tag")
image_digest=$(printf '%s' "$image_inspect" | run_tool validate-connected-build "$tag") || die 'built image inspect is invalid'
reference="$tag@$image_digest"
[[ "$reference" =~ ^gascan-workspace:[a-z0-9._-]+@sha256:[0-9a-f]{64}$ ]] || die 'final image reference is invalid'

mkdir -p "$artifacts"
ref_tmp=$(mktemp "$artifacts/.workspace-image-ref.XXXXXX")
json_tmp=$(mktemp "$artifacts/.workspace-image-build.XXXXXX")
trap 'rm -f "$ref_tmp" "$json_tmp"; cleanup' EXIT INT TERM
printf '%s\n' "$reference" >"$ref_tmp"
started_at=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
printf '{"platform":"linux/arm64","lock_digest":"%s","context_digest":"%s","image_digest":"%s","apple_version":"%s","started_at":"%s","finished_at":"%s","status":"succeeded"}\n' \
  "$(shasum -a 256 "$lock" | cut -d' ' -f1)" "$context_manifest" "$image_digest" "$(sw_vers -productVersion)" "$started_at" "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" >"$json_tmp"
mv -f "$json_tmp" "$artifacts/workspace-image-build.json"
mv -f "$ref_tmp" "$artifacts/workspace-image-ref"
trap cleanup EXIT INT TERM
printf '%s\n' "$reference"
