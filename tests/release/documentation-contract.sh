#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
readme="$repo_root/README.md"
default_manifest="$repo_root/packaging/macos/default-gascan.toml"

require_text() {
  local path=$1 text=$2
  if ! grep -Fq -- "$text" "$path"; then
    printf 'missing documentation in %s: %s\n' "${path#"$repo_root"/}" "$text" >&2
    exit 1
  fi
}

require_shell_example() {
  local path=$1
  awk '
    $0 == "[shell]" {
      if ((getline == 1 && $0 == "prompt = \"standard\"") &&
          (getline == 1 && $0 == "# prompt = \"starship\"") &&
          (getline == 1 && $0 == "# prompt = \"starship-nerd-font\"")) {
        found = 1
      }
    }
    END { exit !found }
  ' "$path" || {
    printf 'missing exact managed-shell example in %s\n' "${path#"$repo_root"/}" >&2
    exit 1
  }
}

require_shell_example "$readme"
require_shell_example "$default_manifest"

require_text "$readme" 'opens interactive login Bash with colors and completion'
require_text "$readme" 'gascan shell -- <argv>'
require_text "$readme" '`standard` is the default, backward-compatible prompt.'
require_text "$readme" 'It does not activate Starship.'
require_text "$readme" "Both Starship modes use Gas Can's pinned, offline-capable Starship binary."
require_text "$readme" '`starship` requires no special font.'
require_text "$readme" '`starship-nerd-font` requires a Nerd Font installed and selected'
require_text "$readme" 'macOS terminal.'
require_text "$readme" 'Gas Can does not install fonts on the host.'
require_text "$readme" 'The same prompt choice applies to both `gascan shell` and SSH.'
require_text "$readme" 'Run `gascan apply` after changing the prompt.'
require_text "$readme" 'Pre-existing same-user interactive shell customization is trusted caller'
require_text "$readme" 'state; it is not a same-shell isolation boundary.'
require_text "$readme" 'on-demand, per-user daemon'
require_text "$readme" 'start the on-demand daemon when needed'
require_text "$readme" 'automatically replace it after an upgrade.'
require_text "$readme" '`gascan daemon start [--json]`'
require_text "$readme" '`gascan daemon stop [--force] [--json]`'
require_text "$readme" '`gascan daemon restart [--force] [--json]`'
require_text "$readme" '`gascan daemon status [--json]`'
require_text "$readme" 'stop and restart wait for active sandbox operations and attachments'
require_text "$readme" 'to finish gracefully.'
require_text "$readme" 'Use `--force` only when necessary:'
require_text "$readme" 'interrupt active sandbox operations and attachments.'
require_text "$readme" 'gascan daemon status --json | jq'
require_text "$readme" 'Health             healthy'
require_text "$readme" 'PID                <pid>'
require_text "$readme" 'Uptime             <duration>'
require_text "$readme" 'Installed version  <installed-version>'
require_text "$readme" 'Running version    <running-version>'
require_text "$readme" 'Executable         <path-to-gascand>'
require_text "$readme" '"state": "running"'
require_text "$readme" '"health": "healthy"'
require_text "$readme" '"installed_version": "<installed-version>"'
require_text "$readme" '"running_version": "<running-version>"'
require_text "$readme" '"pid": <pid>'
require_text "$readme" '"started_at_millis": <epoch-milliseconds>'
require_text "$readme" '"uptime_millis": <milliseconds>'
require_text "$readme" '"executable": "<path-to-gascand>"'
require_text "$readme" '"legacy": false'
require_text "$readme" '○ Gascan daemon is stopped'
require_text "$readme" 'checks the workspace from which you run `gascan doctor`'
require_text "$readme" "the daemon's working directory."

printf 'PASS: native shell, managed prompt, and daemon lifecycle documentation contract\n'
