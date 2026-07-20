#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-installer-contract.XXXXXX")
daemon_pid=
cleanup() { if [[ -n $daemon_pid ]]; then /bin/kill "$daemon_pid" 2>/dev/null || true; fi; rm -rf "$fixture"; }
trap cleanup EXIT
mkdir -p "$fixture/bin"
touch "$fixture/test.pkg"
log=$fixture/log
: >"$log"
revision=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
hash=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb

write_fake() { local name=$1 body=$2; printf '#!/usr/bin/env bash\nset -euo pipefail\n%s\n' "$body" >"$fixture/bin/$name"; chmod 0755 "$fixture/bin/$name"; }
write_fake uname 'case "$1" in -s) echo Darwin;; -m) echo arm64;; *) exit 1;; esac'
write_fake sw_vers 'echo 26.5.1'
write_fake git 'if [[ $* == *verify-commit* ]]; then exit 0; fi; echo "$FIXTURE_REVISION"'
write_fake cargo 'printf "%s\n" "{\"packages\":[{\"name\":\"gascan\",\"version\":\"0.1.0\"}]}"'
write_fake container '
printf "container:%s\\n" "$*" >>"$FIXTURE_LOG"
case "$*" in
  "system version --format json") printf "%s\\n" "${FIXTURE_VERSION_JSON}";;
  "system status --format json") printf "%s\\n" "${FIXTURE_STATUS_JSON}";;
  *) exit 64;;
esac'
write_fake pkgutil '
case "$1" in
  --expand) mkdir -p "$3"; : >"$3/Payload"; [[ ${FIXTURE_SCRIPTS:-0} == 0 ]] || mkdir "$3/Scripts"; printf "<pkg-info identifier=\\\"%s\\\" version=\\\"%s\\\" install-location=\\\"/\\\"/>\\n" "${FIXTURE_PACKAGE_ID}" "${FIXTURE_VERSION}" >"$3/PackageInfo";;
  --payload-files) printf "%s\\n" . ./usr ./usr/local ./usr/local/bin ./usr/local/bin/gascan ./usr/local/bin/gascan-apple-attach ./usr/local/bin/gascand ./usr/local/share ./usr/local/share/gascan ./usr/local/share/gascan/LICENSE ./usr/local/share/gascan/build-manifest.json ./usr/local/share/gascan/default-gascan.toml; [[ ${FIXTURE_EXTRA_PAYLOAD:-0} == 0 ]] || echo ./evil;;
  --pkg-info) exit 1;;
  *) exit 64;;
esac'
write_fake gzip 'exit 0'
write_fake cpio '
mkdir -p usr/local/bin usr/local/share/gascan
: >usr/local/bin/gascan; : >usr/local/bin/gascan-apple-attach; : >usr/local/bin/gascand
printf license >usr/local/share/gascan/LICENSE; printf config >usr/local/share/gascan/default-gascan.toml
printf "%s\\n" "{\"architecture\":\"arm64\",\"files\":[{\"path\":\"usr/local/bin/gascan\",\"sha256\":\"$FIXTURE_MANIFEST_HASH\"},{\"path\":\"usr/local/bin/gascan-apple-attach\",\"sha256\":\"$FIXTURE_MANIFEST_HASH\"},{\"path\":\"usr/local/bin/gascand\",\"sha256\":\"$FIXTURE_MANIFEST_HASH\"}],\"product\":\"Gas Can\",\"schema\":1,\"source_revision\":\"$FIXTURE_REVISION\",\"version\":\"0.1.0\"}" >usr/local/share/gascan/build-manifest.json'
write_fake shasum 'printf "%s  %s\\n" "$FIXTURE_OBSERVED_HASH" "$3"'
write_fake lipo 'echo "$FIXTURE_ARCHS"'
write_fake sudo 'printf "sudo:%s\\n" "$*" >>"$FIXTURE_LOG"'
write_fake realpath 'printf "%s\\n" "$1"'
write_fake ps '
pid=$2; /bin/kill -0 "$pid" 2>/dev/null || exit 1
case "$4" in command=) echo "$FIXTURE_OBSERVED_EXECUTABLE";; lstart=) echo " $FIXTURE_OBSERVED_START ";; *) exit 64;; esac'
write_fake gascan '
printf "gascan:%s\\n" "$*" >>"$FIXTURE_LOG"
if [[ $1 == daemon-attest ]]; then [[ $FIXTURE_DAEMON_PID != 999999 ]] || exit 1; printf "{\\\"pid\\\":%s,\\\"executable\\\":\\\"%s\\\",\\\"start_identity\\\":\\\"%s\\\",\\\"instance_token\\\":\\\"%s\\\"}\\n" "$FIXTURE_DAEMON_PID" "$FIXTURE_ATTESTED_EXECUTABLE" "$FIXTURE_ATTESTED_START" "$FIXTURE_ATTESTED_TOKEN";
elif [[ $1 == list ]]; then printf "%s\\n" "$FIXTURE_SANDBOX_JSON"; fi'

export PATH="$fixture/bin:/usr/bin:/bin:/usr/sbin:/sbin" FIXTURE_LOG=$log FIXTURE_REVISION=$revision FIXTURE_HASH=$hash
export GASCAN_EXPECTED_SOURCE_REVISION=$revision GASCAN_EXPECTED_VERSION=0.1.0
export FIXTURE_PACKAGE_ID=dev.gascan.pkg FIXTURE_VERSION=0.1.0
export FIXTURE_MANIFEST_HASH=$hash FIXTURE_OBSERVED_HASH=$hash FIXTURE_ARCHS=arm64
export FIXTURE_OBSERVED_EXECUTABLE=/usr/local/bin/gascand FIXTURE_OBSERVED_START=START
export FIXTURE_ATTESTED_EXECUTABLE=/usr/local/bin/gascand FIXTURE_ATTESTED_START=START FIXTURE_ATTESTED_TOKEN=TOKEN
export FIXTURE_VERSION_JSON='[{"appName":"container","buildType":"release","commit":"5973b9cc626a3e7a499bb316a958237ebe14e2ed","version":"1.1.0"},{"appName":"container-apiserver","buildType":"release","commit":"5973b9cc626a3e7a499bb316a958237ebe14e2ed","version":"container-apiserver version 1.1.0 (build: release, commit: 5973b9c)"}]'
export FIXTURE_STATUS_JSON='{"apiServerAppName":"container-apiserver","apiServerBuild":"release","apiServerCommit":"5973b9cc626a3e7a499bb316a958237ebe14e2ed","apiServerVersion":"container-apiserver version 1.1.0 (build: release, commit: 5973b9c)","status":"running"}'

FIXTURE_EXTRA_PAYLOAD=1 "$repo_root/packaging/macos/install.sh" "$fixture/test.pkg" >/dev/null 2>&1 && { echo 'extra payload accepted' >&2; exit 1; }
test ! -s "$log"
for condition in package-id package-version scripts checksum architecture; do
  export FIXTURE_PACKAGE_ID=dev.gascan.pkg FIXTURE_VERSION=0.1.0 FIXTURE_SCRIPTS=0 FIXTURE_MANIFEST_HASH=$hash FIXTURE_ARCHS=arm64
  case $condition in
    package-id) export FIXTURE_PACKAGE_ID=dev.foreign.pkg;;
    package-version) export FIXTURE_VERSION=9.9.9;;
    scripts) export FIXTURE_SCRIPTS=1;;
    checksum) export FIXTURE_MANIFEST_HASH=cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc;;
    architecture) export FIXTURE_ARCHS='x86_64 arm64';;
  esac
  "$repo_root/packaging/macos/install.sh" "$fixture/test.pkg" >/dev/null 2>&1 && { echo "$condition accepted" >&2; exit 1; }
  ! grep -q '^sudo:' "$log"
done
export FIXTURE_PACKAGE_ID=dev.gascan.pkg FIXTURE_VERSION=0.1.0 FIXTURE_SCRIPTS=0 FIXTURE_MANIFEST_HASH=$hash FIXTURE_ARCHS=arm64
good_version=$FIXTURE_VERSION_JSON; good_status=$FIXTURE_STATUS_JSON
for condition in stopped-service wrong-commit duplicate-client malformed-version; do
  export FIXTURE_VERSION_JSON=$good_version FIXTURE_STATUS_JSON=$good_status
  case $condition in
    stopped-service) export FIXTURE_STATUS_JSON=${good_status/running/stopped};;
    wrong-commit) export FIXTURE_VERSION_JSON=${good_version/5973b9cc626a3e7a499bb316a958237ebe14e2ed/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa};;
    duplicate-client) FIXTURE_VERSION_JSON=$(jq -c '.[0] as $client | . + [$client]' <<<"$good_version"); export FIXTURE_VERSION_JSON;;
    malformed-version) export FIXTURE_VERSION_JSON='{}';;
  esac
  "$repo_root/packaging/macos/install.sh" "$fixture/test.pkg" >/dev/null 2>&1 && { echo "$condition accepted" >&2; exit 1; }
  ! grep -q '^sudo:' "$log"
done
export FIXTURE_VERSION_JSON=$good_version FIXTURE_STATUS_JSON=$good_status
"$repo_root/packaging/macos/install.sh" "$fixture/test.pkg" >/dev/null
grep -qx "sudo:installer -pkg $fixture/test.pkg -target /" "$log"

: >"$log"; sleep 1000 & daemon_pid=$!; export FIXTURE_DAEMON_PID=$daemon_pid FIXTURE_SANDBOX_JSON='[]'
for condition in attested-start observed-start executable empty-token; do
  export FIXTURE_ATTESTED_START=START FIXTURE_OBSERVED_START=START FIXTURE_ATTESTED_EXECUTABLE=/usr/local/bin/gascand FIXTURE_ATTESTED_TOKEN=TOKEN
  case $condition in
    attested-start) export FIXTURE_ATTESTED_START=REUSED;;
    observed-start) export FIXTURE_OBSERVED_START=REUSED;;
    executable) export FIXTURE_ATTESTED_EXECUTABLE=/tmp/foreign;;
    empty-token) export FIXTURE_ATTESTED_TOKEN='';;
  esac
  "$repo_root/packaging/macos/uninstall.sh" >/dev/null 2>&1 && { echo "$condition mismatch accepted" >&2; exit 1; }
  /bin/kill -0 "$daemon_pid"
  ! grep -q '^sudo:' "$log"
done
export FIXTURE_ATTESTED_START=START FIXTURE_OBSERVED_START=START FIXTURE_ATTESTED_EXECUTABLE=/usr/local/bin/gascand FIXTURE_ATTESTED_TOKEN=TOKEN
"$repo_root/packaging/macos/uninstall.sh" --remove-data >/dev/null
! /bin/kill -0 "$daemon_pid" 2>/dev/null; daemon_pid=
grep -qx 'gascan:list --json' "$log"

: >"$log"; export FIXTURE_DAEMON_PID=999999 FIXTURE_SANDBOX_JSON='[{"sandbox_id":"one"},{"sandbox_id":"two"}]'
"$repo_root/packaging/macos/uninstall.sh" --remove-data >/dev/null
grep -qx 'gascan:--sandbox one destroy --yes' "$log"
grep -qx 'gascan:--sandbox two destroy --yes' "$log"
for invalid in '[{"sandbox_id":"same"},{"sandbox_id":"same"}]' '[{"sandbox_id":""}]' '{}'; do
  export FIXTURE_SANDBOX_JSON=$invalid
  "$repo_root/packaging/macos/uninstall.sh" --remove-data >/dev/null 2>&1 && { echo 'invalid sandbox inventory accepted' >&2; exit 1; }
done

printf 'PASS: Gas Can installer contract\n'
