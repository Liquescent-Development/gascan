#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd -P)
pin_file=${GASCAN_ARCA_PIN_FILE:-$repo_root/engine/arca-pin.json}
cache_root=${GASCAN_ARCA_ENGINE_CACHE:-$repo_root/.artifacts/arca-engine}
allowed_signers=${GASCAN_ARCA_ALLOWED_SIGNERS:-$repo_root/engine/allowed-signers}

for command in git jq swift; do
  command -v "$command" >/dev/null || {
    printf 'required command is unavailable: %s\n' "$command" >&2
    exit 69
  }
done

[[ -f $pin_file ]] || {
  printf 'engine pin file is missing: %s\n' "$pin_file" >&2
  exit 64
}
[[ -f $allowed_signers ]] || {
  printf 'engine allowed-signers file is missing: %s\n' "$allowed_signers" >&2
  exit 64
}
jq -e '
  (.schema == 1) and
  (.name | type == "string" and length > 0) and
  (.url | type == "string" and length > 0) and
  (.tag | type == "string" and length > 0) and
  (.revision | type == "string" and test("^[0-9a-f]{40}$"))
' "$pin_file" >/dev/null 2>&1 || {
  printf 'engine pin file is malformed: %s\n' "$pin_file" >&2
  exit 64
}

url=$(jq -er '.url' "$pin_file")
tag=$(jq -er '.tag' "$pin_file")
revision=$(jq -er '.revision' "$pin_file")

checkout=$cache_root/arca
mkdir -p "$cache_root"
[[ -d $checkout/.git ]] || git clone --quiet "$url" "$checkout"
git -C "$checkout" remote set-url origin "$url"
# --force accepts a moved tag deliberately. A moved tag is not silently trusted:
# it fails below on the tag-target assertion, which is the real gate and reports
# the actual mismatch instead of an opaque fetch rejection.
git -C "$checkout" fetch --quiet --tags --force origin

git -C "$checkout" cat-file -e "${revision}^{commit}" 2>/dev/null || {
  printf 'pinned revision is absent from %s after fetch: %s\n' "$url" "$revision" >&2
  exit 65
}
git -C "$checkout" -c "gpg.ssh.allowedSignersFile=$allowed_signers" \
  verify-tag "$tag" >/dev/null 2>&1 || {
  printf 'engine pin tag signature does not verify against %s: %s\n' \
    "$allowed_signers" "$tag" >&2
  exit 65
}
tag_target=$(git -C "$checkout" rev-parse --verify "refs/tags/${tag}^{}") || {
  printf 'engine pin tag is absent: %s\n' "$tag" >&2
  exit 65
}
[[ $tag_target == "$revision" ]] || {
  printf 'engine pin tag %s resolves to %s, not the pinned revision %s\n' \
    "$tag" "$tag_target" "$revision" >&2
  exit 65
}

git -C "$checkout" checkout --quiet --detach "$revision"
git -C "$checkout" submodule update --init --recursive --quiet

swift build --package-path "$checkout" --configuration release --target ContainerBridge >&2

printf '%s\n' "$checkout"
