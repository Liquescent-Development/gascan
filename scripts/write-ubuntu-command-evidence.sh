#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

die() { printf 'ubuntu command writer: %s\n' "$*" >&2; exit 1; }

[[ $# == 1 ]] || die "usage: $0 EVIDENCE_DIRECTORY"
evidence=$1
manifest="$evidence/package-manifest.tsv"
destination="$evidence/command-providers.tsv"
[[ -f $manifest ]] || die "package manifest is missing"
dpkg_query=${DPKG_QUERY:-dpkg-query}
readlink_command=${READLINK:-readlink}
temporary="$destination.tmp"
trap 'rm -f -- "$temporary"' EXIT

while IFS=$'\t' read -r command package; do
  identity=$(awk -F $'\t' -v package="$package" '$1 == package {print $2 "\t" $3}' "$manifest")
  [[ -n $identity && $identity != *$'\n'* ]] || die "command provider is not uniquely selected: $package"
  installed=$("$dpkg_query" -W -f='${Version}\t${Architecture}' "$package") ||
    die "command provider is not installed: $package"
  [[ $installed == "$identity" ]] ||
    die "installed command provider differs from manifest: $package"
  path=$(command -v "$command") || die "required command is missing: $command"
  [[ $path == /* && -x $path ]] || die "required command is not an absolute executable: $command"
  resolved=$("$readlink_command" -f -- "$path") || die "cannot resolve command path: $command"
  ownership_path=
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
    case $ownership in
      "$package: $candidate"|"$package:arm64: $candidate"|"$package:all: $candidate")
        ownership_path=$candidate
        break
        ;;
    esac
  done
  [[ -n $ownership_path ]] || die "command path has wrong or missing package owner: $command"
  if [[ $command == pico ]]; then
    [[ -L $path ]] || die "pico alternative is missing"
    nano=$(command -v nano) || die "nano command is missing"
    [[ $resolved == "$("$readlink_command" -f -- "$nano")" ]] ||
      die "pico alternative does not resolve to nano"
  fi
  printf '%s\t%s\t%s\n' "$command" "$package" "$path"
done <<'EOF' | LC_ALL=C sort -u >"$temporary"
dig	bind9-dnsutils
file	file
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

mv -- "$temporary" "$destination"
trap - EXIT
