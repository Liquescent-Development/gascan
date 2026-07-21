#!/bin/sh
set -eu

test "$(uname -s)" = Darwin
test "$(uname -m)" = arm64
printf 'macOS: %s\n' "$(sw_vers -productVersion)"
printf 'architecture: %s\n' "$(uname -m)"
container system version --format json
