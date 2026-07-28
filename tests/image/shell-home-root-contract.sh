#!/bin/bash
set -euo pipefail

die()
{
    printf 'shell home root contract: %s\n' "$*" >&2
    exit 1
}

source_root=${1:-/source}
configurator=/usr/local/bin/configure-shell-home
hook=/etc/gascan/bashrc
home=/home/workspace
gascan_root=$home/.config/gascan
shell_dir=$gascan_root/shell
selector=$shell_dir/prompt
config=$shell_dir/starship.toml
lock=$gascan_root/.shell.lock
immutable_root=/opt/gascan/shell

test "$(/usr/bin/id -u)" = 0 || die 'contract must run as real root'
test "$(/usr/bin/id -u workspace)" = 1000 || die 'workspace uid changed'
workspace_gid=$(/usr/bin/id -g workspace)
test "$workspace_gid" = 1000 || die 'workspace gid changed'

install -D -o root -g root -m 0555 \
    "$source_root/images/workspace/bin/configure-shell-home" "$configurator"
install -D -o root -g root -m 0444 \
    "$source_root/images/workspace/etc/gascan/bashrc" "$hook"
install -d -o root -g root -m 0555 "$immutable_root/presets"
install -o root -g root -m 0444 \
    "$source_root/images/workspace/etc/gascan/starship.toml" \
    "$immutable_root/presets/starship.toml"
install -o root -g root -m 0444 \
    "$source_root/images/workspace/etc/gascan/starship-nerd-font.toml" \
    "$immutable_root/presets/starship-nerd-font.toml"
chmod 0555 "$immutable_root" "$immutable_root/presets"

install -d -o workspace -g workspace -m 0755 "$home"
install -d -o root -g workspace -m 1770 "$home/.config" "$gascan_root"
rm -rf "$shell_dir"
rm -f "$lock"

run_configurator()
{
    HOME=$home "$configurator" "$1"
}

assert_metadata()
{
    path=$1
    expected=$2
    actual=$(stat -c %U:%G:%a "$path")
    test "$actual" = "$expected" ||
        die "$path metadata is $actual, expected $expected"
}

assert_selected_config()
{
    selected=$(cat "$selector")
    case "$selected" in
        standard)
            test ! -e "$config" || die 'standard retained generated config'
            ;;
        starship|starship-nerd-font)
            cmp -s "$config" "$immutable_root/presets/$selected.toml" ||
                die "$selected selector/config transaction is inconsistent"
            ;;
        *)
            die "invalid selected prompt after concurrent transaction: $selected"
            ;;
    esac
}

# Root and fixed HOME are production preconditions, not fixture overrides.
if /usr/sbin/runuser -u workspace -- env HOME="$home" "$configurator" standard; then
    die 'non-root configurator invocation succeeded'
fi
test ! -e "$shell_dir" && test ! -e "$lock" ||
    die 'non-root rejection mutated managed state'
wrong_home=/tmp/gascan-shell-wrong-home
rm -rf "$wrong_home"
install -d -o workspace -g workspace -m 0755 "$wrong_home"
if HOME=$wrong_home "$configurator" standard; then
    die 'root configurator accepted a non-workspace HOME'
fi
test ! -e "$wrong_home/.config" || die 'wrong-HOME rejection mutated state'
rm -rf "$wrong_home"

run_configurator standard
assert_metadata "$shell_dir" root:workspace:750
assert_metadata "$selector" root:workspace:640
assert_metadata "$lock" root:root:600
test "$(cat "$selector")" = standard || die 'initial standard selector differs'
test ! -e "$config" || die 'initial standard created Starship config'

run_configurator starship
assert_metadata "$selector" root:workspace:640
assert_metadata "$config" root:workspace:640
test "$(cat "$selector")" = starship || die 'compatible selector differs'
cmp -s "$config" "$immutable_root/presets/starship.toml" ||
    die 'compatible generated config differs'

run_configurator starship-nerd-font
test "$(cat "$selector")" = starship-nerd-font || die 'Nerd selector differs'
cmp -s "$config" "$immutable_root/presets/starship-nerd-font.toml" ||
    die 'Nerd generated config differs'

run_configurator standard
test "$(cat "$selector")" = standard || die 'disable selector differs'
test ! -e "$config" || die 'disable retained generated config'

# Workspace can read state but cannot mutate, replace, or create transaction files.
/usr/sbin/runuser -u workspace -- cat "$selector" >/dev/null
if /usr/sbin/runuser -u workspace -- sh -c 'printf hacked >"$1"' sh "$selector" 2>/dev/null; then
    die 'workspace modified root-owned selector'
fi
if /usr/sbin/runuser -u workspace -- sh -c 'mv "$1" "$1.moved"' sh "$selector" 2>/dev/null; then
    die 'workspace renamed root-owned selector'
fi
if /usr/sbin/runuser -u workspace -- sh -c 'printf staged >"$1"' sh \
    "$shell_dir/.prompt.gascan-stage" 2>/dev/null; then
    die 'workspace created transaction staging state'
fi
test "$(cat "$selector")" = standard || die 'workspace mutation changed selector'

# Unsafe objects remain untouched and block the complete transaction.
outside=/tmp/gascan-shell-outside
printf 'outside survives\n' >"$outside"
rm -f "$selector"
ln -s "$outside" "$selector"
if run_configurator starship; then die 'selector symlink was accepted'; fi
test "$(cat "$outside")" = 'outside survives' || die 'selector symlink target changed'
rm -f "$selector"
install -o root -g workspace -m 0640 /dev/null "$selector"
printf 'standard\n' >"$selector"
chown workspace:workspace "$selector"
if run_configurator starship; then die 'workspace-owned selector was accepted'; fi
assert_metadata "$selector" workspace:workspace:640
chown root:workspace "$selector"
chmod 0644 "$selector"
if run_configurator starship; then die 'permissive selector was accepted'; fi
assert_metadata "$selector" root:workspace:644
chmod 0640 "$selector"
printf 'standard\n' >"$selector"
printf 'foreign\n' >"$shell_dir/notes"
chown root:workspace "$shell_dir/notes"
chmod 0640 "$shell_dir/notes"
if run_configurator starship; then die 'unexpected shell entry was accepted'; fi
test -f "$shell_dir/notes" || die 'unexpected entry was removed'
rm -f "$shell_dir/notes"

# The complete transaction blocks behind one root-owned advisory lock.
exec 9<>"$lock"
/usr/bin/flock -x 9
run_configurator starship &
blocked_pid=$!
lock_seen=
for ignored in $(seq 1 10000); do
    kill -0 "$blocked_pid" 2>/dev/null ||
        die 'configurator completed while the transaction lock was held'
    for fd in /proc/"$blocked_pid"/fd/*; do
        if test "$(readlink "$fd" 2>/dev/null || true)" = "$lock"; then
            lock_seen=1
            break 2
        fi
    done
done
test "$lock_seen" = 1 || die 'blocked configurator never opened the advisory lock'
test "$(cat "$selector")" = standard ||
    die 'blocked configurator mutated state before acquiring the lock'
/usr/bin/flock -u 9
wait "$blocked_pid"
exec 9>&-
test "$(cat "$selector")" = starship || die 'blocked transaction did not resume'

# Concurrent valid writers must leave one complete selector/config pair.
run_configurator starship &
first_pid=$!
run_configurator starship-nerd-font &
second_pid=$!
wait "$first_pid"
wait "$second_pid"
assert_selected_config

# Install a realistic two-phase Starship double and a PATH attacker.
install -d -o root -g root -m 0755 "$immutable_root/bin"
stable_log=/tmp/gascan-stable-starship.log
path_log=/tmp/gascan-path-starship.log
: >"$stable_log"
: >"$path_log"
chmod 0666 "$stable_log" "$path_log"
cat >"$immutable_root/bin/starship" <<'STARSHIP'
#!/bin/sh
printf '%s|%s|%s\n' "$*" "${STARSHIP_CONFIG-}" "${STARSHIP_EXECUTABLE-}" \
    >>"${GASCAN_TEST_STABLE_LOG:?}"
case "$*" in
    'init bash')
        printf '%s\n' 'eval "$(starship init bash --print-full-init)"'
        ;;
    'init bash --print-full-init')
        test "${GASCAN_TEST_GENERATION_FAIL:-0}" != 1 || exit 23
        if test "${GASCAN_TEST_EVAL_FAIL:-0}" = 1; then
            printf '%s\n' "PS1='broken-starship'; false"
        else
            cat <<'FULL_INIT'
__gascan_test_starship_runtime()
{
    "$STARSHIP_EXECUTABLE" prompt
}
PS1='managed-starship'
FULL_INIT
        fi
        ;;
    prompt)
        printf 'stable-runtime\n'
        ;;
    *)
        exit 64
        ;;
esac
STARSHIP
chmod 0555 "$immutable_root/bin/starship"
chmod 0555 "$immutable_root/bin" "$immutable_root"

attacker_bin=$home/.local/bin
install -d -o workspace -g workspace -m 0755 "$attacker_bin"
cat >"$attacker_bin/starship" <<'ATTACKER'
#!/bin/sh
printf '%s\n' "$*" >>"${GASCAN_TEST_PATH_LOG:?}"
printf '%s\n' "PS1='path-starship'"
ATTACKER
chown workspace:workspace "$attacker_bin/starship"
chmod 0755 "$attacker_bin/starship"

export GASCAN_TEST_STABLE_LOG=$stable_log
export GASCAN_TEST_PATH_LOG=$path_log
run_configurator starship

root_output=$(
    PATH="$attacker_bin:/usr/bin:/bin" /bin/bash --noprofile --norc -i -c \
        "PS1='native-root'; . '$hook'; __gascan_test_starship_runtime; \
         printf 'PS1=%s\nCONFIG=%s\nEXEC=%s\n' \
         \"\$PS1\" \"\$STARSHIP_CONFIG\" \"\$STARSHIP_EXECUTABLE\""
)
printf '%s\n' "$root_output" | grep -Fqx stable-runtime ||
    die 'root prompt runtime did not use stable Starship'
printf '%s\n' "$root_output" | grep -Fqx PS1=managed-starship ||
    die 'root prompt did not evaluate full init'
printf '%s\n' "$root_output" |
    grep -Fqx "CONFIG=$immutable_root/presets/starship.toml" ||
    die 'root prompt retained mutable STARSHIP_CONFIG'
printf '%s\n' "$root_output" |
    grep -Fqx "EXEC=$immutable_root/bin/starship" ||
    die 'root prompt did not retain immutable STARSHIP_EXECUTABLE'
test ! -s "$path_log" || die 'two-phase or runtime Starship resolved through user PATH'
grep -Fqx \
    "init bash --print-full-init|$immutable_root/presets/starship.toml|$immutable_root/bin/starship" \
    "$stable_log" || die 'full-init generation did not use the immutable command/config'
grep -Fqx \
    "prompt|$immutable_root/presets/starship.toml|$immutable_root/bin/starship" \
    "$stable_log" || die 'prompt runtime did not use the immutable command/config'

: >"$stable_log"
workspace_output=$(
    /usr/sbin/runuser -u workspace -- env \
        PATH="$attacker_bin:/usr/bin:/bin" \
        GASCAN_TEST_STABLE_LOG="$stable_log" \
        GASCAN_TEST_PATH_LOG="$path_log" \
        /bin/bash --noprofile --norc -i -c \
        "PS1='native-workspace'; . '$hook'; __gascan_test_starship_runtime; \
         printf 'PS1=%s\nCONFIG=%s\nEXEC=%s\n' \
         \"\$PS1\" \"\$STARSHIP_CONFIG\" \"\$STARSHIP_EXECUTABLE\""
)
printf '%s\n' "$workspace_output" | grep -Fqx stable-runtime ||
    die 'workspace prompt runtime did not use stable Starship'
printf '%s\n' "$workspace_output" |
    grep -Fqx "CONFIG=$config" ||
    die 'workspace prompt did not use managed root-owned config'
test ! -s "$path_log" || die 'workspace prompt resolved Starship through user PATH'

failure_output=$(
    GASCAN_TEST_GENERATION_FAIL=1 PATH="$attacker_bin:/usr/bin:/bin" \
        /bin/bash --noprofile --norc -i -c \
        "PS1='native-root'; . '$hook'; . '$hook'; printf 'PS1=%s\n' \"\$PS1\"" \
        2>&1
)
test "$(printf '%s\n' "$failure_output" |
    grep -Fc 'gascan: Starship prompt unavailable; using standard Bash prompt.')" = 1 ||
    die 'full-init generation failure did not warn exactly once'
printf '%s\n' "$failure_output" | grep -Fqx PS1=native-root ||
    die 'generation failure did not restore Bash prompt'

eval_failure_output=$(
    GASCAN_TEST_EVAL_FAIL=1 PATH="$attacker_bin:/usr/bin:/bin" \
        /bin/bash --noprofile --norc -i -c \
        "PS1='native-root'; . '$hook'; printf 'PS1=%s\n' \"\$PS1\"" 2>&1
)
printf '%s\n' "$eval_failure_output" |
    grep -Fq 'gascan: Starship prompt unavailable; using standard Bash prompt.' ||
    die 'full-init eval failure did not warn'
printf '%s\n' "$eval_failure_output" | grep -Fqx PS1=native-root ||
    die 'full-init eval failure did not restore Bash prompt'

# Bash strings cannot preserve NUL, so selector validation must compare bytes.
printf 'starship\0\n' >"$selector"
chown root:workspace "$selector"
chmod 0640 "$selector"
nul_log_before=$(wc -c <"$stable_log")
nul_output=$(
    PATH="$attacker_bin:/usr/bin:/bin" /bin/bash --noprofile --norc -i -c \
        "PS1='native-root'; . '$hook'; printf 'PS1=%s\n' \"\$PS1\"" 2>&1
)
printf '%s\n' "$nul_output" |
    grep -Fq 'gascan: Starship prompt unavailable; using standard Bash prompt.' ||
    die 'NUL selector did not fall back'
printf '%s\n' "$nul_output" | grep -Fqx PS1=native-root ||
    die 'NUL selector changed the prompt'
test "$(wc -c <"$stable_log")" = "$nul_log_before" ||
    die 'NUL selector invoked Starship'

printf 'shell-home-root-contract-ok\n'
