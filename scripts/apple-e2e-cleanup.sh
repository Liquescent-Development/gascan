#!/bin/sh
set -eu

manifest=${1:?cleanup manifest required}
trusted_cli=${2:?trusted cleanup CLI required}
trusted_root=${3:?trusted cleanup root required}
test -f "$manifest" || exit 0

canonical_existing() {
  value=$(realpath "$1" 2>/dev/null) || return 1
  test "$value" = "$1" || return 1
  printf '%s\n' "$value"
}
owned_private_directory() {
  directory=$1
  test -d "$directory" && test ! -L "$directory" || return 1
  if metadata=$(stat -f '%Lp %u' "$directory" 2>/dev/null); then :; else metadata=$(stat -c '%a %u' "$directory"); fi
  test "$metadata" = "700 $(id -u)"
}
owned_private_file() {
  file=$1
  if metadata=$(stat -f '%Lp %u' "$file" 2>/dev/null); then :; else metadata=$(stat -c '%a %u' "$file"); fi
  test "$metadata" = "600 $(id -u)"
}
recorded_child_directory() {
  child=$1
  parent=$2
  description=$3
  base=$(basename "$child")
  test "$child" = "$parent/$base" || { printf 'refusing %s path escape\n' "$description" >&2; return 1; }
  test ! -L "$child" || { printf 'refusing %s path escape\n' "$description" >&2; return 1; }
  if test -e "$child"; then
    canonical=$(canonical_existing "$child") || { printf 'refusing %s path escape\n' "$description" >&2; return 1; }
    test "$canonical" = "$child" || { printf 'refusing %s path escape\n' "$description" >&2; return 1; }
    owned_private_directory "$child" || { printf 'refusing unsafe %s root\n' "$description" >&2; return 1; }
  fi
  printf '%s\n' "$child"
}

trusted_cli=$(canonical_existing "$trusted_cli") || { printf 'refusing untrusted cleanup CLI\n' >&2; exit 65; }
trusted_root=$(canonical_existing "$trusted_root") || { printf 'refusing untrusted cleanup root\n' >&2; exit 65; }
owned_private_directory "$trusted_root" || { printf 'refusing unsafe cleanup root\n' >&2; exit 65; }
manifest=$(canonical_existing "$manifest") || { printf 'refusing noncanonical cleanup manifest\n' >&2; exit 65; }
owned_private_file "$manifest" || { printf 'refusing unsafe cleanup manifest\n' >&2; exit 65; }
test "$(dirname "$manifest")" = "$trusted_root" || { printf 'refusing cleanup manifest path escape\n' >&2; exit 65; }
case $(basename "$manifest") in *.json) ;; *) printf 'refusing cleanup manifest name\n' >&2; exit 65 ;; esac

version=$(jq -er '.version' "$manifest")
id=$(jq -er '.sandbox_id' "$manifest")
managed=$(jq -er '.managed_by' "$manifest")
token=$(jq -er '.owner_token' "$manifest")
instance=$(jq -er '.daemon_instance_path' "$manifest")
daemon_executable=$(jq -er '.daemon_executable' "$manifest")
daemon_cli=$(jq -er '.daemon_cli' "$manifest")
runtime_root=$(jq -er '.runtime_root' "$manifest")
project_root=$(jq -er '.project_root' "$manifest")
session_root=$(jq -er '.session_root' "$manifest")

test "$version" = 1
test "$managed" = gascan
test "$daemon_cli" = "$trusted_cli" || { printf 'refusing cleanup CLI mismatch\n' >&2; exit 65; }
case $(basename "$session_root") in session-*) ;; *) printf 'refusing session name\n' >&2; exit 65 ;; esac
session_root=$(recorded_child_directory "$session_root" "$trusted_root" session) || exit 65
case $(basename "$runtime_root") in gascan-gate4-runtime-*) ;; *) printf 'refusing runtime name\n' >&2; exit 65 ;; esac
case $(basename "$project_root") in gascan-gate4-root-*) ;; *) printf 'refusing project name\n' >&2; exit 65 ;; esac
runtime_root=$(recorded_child_directory "$runtime_root" "$session_root" runtime) || exit 65
project_root=$(recorded_child_directory "$project_root" "$session_root" project) || exit 65
test "$instance" = "$runtime_root/daemon-instance.json" || { printf 'refusing instance record path escape\n' >&2; exit 65; }
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

residue=false
if test -f "$instance"; then
  owned_private_file "$instance" || { printf 'refusing unsafe instance record\n' >&2; exit 65; }
  record_token=$(jq -er '.owner_token' "$instance")
  pid=$(jq -er '.pid' "$instance")
  case $pid in ''|*[!0-9]*|0) printf 'refusing invalid daemon pid\n' >&2; exit 65 ;; esac
  record_executable=$(jq -er '.executable' "$instance")
  record_start=$(jq -er '.start_identity' "$instance")
  record_instance=$(jq -er '.instance_token' "$instance")
  observed_command=$(ps -p "$pid" -o command= 2>/dev/null || true)
  observed_start=$(ps -p "$pid" -o lstart= 2>/dev/null | sed 's/^ *//;s/ *$//' || true)
  observed_executable=${observed_command%% *}
  if test -n "$observed_executable"; then observed_executable=$(realpath "$observed_executable" 2>/dev/null || true); fi
  if test "$observed_executable" = "$daemon_executable"; then command_matches=true; else command_matches=false; fi
  attestation=$(XDG_RUNTIME_DIR="$runtime_root" "$trusted_cli" daemon-attest 2>/dev/null || true)
  attested_instance=$(printf '%s' "$attestation" | jq -er '.instance_token' 2>/dev/null || true)
  attested_pid=$(printf '%s' "$attestation" | jq -er '.pid' 2>/dev/null || true)
  attested_executable=$(printf '%s' "$attestation" | jq -er '.executable' 2>/dev/null || true)
  attested_start=$(printf '%s' "$attestation" | jq -er '.start_identity' 2>/dev/null || true)
  if test "$record_token" = "$token" && test "$record_executable" = "$daemon_executable" &&
     test "$command_matches" = true && test "$record_start" = "$observed_start" &&
     test "$record_instance" = "$attested_instance" && test "$pid" = "$attested_pid" &&
     test "$record_executable" = "$attested_executable" && test "$record_start" = "$attested_start"; then
    env kill -TERM "$pid"
    deadline=50
    while test "$deadline" -gt 0 && test "$(ps -p "$pid" -o lstart= 2>/dev/null | sed 's/^ *//;s/ *$//' || true)" = "$record_start"; do
      sleep 0.1
      deadline=$((deadline - 1))
    done
    if test "$(ps -p "$pid" -o lstart= 2>/dev/null | sed 's/^ *//;s/ *$//' || true)" = "$record_start"; then
      observed_command=$(ps -p "$pid" -o command= 2>/dev/null || true)
      observed_executable=${observed_command%% *}
      if test -n "$observed_executable"; then observed_executable=$(realpath "$observed_executable" 2>/dev/null || true); fi
      attestation=$(XDG_RUNTIME_DIR="$runtime_root" "$trusted_cli" daemon-attest 2>/dev/null || true)
      test "$observed_executable" = "$daemon_executable" &&
        test "$(printf '%s' "$attestation" | jq -er '.instance_token' 2>/dev/null || true)" = "$record_instance" &&
        test "$(printf '%s' "$attestation" | jq -er '.pid' 2>/dev/null || true)" = "$pid" ||
        { printf 'refusing KILL after identity changed\n' >&2; exit 1; }
      env kill -KILL "$pid"
      deadline=50
      while test "$deadline" -gt 0 && test "$(ps -p "$pid" -o lstart= 2>/dev/null | sed 's/^ *//;s/ *$//' || true)" = "$record_start"; do
        sleep 0.1
        deadline=$((deadline - 1))
      done
    fi
    if test "$(ps -p "$pid" -o lstart= 2>/dev/null | sed 's/^ *//;s/ *$//' || true)" = "$record_start"; then
      printf 'validated daemon instance survived TERM and KILL\n' >&2
      residue=true
    else
      rm -f "$instance"
    fi
  elif test -n "$observed_command"; then
    printf 'refusing unvalidated daemon pid %s\n' "$pid" >&2
    residue=true
  else
    rm -f "$instance"
  fi
fi

container inspect "$id" >/dev/null 2>&1 && residue=true || true
for name in "gascan-mise-$id" "gascan-cache-$id" "gascan-config-$id"; do
  container volume inspect "$name" >/dev/null 2>&1 && residue=true || true
done
if test "$residue" = true; then
  printf 'Gate 4 cleanup residue remains for exact sandbox %s\n' "$id" >&2
  exit 1
fi
rm -rf -- "$runtime_root" "$project_root"
rmdir "$session_root" 2>/dev/null || true
rm -f "$manifest"
