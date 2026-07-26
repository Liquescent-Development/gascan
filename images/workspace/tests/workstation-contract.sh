#!/bin/sh
set -eu

die()
{
    printf 'workstation contract: %s\n' "$*" >&2
    exit 1
}

capture()
{
    output=$("$@" 2>&1) || die "$* failed"
    printf '%s\n' "$output"
}

first_line()
{
    capture "$@" | sed -n '1p'
}

expect_exact()
{
    expected=$1
    shift
    actual=$(capture "$@")
    test "$actual" = "$expected" || die "$* reported '$actual', expected '$expected'"
}

locked=/opt/gascan/workstation/versions.json
mise_locked=/opt/gascan/image-tool-versions.json
test -r "$locked" && test -r "$mise_locked" || die 'locked version evidence is unavailable'
test "$(stat -c %U:%G "$locked")" = root:root || die 'workstation version evidence owner changed'
test "$(stat -c %a "$locked")" = 444 || die 'workstation version evidence mode changed'

locked_version()
{
    jq -er --arg tool "$1" '.[$tool] | select(type == "string" and length > 0)' "$locked"
}

mise_version()
{
    jq -er --arg tool "$1" '.[$tool] | select(type == "string" and length > 0)' "$mise_locked"
}

expect_exact "$(locked_version claude) (Claude Code)" claude --version
expect_exact "codex-cli $(locked_version codex)" codex --version
expect_exact "$(locked_version pi)" pi --version
expect_exact "herdr $(locked_version herdr)" herdr --version
test "$(first_line glab --version | awk '{print $1, $2}')" = "glab $(locked_version glab)" ||
    die 'glab normalized version differs from the workstation lock'
test "$(first_line nvim --version)" = "NVIM v$(locked_version nvim)" ||
    die 'nvim version differs from the workstation lock'

go_version=$(mise_version go)
expect_exact "go version go${go_version} linux/arm64" go version
test "$(first_line rustc --version | awk '{print $1, $2}')" = "$(locked_version rustc)" ||
    die 'rustc normalized version differs from the workstation lock'
test "$(first_line cargo --version | awk '{print $1, $2}')" = "$(locked_version cargo)" ||
    die 'cargo normalized version differs from the workstation lock'
test "$(first_line vim --version | awk '{print $1, $2, $3, $4, $5}')" = "$(locked_version vim)" ||
    die 'vim normalized version differs from the workstation lock'
test "$(first_line emacs --version | awk '{print $1, $2, $3}')" = "$(locked_version emacs)" ||
    die 'emacs normalized version differs from the workstation lock'
test "$(first_line pico --version | awk '{print $1, $2, $3, $4}')" = "$(locked_version pico)" ||
    die 'pico normalized version differs from the workstation lock'
test "$(first_line gh --version | awk '{print $1, $2, $3}')" = "$(locked_version gh)" ||
    die 'gh normalized version differs from the workstation lock'
expect_exact "$(locked_version git)" git --version
test "$(first_line ip -Version | cut -d, -f1,2)" = "$(locked_version ip)" ||
    die 'ip normalized version differs from the workstation lock'
expect_exact "$(locked_version ss)" ss --version
test "$(first_line ping -V)" = "$(locked_version ping)" ||
    die 'ping normalized version differs from the workstation lock'
test "$(first_line ifconfig --version)" = "$(locked_version ifconfig)" ||
    die 'ifconfig normalized version differs from the workstation lock'
test "$(first_line netstat --version)" = "$(locked_version netstat)" ||
    die 'netstat normalized version differs from the workstation lock'
expect_exact "$(locked_version dig)" dig -v
test "$(first_line traceroute --version)" = "$(locked_version traceroute)" ||
    die 'traceroute normalized version differs from the workstation lock'
nc_output=$(nc -h 2>&1) || nc_status=$?
case ${nc_status:-0} in 0|1) ;; *) die 'nc -h failed unexpectedly' ;; esac
test "$(printf '%s\n' "$nc_output" | sed -n '1p')" = "$(locked_version nc)" ||
    die 'nc normalized version differs from the workstation lock'
test "$(first_line rg --version)" = "$(locked_version rg)" ||
    die 'rg normalized version differs from the workstation lock'
expect_exact "$(locked_version fd)" fd --version
expect_exact "$(locked_version fzf)" fzf --version
expect_exact "$(locked_version tmux)" tmux -V

test "$MISE_DATA_DIR" = /home/workspace/.local/share/mise ||
    die 'writable mise data root differs from production policy'
test "$MISE_SYSTEM_DATA_DIR" = /opt/gascan/mise ||
    die 'immutable mise system root differs from production policy'
test "$MISE_CACHE_DIR" = /home/workspace/.cache/mise ||
    die 'mise cache root differs from production policy'
test "$MISE_GLOBAL_CONFIG_FILE" = /home/workspace/.config/gascan/mise.toml ||
    die 'mise config root differs from production policy'
case "$PATH" in
    /home/workspace/.local/share/mise/shims:/opt/gascan/mise/shims:*) ;;
    *) die 'writable mise shims do not precede immutable system shims' ;;
esac
test -r /opt/gascan/image-tool-versions.json ||
    die 'immutable reviewed mise defaults are unavailable'

for mapping in \
    "$CLAUDE_CONFIG_DIR:/home/workspace/.config/gascan" \
    "$CODEX_HOME:/home/workspace/.config/gascan" \
    "$PI_CODING_AGENT_DIR:/home/workspace/.config/gascan" \
    "$HERDR_CONFIG_PATH:/home/workspace/.config/gascan" \
    "$GH_CONFIG_DIR:/home/workspace/.config/gascan" \
    "$GLAB_CONFIG_DIR:/home/workspace/.config/gascan" \
    "$MISE_CACHE_DIR:/home/workspace/.cache" \
    "$PI_CODING_AGENT_SESSION_DIR:/home/workspace/.cache"
do
    path=${mapping%%:*}
    root=${mapping#*:}
    resolved=$(realpath -m "$path")
    case "$resolved" in "$root"|"$root"/*) ;; *) die "persistent path escaped its volume: $path" ;; esac
done

for volume in /home/workspace/.local/share/mise /home/workspace/.cache /home/workspace/.config/gascan
do
    target=$(findmnt -n -o TARGET -T "$volume") || die "volume is not mounted: $volume"
    test "$target" = "$volume" || die "persistent path is not its own mount: $volume ($target)"
    if test "$volume" = /home/workspace/.config/gascan; then
        case "$(stat -c %U:%G:%a "$volume")" in
            workspace:workspace:700|root:workspace:1770) ;;
            *) die "persistent config root metadata changed: $volume" ;;
        esac
        probe="$volume/.workstation-write-probe"
        printf 'write-ok\n' >"$probe"
        test "$(cat "$probe")" = write-ok || die 'persistent config root is not writable'
        rm -f "$probe"
    else
        test "$(stat -c %U:%G "$volume")" = workspace:workspace ||
            die "persistent path owner changed: $volume"
    fi
done

test "$(id -u)" = 1000 || die 'workstation contract is not running as workspace'
test "$(awk '$1 == "CapEff:" {print $2}' /proc/self/status)" = 0000000000000000 ||
    die 'workstation process has effective Linux capabilities'

for forbidden in \
    /run/host-services/ssh-auth.sock \
    /var/run/docker.sock \
    /home/workspace/.ssh \
    /root/.ssh \
    /Users \
    /Library/Keychains \
    /System/Library/Keychains \
    /workspace/.ssh
do
    test ! -r "$forbidden" || die "forbidden host/private path is readable: $forbidden"
done
for name in $(env | sed 's/=.*//' | LC_ALL=C sort -u); do
    case "$name" in
        ANTHROPIC_API_KEY|OPENAI_API_KEY|GITHUB_TOKEN|GH_TOKEN|GITLAB_TOKEN|GLAB_TOKEN|DOCKER_AUTH_CONFIG|SSH_AUTH_SOCK|*TOKEN*|*API_KEY*)
            die "credential-like environment variable is present: $name"
            ;;
    esac
done
grep -Fq '/Users/' /proc/self/mountinfo && die 'a macOS host-home path appears in mount inventory'

printf 'workstation-contract-ok\n'
