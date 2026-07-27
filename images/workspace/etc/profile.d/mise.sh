export XDG_DATA_HOME=${XDG_DATA_HOME:-/home/workspace/.local/share}
export XDG_CACHE_HOME=${XDG_CACHE_HOME:-/home/workspace/.cache}
export XDG_CONFIG_HOME=${XDG_CONFIG_HOME:-/home/workspace/.config}
export MISE_DATA_DIR=${MISE_DATA_DIR:-/home/workspace/.local/share/mise}
export MISE_SYSTEM_DATA_DIR=${MISE_SYSTEM_DATA_DIR:-/opt/gascan/mise}
export MISE_CACHE_DIR=${MISE_CACHE_DIR:-/home/workspace/.cache/mise}
export MISE_GLOBAL_CONFIG_FILE=${MISE_GLOBAL_CONFIG_FILE:-/home/workspace/.config/gascan/mise.toml}
export MISE_SYSTEM_CONFIG_FILE=${MISE_SYSTEM_CONFIG_FILE:-/etc/mise/config.toml}
export MISE_STATE_DIR=${MISE_STATE_DIR:-/home/workspace/.config/gascan/mise-state}
export CARGO_HOME=${CARGO_HOME:-/home/workspace/.local/share/cargo}
export MISE_CARGO_HOME=${MISE_CARGO_HOME:-/home/workspace/.local/share/cargo}
export RUSTUP_HOME=${RUSTUP_HOME:-/home/workspace/.local/share/rustup}
export MISE_RUSTUP_HOME=${MISE_RUSTUP_HOME:-/home/workspace/.local/share/rustup}
export NPM_CONFIG_PREFIX=${NPM_CONFIG_PREFIX:-/home/workspace/.local}
export NPM_CONFIG_CACHE=${NPM_CONFIG_CACHE:-/home/workspace/.cache/npm}
export GOPATH=${GOPATH:-/home/workspace/.local/share/go}
export GOCACHE=${GOCACHE:-/home/workspace/.cache/go-build}
export GOMODCACHE=${GOMODCACHE:-/home/workspace/.cache/go-mod}
export PYTHONUSERBASE=${PYTHONUSERBASE:-/home/workspace/.local}
export GEM_HOME=${GEM_HOME:-/home/workspace/.local/share/gem}
export MIX_HOME=${MIX_HOME:-/home/workspace/.local/share/mix}
export HEX_HOME=${HEX_HOME:-/home/workspace/.local/share/hex}
export REBAR_CACHE_DIR=${REBAR_CACHE_DIR:-/home/workspace/.cache/rebar3}
export PATH=/home/workspace/.local/bin:/home/workspace/.local/share/cargo/bin:/home/workspace/.local/share/go/bin:/home/workspace/.local/share/gem/bin:/home/workspace/.local/share/mise/shims:/opt/gascan/mise/shims:/usr/local/sbin:/usr/local/bin:/opt/gascan/workstation/bin:/usr/sbin:/usr/bin:/sbin:/bin

case $- in
  *i*)
    if [ -n "${BASH_VERSION:-}" ]; then
      eval "$(mise activate bash)"
    fi
    ;;
esac
