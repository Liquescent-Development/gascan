export MISE_DATA_DIR=/opt/gascan/mise
export MISE_CACHE_DIR=/home/workspace/.cache/mise
export MISE_GLOBAL_CONFIG_FILE=/etc/mise/config.toml
export PATH="$MISE_DATA_DIR/shims:$PATH"

case $- in
  *i*)
    if [ -n "${BASH_VERSION:-}" ]; then
      eval "$(mise activate bash)"
    fi
    ;;
esac
