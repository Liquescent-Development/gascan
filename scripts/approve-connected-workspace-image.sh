#!/usr/bin/env bash
set -euo pipefail

tool_root=$(cd "$(dirname "$0")/.." && pwd -P)
configured_root=${GASCAN_APPROVAL_TEST_ROOT:-$tool_root}
root=$(cd "$configured_root" 2>/dev/null && pwd -P) || {
  printf 'connected image approval: configured root is unavailable\n' >&2
  exit 1
}
artifacts=${GASCAN_GATE_ARTIFACTS:-"$root/.artifacts"}
candidate_file="$artifacts/connected-workspace-image-candidate.txt"
live_file="$artifacts/connected-workspace-image-apple-live.txt"
reference_file="$artifacts/workspace-image-ref"
receipt_file="$artifacts/workspace-image-build.json"
evidence_file="$root/docs/evidence/connected-workspace-image.md"
approved_file="$root/images/workspace/approved-image.txt"
validator=${GASCAN_APPROVAL_RECEIPT_VALIDATOR:-"$root/scripts/validate-connected-image-receipt.sh"}
die() {
  printf 'connected image approval: %s\n' "$*" >&2
  exit 1
}

exact_reference() {
  local file=$1 value lines
  test -f "$file" || die "required receipt is unavailable: $file"
  lines=$(wc -l <"$file" | tr -d ' ')
  test "$lines" = 1 || die "receipt must contain exactly one line: $file"
  IFS= read -r value <"$file"
  [[ "$value" =~ ^[a-z0-9][a-z0-9._/-]*:[a-zA-Z0-9._-]+@sha256:[0-9a-f]{64}$ ]] ||
    die "receipt reference is not immutable: $file"
  printf '%s\n' "$value"
}

candidate=$(exact_reference "$candidate_file")
live=$(exact_reference "$live_file")
validated=$("$validator" "$reference_file" "$receipt_file") ||
  die 'build receipt pair is invalid'
test "$candidate" = "$validated" || die 'candidate differs from the validated build receipt'
test "$live" = "$candidate" || die 'Apple live acceptance differs from the candidate'

mkdir -p "$(dirname "$evidence_file")" "$(dirname "$approved_file")"
evidence_tmp=$(mktemp "$(dirname "$evidence_file")/.connected-workspace-image.XXXXXX")
approved_tmp=$(mktemp "$(dirname "$approved_file")/.approved-image.XXXXXX")
evidence_backup=$(mktemp "$(dirname "$evidence_file")/.connected-workspace-image-backup.XXXXXX")
approved_backup=$(mktemp "$(dirname "$approved_file")/.approved-image-backup.XXXXXX")
evidence_existed=false
approved_existed=false
published_evidence=false
published_approval=false
if test -f "$evidence_file"; then
  cp -p "$evidence_file" "$evidence_backup"
  cp -p "$evidence_file" "$evidence_tmp"
  evidence_existed=true
else
  chmod 0644 "$evidence_tmp"
fi
if test -f "$approved_file"; then
  cp -p "$approved_file" "$approved_backup"
  cp -p "$approved_file" "$approved_tmp"
  approved_existed=true
else
  chmod 0644 "$approved_tmp"
fi
rollback() {
  if $published_approval; then
    if $approved_existed; then mv -f "$approved_backup" "$approved_file"; else rm -f "$approved_file"; fi
  fi
  if $published_evidence; then
    if $evidence_existed; then mv -f "$evidence_backup" "$evidence_file"; else rm -f "$evidence_file"; fi
  fi
  rm -f "$evidence_tmp" "$approved_tmp" "$evidence_backup" "$approved_backup"
}
on_signal() {
  code=$1
  trap - EXIT INT TERM
  rollback
  exit "$code"
}
test_boundary() {
  boundary=$1
  legacy=${2:-}
  if test "${GASCAN_APPROVAL_TEST_BOUNDARY:-}" = "$boundary" ||
    { test -n "$legacy" && test "${GASCAN_APPROVAL_TEST_BOUNDARY:-}" = "$legacy"; }
  then
    case "${GASCAN_APPROVAL_TEST_ACTION:-}" in
      FAIL) false ;;
      INT) kill -INT $$ ;;
      TERM) kill -TERM $$ ;;
    esac
  fi
}
trap rollback EXIT
trap 'on_signal 130' INT
trap 'on_signal 143' TERM
lock_digest=$(shasum -a 256 "$root/images/workspace/versions.lock" | awk '{print $1}')
receipt_digest=$(shasum -a 256 "$receipt_file" | awk '{print $1}')
printf '# Connected workspace image evidence\n\n- status: `PASS`\n- platform: `linux/arm64`\n- image: `%s`\n- versions lock SHA-256: `%s`\n- build receipt SHA-256: `%s`\n- final current-token residue: `absent`\n' \
  "$candidate" "$lock_digest" "$receipt_digest" >"$evidence_tmp"
printf '%s' "$candidate" >"$approved_tmp"
published_evidence=true
test_boundary before-evidence-replacement
mv -f "$evidence_tmp" "$evidence_file"
test_boundary after-evidence-replacement after-evidence
evidence_tmp=''
published_approval=true
test_boundary before-approval-replacement
mv -f "$approved_tmp" "$approved_file"
test_boundary after-approval-replacement
approved_tmp=''
trap - EXIT INT TERM
rm -f "$evidence_backup" "$approved_backup"
printf '%s\n' "$candidate"
