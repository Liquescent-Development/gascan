#!/usr/bin/env bash
set -euo pipefail

outside=${1:?synthetic outside sentinel path required}
host_user=${2:?host user name required}

test "$(id -u)" = 1000
test "$(cat /workspace/workspace-sentinel)" = workspace-visible
test ! -r "$outside"
ln -s "$outside" /workspace/canonical-escape
test ! -r /workspace/canonical-escape
test ! -r "/Users/$host_user/.ssh/id_ed25519"
test ! -r "/Users/$host_user/.aws/credentials"
test ! -S /var/run/docker.sock
test ! -S /run/host-services/ssh-auth.sock
test -z "${GASCAN_SECURITY_SENTINEL+x}"
test "$(sudo -n id -u)" = 0
