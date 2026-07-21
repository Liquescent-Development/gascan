#!/bin/bash
set -euo pipefail

test "$(id -u)" -eq 0 || { echo 'install helper as root' >&2; exit 1; }
source_binary=${1:?usage: install-snapshot-helper.sh COMPILED_HELPER EXPECTED_SHA256 EXPECTED_SUDOERS_SHA256}
expected_sha256=${2:?usage: install-snapshot-helper.sh COMPILED_HELPER EXPECTED_SHA256 EXPECTED_SUDOERS_SHA256}
expected_sudoers_sha256=${3:?usage: install-snapshot-helper.sh COMPILED_HELPER EXPECTED_SHA256 EXPECTED_SUDOERS_SHA256}
destination='/Library/PrivilegedHelperTools/dev.gascan.snapshot-workspace-context'
sudoers_source=$(cd "$(dirname "$0")" && pwd -P)/snapshot-workspace-context.sudoers
sudoers_destination='/etc/sudoers.d/dev.gascan.snapshot-workspace-context'
test -f "$source_binary" && test ! -L "$source_binary"
actual_sha256=$(shasum -a 256 "$source_binary" | awk '{print $1}')
test "$actual_sha256" = "$expected_sha256" || { echo 'compiled helper digest mismatch' >&2; exit 1; }
install -d -o root -g wheel -m 0755 /Library/PrivilegedHelperTools
install -o root -g wheel -m 0555 "$source_binary" "$destination.new"
install -o root -g wheel -m 0440 "$sudoers_source" "$sudoers_destination.new"
test "$(shasum -a 256 "$destination.new" | awk '{print $1}')" = "$expected_sha256" || { echo 'staged helper digest mismatch' >&2; exit 1; }
test "$(shasum -a 256 "$sudoers_destination.new" | awk '{print $1}')" = "$expected_sudoers_sha256" || { echo 'staged sudoers digest mismatch' >&2; exit 1; }
visudo -cf "$sudoers_destination.new"
mv -f "$destination.new" "$destination"
mv -f "$sudoers_destination.new" "$sudoers_destination"
visudo -cf "$sudoers_destination"
echo "installed $destination and validated $sudoers_destination"
