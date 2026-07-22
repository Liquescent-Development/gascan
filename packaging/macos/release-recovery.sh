#!/usr/bin/env bash
# What to tell an operator when a release step fails after publish.sh has
# already flipped the release out of draft.
#
# This lives outside release.sh so it can be tested. It runs only when a
# release is public, which no test can arrange, and the defects found in it so
# far -- a recipe wrong for two of its three states, rejected values presented
# as authoritative -- are not the kind a source grep can see.
#
# Both functions write to stdout. The caller redirects.

gascan_print_release_values() {
  printf '  asset:  %s\n' "$1"
  printf '  sha256: %s\n' "$2"
}

# gascan_report_live_release VERSION TAP_PATH REPO_ROOT STAGE ASSET_URL
#                           CHECKSUM PUBLISHED
#
# ASSET_URL and CHECKSUM are empty when publish.sh's output did not validate.
# PUBLISHED is that raw output, shown instead so the operator sees what
# actually came back rather than a value already rejected.
gascan_report_live_release() {
  local version=$1 tap=$2 repo=$3 stage=$4 url=$5 sum=$6 raw=$7
  printf '\nthe GitHub release for v%s is already published; do not delete it\n' \
    "$version"
  if [[ -n $url && -n $sum ]]; then
    gascan_print_release_values "$url" "$sum"
  elif [[ -n $raw ]]; then
    printf 'publish.sh printed:\n%s\n' "$raw"
  else
    printf 'publish.sh printed nothing before it stopped.\n'
    printf 'the checksum is the release'"'"'s uploaded .sha256 asset, or:\n'
    printf '  shasum -a 256 <the .pkg in .artifacts/release>\n'
  fi
  printf 'finish the cask by hand:\n'
  if [[ $stage == none ]]; then
    # `none` also covers a failed `pull --ff-only`, which means origin/main
    # moved under the tap. Committing on the stale base would only get the
    # push rejected, so name the reconcile step before the rest.
    printf '  # if the tap could not fast-forward, reconcile it first:\n'
    printf '  #   git -C %s pull --ff-only origin main\n' "$tap"
    # render-cask.sh rejects a checksum that is not 64 hex characters, so name
    # the placeholder rather than emitting a command that pastes and fails.
    printf '  mkdir -p %s/Casks\n' "$tap"
    printf '  %s/packaging/macos/render-cask.sh %s %s > %s/Casks/gascan.rb\n' \
      "$repo" "$version" "${sum:-<sha256>}" "$tap"
  fi
  if [[ $stage == rendered ]]; then
    printf '  # %s/Casks/gascan.rb is already rendered; check it before staging\n' \
      "$tap"
  fi
  if [[ $stage == none || $stage == rendered ]]; then
    printf '  git -C %s add Casks/gascan.rb\n' "$tap"
  fi
  if [[ $stage != committed ]]; then
    printf "  git -C %s commit -m 'gascan %s'\n" "$tap" "$version"
  fi
  # Always last, and always explicit: a tap without upstream tracking rejects
  # a bare push, which is the failure this recovery most often follows.
  printf '  git -C %s push origin main\n' "$tap"
}
