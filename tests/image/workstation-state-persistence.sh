#!/bin/sh
set -eu

mode=${1-}
case "$mode" in seed|probe|files) ;; *)
    printf 'usage: workstation-state-persistence.sh {seed|probe|files}\n' >&2
    exit 64
esac

: "${CLAUDE_CONFIG_DIR:=/home/workspace/.config/gascan/agents/claude}"
: "${CODEX_HOME:=/home/workspace/.config/gascan/agents/codex}"
: "${PI_CODING_AGENT_DIR:=/home/workspace/.config/gascan/agents/pi}"
: "${HERDR_CONFIG_PATH:=/home/workspace/.config/gascan/herdr/config.toml}"
: "${GH_CONFIG_DIR:=/home/workspace/.config/gascan/gh}"
: "${GLAB_CONFIG_DIR:=/home/workspace/.config/gascan/glab}"

claude_config=$CLAUDE_CONFIG_DIR/.claude.json
codex_config=$CODEX_HOME/config.toml
pi_config=$PI_CODING_AGENT_DIR/settings.json
herdr_config=$HERDR_CONFIG_PATH
gh_config=$GH_CONFIG_DIR/config.yml
glab_config=$GLAB_CONFIG_DIR/config.yml

assert_managed_path()
{
    path=$1
    root=$2
    resolved=$(realpath -m "$path")
    case "$resolved" in "$root"|"$root"/*) ;; *)
        printf 'workstation state path escaped managed volume: %s\n' "$path" >&2
        exit 1
    esac
}

for mapping in \
    "$claude_config:/home/workspace/.config/gascan" \
    "$codex_config:/home/workspace/.config/gascan" \
    "$pi_config:/home/workspace/.config/gascan" \
    "$herdr_config:/home/workspace/.config/gascan" \
    "$gh_config:/home/workspace/.config/gascan" \
    "$glab_config:/home/workspace/.config/gascan" \
    "/home/workspace/.cache/claude/doctor.txt:/home/workspace/.cache" \
    "/home/workspace/.cache/codex/mcp.txt:/home/workspace/.cache" \
    "/home/workspace/.cache/pi/packages.txt:/home/workspace/.cache" \
    "/home/workspace/.cache/herdr/version.txt:/home/workspace/.cache" \
    "/home/workspace/.cache/gh/editor.txt:/home/workspace/.cache" \
    "/home/workspace/.cache/glab/editor.txt:/home/workspace/.cache"
do
    assert_managed_path "${mapping%%:*}" "${mapping#*:}"
done

if test "$mode" = seed; then
    timeout 30 claude doctor > /home/workspace/.cache/claude/doctor.txt 2>&1
    codex mcp remove gascan-persistence >/dev/null 2>&1 || true
    codex mcp add gascan-persistence -- /bin/true >/dev/null
    codex mcp get gascan-persistence > /home/workspace/.cache/codex/mcp.txt
    pi install /workspace/.gascan/gascan-pi-extension.js \
        > /home/workspace/.cache/pi/install.txt
    pi list --no-approve > /home/workspace/.cache/pi/packages.txt
    herdr --default-config >"$herdr_config"
    herdr --version > /home/workspace/.cache/herdr/version.txt
    gh config set editor vim
    gh config get editor > /home/workspace/.cache/gh/editor.txt
    GLAB_CHECK_UPDATE=false glab config set editor vim --global >/dev/null 2>&1
    GLAB_CHECK_UPDATE=false glab config get editor --global \
        > /home/workspace/.cache/glab/editor.txt 2>/dev/null
fi

for file in \
    "$claude_config" \
    "$codex_config" \
    "$pi_config" \
    "$herdr_config" \
    "$gh_config" \
    "$glab_config" \
    /home/workspace/.cache/claude/doctor.txt \
    /home/workspace/.cache/codex/mcp.txt \
    /home/workspace/.cache/pi/packages.txt \
    /home/workspace/.cache/herdr/version.txt \
    /home/workspace/.cache/gh/editor.txt \
    /home/workspace/.cache/glab/editor.txt
do
    test -s "$file" || {
        printf 'workstation state file is absent or empty: %s\n' "$file" >&2
        exit 1
    }
done

grep -Fq 'Claude Code doctor' /home/workspace/.cache/claude/doctor.txt
grep -Fq 'gascan-persistence' "$codex_config"
grep -Fq 'gascan-persistence' /home/workspace/.cache/codex/mcp.txt
grep -Fq 'gascan-pi-extension.js' "$pi_config"
grep -Fq 'gascan-pi-extension.js' /home/workspace/.cache/pi/packages.txt
grep -Fq '# herdr configuration' "$herdr_config"
grep -Fq 'herdr ' /home/workspace/.cache/herdr/version.txt
test "$(cat /home/workspace/.cache/gh/editor.txt)" = vim
test "$(cat /home/workspace/.cache/glab/editor.txt)" = vim

if test "$mode" = probe; then
    timeout 30 claude doctor >/dev/null 2>&1
    codex mcp get gascan-persistence | grep -Fq gascan-persistence
    pi list --no-approve | grep -Fq gascan-pi-extension.js
    herdr --default-config | cmp -s - "$herdr_config"
    test "$(gh config get editor)" = vim
    test "$(GLAB_CHECK_UPDATE=false glab config get editor --global 2>/dev/null)" = vim
fi

for name in ANTHROPIC_API_KEY OPENAI_API_KEY GITHUB_TOKEN GH_TOKEN GITLAB_TOKEN GLAB_TOKEN
do
    eval "value=\${$name:-}"
    test -z "$value" || {
        printf 'credential unexpectedly present during workstation state probe: %s\n' "$name" >&2
        exit 1
    }
done

printf 'workstation-state-%s-ok\n' "$mode"
