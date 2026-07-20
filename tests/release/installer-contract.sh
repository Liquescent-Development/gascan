#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-installer-contract.XXXXXX")
daemon_pid=
cleanup() {
  if [[ -n $daemon_pid ]]; then kill "$daemon_pid" 2>/dev/null || true; fi
  rm -rf "$fixture"
}
trap cleanup EXIT
mkdir -p "$fixture/bin"
touch "$fixture/test.pkg"
log="$fixture/log"

write_fake() {
  local name=$1 body=$2
  printf '#!/usr/bin/env bash\nset -euo pipefail\n%s\n' "$body" >"$fixture/bin/$name"
  chmod 0755 "$fixture/bin/$name"
}

write_fake uname 'case "$1" in -s) echo Darwin;; -m) echo arm64;; *) exit 1;; esac'
write_fake sw_vers 'echo 26.5.1'
write_fake container 'exit 0'
write_fake pkgutil '
case "$1" in
  --payload-files)
    printf "%s\\n" ./usr/local/bin/gascan ./usr/local/bin/gascand ./usr/local/bin/gascan-apple-attach ./usr/local/share/gascan/LICENSE ./usr/local/share/gascan/default-gascan.toml ./usr/local/share/gascan/build-manifest.json
    ;;
  --expand)
    mkdir -p "$3"
    printf "<pkg-info identifier=\\\"%s\\\" version=\\\"0.1.0\\\"/>\\n" "${FIXTURE_PACKAGE_ID:-dev.foreign.pkg}" >"$3/PackageInfo"
    ;;
  --pkg-info) exit 1;;
  *) exit 1;;
esac'
write_fake sudo 'printf "sudo:%s\\n" "$*" >>"$FIXTURE_LOG"'
write_fake gascan '
printf "gascan:%s\\n" "$*" >>"$FIXTURE_LOG"
if [[ $1 == daemon-attest ]]; then
  printf "{\\\"pid\\\":%s,\\\"executable\\\":\\\"/usr/local/bin/gascand\\\"}\\n" "$FIXTURE_DAEMON_PID"
elif [[ $1 == list ]]; then
  printf "[{\\\"sandbox_id\\\":\\\"owned-one\\\"}]\\n"
fi'

export PATH="$fixture/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export FIXTURE_LOG="$log"

if "$repo_root/packaging/macos/install.sh" "$fixture/test.pkg" 2>"$fixture/error"; then
  printf 'installer accepted a package with the wrong identifier\n' >&2
  exit 1
fi
grep -q 'unexpected package identifier' "$fixture/error"
test ! -e "$log"

FIXTURE_PACKAGE_ID=dev.gascan.pkg "$repo_root/packaging/macos/install.sh" "$fixture/test.pkg"
grep -qx "sudo:installer -pkg $fixture/test.pkg -target /" "$log"

: >"$log"
sleep 1000 &
daemon_pid=$!
export FIXTURE_DAEMON_PID=$daemon_pid
"$repo_root/packaging/macos/uninstall.sh"
grep -q '^sudo:rm -f /usr/local/bin/gascan ' "$log"
grep -qx 'gascan:daemon-attest' "$log"
if kill -0 "$daemon_pid" 2>/dev/null; then
  printf 'default uninstall left the exact on-demand daemon running\n' >&2
  exit 1
fi
daemon_pid=
unset FIXTURE_DAEMON_PID
if grep -Eq '^gascan:(list|--sandbox)' "$log"; then
  printf 'default uninstall accessed sandbox data\n' >&2
  exit 1
fi

: >"$log"
"$repo_root/packaging/macos/uninstall.sh" --remove-data
grep -qx 'gascan:list --json' "$log"
grep -qx 'gascan:--sandbox owned-one destroy --yes' "$log"

printf 'PASS: Gas Can installer contract\n'
