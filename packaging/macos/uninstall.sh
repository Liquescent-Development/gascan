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

gascan_uninstall_validate_directory_entry() {
  local entry=$1 display_path=$2 expected_uid=$3 require_user=$4 private=$5
  local owner mode special mode_value
  [[ -e $entry || -L $entry ]] || return 2
  [[ -d $entry && ! -L $entry ]] ||
    { gascan_uninstall_refuse_path "$display_path"; return 65; }
  owner=$(stat -f '%u' "$entry") ||
    { gascan_uninstall_refuse_path "$display_path"; return 65; }
  mode=$(stat -f '%Lp' "$entry") ||
    { gascan_uninstall_refuse_path "$display_path"; return 65; }
  special=$(stat -f '%Mp' "$entry") ||
    { gascan_uninstall_refuse_path "$display_path"; return 65; }
  [[ $mode =~ ^[0-7]{3}$ && $special =~ ^[0-7]+$ ]] ||
    { gascan_uninstall_refuse_path "$display_path"; return 65; }
  if [[ $require_user == true ]]; then
    [[ $owner == "$expected_uid" ]] ||
      { gascan_uninstall_refuse_path "$display_path"; return 65; }
  else
    [[ $owner == 0 || $owner == "$expected_uid" ]] ||
      { gascan_uninstall_refuse_path "$display_path"; return 65; }
  fi
  mode_value=$((8#$mode))
  if [[ $owner == 0 && $display_path == /private/tmp ]]; then
    [[ $special == 1 ]] && ((mode_value == 0777)) ||
      { gascan_uninstall_refuse_path "$display_path"; return 65; }
  elif [[ $private == true ]]; then
    [[ $owner == "$expected_uid" && $special == 0 ]] &&
      ((mode_value == 0700)) ||
      { gascan_uninstall_refuse_path "$display_path"; return 65; }
  else
    [[ $special == 0 ]] && (( (mode_value & 0022) == 0 )) ||
      { gascan_uninstall_refuse_path "$display_path"; return 65; }
  fi
}

gascan_uninstall_remove_absolute_private_child() {
  local parent=$1 child=$2 expected_uid=$3 user_base=$4 parent_private=$5
  local component current entry identity require_user private status
  local -a components
  gascan_uninstall_validate_absolute_base "$parent" || return $?
  [[ $child != */* && $child != . && $child != .. && -n $child ]] || {
    gascan_uninstall_refuse_path "$parent/$child"
    return 65
  }
  if [[ -n $user_base ]]; then
    gascan_uninstall_validate_absolute_base "$user_base" || return $?
    [[ $parent == "$user_base" || $parent == "$user_base/"* ]] || {
      gascan_uninstall_refuse_path "$parent"
      return 65
    }
  fi

  IFS=/ read -r -a components <<<"${parent#/}"
  (
    builtin cd -P / || exit 65
    gascan_uninstall_validate_directory_entry . / "$expected_uid" false false || exit $?
    current=
    for component in "${components[@]}"; do
      [[ -n $component && $component != . && $component != .. ]] || {
        gascan_uninstall_refuse_path "$parent"
        exit 65
      }
      current=$current/$component
      entry=./$component
      require_user=false
      if [[ -n $user_base && \
        ($current == "$user_base" || $current == "$user_base/"*) ]]; then
        require_user=true
      fi
      private=false
      [[ $current == "$parent" && $parent_private == true ]] && private=true
      gascan_uninstall_validate_directory_entry \
        "$entry" "$current" "$expected_uid" "$require_user" "$private" || {
        status=$?
        if [[ $status == 2 && -n $user_base && \
          ($user_base == "$current" || $user_base == "$current/"*) ]]; then
          gascan_uninstall_refuse_path "$current"
          exit 65
        fi
        [[ $status == 2 && -z $user_base ]] && {
          gascan_uninstall_refuse_path "$current"
          exit 65
        }
        exit "$status"
      }
      identity=$(stat -f '%d:%i' "$entry") || {
        gascan_uninstall_refuse_path "$current"
        exit 65
      }
      builtin cd -P -- "$entry" || {
        gascan_uninstall_refuse_path "$current"
        exit 65
      }
      [[ $(stat -f '%d:%i' .) == "$identity" ]] || {
        gascan_uninstall_refuse_path "$current"
        exit 65
      }
    done
    gascan_uninstall_validate_directory_entry \
      "./$child" "$parent/$child" "$expected_uid" true true || exit $?
    /bin/rm -rf -- "./$child" || exit $?
  )
}

# Which controller store the CLI that just answered was actually reading.
#
# Taken from the daemon's own instance record rather than re-derived from the
# environment: backend selection lives in Rust (`gascan_core::backend`) so that
# the client and the daemon cannot disagree about it, and a second copy of the
# rule here would be a third answer nobody reconciles.
gascan_uninstall_enumerated_store() {
  local runtime_root instance backend controller_root
  if [[ -n ${XDG_RUNTIME_DIR:-} ]]; then
    runtime_root=$XDG_RUNTIME_DIR/gascan
  else
    runtime_root=$(gascan_user_runtime_root)
  fi
  instance=$runtime_root/daemon-instance.json
  [[ -f $instance ]] || return 1
  backend=$(jq -er '.backend | select(type == "string" and length > 0)' "$instance") || return 1
  controller_root=$(gascan_user_controller_root)
  # The unscoped path is Apple's, historically and by default; every other
  # backend keeps its store under a child named for it.
  if [[ $backend == apple ]]; then
    printf '%s/state.sqlite3\n' "$controller_root"
  else
    printf '%s/%s/state.sqlite3\n' "$controller_root" "$backend"
  fi
}

# **Refuse to delete a store whose sandboxes were never destroyed.**
#
# `--remove-data` is a two-stage contract: destroy every owned sandbox through
# `gascan`, then delete the controller directory. Since the store was scoped by
# backend, stage one reads ONE backend's records while stage two's `rm -rf` of
# `controller/` takes every backend's with it. A user who had run work under
# `GASCAN_ARCA_BACKEND` and then uninstalled from a plain shell would have had
# their Apple records destroyed properly and their Arca records deleted while
# the engine kept those sandboxes running, unreferenced -- which is the exact
# harm the scoping exists to prevent, reintroduced at uninstall time and made
# deterministic rather than accidental.
#
# Refusing rather than destroying blind: this script cannot select another
# backend without owning a second copy of the selection rule, and destroying
# through the wrong one is worse than stopping.
gascan_uninstall_assert_every_store_enumerated() {
  local enumerated controller_root store stores=() unenumerated=()
  controller_root=$(gascan_user_controller_root)
  for store in "$controller_root"/state.sqlite3 "$controller_root"/*/state.sqlite3; do
    [[ -e $store ]] || continue
    stores+=("$store")
  done
  # Nothing to lose, or one store which is necessarily the one the `gascan list`
  # above enumerated. A fresh install that never started a daemon has no
  # instance record to read, and refusing to uninstall it would be absurd.
  [[ ${#stores[@]} -le 1 ]] && return 0
  enumerated=$(gascan_uninstall_enumerated_store) || {
    printf 'refusing to remove data: more than one controller store exists and the running\n' >&2
    printf 'daemon did not record which backend it serves\n' >&2
    printf '  %s\n' "${stores[@]}" >&2
    return 65
  }
  for store in "${stores[@]}"; do
    [[ $store == "$enumerated" ]] && continue
    unenumerated+=("$store")
  done
  [[ ${#unenumerated[@]} -eq 0 ]] && return 0
  printf 'refusing to remove data: these controller stores hold sandboxes this run did not destroy\n' >&2
  printf '  %s\n' "${unenumerated[@]}" >&2
  printf 'Destroy them first by re-running with that backend selected, then run --remove-data again.\n' >&2
  return 65
}

gascan_uninstall_remove_controller_data() {
  local expected_uid=$1 controller_root controller_parent status
  controller_root=$(gascan_user_controller_root)
  controller_parent=${controller_root%/controller}
  gascan_uninstall_remove_absolute_private_child \
    "$controller_parent" controller "$expected_uid" "$HOME" true || {
    status=$?
    [[ $status == 2 ]] && return 0
    return "$status"
  }
}

# The engine's fetched boot artifacts. Removed under --remove-data with the
# other per-user state, and through the same guarded helper the controller
# directory uses: it refuses a path that is not an absolute private child of
# $HOME owned by this uid, which is what keeps an `rm -rf` of ~83MB from ever
# being pointed somewhere else by a hostile HOME.
#
# The cask's `uninstall delete:` list is deliberately NOT extended. It does not
# remove per-user state today, and making the artifacts the one exception would
# mean a `brew uninstall` silently deleted a download the user would have to
# repeat.
gascan_uninstall_remove_engine_data() {
  local expected_uid=$1 engine_root engine_parent status
  engine_root=$(gascan_user_engine_root)
  engine_parent=${engine_root%/engine}
  gascan_uninstall_remove_absolute_private_child \
    "$engine_parent" engine "$expected_uid" "$HOME" true || {
    status=$?
    [[ $status == 2 ]] && return 0
    return "$status"
  }
}

gascan_uninstall_remove_runtime_data() {
  local expected_uid=$1 runtime_parent runtime_root runtime_child user_base status
  if [[ -n ${XDG_RUNTIME_DIR:-} ]]; then
    runtime_parent=$XDG_RUNTIME_DIR
    runtime_root=$runtime_parent/gascan
    runtime_child=gascan
    user_base=$runtime_parent
  else
    runtime_root=$(gascan_user_runtime_root)
    runtime_parent=${runtime_root%/*}
    runtime_child=${runtime_root##*/}
    user_base=
    [[ $runtime_parent == /private/tmp && $runtime_child == "gascan-$expected_uid" ]] || {
      gascan_uninstall_refuse_path "$runtime_root"
      return 65
    }
  fi
  gascan_uninstall_remove_absolute_private_child \
    "$runtime_parent" "$runtime_child" "$expected_uid" "$user_base" \
    "$([[ -n $user_base ]] && printf true || printf false)" || {
    status=$?
    [[ $status == 2 ]] && return 0
    return "$status"
  }
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
  gascan_preserved_controller_root=$(gascan_user_controller_root)
  printf 'Preserved durable controller state: %s/state.sqlite3\n' \
    "$gascan_preserved_controller_root"
  # One store per backend since the controller store was scoped: the unscoped
  # database above is the Apple backend's, and every other backend keeps its own
  # under a child named for it. Enumerated rather than described with a
  # placeholder, so a user backing these up before reinstalling is told the
  # paths that exist rather than the shape they take.
  for gascan_preserved_scoped in "$gascan_preserved_controller_root"/*/state.sqlite3; do
    [[ -e $gascan_preserved_scoped ]] || continue
    printf 'Preserved durable controller state: %s\n' "$gascan_preserved_scoped"
  done
  printf 'Reinstall Gas Can to recover these sandboxes and volumes.\n'
  printf 'To remove them explicitly, run ./packaging/macos/uninstall.sh --remove-data\n'
else
  command -v gascan >/dev/null || {
    printf 'gascan is required to remove owned data safely\n' >&2
    exit 69
  }
  # Before anything is destroyed, so a refusal costs nothing.
  gascan list --json >/dev/null || {
    printf 'gascan could not report its sandbox inventory\n' >&2
    exit 65
  }
  gascan_uninstall_assert_every_store_enumerated || exit $?
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
  gascan_uninstall_remove_engine_data "$expected_uid" || exit $?
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
