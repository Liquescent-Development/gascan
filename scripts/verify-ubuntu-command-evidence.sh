#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

die() { printf 'ubuntu command evidence: %s\n' "$*" >&2; exit 1; }

[[ $# == 1 ]] || die "usage: $0 EVIDENCE_DIRECTORY"
evidence=$1
manifest="$evidence/package-manifest.tsv"
expected="$evidence/command-providers.tsv"
[[ -f $manifest && -f $expected ]] || die "required evidence is missing"
dpkg_query=${DPKG_QUERY:-dpkg-query}
readlink_command=${READLINK:-readlink}

while IFS=$'\t' read -r package version architecture _; do
  [[ -n $package && -n $version && -n $architecture ]] || die "invalid package manifest"
  installed=$("$dpkg_query" -W -f='${Version}\t${Architecture}' "$package") ||
    die "manifest package is not installed: $package"
  [[ $installed == "$version"$'\t'"$architecture" ]] ||
    die "installed package identity differs from manifest: $package"
done <"$manifest"

actual=$(mktemp)
trap 'rm -f -- "$actual"' EXIT
while IFS=$'\t' read -r command package; do
  path=$(command -v "$command") || die "required command is missing: $command"
  [[ $path == /* && -x $path ]] || die "required command is not an absolute executable: $command"
  resolved=$("$readlink_command" -f -- "$path") || die "cannot resolve command path: $command"
  [[ $resolved == /* && -x $resolved ]] || die "resolved command is not executable: $command"
  owned=false
  candidates=("$resolved")
  case $resolved in
    /usr/bin/*)
      [[ $("$readlink_command" -f -- /usr/bin) == "$("$readlink_command" -f -- /bin)" ]] &&
        candidates+=("/bin/${resolved#/usr/bin/}")
      ;;
    /usr/sbin/*)
      [[ $("$readlink_command" -f -- /usr/sbin) == "$("$readlink_command" -f -- /sbin)" ]] &&
        candidates+=("/sbin/${resolved#/usr/sbin/}")
      ;;
  esac
  for candidate in "${candidates[@]}"; do
    ownership=$("$dpkg_query" -S "$candidate" 2>/dev/null) || continue
    while IFS= read -r record; do
      case $record in
        "$package: $candidate"|"$package:arm64: $candidate"|"$package:all: $candidate")
          owned=true
          ;;
      esac
    done <<<"$ownership"
  done
  [[ $owned == true ]] || die "wrong command provider: $command"
  if [[ $command == pico ]]; then
    [[ -L $path ]] || die "pico alternative is missing"
    nano=$(command -v nano) || die "nano command is missing"
    [[ $resolved == "$("$readlink_command" -f -- "$nano")" ]] || die "pico alternative does not resolve to nano"
  fi
  printf '%s\t%s\t%s\n' "$command" "$package" "$path"
done <<'EOF' | LC_ALL=C sort -u >"$actual"
dig	bind9-dnsutils
ifconfig	net-tools
ip	iproute2
nano	nano
netstat	net-tools
nslookup	bind9-dnsutils
pico	nano
ping	iputils-ping
ps	procps
pstree	psmisc
ss	iproute2
top	procps
EOF

cmp --silent "$actual" "$expected" || die "command provider evidence differs from runtime recomputation"
