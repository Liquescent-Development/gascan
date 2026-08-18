#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-installer-contract.XXXXXX")
fixture=$(cd "$fixture" && pwd -P)
daemon_pid=
default_runtime_fixture=
default_runtime_fixture_identity=
default_runtime_fixture_owner=
default_runtime_fixture_mode=
default_runtime_fixture_flags=

remove_captured_directory() {
  local path=$1 expected_identity=$2 expected_owner=$3 expected_mode=$4 expected_flags=$5
  local parent child identity owner mode flags

  [[ $path == /* && $path != / && $path != */../* && $path != */./* ]] || return 1
  parent=${path%/*}
  child=${path##*/}
  [[ -n $parent && -n $child && $child != . && $child != .. ]] || return 1
  [[ $expected_owner == "$(/usr/bin/id -u)" && $expected_mode == 700 && $expected_flags == 0 ]] || return 1

  (
    cd -P "$parent" || exit 1
    [[ ! -L ./$child && -d ./$child ]] || exit 1
    identity=$(/usr/bin/stat -f '%d:%i' "./$child") || exit 1
    owner=$(/usr/bin/stat -f '%u' "./$child") || exit 1
    mode=$(/usr/bin/stat -f '%Lp' "./$child") || exit 1
    flags=$(/usr/bin/stat -f '%Mp' "./$child") || exit 1
    [[ $identity == "$expected_identity" && \
      $owner == "$expected_owner" && \
      $mode == "$expected_mode" && \
      $flags == "$expected_flags" ]] || exit 1
    /bin/rm -rf -- "./$child"
  )
}

cleanup() {
  if [[ -n $daemon_pid ]]; then /bin/kill "$daemon_pid" 2>/dev/null || true; fi
  if [[ -n $default_runtime_fixture ]]; then
    remove_captured_directory \
      "$default_runtime_fixture" \
      "$default_runtime_fixture_identity" \
      "$default_runtime_fixture_owner" \
      "$default_runtime_fixture_mode" \
      "$default_runtime_fixture_flags" || true
  fi
  rm -rf "$fixture"
}
trap cleanup EXIT

cleanup_probe=$fixture/cleanup-identity-probe
mkdir -p "$cleanup_probe/exact"
chmod 0700 "$cleanup_probe/exact"
probe_identity=$(/usr/bin/stat -f '%d:%i' "$cleanup_probe/exact")
probe_owner=$(/usr/bin/stat -f '%u' "$cleanup_probe/exact")
probe_mode=$(/usr/bin/stat -f '%Lp' "$cleanup_probe/exact")
probe_flags=$(/usr/bin/stat -f '%Mp' "$cleanup_probe/exact")
(
  fixture=$cleanup_probe/unused-fixture
  default_runtime_fixture=$cleanup_probe/exact
  default_runtime_fixture_identity=$probe_identity
  default_runtime_fixture_owner=$probe_owner
  default_runtime_fixture_mode=$probe_mode
  default_runtime_fixture_flags=$probe_flags
  cleanup
)
[[ ! -e $cleanup_probe/exact && ! -L $cleanup_probe/exact ]] || {
  printf 'cleanup did not remove the exact directory identity created by the test\n' >&2
  exit 1
}

mkdir "$cleanup_probe/swapped"
chmod 0700 "$cleanup_probe/swapped"
probe_identity=$(/usr/bin/stat -f '%d:%i' "$cleanup_probe/swapped")
probe_owner=$(/usr/bin/stat -f '%u' "$cleanup_probe/swapped")
probe_mode=$(/usr/bin/stat -f '%Lp' "$cleanup_probe/swapped")
probe_flags=$(/usr/bin/stat -f '%Mp' "$cleanup_probe/swapped")
mv "$cleanup_probe/swapped" "$cleanup_probe/created"
mkdir "$cleanup_probe/swapped"
chmod 0700 "$cleanup_probe/swapped"
(
  fixture=$cleanup_probe/unused-fixture
  default_runtime_fixture=$cleanup_probe/swapped
  default_runtime_fixture_identity=$probe_identity
  default_runtime_fixture_owner=$probe_owner
  default_runtime_fixture_mode=$probe_mode
  default_runtime_fixture_flags=$probe_flags
  cleanup
)
replacement_preserved=0
[[ -d $cleanup_probe/swapped ]] && replacement_preserved=1
rm -rf "$cleanup_probe"
[[ $replacement_preserved == 1 ]] || {
  printf 'cleanup removed a replacement with a different directory identity\n' >&2
  exit 1
}

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
  --payload-files) printf "%s\\n" . ./._usr ./usr ./usr/._local ./usr/local ./usr/local/._bin ./usr/local/._share ./usr/local/bin ./usr/local/bin/._gascan ./usr/local/bin/._gascan-apple-attach ./usr/local/bin/._gascand ./usr/local/bin/gascan ./usr/local/bin/gascan-apple-attach ./usr/local/bin/gascand ./usr/local/share ./usr/local/share/._gascan ./usr/local/share/gascan ./usr/local/share/gascan/._LICENSE ./usr/local/share/gascan/._build-manifest.json ./usr/local/share/gascan/._default-gascan.toml ./usr/local/share/gascan/LICENSE ./usr/local/share/gascan/build-manifest.json ./usr/local/share/gascan/default-gascan.toml; [[ ${FIXTURE_EXTRA_PAYLOAD:-0} == 0 ]] || echo ./evil;;
  --pkg-info) exit 1;;
  *) exit 64;;
esac'
write_fake gzip 'exit 0'
write_fake xattr 'printf "%s\\n" com.apple.provenance'
write_fake cpio '
mkdir -p usr/local/bin usr/local/share/gascan
: >usr/local/bin/gascan; : >usr/local/bin/gascan-apple-attach; : >usr/local/bin/gascand
printf license >usr/local/share/gascan/LICENSE; printf config >usr/local/share/gascan/default-gascan.toml
printf "%s\\n" "{\"architecture\":\"arm64\",\"engine\":$FIXTURE_ENGINE_JSON,\"files\":[{\"path\":\"usr/local/bin/gascan\",\"sha256\":\"$FIXTURE_MANIFEST_HASH\"},{\"path\":\"usr/local/bin/gascan-apple-attach\",\"sha256\":\"$FIXTURE_MANIFEST_HASH\"},{\"path\":\"usr/local/bin/gascand\",\"sha256\":\"$FIXTURE_MANIFEST_HASH\"}],\"product\":\"Gas Can\",\"schema\":2,\"source_revision\":\"$FIXTURE_REVISION\",\"version\":\"0.1.0\"}" >usr/local/share/gascan/build-manifest.json'
write_fake shasum 'printf "%s  %s\\n" "$FIXTURE_OBSERVED_HASH" "$3"'
write_fake lipo 'echo "$FIXTURE_ARCHS"'
write_fake sudo 'printf "sudo:%s\\n" "$*" >>"$FIXTURE_LOG"'
write_fake realpath 'printf "%s\\n" "$1"'
write_fake id '
[[ $1 == -u ]]
printf "%s\\n" "${FIXTURE_ID_UID:-$(/usr/bin/id -u)}"'
write_fake stat '
[[ $1 == -f && $# == 3 ]]
format=$2
path=$3
case $path in
  .) absolute_path=$(/bin/pwd -P);;
  ./*) absolute_path=$(/bin/pwd -P)/${path#./};;
  *) absolute_path=$path;;
esac
case $format in
  %u)
    if [[ -n ${FIXTURE_STAT_OWNER_PREFIX:-} && \
      ($absolute_path == "$FIXTURE_STAT_OWNER_PREFIX" || \
       $absolute_path == "$FIXTURE_STAT_OWNER_PREFIX/"*) ]]; then
      printf "%s\\n" "$FIXTURE_STAT_OWNER_VALUE"
    elif [[ -n ${FIXTURE_STAT_OWNER_PATH:-} && $absolute_path == "$FIXTURE_STAT_OWNER_PATH" ]]; then
      printf "%s\\n" "$FIXTURE_STAT_OWNER_VALUE"
    elif [[ -n ${FIXTURE_FOREIGN_STAT_PATH:-} && $absolute_path == "$FIXTURE_FOREIGN_STAT_PATH" ]]; then
      printf "999999\\n"
    else
      /usr/bin/stat -f %u "$path"
    fi
    ;;
  %Lp) /usr/bin/stat -f %Lp "$path";;
  %Mp) /usr/bin/stat -f %Mp "$path";;
  %d:%i) /usr/bin/stat -f %d:%i "$path";;
  *) exit 64;;
esac'
write_fake ps '
pid=$2; /bin/kill -0 "$pid" 2>/dev/null || exit 1
printf "ps-env:%s:%s:%s\n" "${LC_ALL:-}" "${LANG:-}" "${TZ:-}" >>"$FIXTURE_LOG"
case "$4" in command=) echo "$FIXTURE_OBSERVED_EXECUTABLE";; lstart=) echo " $FIXTURE_OBSERVED_START ";; *) exit 64;; esac'
write_fake gascan '
printf "gascan:%s\\n" "$*" >>"$FIXTURE_LOG"
if [[ $1 == daemon-attest ]]; then [[ $FIXTURE_DAEMON_PID != 999999 ]] || exit 1; printf "{\\\"pid\\\":%s,\\\"executable\\\":\\\"%s\\\",\\\"start_identity\\\":\\\"%s\\\",\\\"instance_token\\\":\\\"%s\\\"}\\n" "$FIXTURE_DAEMON_PID" "$FIXTURE_ATTESTED_EXECUTABLE" "$FIXTURE_ATTESTED_START" "$FIXTURE_ATTESTED_TOKEN";
elif [[ $1 == list && ${2:-} == --all ]]; then printf "%s\\n" "$FIXTURE_ALL_SANDBOX_JSON";
elif [[ $1 == list ]]; then printf "%s\\n" "$FIXTURE_SANDBOX_JSON"; fi'

export PATH="$fixture/bin:/usr/bin:/bin:/usr/sbin:/sbin" FIXTURE_LOG=$log FIXTURE_REVISION=$revision FIXTURE_HASH=$hash
export GASCAN_EXPECTED_SOURCE_REVISION=$revision GASCAN_EXPECTED_VERSION=0.1.0
export FIXTURE_PACKAGE_ID=dev.gascan.pkg FIXTURE_VERSION=0.1.0
# **Derived from the real pin with package.sh's own jq program, never
# hand-written.** A hand-written engine block is why a packager that started
# emitting `artifacts` could disagree with verify-package.sh's key list while
# every contract stayed green: both fixtures satisfied the old shape and the
# real packager did not, and nothing in *-contract.sh builds a package from the
# real pin.
FIXTURE_ENGINE_JSON=$(jq -cS '{name, url, tag, revision, artifacts}' "$repo_root/engine/arca-pin.json")
export FIXTURE_ENGINE_JSON
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
for condition in stopped-service wrong-commit duplicate-client malformed-version trailing-version; do
  export FIXTURE_VERSION_JSON=$good_version FIXTURE_STATUS_JSON=$good_status
  case $condition in
    stopped-service) export FIXTURE_STATUS_JSON=${good_status/running/stopped};;
    wrong-commit) export FIXTURE_VERSION_JSON=${good_version/5973b9cc626a3e7a499bb316a958237ebe14e2ed/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa};;
    duplicate-client) FIXTURE_VERSION_JSON=$(jq -c '.[0] as $client | . + [$client]' <<<"$good_version"); export FIXTURE_VERSION_JSON;;
    malformed-version) export FIXTURE_VERSION_JSON='{}';;
    trailing-version) export FIXTURE_VERSION_JSON=${good_version/container-apiserver version 1.1.0 (build: release, commit: 5973b9c)/container-apiserver version 1.1.0 (build: release, commit: 5973b9c) trailing};;
  esac
  "$repo_root/packaging/macos/install.sh" "$fixture/test.pkg" >/dev/null 2>&1 && { echo "$condition accepted" >&2; exit 1; }
  ! grep -q '^sudo:' "$log"
done
export FIXTURE_VERSION_JSON=$good_version FIXTURE_STATUS_JSON=$good_status
"$repo_root/packaging/macos/install.sh" "$fixture/test.pkg" >/dev/null
grep -qx "sudo:installer -pkg $fixture/test.pkg -target /" "$log"

fixture_user_home=$fixture/uninstall-home
fixture_runtime_base=$fixture/uninstall-runtime
fixture_controller_root="$fixture_user_home/Library/Application Support/dev.gascan/controller"
fixture_runtime_root=$fixture_runtime_base/gascan
untargeted_user_home=$fixture/untargeted-home
untargeted_runtime_base=$fixture/untargeted-runtime
untargeted_controller_root="$untargeted_user_home/Library/Application Support/dev.gascan/controller"
untargeted_runtime_root=$untargeted_runtime_base/gascan

prepare_uninstall_roots() {
  rm -rf "$fixture_user_home" "$fixture_runtime_base"
  mkdir -p "$fixture_controller_root" "$fixture_runtime_root"
  chmod 0700 \
    "$fixture_user_home" \
    "$fixture_user_home/Library" \
    "$fixture_user_home/Library/Application Support" \
    "$fixture_user_home/Library/Application Support/dev.gascan" \
    "$fixture_controller_root" \
    "$fixture_runtime_base" \
    "$fixture_runtime_root"
  printf 'fixture controller state\n' >"$fixture_controller_root/state.sqlite3"
  printf 'fixture runtime state\n' >"$fixture_runtime_root/daemon-instance.json"
}

run_uninstall() {
  env HOME="$fixture_user_home" XDG_RUNTIME_DIR="$fixture_runtime_base" \
    "$repo_root/packaging/macos/uninstall.sh" "$@"
}

assert_untargeted_sentinels() {
  [[ $(<"$untargeted_controller_root/developer-sentinel") == do-not-remove-controller ]]
  [[ $(<"$untargeted_runtime_root/developer-sentinel") == do-not-remove-runtime ]]
}

prepare_uninstall_roots
mkdir -p "$untargeted_controller_root" "$untargeted_runtime_root"
printf 'do-not-remove-controller\n' >"$untargeted_controller_root/developer-sentinel"
printf 'do-not-remove-runtime\n' >"$untargeted_runtime_root/developer-sentinel"
# Even a future destructive invocation that bypasses run_uninstall remains
# confined to test-owned roots rather than inheriting the developer's paths.
export HOME=$untargeted_user_home XDG_RUNTIME_DIR=$untargeted_runtime_base

export FIXTURE_DAEMON_PID=999999 FIXTURE_SANDBOX_JSON='[]' FIXTURE_ALL_SANDBOX_JSON='[]'
preserve_output=$(run_uninstall)
grep -Fqx "Preserved durable controller state: $fixture_controller_root/state.sqlite3" <<<"$preserve_output"
grep -Fq 'Reinstall Gas Can to recover these sandboxes and volumes.' <<<"$preserve_output"
grep -Fq './packaging/macos/uninstall.sh --remove-data' <<<"$preserve_output"
[[ -f $fixture_controller_root/state.sqlite3 ]]
[[ -f $fixture_runtime_root/daemon-instance.json ]]
assert_untargeted_sentinels

: >"$log"; sleep 1000 & daemon_pid=$!; export FIXTURE_DAEMON_PID=$daemon_pid FIXTURE_SANDBOX_JSON='[]' FIXTURE_ALL_SANDBOX_JSON='[]'
export LC_ALL=C LANG=C TZ=America/Phoenix
for condition in attested-start observed-start executable empty-token; do
  export FIXTURE_ATTESTED_START=START FIXTURE_OBSERVED_START=START FIXTURE_ATTESTED_EXECUTABLE=/usr/local/bin/gascand FIXTURE_ATTESTED_TOKEN=TOKEN
  case $condition in
    attested-start) export FIXTURE_ATTESTED_START=REUSED;;
    observed-start) export FIXTURE_OBSERVED_START=REUSED;;
    executable) export FIXTURE_ATTESTED_EXECUTABLE=/tmp/foreign;;
    empty-token) export FIXTURE_ATTESTED_TOKEN='';;
  esac
  run_uninstall >/dev/null 2>&1 && { echo "$condition mismatch accepted" >&2; exit 1; }
  /bin/kill -0 "$daemon_pid"
  ! grep -q '^sudo:' "$log"
done
export FIXTURE_ATTESTED_START=START FIXTURE_OBSERVED_START=START FIXTURE_ATTESTED_EXECUTABLE=/usr/local/bin/gascand FIXTURE_ATTESTED_TOKEN=TOKEN
run_uninstall --remove-data >/dev/null
! /bin/kill -0 "$daemon_pid" 2>/dev/null; daemon_pid=
grep -qx 'gascan:list --json' "$log"
grep -qx 'gascan:list --all --json' "$log"
[[ ! -e $fixture_controller_root && ! -L $fixture_controller_root ]]
[[ ! -e $fixture_runtime_root && ! -L $fixture_runtime_root ]]
assert_untargeted_sentinels
if grep '^ps-env:' "$log" | grep -vx 'ps-env:C:C:UTC'; then
  printf 'daemon stop inspected process identity without deterministic UTC environment\n' >&2
  exit 1
fi

: >"$log"; export FIXTURE_DAEMON_PID=999999 FIXTURE_SANDBOX_JSON='[{"sandbox_id":"one"},{"sandbox_id":"two"}]' FIXTURE_ALL_SANDBOX_JSON='[{"sandbox_id":"one","actual_state":"absent"},{"sandbox_id":"two","actual_state":"absent"}]'
run_uninstall --remove-data >/dev/null
grep -qx 'gascan:--sandbox one destroy --yes' "$log"
grep -qx 'gascan:--sandbox two destroy --yes' "$log"
grep -qx 'gascan:list --all --json' "$log"
export FIXTURE_ALL_SANDBOX_JSON='[{"sandbox_id":"one","actual_state":"running"}]'
run_uninstall --remove-data >/dev/null 2>&1 && { echo 'active retained inventory accepted' >&2; exit 1; }
for invalid in '[{"sandbox_id":"same"},{"sandbox_id":"same"}]' '[{"sandbox_id":""}]' '{}'; do
  export FIXTURE_SANDBOX_JSON=$invalid
  run_uninstall --remove-data >/dev/null 2>&1 && { echo 'invalid sandbox inventory accepted' >&2; exit 1; }
done

uninstall_source=$repo_root/packaging/macos/uninstall.sh
for required in \
  'runtime_root=$(gascan_user_runtime_root)' \
  'runtime_parent=${runtime_root%/*}' \
  'runtime_child=${runtime_root##*/}' \
  '[[ $runtime_parent == /private/tmp && $runtime_child == "gascan-$expected_uid" ]]'
do
  grep -Fq "$required" "$uninstall_source" || {
    printf 'default runtime cleanup omits exact path contract: %s\n' "$required" >&2
    exit 1
  }
done

export FIXTURE_SANDBOX_JSON='[]' FIXTURE_ALL_SANDBOX_JSON='[]'
prepare_uninstall_roots
prepare_default_runtime_fixture() {
  local random_value candidate identity owner mode flags

  random_value=$(/usr/bin/od -An -N4 -tu4 /dev/urandom | /usr/bin/tr -d '[:space:]')
  default_runtime_uid=$((3000000000 + random_value % 1000000000))
  candidate=/private/tmp/gascan-$default_runtime_uid
  (umask 077 && /bin/mkdir "$candidate") || {
    printf 'could not atomically create default runtime fixture: %s\n' "$candidate" >&2
    return 1
  }
  identity=$(/usr/bin/stat -f '%d:%i' "$candidate") || return 1
  owner=$(/usr/bin/stat -f '%u' "$candidate") || return 1
  mode=$(/usr/bin/stat -f '%Lp' "$candidate") || return 1
  flags=$(/usr/bin/stat -f '%Mp' "$candidate") || return 1
  [[ ! -L $candidate && -d $candidate && \
    $owner == "$(/usr/bin/id -u)" && $mode == 700 && $flags == 0 ]] || return 1

  default_runtime_fixture=$candidate
  default_runtime_fixture_identity=$identity
  default_runtime_fixture_owner=$owner
  default_runtime_fixture_mode=$mode
  default_runtime_fixture_flags=$flags
}
prepare_default_runtime_fixture
printf 'default runtime sentinel\n' >"$default_runtime_fixture/daemon-instance.json"
export FIXTURE_ID_UID=$default_runtime_uid
export FIXTURE_STAT_OWNER_PATH=$default_runtime_fixture
FIXTURE_STAT_OWNER_PREFIX=$(dirname "$(dirname "$fixture")")
export FIXTURE_STAT_OWNER_PREFIX
export FIXTURE_STAT_OWNER_VALUE=$default_runtime_uid
env -u XDG_RUNTIME_DIR HOME="$fixture_user_home" \
  "$repo_root/packaging/macos/uninstall.sh" --remove-data >/dev/null
[[ ! -e $default_runtime_fixture && ! -L $default_runtime_fixture ]]
default_runtime_fixture=
default_runtime_fixture_identity=
default_runtime_fixture_owner=
default_runtime_fixture_mode=
default_runtime_fixture_flags=
unset FIXTURE_ID_UID FIXTURE_STAT_OWNER_PATH FIXTURE_STAT_OWNER_PREFIX FIXTURE_STAT_OWNER_VALUE

export FIXTURE_SANDBOX_JSON='[]' FIXTURE_ALL_SANDBOX_JSON='[]' FIXTURE_FOREIGN_STAT_PATH=
expect_unsafe_removal_refused() {
  local label=$1
  : >"$log"
  run_uninstall --remove-data >/dev/null 2>&1 && {
    printf 'unsafe uninstall path accepted: %s\n' "$label" >&2
    exit 1
  }
  ! grep -q '^sudo:' "$log"
  assert_untargeted_sentinels
}

prepare_uninstall_roots
mv "$fixture_user_home/Library" "$fixture/symlink-library-target"
ln -s "$fixture/symlink-library-target" "$fixture_user_home/Library"
expect_unsafe_removal_refused 'symlinked Library'
rm "$fixture_user_home/Library"
rm -rf "$fixture/symlink-library-target"

prepare_uninstall_roots
rm -rf "$fixture_user_home/Library/Application Support"
printf 'not a directory\n' >"$fixture_user_home/Library/Application Support"
expect_unsafe_removal_refused 'non-directory Application Support'

prepare_uninstall_roots
chmod 0777 "$fixture_user_home/Library"
expect_unsafe_removal_refused 'unsafe Library mode'

prepare_uninstall_roots
export FIXTURE_FOREIGN_STAT_PATH="$fixture_user_home/Library/Application Support"
expect_unsafe_removal_refused 'foreign Application Support owner'
export FIXTURE_FOREIGN_STAT_PATH=

prepare_uninstall_roots
chmod 0777 "$fixture_runtime_base"
expect_unsafe_removal_refused 'unsafe XDG runtime base mode'

prepare_uninstall_roots
export FIXTURE_FOREIGN_STAT_PATH=$fixture_runtime_root
expect_unsafe_removal_refused 'foreign runtime root owner'
export FIXTURE_FOREIGN_STAT_PATH=

safe_fixture_user_home=$fixture_user_home
safe_fixture_controller_root=$fixture_controller_root
mkdir -p "$fixture/home-ancestor-target"
ln -s "$fixture/home-ancestor-target" "$fixture/symlink-home-parent"
fixture_user_home=$fixture/symlink-home-parent/uninstall-home
fixture_controller_root="$fixture_user_home/Library/Application Support/dev.gascan/controller"
prepare_uninstall_roots
expect_unsafe_removal_refused 'symlinked ancestor above HOME'
fixture_user_home=$safe_fixture_user_home
fixture_controller_root=$safe_fixture_controller_root

safe_fixture_runtime_base=$fixture_runtime_base
safe_fixture_runtime_root=$fixture_runtime_root
mkdir -p "$fixture/runtime-ancestor-target"
ln -s "$fixture/runtime-ancestor-target" "$fixture/symlink-runtime-parent"
fixture_runtime_base=$fixture/symlink-runtime-parent/runtime
fixture_runtime_root=$fixture_runtime_base/gascan
prepare_uninstall_roots
expect_unsafe_removal_refused 'symlinked ancestor above XDG_RUNTIME_DIR'
fixture_runtime_base=$safe_fixture_runtime_base
fixture_runtime_root=$safe_fixture_runtime_root

printf 'PASS: Gas Can installer contract\n'
