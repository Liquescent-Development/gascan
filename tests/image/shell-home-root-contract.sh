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
printf '127.0.0.1 %s\n' "$(/bin/hostname)" >>/etc/hosts
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
    /usr/sbin/runuser -u workspace -- env HOME=/tmp/hostile-home \
        /usr/bin/sudo -n /usr/local/bin/configure-shell-home "$1"
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

# Effective root is required, while the target home is compiled in and does
# not depend on sudo's reset environment or caller-controlled HOME.
if /usr/sbin/runuser -u workspace -- env HOME="$home" "$configurator" standard; then
    die 'non-root configurator invocation succeeded'
fi
test ! -e "$shell_dir" && test ! -e "$lock" ||
    die 'non-root rejection mutated managed state'
if /usr/sbin/runuser -u workspace -- \
    /usr/bin/sudo -n /usr/local/bin/configure-shell-home standard ignored; then
    die 'configurator accepted untrusted extra input'
fi
test ! -e "$shell_dir" && test ! -e "$lock" ||
    die 'extra-input rejection mutated managed state'
sudo_home=$(
    /usr/sbin/runuser -u workspace -- env HOME=/tmp/hostile-home \
        /usr/bin/sudo -n /bin/sh -c 'printf %s "$HOME"'
)
test "$sudo_home" = /root ||
    die "sudo did not reset HOME to /root (got $sudo_home)"

run_configurator standard
test ! -e /tmp/hostile-home ||
    die 'caller-controlled HOME was used as the managed target'
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

# Install the production relative symlink topology, a realistic two-phase
# Starship double at its immutable target, and a PATH attacker.
install -d -o root -g root -m 0555 "$immutable_root/bin"
stable_target=/opt/gascan/workstation/bin/starship
install -d -o root -g root -m 0555 /opt/gascan/workstation/bin
stable_log=/tmp/gascan-stable-starship.log
path_log=/tmp/gascan-path-starship.log
: >"$stable_log"
: >"$path_log"
chmod 0666 "$stable_log" "$path_log"
cat >"$stable_target" <<'STARSHIP'
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
            cat <<'PARTIAL_INIT'
PS1='broken-starship'
PS2='broken-continuation'
PROMPT_COMMAND='broken-prompt-command'
STARSHIP_PARTIAL='leaked-variable'
starship_partial_leak()
{
    printf leaked
}
trap 'STARSHIP_TRAP_LEAK=1' DEBUG
false
true
PARTIAL_INIT
        else
            if test "${GASCAN_TEST_SIGNAL_COLLISION:-0}" = 1; then
                printf '%s\n' 'kill -USR1 "$GASCAN_TEST_PARENT_PID"'
            fi
            cat <<'FULL_INIT'
_starship_set_return()
{
    return "${1:-0}"
}
starship_preexec()
{
    :
}
starship_preexec_all()
{
    :
}
starship_preexec_ps0()
{
    :
}
starship_precmd()
{
    if test -n "${STARSHIP_PROMPT_COMMAND:-}"; then
        eval "$STARSHIP_PROMPT_COMMAND"
    fi
    "$STARSHIP_EXECUTABLE" prompt
}
STARSHIP_START_TIME=1
STARSHIP_SHELL=bash
STARSHIP_SESSION_KEY=1234567890123456
PS0='managed-ps0'
PS1='managed-starship'
PS2='managed-continuation'
if test -n "${PROMPT_COMMAND:-}" &&
    [[ "$PROMPT_COMMAND" != *"starship_precmd"* ]]; then
    STARSHIP_PROMPT_COMMAND=$PROMPT_COMMAND
fi
PROMPT_COMMAND=starship_precmd
shopt -s checkwinsize
trap ':' DEBUG
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
chown root:root "$stable_target"
chmod 0555 "$stable_target"
ln -s ../../workstation/bin/starship "$immutable_root/bin/starship"
chown -h root:root "$immutable_root/bin/starship"
chmod 0555 "$immutable_root/bin" "$immutable_root"
test "$(readlink "$immutable_root/bin/starship")" = ../../workstation/bin/starship ||
    die 'stable Starship link topology differs from production'
assert_metadata "$stable_target" root:root:555

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

chmod 0755 "$stable_target"
unsafe_target_output=$(
    PATH="$attacker_bin:/usr/bin:/bin" /bin/bash --noprofile --norc -i -c \
        "PS1='native-root'; . '$hook'; printf 'PS1=%s\n' \"\$PS1\"" 2>&1
)
printf '%s\n' "$unsafe_target_output" | grep -Fqx PS1=native-root ||
    die 'unsafe stable-link target changed the root prompt'
printf '%s\n' "$unsafe_target_output" |
    grep -Fq 'gascan: Starship prompt unavailable; using standard Bash prompt.' ||
    die 'unsafe stable-link target did not fail closed'
test ! -s "$stable_log" ||
    die 'hook executed a stable-link target with the wrong mode'
chmod 0555 "$stable_target"

root_output=$(
    PATH="$attacker_bin:/usr/bin:/bin" /bin/bash --noprofile --norc -i -c \
        "PS1='native-root'; . '$hook'; starship_precmd; \
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
test "$(grep -Fc 'init bash --print-full-init' "$stable_log")" = 1 ||
    die 'root full init executed more than once'

: >"$stable_log"
workspace_output=$(
    /usr/sbin/runuser -u workspace -- env \
        PATH="$attacker_bin:/usr/bin:/bin" \
        GASCAN_TEST_STABLE_LOG="$stable_log" \
        GASCAN_TEST_PATH_LOG="$path_log" \
        /bin/bash --noprofile --norc -i -c \
        "PS1='native-workspace'; . '$hook'; starship_precmd; \
         printf 'PS1=%s\nCONFIG=%s\nEXEC=%s\n' \
         \"\$PS1\" \"\$STARSHIP_CONFIG\" \"\$STARSHIP_EXECUTABLE\""
)
printf '%s\n' "$workspace_output" | grep -Fqx stable-runtime ||
    die 'workspace prompt runtime did not use stable Starship'
printf '%s\n' "$workspace_output" |
    grep -Fqx "CONFIG=$config" ||
    die 'workspace prompt did not use managed root-owned config'
test ! -s "$path_log" || die 'workspace prompt resolved Starship through user PATH'
test "$(grep -Fc 'init bash --print-full-init' "$stable_log")" = 1 ||
    die 'workspace full init executed more than once'

custom_log=/tmp/gascan-prompt-customization.log
custom_output=$(
    GASCAN_TEST_STABLE_LOG="$custom_log" \
        PATH="$attacker_bin:/usr/bin:/bin" \
        /bin/bash --noprofile --norc -i -c \
        "PS1='native-root'; \
         caller_prompt_command() { printf 'caller-customization\n'; }; \
         PROMPT_COMMAND=caller_prompt_command; . '$hook'; \
         printf 'COMMAND=%s\nPRESERVED=%s\n' \
         \"\$PROMPT_COMMAND\" \"\$STARSHIP_PROMPT_COMMAND\"; \
         starship_precmd"
)
printf '%s\n' "$custom_output" | grep -Fqx COMMAND=starship_precmd ||
    die 'managed prompt did not install its prompt command'
printf '%s\n' "$custom_output" |
    grep -Fqx PRESERVED=caller_prompt_command ||
    die 'managed prompt did not preserve supported prompt customization'
printf '%s\n' "$custom_output" | grep -Fqx caller-customization ||
    die 'managed prompt did not execute supported prompt customization'
printf '%s\n' "$custom_output" | grep -Fqx stable-runtime ||
    die 'prompt customization prevented managed prompt runtime'
test "$(grep -Fc 'init bash --print-full-init' "$custom_log")" = 1 ||
    die 'customized prompt full init executed more than once'

debug_output=$(
    PATH="$attacker_bin:/usr/bin:/bin" /bin/bash --noprofile --norc -i -c \
        "PS1='native-root'; set -T; \
         trap 'if [ \"\$BASH_SUBSHELL\" -gt 0 ]; then starship_precmd() { printf attacker; }; STARSHIP_START_TIME=attacker; fi' DEBUG; \
         before=\$(trap -p DEBUG); . '$hook'; after=\$(trap -p DEBUG); \
         declare -F starship_precmd >/dev/null && function=present || function=absent; \
         printf 'PS1=%s\nFUNCTION=%s\nSTART=%s\nBEFORE=%s\nAFTER=%s\n' \
         \"\$PS1\" \"\$function\" \"\${STARSHIP_START_TIME-unset}\" \
         \"\$before\" \"\$after\"" 2>&1
)
printf '%s\n' "$debug_output" | grep -Fqx PS1=native-root ||
    die 'inherited DEBUG trap changed the native prompt'
printf '%s\n' "$debug_output" | grep -Fqx FUNCTION=absent ||
    die 'inherited DEBUG trap serialized an attacker function'
printf '%s\n' "$debug_output" | grep -Fqx START=unset ||
    die 'inherited DEBUG trap serialized attacker state'
test "$(printf '%s\n' "$debug_output" | grep -Fc \
    "trap -- 'if [ \"\$BASH_SUBSHELL\" -gt 0 ]; then starship_precmd() { printf attacker; }; STARSHIP_START_TIME=attacker; fi' DEBUG")" = 2 ||
    die 'inherited DEBUG trap was not preserved exactly'
test "$(printf '%s\n' "$debug_output" |
    grep -Fc 'gascan: Starship prompt unavailable; using standard Bash prompt.')" = 1 ||
    die 'inherited DEBUG trap did not warn exactly once'
test "$(grep -Fc 'init bash --print-full-init' "$stable_log")" = 1 ||
    die 'inherited DEBUG trap reached Starship'

spoofed_debug_output=$(
    PATH="$attacker_bin:/usr/bin:/bin" /bin/bash --noprofile --norc -i -c \
        "PS1='native-root'; set -T; \
         builtin trap 'trap() { :; }' DEBUG; \
         before=\$(builtin trap -p DEBUG); . '$hook'; \
         after=\$(builtin trap -p DEBUG); \
         declare -F trap >/dev/null && spoof=present || spoof=absent; \
         printf 'PS1=%s\nSPOOF=%s\nBEFORE=%s\nAFTER=%s\n' \
         \"\$PS1\" \"\$spoof\" \"\$before\" \"\$after\"" 2>&1
)
printf '%s\n' "$spoofed_debug_output" | grep -Fqx PS1=native-root ||
    die 'spoofed inherited DEBUG trap changed the native prompt'
printf '%s\n' "$spoofed_debug_output" | grep -Fqx SPOOF=present ||
    die 'spoofed inherited DEBUG trap did not shadow trap'
test "$(printf '%s\n' "$spoofed_debug_output" |
    grep -Fc "trap -- 'trap() { :; }' DEBUG")" = 2 ||
    die 'spoofed inherited DEBUG trap was not preserved exactly'
test "$(printf '%s\n' "$spoofed_debug_output" |
    grep -Fc 'gascan: Starship prompt unavailable; using standard Bash prompt.')" = 1 ||
    die 'spoofed inherited DEBUG trap did not warn exactly once'
test "$(grep -Fc 'init bash --print-full-init' "$stable_log")" = 1 ||
    die 'spoofed inherited DEBUG trap reached Starship'

self_clearing_debug_output=$(
    PATH="$attacker_bin:/usr/bin:/bin" /bin/bash --noprofile --norc -i -c \
        "PS1='native-root'; set -T; \
         builtin trap 'builtin trap - DEBUG; STARSHIP_START_TIME=attacker' DEBUG; \
         . '$hook'; printf 'PS1=%s\nSTART=%s\nTRAP=%s\n' \
         \"\$PS1\" \"\${STARSHIP_START_TIME-unset}\" \
         \"\$(builtin trap -p DEBUG)\"" 2>&1
)
printf '%s\n' "$self_clearing_debug_output" | grep -Fqx PS1=native-root ||
    die 'self-clearing DEBUG trap changed the native prompt'
printf '%s\n' "$self_clearing_debug_output" | grep -Fqx START=attacker ||
    die 'self-clearing DEBUG trap mutation was not preserved'
printf '%s\n' "$self_clearing_debug_output" | grep -Fqx TRAP= ||
    die 'self-clearing DEBUG trap remained active'
test "$(printf '%s\n' "$self_clearing_debug_output" |
    grep -Fc 'gascan: Starship prompt unavailable; using standard Bash prompt.')" = 1 ||
    die 'self-clearing DEBUG mutation did not warn exactly once'
test "$(grep -Fc 'init bash --print-full-init' "$stable_log")" = 1 ||
    die 'self-clearing DEBUG mutation reached Starship'

signal_output=$(
    GASCAN_TEST_SIGNAL_COLLISION=1 \
        PATH="$attacker_bin:/usr/bin:/bin" \
        /bin/bash --noprofile --norc -i -c \
        "PS1='native-root'; \
         trap 'starship_precmd() { printf attacker; }; readonly -f starship_precmd' USR1; \
         export GASCAN_TEST_PARENT_PID=\$\$; . '$hook'; \
         printf 'PS1=%s\nFUNCTION=' \"\$PS1\"; starship_precmd; printf '\n'" 2>&1
)
printf '%s\n' "$signal_output" | grep -Fqx PS1=native-root ||
    die 'readonly post-preflight function changed the native prompt'
printf '%s\n' "$signal_output" | grep -Fqx FUNCTION=attacker ||
    die 'readonly post-preflight function was replaced'
test "$(printf '%s\n' "$signal_output" |
    grep -Fc 'gascan: Starship prompt unavailable; using standard Bash prompt.')" = 1 ||
    die 'readonly function apply failure did not warn exactly once'
test "$(grep -Fc 'init bash --print-full-init' "$stable_log")" = 2 ||
    die 'readonly function race did not stop after one generated init'

writable_signal_output=$(
    GASCAN_TEST_SIGNAL_COLLISION=1 \
        PATH="$attacker_bin:/usr/bin:/bin" \
        /bin/bash --noprofile --norc -i -c \
        "PS1='native-root'; \
         trap 'starship_precmd() { printf attacker; }' USR1; \
         export GASCAN_TEST_PARENT_PID=\$\$; . '$hook'; \
         printf 'PS1=%s\nFUNCTION=' \"\$PS1\"; starship_precmd; printf '\n'" 2>&1
)
printf '%s\n' "$writable_signal_output" | grep -Fqx PS1=native-root ||
    die 'writable post-preflight function changed the native prompt'
printf '%s\n' "$writable_signal_output" | grep -Fqx FUNCTION=attacker ||
    die 'writable post-preflight function was replaced'
test "$(printf '%s\n' "$writable_signal_output" |
    grep -Fc 'gascan: Starship prompt unavailable; using standard Bash prompt.')" = 1 ||
    die 'writable function apply collision did not warn exactly once'
test "$(grep -Fc 'init bash --print-full-init' "$stable_log")" = 3 ||
    die 'writable function race did not stop after one generated init'

readonly_output=$(
    PATH="$attacker_bin:/usr/bin:/bin" /bin/bash --noprofile --norc -i -c \
        "PS1='native-root'; \
         STARSHIP_CONFIG='readonly-config'; readonly STARSHIP_CONFIG; \
         STARSHIP_EXECUTABLE='readonly-executable'; readonly STARSHIP_EXECUTABLE; \
         readonly PATH; . '$hook'; \
         printf 'SURVIVED=%s\nPS1=%s\n' yes \"\$PS1\"" 2>&1
)
printf '%s\n' "$readonly_output" | grep -Fqx SURVIVED=yes ||
    die 'readonly managed variables aborted shell startup'
printf '%s\n' "$readonly_output" | grep -Fqx PS1=native-root ||
    die 'readonly managed variables changed the native prompt'
test "$(printf '%s\n' "$readonly_output" |
    grep -Fc 'gascan: Starship prompt unavailable; using standard Bash prompt.')" = 1 ||
    die 'readonly managed variables did not warn exactly once'
test "$(grep -Fc 'init bash --print-full-init' "$stable_log")" = 3 ||
    die 'readonly managed variables reached Starship'

collision_output=$(
    PATH="$attacker_bin:/usr/bin:/bin" /bin/bash --noprofile --norc -i -c \
        "PS1='native-root'; STARSHIP_SHELL=attacker; \
         starship_precmd() { printf attacker; }; \
         . '$hook'; printf 'FUNCTION='; starship_precmd; \
         printf '\nVARIABLE=%s\nPS1=%s\n' \"\$STARSHIP_SHELL\" \"\$PS1\"" 2>&1
)
printf '%s\n' "$collision_output" | grep -Fqx FUNCTION=attacker ||
    die 'managed function collision was replaced'
printf '%s\n' "$collision_output" | grep -Fqx VARIABLE=attacker ||
    die 'managed internal-variable collision was replaced'
printf '%s\n' "$collision_output" | grep -Fqx PS1=native-root ||
    die 'managed provenance collision changed the native prompt'
test "$(grep -Fc 'init bash --print-full-init' "$stable_log")" = 3 ||
    die 'managed provenance collision reached Starship'

failure_output=$(
    GASCAN_TEST_GENERATION_FAIL=1 PATH="$attacker_bin:/usr/bin:/bin" \
        /bin/bash --noprofile --norc -i -c \
        "PS1='native-root'; \
         STARSHIP_CONFIG='preexisting-config'; \
         STARSHIP_EXECUTABLE='preexisting-executable'; \
         . '$hook'; . '$hook'; \
         declare -p __gascan_starship_euid >/dev/null 2>&1 && helper=set || helper=unset; \
         printf 'PS1=%s\nCONFIG=%s\nEXEC=%s\nCONFIG_DECL=%s\nEXEC_DECL=%s\nHELPER=%s\n' \
         \"\$PS1\" \"\$STARSHIP_CONFIG\" \"\$STARSHIP_EXECUTABLE\" \
         \"\$(declare -p STARSHIP_CONFIG)\" \
         \"\$(declare -p STARSHIP_EXECUTABLE)\" \"\$helper\"" \
        2>&1
)
test "$(printf '%s\n' "$failure_output" |
    grep -Fc 'gascan: Starship prompt unavailable; using standard Bash prompt.')" = 1 ||
    die 'full-init generation failure did not warn exactly once'
printf '%s\n' "$failure_output" | grep -Fqx PS1=native-root ||
    die 'generation failure did not restore Bash prompt'
printf '%s\n' "$failure_output" | grep -Fqx CONFIG=preexisting-config ||
    die 'generation failure did not restore pre-existing STARSHIP_CONFIG'
printf '%s\n' "$failure_output" | grep -Fqx EXEC=preexisting-executable ||
    die 'generation failure did not restore pre-existing STARSHIP_EXECUTABLE'
printf '%s\n' "$failure_output" |
    grep -Fqx 'CONFIG_DECL=declare -- STARSHIP_CONFIG="preexisting-config"' ||
    die 'generation failure changed STARSHIP_CONFIG export state'
printf '%s\n' "$failure_output" |
    grep -Fqx 'EXEC_DECL=declare -- STARSHIP_EXECUTABLE="preexisting-executable"' ||
    die 'generation failure changed STARSHIP_EXECUTABLE export state'
printf '%s\n' "$failure_output" | grep -Fqx HELPER=unset ||
    die 'generation failure leaked the Gas Can EUID helper'

eval_failure_output=$(
    GASCAN_TEST_EVAL_FAIL=1 PATH="$attacker_bin:/usr/bin:/bin" \
        /bin/bash --noprofile --norc -i -c \
        "PS1='native-root'; PS2='native-continuation'; \
         PROMPT_COMMAND='native-prompt-command'; \
         STARSHIP_CONFIG='preexisting-config'; \
         STARSHIP_EXECUTABLE='preexisting-executable'; \
         trap 'STARSHIP_NATIVE_TRAP=1' DEBUG; \
         . '$hook'; \
         declare -F starship_partial_leak >/dev/null && printf 'FUNCTION=leaked\n'; \
         declare -p __gascan_starship_euid >/dev/null 2>&1 && helper=set || helper=unset; \
         printf 'PS1=%s\nPS2=%s\nPROMPT=%s\nCONFIG=%s\nEXEC=%s\nPARTIAL=%s\nTRAP=%s\nCONFIG_DECL=%s\nEXEC_DECL=%s\nHELPER=%s\n' \
         \"\$PS1\" \"\$PS2\" \"\$PROMPT_COMMAND\" \
         \"\$STARSHIP_CONFIG\" \"\$STARSHIP_EXECUTABLE\" \
         \"\${STARSHIP_PARTIAL-unset}\" \"\$(trap -p DEBUG)\" \
         \"\$(declare -p STARSHIP_CONFIG)\" \
         \"\$(declare -p STARSHIP_EXECUTABLE)\" \"\$helper\"" 2>&1
)
printf '%s\n' "$eval_failure_output" |
    grep -Fq 'gascan: Starship prompt unavailable; using standard Bash prompt.' ||
    die 'full-init eval failure did not warn'
printf '%s\n' "$eval_failure_output" | grep -Fqx PS1=native-root ||
    die 'full-init eval failure did not restore Bash prompt'
printf '%s\n' "$eval_failure_output" | grep -Fqx PS2=native-continuation ||
    die 'partial init leaked continuation prompt state'
printf '%s\n' "$eval_failure_output" |
    grep -Fqx PROMPT=native-prompt-command ||
    die 'partial init leaked PROMPT_COMMAND state'
printf '%s\n' "$eval_failure_output" | grep -Fqx CONFIG=preexisting-config ||
    die 'eval failure did not restore pre-existing STARSHIP_CONFIG'
printf '%s\n' "$eval_failure_output" | grep -Fqx EXEC=preexisting-executable ||
    die 'eval failure did not restore pre-existing STARSHIP_EXECUTABLE'
printf '%s\n' "$eval_failure_output" |
    grep -Fqx 'CONFIG_DECL=declare -- STARSHIP_CONFIG="preexisting-config"' ||
    die 'eval failure changed STARSHIP_CONFIG export state'
printf '%s\n' "$eval_failure_output" |
    grep -Fqx 'EXEC_DECL=declare -- STARSHIP_EXECUTABLE="preexisting-executable"' ||
    die 'eval failure changed STARSHIP_EXECUTABLE export state'
printf '%s\n' "$eval_failure_output" | grep -Fqx PARTIAL=unset ||
    die 'partial init leaked a Starship variable'
test "$(printf '%s\n' "$eval_failure_output" | grep -Fc FUNCTION=leaked)" = 0 ||
    die 'partial init leaked a Starship function'
printf '%s\n' "$eval_failure_output" |
    grep -Fq "trap -- 'STARSHIP_NATIVE_TRAP=1' DEBUG" ||
    die 'partial init replaced the existing DEBUG trap'
printf '%s\n' "$eval_failure_output" | grep -Fqx HELPER=unset ||
    die 'eval failure leaked the Gas Can EUID helper'
full_init_count=$(grep -Fc 'init bash --print-full-init' "$stable_log")
test "$full_init_count" = 6 ||
    die "full init invocation count is $full_init_count, expected 6"

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
