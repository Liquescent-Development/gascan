#!/bin/sh
# Fail if the set of #[ignore]d tests drifts from the recorded baseline, in
# either direction: a new quarantine, or a heavy test that vanished.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

expected=tests/ci/expected-ignored-tests.txt
test -f "$expected" || {
  printf 'ci-check-ignored-tests: %s is missing\n' "$expected" >&2
  exit 1
}

listing=$(mktemp)
actual=$(mktemp)
errors=$(mktemp)
trap 'rm -f "$listing" "$actual" "$errors"' EXIT INT TERM HUP

# cargo writes compile progress to stderr, so hold it aside rather than let it
# drown the diff — but replay it on failure. Discarding it outright would turn a
# compile error into an exit code with no explanation.
if cargo test --workspace -- --ignored --list >"$listing" 2>"$errors"; then
  :
else
  list_rc=$?
  printf 'ci-check-ignored-tests: listing exited %d\n' "$list_rc" >&2
  cat "$errors" >&2
  exit "$list_rc"
fi

sed -n 's/: test$//p' "$listing" | sort >"$actual"

if diff -u "$expected" "$actual"; then
  printf 'ci-check-ignored-tests: %s ignored test(s), matching the baseline\n' \
    "$(wc -l <"$actual" | tr -d ' ')"
else
  printf '\nci-check-ignored-tests: the ignored-test set changed.\n' >&2
  printf 'If deliberate, regenerate the baseline and say why in the commit:\n' >&2
  printf '  cargo test --workspace -- --ignored --list 2>/dev/null \\\n' >&2
  printf '    | sed -n %ss/: test$//p%s | sort > %s\n' "'" "'" "$expected" >&2
  exit 1
fi
