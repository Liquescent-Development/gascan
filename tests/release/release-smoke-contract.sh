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

printf 'PASS: native shell release smoke contract\n'
