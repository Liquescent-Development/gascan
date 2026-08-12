#!/bin/sh
# The engine must not reach Arca's Docker surface, transitively or directly.
#
# This is the property that makes "Gas Can builds only the targets it ships"
# checkable rather than aspirational. Contract §2 states why: a Docker-shaped
# API on the engine socket is a policy-bypass surface sitting beside the policy
# gate. An edge added to make something compile would forfeit that silently.
#
# Named `-check` and not `-contract`: scripts/ci-run-release-contracts.sh globs
# `tests/release/*-contract.sh` and runs each with no arguments, and this one
# needs an Arca checkout to inspect. It is invoked explicitly by ci.yml's engine
# job, which is the only place an Arca tree exists, and by the release checklist
# in docs/release/releasing.md.
#
# Not side-effect-free on its subject: `swift package describe` can create
# .build/ and rewrite Package.resolved in the checkout it inspects. Point it at
# a tree where that is acceptable.
set -eu

if [ $# -lt 1 ]; then
  printf 'usage: engine-targets-check.sh <arca-checkout>\n' >&2
  exit 64
fi
checkout=$1

for command in swift jq; do
  command -v "$command" >/dev/null || {
    printf 'required command is unavailable: %s\n' "$command" >&2
    exit 69
  }
done

# Both roots, and not just the library target. scripts/build-arca-engine.sh
# builds `--product arca-engine` and ships .build/release/arca-engine, so the
# executable target is the artifact this check exists to certify; ArcaEngine is
# only what that executable happens to be made of today. Rooting at the library
# alone would certify a thing Gas Can does not ship while leaving the thing it
# does ship unchecked. Arca's Package.swift states the property for both
# targets, so the instrument covers both.
roots='["arca-engine", "ArcaEngine"]'

# The names the roots must not reach, in one place because two things read them:
# the liveness guard immediately below and the `forbidden` predicate in the walk.
# Two copies of a list like this is how one of them silently stops matching.
banned='["DockerAPI", "ArcaDaemon"]'

describe=$(swift package describe --package-path "$checkout" --type json)

# A renamed root would make every assertion below vacuously true, so the roots
# are required to exist before their closures are believed. The forbidden names
# need exactly the same guard for exactly the same reason, and did not have it:
# they are matched by literal string, against names nothing required to be
# present. EXECUTED against a scratch package whose only change was renaming
# DockerAPI to DockerHTTPAPI -- with the engine still reaching it, this script
# printed `PASS: neither arca-engine nor ArcaEngine reaches DockerAPI or
# ArcaDaemon` and exited 0. Arca is a separate repository whose renames Gas Can
# does not review, so that is not a hypothetical.
missing=$(printf '%s' "$describe" | jq -r --argjson roots "$roots" --argjson banned "$banned" '
  [.targets[].name] as $names
  | [($roots + $banned)[] | select(. as $wanted | $names | index($wanted) | not)]
  | join(" ")
')

if [ -n "$missing" ]; then
  printf 'target(s) this check depends on are absent from the manifest: %s\n' "$missing" >&2
  printf 'the roots or the forbidden targets were renamed, or the manifest\n' >&2
  printf 'shape changed; this check is not measuring anything.\n' >&2
  exit 65
fi

# One walk, two derivations: `search` returns the path to every reachable node,
# from which both the positive control and the violation list are read off. A
# second walk would be a second thing to keep correct.
#
# Adjacency is a `reduce`-built lookup object rather than a `select` over the
# target list on purpose. `($targets[] | select(.name == $name) | .deps) as
# $deps | ...` emits nothing at all when a name matches no target, which
# collapses the recursion to empty and prints PASS -- a silent pass is the one
# failure mode this check must not have. `$graph[$name] // $empty` always
# yields exactly one value.
#
# Product dependencies are checked but not walked: `describe` reports only this
# package's targets, so there is nothing to walk into. Checking them costs one
# line and catches the day someone moves DockerAPI into its own SwiftPM package
# *under the same name*, after which a target-only check would go silently
# green. It does not catch a rename on the way out -- a product called
# ArcaDockerKit re-exporting the same code passes -- and the liveness guard
# above cannot help here, because a forbidden product need not exist today.
# Stated rather than implied, so nobody reads this line as more than it is.
#
# MANIFEST_SHAPE_CHANGED rather than an empty result when the positive control
# fails: if `target_dependencies` is ever renamed or dropped, every adjacency
# list becomes [] and every closure collapses to its root, which is a green run
# that proves nothing. The root-existence guard above does not catch it, because
# the root names still exist. A known-true edge must therefore be present.
findings=$(printf '%s' "$describe" | jq -r --argjson roots "$roots" --argjson banned "$banned" '
  ({targets: [], products: []}) as $empty
  | (reduce .targets[] as $target ({};
      .[$target.name] = {
        targets: ($target.target_dependencies // []),
        products: ($target.product_dependencies // [])
      })) as $graph
  | def forbidden($name): ($banned | index($name)) != null;
    def search($frontier; $seen):
      if ($frontier | length) == 0 then []
      else ($frontier[0]) as $path
        | ($path[-1]) as $name
        | (($graph[$name] // $empty).targets
           | map(select(. as $dep | $seen | index($dep) | not))) as $fresh
        | [$path]
          + search(($frontier[1:] + ($fresh | map($path + [.]))); ($seen + $fresh))
      end;
    [$roots[] | search([[.]]; [.])] as $walks
  | ($walks | add) as $paths
  | ($paths | map(select((.[0] == "arca-engine") and (.[-1] == "ArcaEngine"))) | length > 0) as $control
  | ($paths
     | map(. as $path | ($path[-1]) as $name
           | (if forbidden($name) then [$path] else [] end)
             + (($graph[$name] // $empty).products
                | map(select(forbidden(.)))
                | map($path + [. + " (product dependency)"])))
     | add
     | map(join(" -> "))) as $violations
  | if $control then $violations[] else "MANIFEST_SHAPE_CHANGED" end
')

if [ "$findings" = MANIFEST_SHAPE_CHANGED ]; then
  printf 'arca-engine no longer reaches ArcaEngine, which cannot be true.\n' >&2
  printf 'the manifest shape changed; this check is not measuring anything.\n' >&2
  exit 65
fi

if [ -n "$findings" ]; then
  printf 'the engine reaches a forbidden target:\n' >&2
  printf '%s\n' "$findings" >&2
  exit 1
fi

printf 'PASS: neither arca-engine nor ArcaEngine reaches DockerAPI or ArcaDaemon\n'
