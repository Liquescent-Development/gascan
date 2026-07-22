#!/usr/bin/env bash
# Resolve one required release configuration value.
#
# Nothing is defaulted. Signing identities and a keychain profile name are
# organization- and machine-specific, so a repository script that carries one
# operator's setup breaks silently for everyone else.
#
# Precedence, first match wins: flag, environment, config file.
#
# The config file is read as data. It is never sourced and never evaluated, so
# shell syntax inside it is preserved as a value rather than executed.

gascan_release_config_flag() {
  case $1 in
    GASCAN_CODESIGN_IDENTITY) printf -- '--codesign-identity';;
    GASCAN_INSTALLER_SIGNING_IDENTITY) printf -- '--installer-identity';;
    GASCAN_NOTARYTOOL_PROFILE) printf -- '--notary-profile';;
    GASCAN_TAP_PATH) printf -- '--tap';;
    *) return 1;;
  esac
}

# Value is everything after the first '=', preserved verbatim.
gascan_release_config_file_value() {
  local file=$1 key=$2 line
  [[ -r $file ]] || return 1
  while IFS= read -r line || [[ -n $line ]]; do
    [[ $line == "$key="* ]] || continue
    printf '%s' "${line#"$key="}"
    return 0
  done <"$file"
  return 1
}

gascan_release_config() {
  local name=$1 flag_value=$2 file=$3 value flag
  if [[ -n $flag_value ]]; then
    printf '%s' "$flag_value"
    return 0
  fi
  value=${!name-}
  if [[ -n $value ]]; then
    printf '%s' "$value"
    return 0
  fi
  if value=$(gascan_release_config_file_value "$file" "$name") && [[ -n $value ]]; then
    printf '%s' "$value"
    return 0
  fi
  flag=$(gascan_release_config_flag "$name") || flag='(no flag)'
  printf 'missing required release configuration: %s\n' "$name" >&2
  printf 'supply it with %s, the %s environment variable, or a %s= line in %s\n' \
    "$flag" "$name" "$name" "$file" >&2
  return 65
}
