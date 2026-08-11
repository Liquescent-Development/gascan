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

# Both roots, and not just the library target. scripts/build-arca-engine.sh
# builds `--product arca-engine` and ships .build/release/arca-engine, so the
# executable target is the artifact this contract exists to certify; ArcaEngine
# is only what that executable happens to be made of today. Rooting at the
# library alone would certify a thing Gas Can does not ship while leaving the
# thing it does ship unchecked. Arca's Package.swift states the property for
# both targets, so the instrument covers both.
roots='["arca-engine", "ArcaEngine"]'

describe=$(swift package describe --package-path "$checkout" --type json)

# A renamed root would make every assertion below vacuously true, so the roots
# are required to exist before their closures are believed.
missing=$(printf '%s' "$describe" | jq -r --argjson roots "$roots" '
  [.targets[].name] as $names
  | [$roots[] | select(. as $r | $names | index($r) | not)]
  | join(" ")
')

if [ -n "$missing" ]; then
  printf 'contract root target(s) absent from the manifest: %s\n' "$missing" >&2
  printf 'the roots were renamed, or the manifest shape changed.\n' >&2
  exit 65
fi

# Walk each root's transitive target closure and report the path to any
# forbidden name, because "ArcaEngine reaches DockerAPI" does not say which of
# the two roots reached it or by which edge, and that is the first thing anyone
# reading the failure needs to know.
#
# Adjacency is a lookup object rather than a `select` over the target list on
# purpose. `($targets[] | select(.name == $name) | .deps) as $deps | ...` emits
# nothing at all when a name matches no target, which collapses the recursion to
# empty and prints PASS -- a silent pass is the one failure mode a contract test
# must not have. `$deps[$name] // []` always yields exactly one value.
violations=$(printf '%s' "$describe" | jq -r --argjson roots "$roots" '
  (reduce .targets[] as $target ({}; .[$target.name] = ($target.target_dependencies // []))) as $deps
  | def forbidden($name): $name == "DockerAPI" or $name == "ArcaDaemon";
    def search($frontier; $seen):
      if ($frontier | length) == 0 then []
      else ($frontier[0]) as $path
        | ($path[-1]) as $name
        | (($deps[$name] // []) | map(select(. as $dep | $seen | index($dep) | not))) as $fresh
        | (if forbidden($name) then [$path] else [] end)
          + search(($frontier[1:] + ($fresh | map($path + [.]))); ($seen + $fresh))
      end;
    [$roots[] | search([[.]]; [.])]
  | add
  | map(join(" -> "))
  | .[]
')

if [ -n "$violations" ]; then
  printf 'the engine reaches a forbidden target:\n' >&2
  printf '%s\n' "$violations" >&2
  exit 1
fi

printf 'PASS: neither arca-engine nor ArcaEngine reaches DockerAPI or ArcaDaemon\n'
