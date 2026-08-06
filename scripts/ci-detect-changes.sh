#!/bin/sh
# Resolve the PR diff and classify it. Impure half of the change detection.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

test -n "${GITHUB_OUTPUT:-}" || {
  printf 'ci-detect-changes: GITHUB_OUTPUT is unset\n' >&2
  exit 1
}

# Path filtering applies only to pull requests. On a push there is no reliable
# base — force-pushes and the initial-push 000…0 sentinel would both need
# fallback logic — so every area runs.
if test "${EVENT_NAME:-}" != pull_request; then
  printf 'ci-detect-changes: event=%s; running every area\n' "${EVENT_NAME:-unset}"
  {
    printf 'rust=true\n'
    printf 'contracts=true\n'
    printf 'engine=true\n'
  } >>"$GITHUB_OUTPUT"
  exit 0
fi

test -n "${BASE_SHA:-}" || { printf 'ci-detect-changes: BASE_SHA is empty\n' >&2; exit 1; }
test -n "${HEAD_SHA:-}" || { printf 'ci-detect-changes: HEAD_SHA is empty\n' >&2; exit 1; }

# Diff explicit SHAs, not HEAD: actions/checkout gives pull requests a synthetic
# merge ref, so HEAD is not the PR head. Three-dot yields changes on the head
# since the merge base.
diff_file=$(mktemp)
classified=$(mktemp)
trap 'rm -f "$diff_file" "$classified"' EXIT INT TERM HUP
git diff --name-only "$BASE_SHA...$HEAD_SHA" >"$diff_file"

printf 'ci-detect-changes: %s changed path(s)\n' "$(wc -l <"$diff_file" | tr -d ' ')"

# Run the classifier as an `if` condition and read $? in the `else` branch. A
# plain call followed by `rc=$?` is dead code under `set -e` — the shell would
# already have exited — and piping into grep would make $? grep's status, which
# is the trap this plan's Global Constraints exist to prevent.
if "$root/scripts/ci-classify-paths.sh" <"$diff_file" >"$classified"; then
  :
else
  classify_rc=$?
  printf 'ci-detect-changes: classifier exited %d\n' "$classify_rc" >&2
  exit "$classify_rc"
fi

# Notices go to the log; only key=value pairs reach the output file.
grep '^::notice::' "$classified" || true
grep -v '^::notice::' "$classified" >>"$GITHUB_OUTPUT"
cat "$GITHUB_OUTPUT"
