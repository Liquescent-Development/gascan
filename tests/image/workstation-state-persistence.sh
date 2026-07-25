#!/bin/sh
set -eu

mode=${1-}
case "$mode" in seed|probe|files) ;; *)
    printf 'usage: workstation-state-persistence.sh {seed|probe|files}\n' >&2
    exit 64
esac

# The acceptance probe must never let an installed tool observe a credential.
for name in \
    ANTHROPIC_API_KEY \
    ANTHROPIC_OAUTH_TOKEN \
    CLAUDE_CODE_OAUTH_TOKEN \
    OPENAI_API_KEY \
    AZURE_OPENAI_API_KEY \
    GITHUB_TOKEN \
    GH_TOKEN \
    GITLAB_TOKEN \
    GLAB_TOKEN \
    AWS_ACCESS_KEY_ID \
    AWS_SECRET_ACCESS_KEY \
    AWS_SESSION_TOKEN \
    AWS_BEARER_TOKEN_BEDROCK \
    GOOGLE_API_KEY \
    GEMINI_API_KEY
do
    eval "value=\${$name:-}"
    test -z "$value" || {
        printf 'credential unexpectedly present during workstation state probe: %s\n' "$name" >&2
        exit 1
    }
done

: "${GASCAN_CONFIG_ROOT:=/home/workspace/.config/gascan}"
: "${GASCAN_CACHE_ROOT:=/home/workspace/.cache}"
: "${CLAUDE_CONFIG_DIR:=$GASCAN_CONFIG_ROOT/agents/claude}"
: "${CODEX_HOME:=$GASCAN_CONFIG_ROOT/agents/codex}"
: "${PI_CODING_AGENT_DIR:=$GASCAN_CONFIG_ROOT/agents/pi}"
: "${HERDR_CONFIG_PATH:=$GASCAN_CONFIG_ROOT/herdr/config.toml}"
: "${GH_CONFIG_DIR:=$GASCAN_CONFIG_ROOT/gh}"
: "${GLAB_CONFIG_DIR:=$GASCAN_CONFIG_ROOT/glab}"
: "${GASCAN_PI_EXTENSION:=/workspace/.gascan/gascan-pi-extension.js}"

claude_config=$CLAUDE_CONFIG_DIR/.claude.json
codex_config=$CODEX_HOME/config.toml
pi_config=$PI_CODING_AGENT_DIR/settings.json
gh_config=$GH_CONFIG_DIR/config.yml
glab_config=$GLAB_CONFIG_DIR/config.yml

assert_managed_path()
{
    python3 - "$1" "$2" <<'PY'
import os
import sys

path = os.path.realpath(sys.argv[1])
root = os.path.realpath(sys.argv[2])
if path != root and not path.startswith(root + os.sep):
    print(f"workstation state path escaped managed volume: {sys.argv[1]}", file=sys.stderr)
    raise SystemExit(1)
PY
}

for path in \
    "$claude_config" \
    "$codex_config" \
    "$pi_config" \
    "$HERDR_CONFIG_PATH" \
    "$gh_config" \
    "$glab_config"
do
    assert_managed_path "$path" "$GASCAN_CONFIG_ROOT"
done
assert_managed_path "$GASCAN_CACHE_ROOT" "$GASCAN_CACHE_ROOT"

if test "$mode" = seed; then
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 timeout 30 claude doctor >/dev/null 2>&1
    codex mcp remove gascan-persistence >/dev/null 2>&1 || true
    codex mcp add gascan-persistence -- /bin/true >/dev/null
    codex mcp get gascan-persistence | grep -Fq gascan-persistence
    PI_OFFLINE=1 PI_TELEMETRY=0 pi install "$GASCAN_PI_EXTENSION" >/dev/null
    PI_OFFLINE=1 PI_TELEMETRY=0 pi list --no-approve |
        grep -Fq gascan-pi-extension.js
    herdr --help >/dev/null
    gh config set editor vim
    test "$(gh config get editor)" = vim
    GLAB_CHECK_UPDATE=false glab config set editor vim --global >/dev/null 2>&1
    test "$(GLAB_CHECK_UPDATE=false glab config get editor --global 2>/dev/null)" = vim
fi

for file in \
    "$claude_config" \
    "$codex_config" \
    "$pi_config" \
    "$gh_config" \
    "$glab_config"
do
    test -s "$file" || {
        printf 'workstation state file is absent or empty: %s\n' "$file" >&2
        exit 1
    }
done

grep -Fq 'gascan-persistence' "$codex_config"
grep -Fq 'gascan-pi-extension.js' "$pi_config"

if test "$mode" = probe; then
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 timeout 30 claude doctor >/dev/null 2>&1
    codex mcp get gascan-persistence | grep -Fq gascan-persistence
    PI_OFFLINE=1 PI_TELEMETRY=0 pi list --no-approve |
        grep -Fq gascan-pi-extension.js
    herdr --help >/dev/null
    test "$(gh config get editor)" = vim
    test "$(GLAB_CHECK_UPDATE=false glab config get editor --global 2>/dev/null)" = vim
fi

printf 'workstation-state-%s-ok\n' "$mode"
