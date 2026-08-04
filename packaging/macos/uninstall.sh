#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
source "$repo_root/packaging/macos/release-common.sh"

gascan_uninstall_refuse_path() {
  printf 'refusing unsafe uninstall path: %s\n' "$1" >&2
  return 65
}

gascan_uninstall_validate_absolute_base() {
  local path=$1
  [[ $path == /* && $path != / && $path != */ && $path != *//* && \
    $path != */../* && $path != */.. && $path != */./* && $path != */. ]] ||
    { gascan_uninstall_refuse_path "$path"; return 65; }
}

gascan_uninstall_validate_owned_directory() {
  local path=$1 expected_uid=$2 private=$3 owner mode mode_value
  [[ -e $path || -L $path ]] || return 2
  [[ -d $path && ! -L $path ]] ||
    { gascan_uninstall_refuse_path "$path"; return 65; }
  owner=$(stat -f '%u' "$path") ||
    { gascan_uninstall_refuse_path "$path"; return 65; }
  mode=$(stat -f '%Lp' "$path") ||
    { gascan_uninstall_refuse_path "$path"; return 65; }
  [[ $owner == "$expected_uid" && $mode =~ ^[0-7]{3,4}$ ]] ||
    { gascan_uninstall_refuse_path "$path"; return 65; }
  mode_value=$((8#$mode))
  if [[ $private == true ]]; then
    ((mode_value == 0700)) ||
      { gascan_uninstall_refuse_path "$path"; return 65; }
  else
    (( (mode_value & 07022) == 0 )) ||
      { gascan_uninstall_refuse_path "$path"; return 65; }
  fi
}

gascan_uninstall_validate_system_directory() {
  local path=$1 expected_mode=$2 owner mode
  [[ -d $path && ! -L $path ]] ||
    { gascan_uninstall_refuse_path "$path"; return 65; }
  owner=$(stat -f '%u' "$path") ||
    { gascan_uninstall_refuse_path "$path"; return 65; }
  mode=$(stat -f '%Lp' "$path") ||
    { gascan_uninstall_refuse_path "$path"; return 65; }
  [[ $owner == 0 && $mode == "$expected_mode" ]] ||
    { gascan_uninstall_refuse_path "$path"; return 65; }
}

gascan_uninstall_remove_bound_private_child() {
  local parent=$1 child=$2 expected_uid=$3 parent_private=$4 parent_identity
  parent_identity=$(stat -f '%d:%i' "$parent") ||
    { gascan_uninstall_refuse_path "$parent"; return 65; }
  (
    builtin cd -P -- "$parent" ||
      { gascan_uninstall_refuse_path "$parent"; exit 65; }
    [[ $(stat -f '%d:%i' .) == "$parent_identity" ]] ||
      { gascan_uninstall_refuse_path "$parent"; exit 65; }
    gascan_uninstall_validate_owned_directory \
      . "$expected_uid" "$parent_private" || exit $?
    gascan_uninstall_validate_owned_directory \
      "./$child" "$expected_uid" true || exit $?
    /bin/rm -rf -- "./$child" || exit $?
  )
}

gascan_uninstall_remove_controller_data() {
  local expected_uid=$1 component current controller_parent private status
  local -a components=(Library 'Application Support' dev.gascan controller)
  gascan_uninstall_validate_absolute_base "$HOME" || return $?
  gascan_uninstall_validate_owned_directory "$HOME" "$expected_uid" false || {
    status=$?
    if [[ $status == 2 ]]; then
      gascan_uninstall_refuse_path "$HOME"
      return 65
    fi
    return "$status"
  }
  current=$HOME
  for component in "${components[@]}"; do
    current=$current/$component
    private=false
    [[ $component == dev.gascan || $component == controller ]] && private=true
    gascan_uninstall_validate_owned_directory \
      "$current" "$expected_uid" "$private" || {
      status=$?
      [[ $status == 2 ]] && return 0
      return "$status"
    }
  done
  controller_parent=${current%/controller}
  gascan_uninstall_remove_bound_private_child \
    "$controller_parent" controller "$expected_uid" true
}

gascan_uninstall_remove_runtime_data() {
  local expected_uid=$1 runtime_parent runtime_root status
  if [[ -n ${XDG_RUNTIME_DIR:-} ]]; then
    runtime_parent=$XDG_RUNTIME_DIR
    gascan_uninstall_validate_absolute_base "$runtime_parent" || return $?
    gascan_uninstall_validate_owned_directory \
      "$runtime_parent" "$expected_uid" true || {
      status=$?
      if [[ $status == 2 ]]; then
        gascan_uninstall_refuse_path "$runtime_parent"
        return 65
      fi
      return "$status"
    }
  else
    runtime_parent=/private/tmp
    gascan_uninstall_validate_system_directory /private 755 || return $?
    gascan_uninstall_validate_system_directory "$runtime_parent" 1777 || return $?
  fi
  runtime_root=$runtime_parent/gascan
  gascan_uninstall_validate_owned_directory \
    "$runtime_root" "$expected_uid" true || {
    status=$?
    [[ $status == 2 ]] && return 0
    return "$status"
  }
  gascan_uninstall_remove_bound_private_child \
    "$runtime_parent" gascan "$expected_uid" \
    "$([[ -n ${XDG_RUNTIME_DIR:-} ]] && printf true || printf false)"
}

remove_data=false
case ${1:-} in
  '') ;;
  --remove-data) remove_data=true ;;
  *) printf 'usage: %s [--remove-data]\n' "$0" >&2; exit 64 ;;
esac
[[ $# -le 1 ]] || { printf 'usage: %s [--remove-data]\n' "$0" >&2; exit 64; }

if [[ $remove_data == false ]]; then
  printf 'Preserving all sandboxes, volumes, caches, and user state.\n'
  printf 'Preserved durable controller state: %s/state.sqlite3\n' \
    "$(gascan_user_controller_root)"
  printf 'Reinstall Gas Can to recover these sandboxes and volumes.\n'
  printf 'To remove them explicitly, run ./packaging/macos/uninstall.sh --remove-data\n'
else
  command -v gascan >/dev/null || {
    printf 'gascan is required to remove owned data safely\n' >&2
    exit 69
  }
  sandbox_json=$(gascan list --json)
  jq -e '
    type == "array" and
    all(.[]; type == "object" and (.sandbox_id | type == "string" and length > 0)) and
    ([.[].sandbox_id] | length == (unique | length))
  ' <<<"$sandbox_json" >/dev/null || {
    printf 'sandbox inventory is malformed or ambiguous\n' >&2
    exit 65
  }
  sandbox_ids=$(jq -r '.[].sandbox_id' <<<"$sandbox_json")
  while IFS= read -r sandbox_id; do
    [[ -n $sandbox_id ]] || continue
    gascan --sandbox "$sandbox_id" destroy --yes
  done <<<"$sandbox_ids"

  all_sandbox_json=$(gascan list --all --json)
  jq -e '
    type == "array" and
    all(.[];
      type == "object" and
      (.sandbox_id | type == "string" and length > 0) and
      .actual_state == "absent"
    ) and
    ([.[].sandbox_id] | length == (unique | length))
  ' <<<"$all_sandbox_json" >/dev/null || {
    printf 'owned sandbox inventory did not reach the destroyed state\n' >&2
    exit 65
  }
fi

gascan_stop_attested_daemon gascan /usr/local/bin/gascand
if [[ $remove_data == true ]]; then
  expected_uid=$(id -u)
  gascan_uninstall_remove_runtime_data "$expected_uid" || exit $?
  gascan_uninstall_remove_controller_data "$expected_uid" || exit $?
fi
sudo rm -f \
  /usr/local/bin/gascan \
  /usr/local/bin/gascand \
  /usr/local/bin/gascan-apple-attach \
  /usr/local/share/gascan/LICENSE \
  /usr/local/share/gascan/default-gascan.toml \
  /usr/local/share/gascan/build-manifest.json
sudo rmdir /usr/local/share/gascan 2>/dev/null || true
if pkgutil --pkg-info dev.gascan.pkg >/dev/null 2>&1; then
  sudo pkgutil --forget dev.gascan.pkg >/dev/null
fi
printf 'Gas Can binaries removed.\n'
