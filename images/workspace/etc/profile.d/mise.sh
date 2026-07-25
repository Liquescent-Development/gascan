export MISE_DATA_DIR=${MISE_DATA_DIR:-/opt/gascan/mise}
export MISE_SYSTEM_DATA_DIR=${MISE_SYSTEM_DATA_DIR:-/opt/gascan/mise}
export MISE_CACHE_DIR=${MISE_CACHE_DIR:-/home/workspace/.cache/mise}
export MISE_GLOBAL_CONFIG_FILE=${MISE_GLOBAL_CONFIG_FILE:-/etc/mise/config.toml}
if [ "$MISE_DATA_DIR" = "$MISE_SYSTEM_DATA_DIR" ]; then
  export PATH="$MISE_DATA_DIR/shims:/usr/local/bin:/opt/gascan/workstation/bin:$PATH"
else
  export PATH="$MISE_DATA_DIR/shims:$MISE_SYSTEM_DATA_DIR/shims:/usr/local/bin:/opt/gascan/workstation/bin:$PATH"
fi

case $- in
  *i*)
    if [ -n "${BASH_VERSION:-}" ]; then
      eval "$(mise activate bash)"
    fi
    ;;
esac
