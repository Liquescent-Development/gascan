#!/bin/sh
set -eu

manifest=${1:?cleanup manifest required}
test -f "$manifest" || exit 0

version=$(jq -er '.version' "$manifest")
id=$(jq -er '.sandbox_id' "$manifest")
managed=$(jq -er '.managed_by' "$manifest")
token=$(jq -er '.owner_token' "$manifest")
instance=$(jq -er '.daemon_instance_path' "$manifest")
daemon_executable=$(jq -er '.daemon_executable' "$manifest")

test "$version" = 1
test "$managed" = gascan
case $id in
  *-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]) ;;
  *) printf 'refusing invalid sandbox id in %s\n' "$manifest" >&2; exit 65 ;;
esac

expected=$(printf '%s\n%s\n%s\n%s\n' "$id" "gascan-mise-$id" "gascan-cache-$id" "gascan-config-$id")
actual=$(jq -er '.resources[]' "$manifest")
test "$actual" = "$expected" || { printf 'refusing out-of-scope cleanup manifest\n' >&2; exit 65; }

owned_container=false
if inspected=$(container inspect "$id" 2>/dev/null); then
  labels=$(printf '%s' "$inspected" | jq -er 'if type == "array" then .[0].configuration.labels else .configuration.labels end')
  if test "$(printf '%s' "$labels" | jq -er '."dev.gascan.managed-by"')" = gascan &&
     test "$(printf '%s' "$labels" | jq -er '."dev.gascan.sandbox-id"')" = "$id"; then
    owned_container=true
  else
    printf 'collision: refusing container %s with mismatched labels\n' "$id" >&2
  fi
fi
if test "$owned_container" = true; then
  container stop --time 5 "$id" >/dev/null 2>&1 || true
  container delete "$id"
fi

for name in "gascan-mise-$id" "gascan-cache-$id" "gascan-config-$id"; do
  if inspected=$(container volume inspect "$name" 2>/dev/null); then
    labels=$(printf '%s' "$inspected" | jq -er 'if type == "array" then .[0].configuration.labels else .configuration.labels end')
    if test "$(printf '%s' "$labels" | jq -er '."dev.gascan.managed-by"')" = gascan &&
       test "$(printf '%s' "$labels" | jq -er '."dev.gascan.sandbox-id"')" = "$id"; then
      container volume delete "$name"
    else
      printf 'collision: refusing volume %s with mismatched labels\n' "$name" >&2
    fi
  fi
done

if test -f "$instance"; then
  record_token=$(jq -er '.owner_token' "$instance")
  pid=$(jq -er '.pid' "$instance")
  record_executable=$(jq -er '.executable' "$instance")
  record_start=$(jq -er '.start_identity' "$instance")
  observed_command=$(ps -p "$pid" -o command= 2>/dev/null || true)
  observed_start=$(ps -p "$pid" -o lstart= 2>/dev/null | sed 's/^ *//;s/ *$//' || true)
  case $observed_command in "$daemon_executable"*) command_matches=true ;; *) command_matches=false ;; esac
  if test "$record_token" = "$token" && test "$record_executable" = "$daemon_executable" &&
     test "$command_matches" = true && test "$record_start" = "$observed_start"; then
    kill -TERM "$pid"
  elif test -n "$observed_command"; then
    printf 'refusing unvalidated daemon pid %s\n' "$pid" >&2
  fi
fi

residue=false
container inspect "$id" >/dev/null 2>&1 && residue=true || true
for name in "gascan-mise-$id" "gascan-cache-$id" "gascan-config-$id"; do
  container volume inspect "$name" >/dev/null 2>&1 && residue=true || true
done
if test "$residue" = true; then
  printf 'Gate 4 cleanup residue remains for exact sandbox %s\n' "$id" >&2
  exit 1
fi
rm -f "$manifest"
