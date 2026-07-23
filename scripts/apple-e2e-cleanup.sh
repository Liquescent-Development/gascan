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
remove_outside_sentinel() {
  sentinel=$1
  parent=$2
  require_private_mode=$3
  base=$(basename "$sentinel")
  test "$sentinel" = "$parent/$base" || { printf 'refusing outside sentinel path escape\n' >&2; return 1; }
  printf '%s' "$base" | jq -R -e 'test("^synthetic-outside-[0-9a-f]{32}$")' >/dev/null || { printf 'refusing invalid outside sentinel identity\n' >&2; return 1; }
  if test -e "$sentinel" || test -L "$sentinel"; then
    test -f "$sentinel" && test ! -L "$sentinel" || { printf 'refusing unsafe outside sentinel\n' >&2; return 1; }
    if metadata=$(stat -f '%Lp %u' "$sentinel" 2>/dev/null); then :; else metadata=$(stat -c '%a %u' "$sentinel"); fi
    owner=${metadata#* }
    mode=${metadata% *}
    test "$owner" = "$(id -u)" || { printf 'refusing foreign outside sentinel\n' >&2; return 1; }
    if test "$require_private_mode" = true; then
      test "$mode" = 600 || { printf 'refusing nonprivate outside sentinel\n' >&2; return 1; }
    fi
    rm -f -- "$sentinel"
    test ! -e "$sentinel" && test ! -L "$sentinel" || { printf 'outside sentinel removal failed\n' >&2; return 1; }
  fi
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
jq -e \
  --arg container "$id" \
  --arg mise "gascan-mise-$id" \
  --arg cache "gascan-cache-$id" \
  --arg config "gascan-config-$id" \
  --arg network "gascan-network-$id" '
    (.resources | type) == "array" and
    (.resources | length) == 5 and
    all(.resources[]; type == "string") and
    .resources[0] == $container and
    .resources[1] == $mise and
    .resources[2] == $cache and
    .resources[3] == $config and
    .resources[4] == $network
  ' "$manifest" >/dev/null || { printf 'refusing out-of-scope cleanup manifest\n' >&2; exit 65; }
jq -e '.dns_domain == null or (.dns_domain | type == "string")' "$manifest" >/dev/null || { printf 'refusing invalid DNS cleanup record\n' >&2; exit 65; }
dns_domain=$(jq -r '.dns_domain // ""' "$manifest")
if test -n "$dns_domain"; then
  printf '%s' "$dns_domain" | jq -R -e 'test("^gascan-[0-9a-f]{32}\\.test$")' >/dev/null || { printf 'refusing foreign DNS cleanup identity\n' >&2; exit 65; }
fi
abort_evidence=$(jq -r '.abort_evidence_path // ""' "$manifest")
expected_abort_evidence="$runtime_root/abort-probe-reached.json"
if test -n "$abort_evidence" && test "$abort_evidence" != "$expected_abort_evidence"; then
  printf 'refusing abort evidence path escape\n' >&2
  exit 65
fi
abort_evidence=$expected_abort_evidence
abort_reached=false
if test -e "$abort_evidence" || test -L "$abort_evidence"; then
  test -f "$abort_evidence" && test ! -L "$abort_evidence" || { printf 'refusing unsafe abort evidence\n' >&2; exit 65; }
  owned_private_file "$abort_evidence" || { printf 'refusing unsafe abort evidence metadata\n' >&2; exit 65; }
  jq -e --arg id "$id" --arg token "$token" '
    .version == 1 and
    .kind == "gascan-security-abort-reached" and
    .sandbox_id == $id and
    .owner_token == $token
  ' "$abort_evidence" >/dev/null || { printf 'refusing mismatched abort evidence\n' >&2; exit 65; }
  abort_reached=true
fi
outside_sentinel=$(jq -r '.outside_sentinel_path // ""' "$manifest")
legacy_sentinel=
legacy_count=0
for candidate in "$session_root"/synthetic-outside-*; do
  if test -e "$candidate" || test -L "$candidate"; then
    legacy_count=$((legacy_count + 1))
    legacy_sentinel=$candidate
  fi
done
test "$legacy_count" -le 1 || { printf 'refusing ambiguous legacy outside sentinel cleanup\n' >&2; exit 65; }
if test -n "$outside_sentinel"; then
  test "$legacy_count" -eq 0 || { printf 'refusing mixed outside sentinel cleanup records\n' >&2; exit 65; }
  remove_outside_sentinel "$outside_sentinel" "$runtime_root" true || exit 65
elif test -n "$legacy_sentinel"; then
  remove_outside_sentinel "$legacy_sentinel" "$session_root" false || exit 65
fi

if test -n "$dns_domain"; then
  dns_inventory=$(container system dns list --format json) || { printf 'unable to inventory DNS routes; retaining cleanup manifest\n' >&2; exit 1; }
  printf '%s' "$dns_inventory" | jq -e 'type == "array" and all(.[]; type == "string")' >/dev/null || { printf 'invalid DNS route inventory; retaining cleanup manifest\n' >&2; exit 1; }
  dns_count=$(printf '%s' "$dns_inventory" | jq -r --arg domain "$dns_domain" '[.[] | select(. == $domain)] | length')
  case $dns_count in
    0) ;;
    1)
      sudo -n container system dns delete "$dns_domain" || { printf 'unable to delete exact test-owned DNS route; retaining cleanup manifest\n' >&2; exit 1; }
      ;;
    *) printf 'ambiguous DNS route inventory; retaining cleanup manifest\n' >&2; exit 1 ;;
  esac
  dns_inventory=$(container system dns list --format json) || { printf 'unable to verify DNS route cleanup; retaining cleanup manifest\n' >&2; exit 1; }
  printf '%s' "$dns_inventory" | jq -e 'type == "array" and all(.[]; type == "string")' >/dev/null || { printf 'invalid DNS cleanup inventory; retaining cleanup manifest\n' >&2; exit 1; }
  if printf '%s' "$dns_inventory" | jq -e --arg domain "$dns_domain" 'any(.[]; . == $domain)' >/dev/null; then
    printf 'test-owned DNS route residue remains; retaining cleanup manifest\n' >&2
    exit 1
  fi
fi

fresh_container_record() {
  fresh_inventory=$(container list --all --format json) ||
    { printf 'unable to freshly inventory containers; retaining cleanup manifest\n' >&2; return 1; }
  printf '%s' "$fresh_inventory" |
    jq -e 'type == "array" and all(.[]; type == "object" and ((.configuration.id | type) == "string"))' >/dev/null ||
    { printf 'invalid fresh container inventory; retaining cleanup manifest\n' >&2; return 1; }
  fresh_record=$(printf '%s' "$fresh_inventory" |
    jq -cr --arg id "$id" '[.[] | select(.configuration.id == $id)] | if length == 0 then null elif length == 1 then .[0] else error("duplicate container id") end') ||
    { printf 'ambiguous fresh container inventory; retaining cleanup manifest\n' >&2; return 1; }
  if test "$fresh_record" != null; then
    test "$(printf '%s' "$fresh_record" | jq -er '.configuration.labels."dev.gascan.managed-by"')" = gascan &&
      test "$(printf '%s' "$fresh_record" | jq -er '.configuration.labels."dev.gascan.sandbox-id"')" = "$id" ||
      { printf 'collision: refusing freshly changed container %s\n' "$id" >&2; return 1; }
  fi
  printf '%s\n' "$fresh_record"
}

fresh_volume_record() {
  fresh_name=$1
  fresh_inventory=$(container volume list --format json) ||
    { printf 'unable to freshly inventory volumes; retaining cleanup manifest\n' >&2; return 1; }
  printf '%s' "$fresh_inventory" |
    jq -e 'type == "array" and all(.[]; type == "object" and ((.id | type) == "string") and ((.configuration.name | type) == "string") and (.id == .configuration.name))' >/dev/null ||
    { printf 'invalid fresh volume inventory; retaining cleanup manifest\n' >&2; return 1; }
  fresh_record=$(printf '%s' "$fresh_inventory" |
    jq -cr --arg id "$fresh_name" '[.[] | select(.id == $id and .configuration.name == $id)] | if length == 0 then null elif length == 1 then .[0] else error("duplicate volume id") end') ||
    { printf 'ambiguous fresh volume inventory; retaining cleanup manifest\n' >&2; return 1; }
  if test "$fresh_record" != null; then
    test "$(printf '%s' "$fresh_record" | jq -er '.configuration.labels."dev.gascan.managed-by"')" = gascan &&
      test "$(printf '%s' "$fresh_record" | jq -er '.configuration.labels."dev.gascan.sandbox-id"')" = "$id" ||
      { printf 'collision: refusing freshly changed volume %s\n' "$fresh_name" >&2; return 1; }
  fi
  printf '%s\n' "$fresh_record"
}

fresh_network_record() {
  fresh_inventory=$(container network list --format json) ||
    { printf 'unable to freshly inventory networks; retaining cleanup manifest\n' >&2; return 1; }
  printf '%s' "$fresh_inventory" |
    jq -e 'type == "array" and all(.[]; type == "object" and ((.id | type) == "string") and ((.configuration.name | type) == "string") and (.id == .configuration.name))' >/dev/null ||
    { printf 'invalid fresh network inventory; retaining cleanup manifest\n' >&2; return 1; }
  fresh_record=$(printf '%s' "$fresh_inventory" |
    jq -cr --arg id "$network_name" '[.[] | select(.id == $id and .configuration.name == $id)] | if length == 0 then null elif length == 1 then .[0] else error("duplicate network id") end') ||
    { printf 'ambiguous fresh network inventory; retaining cleanup manifest\n' >&2; return 1; }
  if test "$fresh_record" != null; then
    test "$(printf '%s' "$fresh_record" | jq -er '.configuration.labels."dev.gascan.managed-by"')" = gascan &&
      test "$(printf '%s' "$fresh_record" | jq -er '.configuration.labels."dev.gascan.sandbox-id"')" = "$id" ||
      { printf 'collision: refusing freshly changed network %s\n' "$network_name" >&2; return 1; }
  fi
  printf '%s\n' "$fresh_record"
}

container_inventory=$(container list --all --format json) || { printf 'unable to inventory containers; retaining cleanup manifest\n' >&2; exit 1; }
printf '%s' "$container_inventory" | jq -e 'type == "array" and all(.[]; type == "object" and ((.configuration.id | type) == "string"))' >/dev/null || { printf 'invalid container inventory; retaining cleanup manifest\n' >&2; exit 1; }
volume_inventory=$(container volume list --format json) || { printf 'unable to inventory volumes; retaining cleanup manifest\n' >&2; exit 1; }
printf '%s' "$volume_inventory" | jq -e 'type == "array" and all(.[]; type == "object" and ((.id | type) == "string") and ((.configuration.name | type) == "string") and (.id == .configuration.name))' >/dev/null || { printf 'invalid volume inventory; retaining cleanup manifest\n' >&2; exit 1; }
network_inventory=$(container network list --format json) || { printf 'unable to inventory networks; retaining cleanup manifest\n' >&2; exit 1; }
printf '%s' "$network_inventory" | jq -e 'type == "array" and all(.[]; type == "object" and ((.id | type) == "string") and ((.configuration.name | type) == "string") and (.id == .configuration.name))' >/dev/null || { printf 'invalid network inventory; retaining cleanup manifest\n' >&2; exit 1; }

container_record=$(printf '%s' "$container_inventory" | jq -cr --arg id "$id" '[.[] | select(.configuration.id == $id)] | if length == 0 then null elif length == 1 then .[0] else error("duplicate container id") end') || { printf 'ambiguous container inventory; retaining cleanup manifest\n' >&2; exit 1; }
network_name="gascan-network-$id"
network_record=$(printf '%s' "$network_inventory" | jq -cr --arg id "$network_name" '[.[] | select(.id == $id and .configuration.name == $id)] | if length == 0 then null elif length == 1 then .[0] else error("duplicate network id") end') || { printf 'ambiguous network inventory; retaining cleanup manifest\n' >&2; exit 1; }
if test "$network_record" != null; then
  test "$(printf '%s' "$network_record" | jq -er '.configuration.labels."dev.gascan.managed-by"')" = gascan &&
    test "$(printf '%s' "$network_record" | jq -er '.configuration.labels."dev.gascan.sandbox-id"')" = "$id" ||
    { printf 'collision: refusing network %s with mismatched labels\n' "$network_name" >&2; exit 1; }
fi
for name in "gascan-mise-$id" "gascan-cache-$id" "gascan-config-$id"; do
  volume_record=$(printf '%s' "$volume_inventory" | jq -cr --arg id "$name" '[.[] | select(.configuration.name == $id)] | if length == 0 then null elif length == 1 then .[0] else error("duplicate volume id") end') || { printf 'ambiguous volume inventory; retaining cleanup manifest\n' >&2; exit 1; }
  if test "$volume_record" != null; then
    test "$(printf '%s' "$volume_record" | jq -er '.configuration.labels."dev.gascan.managed-by"')" = gascan &&
      test "$(printf '%s' "$volume_record" | jq -er '.configuration.labels."dev.gascan.sandbox-id"')" = "$id" ||
      { printf 'collision: refusing volume %s with mismatched labels\n' "$name" >&2; exit 1; }
  fi
done
if test "$container_record" != null; then
  test "$(printf '%s' "$container_record" | jq -er '.configuration.labels."dev.gascan.managed-by"')" = gascan &&
    test "$(printf '%s' "$container_record" | jq -er '.configuration.labels."dev.gascan.sandbox-id"')" = "$id" ||
    { printf 'collision: refusing container %s with mismatched labels\n' "$id" >&2; exit 1; }
  fresh_record=$(fresh_container_record) || exit 1
  if test "$fresh_record" != null; then
    container stop --time 5 "$id" >/dev/null 2>&1 || true
    fresh_record=$(fresh_container_record) || exit 1
    if test "$fresh_record" != null; then
      container delete "$id"
    fi
  fi
fi

for name in "gascan-mise-$id" "gascan-cache-$id" "gascan-config-$id"; do
  fresh_record=$(fresh_volume_record "$name") || exit 1
  if test "$fresh_record" != null; then
    container volume delete "$name"
  fi
done

fresh_record=$(fresh_network_record) || exit 1
if test "$fresh_record" != null; then
  container network delete "$network_name"
fi

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

container_inventory=$(container list --all --format json) || { printf 'unable to verify container cleanup; retaining cleanup manifest\n' >&2; exit 1; }
printf '%s' "$container_inventory" | jq -e 'type == "array" and all(.[]; type == "object" and ((.configuration.id | type) == "string"))' >/dev/null || { printf 'invalid container cleanup inventory; retaining cleanup manifest\n' >&2; exit 1; }
volume_inventory=$(container volume list --format json) || { printf 'unable to verify volume cleanup; retaining cleanup manifest\n' >&2; exit 1; }
printf '%s' "$volume_inventory" | jq -e 'type == "array" and all(.[]; type == "object" and ((.id | type) == "string") and ((.configuration.name | type) == "string") and (.id == .configuration.name))' >/dev/null || { printf 'invalid volume cleanup inventory; retaining cleanup manifest\n' >&2; exit 1; }
network_inventory=$(container network list --format json) || { printf 'unable to verify network cleanup; retaining cleanup manifest\n' >&2; exit 1; }
printf '%s' "$network_inventory" | jq -e 'type == "array" and all(.[]; type == "object" and ((.id | type) == "string") and ((.configuration.name | type) == "string") and (.id == .configuration.name))' >/dev/null || { printf 'invalid network cleanup inventory; retaining cleanup manifest\n' >&2; exit 1; }
printf '%s' "$container_inventory" | jq -e --arg id "$id" 'any(.[]; .configuration.id == $id)' >/dev/null && residue=true || true
for name in "gascan-mise-$id" "gascan-cache-$id" "gascan-config-$id"; do
  printf '%s' "$volume_inventory" | jq -e --arg id "$name" 'any(.[]; .configuration.name == $id)' >/dev/null && residue=true || true
done
printf '%s' "$network_inventory" | jq -e --arg id "$network_name" 'any(.[]; .id == $id and .configuration.name == $id)' >/dev/null && residue=true || true
if test "$residue" = true; then
  printf 'Gate 4 cleanup residue remains for exact sandbox %s\n' "$id" >&2
  exit 1
fi
if test "$abort_reached" = true; then
  rm -f "$abort_evidence"
  test ! -e "$abort_evidence" && test ! -L "$abort_evidence" || { printf 'abort evidence removal failed\n' >&2; exit 1; }
fi
rm -rf -- "$runtime_root" "$project_root"
if { test -e "$session_root" || test -L "$session_root"; } && ! rmdir "$session_root" 2>/dev/null; then
  printf 'Gate 4 cleanup session residue remains at %s\n' "$session_root" >&2
  exit 1
fi
rm -f "$manifest"
if test "$abort_reached" = true; then
  printf 'Gate 4 abort recovery reconciled exact recorded resources\n'
fi
