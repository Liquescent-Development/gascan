#!/bin/sh
# The engine must not reach Arca's Docker surface, transitively or directly.
#
# This is the property that makes "Gas Can builds only the targets it ships"
# checkable rather than aspirational. Contract §2 states why: a Docker-shaped
# API on the engine socket is a policy-bypass surface sitting beside the policy
# gate. An edge added to make something compile would forfeit that silently.
set -eu

checkout=${1:?usage: engine-targets-contract.sh <arca-checkout>}

for command in swift jq; do
  command -v "$command" >/dev/null || {
    printf 'required command is unavailable: %s\n' "$command" >&2
    exit 69
  }
done

describe=$(swift package describe --package-path "$checkout" --type json)

# Walk ArcaEngine's transitive target closure and fail on either forbidden name.
forbidden=$(printf '%s' "$describe" | jq -r '
  [.targets[] | {name: .name, deps: (.target_dependencies // [])}] as $targets
  | def closure($frontier; $seen):
      if ($frontier | length) == 0 then $seen
      else ($frontier[0]) as $name
        | ($targets[] | select(.name == $name) | .deps) as $deps
        | closure(($frontier[1:] + [$deps[]? | select(. as $d | $seen | index($d) | not)]);
                  ($seen + [$name]))
      end;
    closure(["ArcaEngine"]; [])
  | map(select(. == "DockerAPI" or . == "ArcaDaemon"))
  | unique
  | join(" ")
')

if [ -n "$forbidden" ]; then
  printf 'ArcaEngine reaches forbidden target(s): %s\n' "$forbidden" >&2
  exit 1
fi

printf 'PASS: ArcaEngine reaches neither DockerAPI nor ArcaDaemon\n'
