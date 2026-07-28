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

printf 'PASS: native shell and managed prompt documentation contract\n'
