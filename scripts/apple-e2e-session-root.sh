#!/bin/sh
set -eu

uid=$(id -u)
cleanup_root=/private/tmp/gascan-gate4-$uid

if test -L "$cleanup_root"; then
  printf 'refusing symlinked Gate 4 cleanup root\n' >&2
  exit 65
fi
if ! test -e "$cleanup_root"; then
  mkdir -m 700 "$cleanup_root"
fi
test -d "$cleanup_root" || { printf 'refusing non-directory Gate 4 cleanup root\n' >&2; exit 65; }
canonical=$(realpath "$cleanup_root" 2>/dev/null) || { printf 'refusing noncanonical Gate 4 cleanup root\n' >&2; exit 65; }
test "$canonical" = "$cleanup_root" || { printf 'refusing redirected Gate 4 cleanup root\n' >&2; exit 65; }
if metadata=$(stat -f '%Lp %u' "$cleanup_root" 2>/dev/null); then :; else metadata=$(stat -c '%a %u' "$cleanup_root"); fi
test "$metadata" = "700 $uid" || { printf 'refusing unsafe Gate 4 cleanup root\n' >&2; exit 65; }

# Darwin sockaddr_un.sun_path is 104 bytes including its terminating NUL. The
# daemon binds a randomized staging name before renaming it to gascand.sock, so
# validate the longest path the runner and tempfile can create, not only the
# final socket name.
longest_bind_path=$cleanup_root/session-XXXXXXXXXXXX/gascan-gate4-runtime-XXXXXX/gascan/.XXXXXXXXXX
path_bytes=$(LC_ALL=C printf '%s' "$longest_bind_path" | wc -c | tr -d ' ')
test "$path_bytes" -lt 104 || { printf 'Gate 4 socket staging path would exceed SUN_LEN\n' >&2; exit 65; }

printf '%s\n' "$cleanup_root"
