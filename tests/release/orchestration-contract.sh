#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-orchestration-contract.XXXXXX")
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/bin" "$fixture/root" "$fixture/tmp"
touch "$fixture/test.pkg"
log=$fixture/log
LAST_OUTPUT=

write_fake() { local name=$1 body=$2; printf '#!/usr/bin/env bash\nset -euo pipefail\n%s\n' "$body" >"$fixture/bin/$name"; chmod 0755 "$fixture/bin/$name"; }
write_fake git 'printf "%040d\n" 0'
write_fake cargo 'printf "%s\n" "{\"packages\":[{\"name\":\"gascan\",\"version\":\"0.1.0\"}]}"'
write_fake pkgutil 'exit 1'
write_fake container '
case "$*" in
  "system version --format json") printf "%s\n" "[{\"appName\":\"container\",\"buildType\":\"release\",\"commit\":\"5973b9cc626a3e7a499bb316a958237ebe14e2ed\",\"version\":\"1.1.0\"},{\"appName\":\"container-apiserver\",\"buildType\":\"release\",\"commit\":\"5973b9cc626a3e7a499bb316a958237ebe14e2ed\",\"version\":\"container-apiserver version 1.1.0 (build: release, commit: 5973b9c)\"}]";;
  "system status --format json") printf "%s\n" "{\"apiServerAppName\":\"container-apiserver\",\"apiServerBuild\":\"release\",\"apiServerCommit\":\"5973b9cc626a3e7a499bb316a958237ebe14e2ed\",\"apiServerVersion\":\"container-apiserver version 1.1.0 (build: release, commit: 5973b9c)\",\"status\":\"running\"}";;
  "list --all --format json"|"volume list --format json"|"system dns list --format json") printf "[]\n";;
  *) exit 64;;
esac'
write_fake package 'printf "%s\n" "$FIXTURE_PACKAGE"'
write_fake verify 'exit 0'
write_fake install 'printf "install\n" >>"$FIXTURE_LOG"; exit "${FIXTURE_INSTALL_STATUS:-0}"'
write_fake smoke 'printf "smoke:%s\n" "${FIXTURE_SMOKE_PHASE:-ok}" >>"$FIXTURE_LOG"; exit "${FIXTURE_SMOKE_STATUS:-0}"'
write_fake uninstall '
printf "uninstall\n" >>"$FIXTURE_LOG"
count=$(grep -c "^uninstall$" "$FIXTURE_LOG")
if [[ ${FIXTURE_UNINSTALL_NONIDEMPOTENT:-0} == 1 && $count -gt 1 ]]; then exit 44; fi
exit "${FIXTURE_UNINSTALL_STATUS:-0}"'
write_fake gascan '[[ $* == "doctor --json" ]] || exit 64; printf "%s\n" "{\"checks\":[]}"'

export PATH="$fixture/bin:/usr/bin:/bin:/usr/sbin:/sbin" FIXTURE_LOG=$log FIXTURE_PACKAGE=$fixture/test.pkg
export GASCAN_RELEASE_TESTING=YES GASCAN_RELEASE_CLEAN_HOST_CONFIRM=YES
export GASCAN_RELEASE_TEST_PACKAGE_BUILDER=$fixture/bin/package
export GASCAN_RELEASE_TEST_PACKAGE_VERIFIER=$fixture/bin/verify
export GASCAN_RELEASE_TEST_INSTALLER=$fixture/bin/install
export GASCAN_RELEASE_TEST_UNINSTALLER=$fixture/bin/uninstall
export GASCAN_RELEASE_TEST_SMOKE=$fixture/bin/smoke
export GASCAN_RELEASE_TEST_INSTALLED_GASCAN=$fixture/bin/gascan
export GASCAN_RELEASE_TEST_INSTALL_ROOT=$fixture/root
export GASCAN_RELEASE_TEST_RUNTIME_ROOT=$fixture/runtime
export TMPDIR=$fixture/tmp

assert_failure() {
  local label=$1 expected=$2 output status=0
  : >"$log"
  output=$("$repo_root/tests/release/clean-host.sh" 2>&1) || status=$?
  [[ $status -eq $expected ]] || { printf '%s returned %s, expected %s\n%s\n' "$label" "$status" "$expected" "$output" >&2; exit 1; }
  [[ $output != *'PASS: Gas Can macOS MVP release gate'* ]] || { printf '%s printed PASS\n' "$label" >&2; exit 1; }
  LAST_OUTPUT=$output
  grep -qx install "$log"
  grep -q '^uninstall$' "$log"
}

export FIXTURE_INSTALL_STATUS=41 FIXTURE_SMOKE_STATUS=0 FIXTURE_UNINSTALL_STATUS=0
assert_failure post-install 41

export FIXTURE_INSTALL_STATUS=0 FIXTURE_UNINSTALL_STATUS=0
for phase in create apply destroy; do
  export FIXTURE_SMOKE_PHASE=$phase FIXTURE_SMOKE_STATUS=42 FIXTURE_UNINSTALL_NONIDEMPOTENT=1
  assert_failure "smoke-$phase" 42
  [[ $(grep -c '^uninstall$' "$log") -eq 1 ]] || { printf 'clean smoke failure retried successful uninstall: %s\n' "$phase" >&2; exit 1; }
  [[ $LAST_OUTPUT != *'clean-host cleanup left recorded resources'* ]] || { printf 'clean smoke failure reported false cleanup residue: %s\n' "$phase" >&2; exit 1; }
done

export FIXTURE_SMOKE_STATUS=0 FIXTURE_UNINSTALL_STATUS=43 FIXTURE_UNINSTALL_NONIDEMPOTENT=0
assert_failure uninstall 43
[[ $(grep -c '^uninstall$' "$log") -ge 2 ]] || { printf 'uninstall failure did not trigger cleanup retry\n' >&2; exit 1; }
[[ $LAST_OUTPUT == *'clean-host cleanup left recorded resources'* ]] || { printf 'uninstall cleanup failure lacked residue diagnostic\n' >&2; exit 1; }

printf 'PASS: Gas Can release failure orchestration contract\n'
