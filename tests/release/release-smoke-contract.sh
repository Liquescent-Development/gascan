#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
release_smoke=$repo_root/packaging/macos/release-smoke.sh

for required in \
  'GASCAN_SHELL_INPUT_READY' \
  '"--sandbox", sandbox_id, "shell"' \
  'BASH_VERSION=' \
  'INTERACTIVE=yes' \
  'LOGIN=yes' \
  'SHELL=/bin/bash' \
  'TERM=gascan-release-term' \
  'COMPLETION=/usr/share/bash-completion/bash_completion' \
  '/opt/gascan/shell/bin/starship --version' \
  'SELECTOR=standard' \
  'SELECTOR=starship' \
  'SELECTOR=starship-nerd-font' \
  'STARSHIP_CONFIG=/home/workspace/.config/gascan/shell/starship.toml' \
  'STARSHIP_EXECUTABLE=/opt/gascan/shell/bin/starship' \
  'STARSHIP_FUNCTION=function'
do
  grep -F "$required" "$release_smoke" >/dev/null || {
    printf 'release smoke omits native shell proof: %s\n' "$required" >&2
    exit 1
  }
done

fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-release-pty-contract.XXXXXX")
probe_pid=
cleanup() {
  if [[ -n $probe_pid ]] && kill -0 "$probe_pid" 2>/dev/null; then
    kill -KILL "$probe_pid" 2>/dev/null || true
  fi
  rm -rf "$fixture"
}
trap cleanup EXIT

probe=$fixture/default-shell-probe.py
awk '
  /^  python3 - "\$gascan_bin" "\$sandbox_id" <<'\''PY'\''$/ {
    copying = 1
    next
  }
  copying && /^PY$/ {
    exit
  }
  copying {
    print
  }
' "$release_smoke" >"$probe"
test -s "$probe" || {
  printf 'release smoke PTY probe could not be extracted\n' >&2
  exit 1
}

python3 - "$probe" <<'PY'
import ast
import errno
import os
import pty
import select
import sys
import time

source = open(sys.argv[1], encoding="utf-8").read()
module = ast.parse(source)
read_until = next(
    node
    for node in module.body
    if isinstance(node, ast.FunctionDef) and node.name == "read_until"
)
namespace = {
    "errno": errno,
    "os": os,
    "select": select,
    "sys": sys,
    "time": time,
}
exec(compile(ast.Module(body=[read_until], type_ignores=[]), sys.argv[1], "exec"), namespace)


class ExitedProcess:
    @staticmethod
    def poll():
        return 0


controller, user = pty.openpty()
try:
    os.write(user, b"GASCAN_RELEASE_SHELL_END\n")
    namespace.update(
        controller=controller,
        captured=bytearray(),
        process=ExitedProcess(),
    )
    namespace["read_until"](
        b"GASCAN_RELEASE_SHELL_END",
        time.monotonic() + 1,
    )
finally:
    os.close(user)
    os.close(controller)
PY

fake_gascan=$fixture/fake-gascan
cat >"$fake_gascan" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail
: "${GASCAN_RELEASE_PTY_PID_FILE:?}"
trap '' TERM
printf '%s\n' "$$" >"$GASCAN_RELEASE_PTY_PID_FILE"
python3 -c 'import os; os.write(1, b"x" * (1024 * 1024 + 65536))'
exec sleep 300
FAKE
chmod 0755 "$fake_gascan"

set +e
GASCAN_RELEASE_PTY_PID_FILE=$fixture/child.pid \
  python3 "$probe" "$fake_gascan" contract-sandbox \
  >"$fixture/stdout" 2>"$fixture/stderr"
probe_status=$?
set -e
test "$probe_status" -ne 0 || {
  printf 'release smoke PTY overflow unexpectedly succeeded\n' >&2
  exit 1
}
grep -F 'default shell output exceeded its limit' "$fixture/stderr" >/dev/null || {
  printf 'release smoke PTY overflow did not reach the bounded-output failure\n' >&2
  exit 1
}
probe_pid=$(<"$fixture/child.pid")
case $probe_pid in
  ''|*[!0-9]*)
    printf 'release smoke PTY fixture did not record a valid child pid\n' >&2
    exit 1
    ;;
esac
if kill -0 "$probe_pid" 2>/dev/null; then
  kill -KILL "$probe_pid" 2>/dev/null || true
  printf 'release smoke PTY failure left its child alive\n' >&2
  exit 1
fi

trap - EXIT
cleanup
printf 'PASS: native shell release smoke contract\n'
