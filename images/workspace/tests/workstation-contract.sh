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

nearest_existing_parent()
{
    candidate=$(realpath -m "$1")
    while test ! -e "$candidate"; do
        parent=$(dirname "$candidate")
        test "$parent" != "$candidate" || die "no existing parent for $1"
        candidate=$parent
    done
    printf '%s\n' "$candidate"
}

audit_writable_destination()
{
    name=$1
    path=$2
    resolved=$(realpath -m "$path")
    case "$resolved" in
        /home/workspace/.local|/home/workspace/.local/*|\
        /home/workspace/.cache|/home/workspace/.cache/*|\
        /home/workspace/.config|/home/workspace/.config/*) ;;
        *) die "$name escaped managed writable roots: $resolved" ;;
    esac
    case "$resolved" in /opt/gascan|/opt/gascan/*) die "$name resolved below /opt/gascan" ;; esac
    parent=$(nearest_existing_parent "$resolved")
    test -d "$parent" && test -w "$parent" ||
        die "$name has no writable existing parent: $parent"
}

path_position()
{
    wanted=$1
    old_ifs=$IFS
    IFS=:
    index=0
    for entry in $PATH; do
        test "$entry" != "$wanted" || {
            IFS=$old_ifs
            printf '%s\n' "$index"
            return
        }
        index=$((index + 1))
    done
    IFS=$old_ifs
    die "PATH entry is absent: $wanted"
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
test "$MISE_SYSTEM_CONFIG_FILE" = /etc/mise/config.toml ||
    die 'immutable mise config differs from production policy'
test -r "$MISE_SYSTEM_CONFIG_FILE" ||
    die 'immutable mise config is unavailable'
test "$PATH" = /home/workspace/.local/bin:/home/workspace/.local/share/cargo/bin:/home/workspace/.local/share/go/bin:/home/workspace/.local/share/gem/bin:/home/workspace/.local/share/mise/shims:/opt/gascan/mise/shims:/usr/local/sbin:/usr/local/bin:/opt/gascan/workstation/bin:/usr/sbin:/usr/bin:/sbin:/bin ||
    die 'PATH differs from production policy'
test -r /opt/gascan/image-tool-versions.json ||
    die 'immutable reviewed mise defaults are unavailable'

test "$(rustup show home)" = "$RUSTUP_HOME" || die 'rustup home differs from runtime policy'
test "$(npm config get prefix)" = "$NPM_CONFIG_PREFIX" || die 'npm prefix differs from runtime policy'
test "$(npm config get cache)" = "$NPM_CONFIG_CACHE" || die 'npm cache differs from runtime policy'
test "$(go env GOPATH)" = "$GOPATH" || die 'Go path differs from runtime policy'
test "$(go env GOCACHE)" = "$GOCACHE" || die 'Go build cache differs from runtime policy'
test "$(go env GOMODCACHE)" = "$GOMODCACHE" || die 'Go module cache differs from runtime policy'
test "$(python -m site --user-base)" = "$PYTHONUSERBASE" ||
    die 'Python user base differs from runtime policy'
test "$(gem env home)" = "$GEM_HOME" || die 'RubyGems home differs from runtime policy'

for mapping in \
    "$XDG_DATA_HOME:/home/workspace/.local" \
    "$XDG_CACHE_HOME:/home/workspace/.cache" \
    "$XDG_CONFIG_HOME:/home/workspace/.config" \
    "$MISE_DATA_DIR:/home/workspace/.local" \
    "$MISE_CACHE_DIR:/home/workspace/.cache" \
    "$MISE_GLOBAL_CONFIG_FILE:/home/workspace/.config" \
    "$MISE_STATE_DIR:/home/workspace/.config" \
    "$CARGO_HOME:/home/workspace/.local" \
    "$MISE_CARGO_HOME:/home/workspace/.local" \
    "$RUSTUP_HOME:/home/workspace/.local" \
    "$MISE_RUSTUP_HOME:/home/workspace/.local" \
    "$NPM_CONFIG_PREFIX:/home/workspace/.local" \
    "$NPM_CONFIG_CACHE:/home/workspace/.cache" \
    "$GOPATH:/home/workspace/.local" \
    "$GOCACHE:/home/workspace/.cache" \
    "$GOMODCACHE:/home/workspace/.cache" \
    "$PYTHONUSERBASE:/home/workspace/.local" \
    "$GEM_HOME:/home/workspace/.local" \
    "$MIX_HOME:/home/workspace/.local" \
    "$HEX_HOME:/home/workspace/.local" \
    "$REBAR_CACHE_DIR:/home/workspace/.cache" \
    "$CLAUDE_CONFIG_DIR:/home/workspace/.config/gascan" \
    "$CODEX_HOME:/home/workspace/.config/gascan" \
    "$PI_CODING_AGENT_DIR:/home/workspace/.config/gascan" \
    "$HERDR_CONFIG_PATH:/home/workspace/.config/gascan" \
    "$GH_CONFIG_DIR:/home/workspace/.config/gascan" \
    "$GLAB_CONFIG_DIR:/home/workspace/.config/gascan" \
    "$PI_CODING_AGENT_SESSION_DIR:/home/workspace/.cache"
do
    path=${mapping%%:*}
    root=${mapping#*:}
    resolved=$(realpath -m "$path")
    case "$resolved" in "$root"|"$root"/*) ;; *) die "persistent path escaped its volume: $path" ;; esac
    audit_writable_destination "$path" "$path"
done

immutable_position=$(path_position /opt/gascan/mise/shims)
for install_bin in \
    /home/workspace/.local/bin \
    /home/workspace/.local/share/cargo/bin \
    /home/workspace/.local/share/go/bin \
    /home/workspace/.local/share/gem/bin \
    /home/workspace/.local/share/mise/shims
do
    test "$(path_position "$install_bin")" -lt "$immutable_position" ||
        die "user install bin does not precede immutable shims: $install_bin"
done

for volume in /home/workspace/.local /home/workspace/.cache /home/workspace/.config
do
    target=$(findmnt -n -o TARGET -T "$volume") || die "volume is not mounted: $volume"
    test "$target" = "$volume" || die "persistent path is not its own mount: $volume ($target)"
    if test "$volume" = /home/workspace/.config; then
        test "$(stat -c %U:%G:%a "$volume")" = root:workspace:1770 ||
            die "persistent config root metadata changed: $volume"
        probe="$volume/.workstation-write-probe"
        mkdir "$probe"
        printf 'write-ok\n' >"$probe/config"
        test "$(cat "$probe/config")" = write-ok || die 'persistent config root is not writable'
        rm -rf "$probe"
    else
        test "$(stat -c %U:%G:%a "$volume")" = workspace:workspace:700 ||
            die "persistent path metadata changed: $volume"
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
