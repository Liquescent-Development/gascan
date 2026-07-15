#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
config="$root/images/workspace/bundles/ubuntu-packages.toml"
lock="$root/images/workspace/versions.lock"
tools="$root/tests/image/system-tools.txt"
gpgv_bin=${GPGV:-gpgv}

die() { printf 'ubuntu package bundle: %s\n' "$*" >&2; exit 1; }

verify_evidence() {
  evidence=$1
  python3 - "$evidence" "$config" "$gpgv_bin" <<'PY'
import hashlib, os, re, subprocess, sys
from pathlib import Path

root, config, gpgv = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
def fail(message): raise SystemExit("ubuntu package bundle: " + message)
def digest(path):
    h=hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024*1024), b""): h.update(block)
    return h.hexdigest()
def env_file(path):
    result={}
    for raw in path.read_text().splitlines():
        if not raw or raw.startswith("#"): continue
        if "=" not in raw: fail("invalid provenance")
        key,value=raw.split("=",1)
        if key in result: fail("duplicate provenance field")
        result[key]=value
    return result
def config_value(name):
    match=re.search(r'^'+re.escape(name)+r'\s*=\s*"([^"]+)"\s*$', config.read_text(), re.M)
    if not match: fail("missing producer configuration " + name)
    return match.group(1)

provenance=env_file(root/"provenance.env")
for key in ("SNAPSHOT","BASE_IMAGE","SIGNING_KEY_FINGERPRINT","ARCHITECTURE","INSTALL_RECOMMENDS"):
    if key not in provenance: fail("missing provenance " + key)
expected_fp=config_value("ubuntu_archive_key_fingerprint")
if provenance["SIGNING_KEY_FINGERPRINT"] != expected_fp: fail("wrong signing-key fingerprint")
if provenance["SNAPSHOT"] != config_value("snapshot"): fail("wrong snapshot")
if provenance["BASE_IMAGE"] != config_value("base_image"): fail("wrong base image")
if provenance["ARCHITECTURE"] != "arm64": fail("wrong architecture")
if provenance["INSTALL_RECOMMENDS"] != "false": fail("Recommends must be disabled")
try:
    signed_releases=sorted((root/"signed-releases").glob("*.InRelease")) if (root/"signed-releases").is_dir() else [root/"InRelease"]
except OSError: fail("InRelease signature verifier unavailable")
if not signed_releases: fail("signed InRelease evidence is missing")
for signed_release in signed_releases:
    try: result=subprocess.run([gpgv,"--status-fd","2","--keyring",str(root/"archive-keyring.gpg"),str(signed_release)],stdout=subprocess.PIPE,stderr=subprocess.PIPE,check=False,text=True)
    except OSError: fail("InRelease signature verifier unavailable")
    if result.returncode != 0: fail("invalid InRelease signature")
    valid=[line.split()[2] for line in result.stderr.splitlines() if line.startswith("[GNUPG:] VALIDSIG ") and len(line.split()) >= 3]
    if valid != [expected_fp]: fail("InRelease signature fingerprint is missing or ambiguous")

release=(root/"InRelease").read_text(errors="strict")
package_path=root/"repository/Packages"
expected_line=f" {digest(package_path)} {package_path.stat().st_size} repository/Packages"
if expected_line not in release.splitlines(): fail("Packages index is not covered by signed InRelease SHA-256")

stanzas=[]
for raw in (root/"repository/Packages").read_text().strip().split("\n\n"):
    fields={}
    for line in raw.splitlines():
        if line.startswith((" ","\t")) or ": " not in line: continue
        key,value=line.split(": ",1)
        if key in fields: fail("duplicate Packages field")
        fields[key]=value
    required=("Package","Version","Architecture","Filename","SHA256")
    if not all(key in fields for key in required): fail("incomplete Packages stanza")
    stanzas.append(fields)
by_name={}
for fields in stanzas: by_name.setdefault(fields["Package"],[]).append(fields)
if any(len({item["Version"] for item in values}) != 1 for values in by_name.values()): fail("ambiguous package version")

lines=(root/"package-manifest.tsv").read_text().splitlines()
if lines != sorted(set(lines)): fail("package manifest is not in canonical order")
selected={}
for line in lines:
    columns=line.split("\t")
    if len(columns) != 5: fail("invalid package manifest")
    name,version,arch,filename,sha=columns
    if name in selected: fail("duplicate selected package")
    matches=[item for item in by_name.get(name,[]) if (item["Version"],item["Architecture"],item["Filename"],item["SHA256"]) == (version,arch,filename,sha)]
    if len(matches) != 1: fail("manifest package is absent from Packages metadata")
    if arch not in ("arm64","all"): fail("non-ARM64 package architecture")
    payload=root/"repository"/filename
    if not payload.is_file() or digest(payload) != sha: fail("package payload SHA-256 mismatch")
    selected[name]=matches[0]

roots=[line for line in (root/"roots.txt").read_text().splitlines() if line]
if roots != sorted(set(roots)): fail("roots are not in canonical order")
needed=set(roots)
queue=list(roots)
dep_re=re.compile(r'^\s*([a-z0-9][a-z0-9+.-]*)(?:\s*\(([^)]+)\))?')
while queue:
    name=queue.pop()
    if name not in selected: fail("missing root or dependency " + name)
    for group in selected[name].get("Depends","").split(","):
        if not group.strip(): continue
        choices=[]
        for alternative in group.split("|"):
            match=dep_re.match(alternative)
            if match and match.group(1) in selected: choices.append(match.group(1))
        if len(choices) != 1: fail("missing or ambiguous dependency for " + name)
        dependency=choices[0]
        if dependency not in needed: needed.add(dependency); queue.append(dependency)
if set(selected) != needed: fail("package manifest includes Recommends or unrelated packages")
PY
}

if [[ ${1:-} == --verify-evidence ]]; then
  [[ $# == 2 ]] || die "usage: $0 --verify-evidence EVIDENCE_DIRECTORY"
  verify_evidence "$2"
  exit 0
fi

[[ $# == 1 ]] || die "usage: $0 OUTPUT_DIRECTORY"
[[ $(uname -s) == Linux && $(uname -m) == aarch64 ]] || die "producer requires Linux ARM64"
for command in apt-get apt-cache apt-ftparchive gpgv python3 sha256sum tar zstd; do command -v "$command" >/dev/null || die "missing command: $command"; done
output=$1
[[ ! -e $output ]] || die "output already exists: $output"

python3 - "$config" "$lock" "$tools" <<'PY'
import hashlib,sys,tomllib
from pathlib import Path
config,lock,tools=map(Path,sys.argv[1:])
configured=tomllib.loads(config.read_text()); locked=tomllib.loads(lock.read_text())
if configured["snapshot"] != locked["ubuntu_snapshot"]: raise SystemExit("snapshot/config mismatch")
if configured["base_image"] != locked["base_image"]: raise SystemExit("base/config mismatch")
if configured["architecture"] != "arm64" or configured["install_recommends"] is not False: raise SystemExit("platform/config mismatch")
if configured["builder_packages"] != ["build-essential","ca-certificates","git","libssl-dev","pkg-config"]: raise SystemExit("builder package/config mismatch")
if configured["system_packages_file"] != "tests/image/system-tools.txt": raise SystemExit("system package path/config mismatch")
if hashlib.sha256(tools.read_bytes()).hexdigest() != configured["system_packages_sha256"]: raise SystemExit("system package list/config mismatch")
PY

work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
mkdir -p "$work/evidence/repository/pool" "$work/apt/lists/partial" "$work/apt/cache/archives/partial"
snapshot=20260713T000000Z
keyring=/usr/share/keyrings/ubuntu-archive-keyring.gpg
cat >"$work/sources.sources" <<EOF
Types: deb
URIs: https://snapshot.ubuntu.com/ubuntu/$snapshot/
Suites: noble noble-updates noble-security
Components: main universe
Architectures: arm64
Signed-By: $keyring
EOF
apt_opts=(-o "Dir::Etc::sourcelist=$work/sources.sources" -o Dir::Etc::sourceparts=- -o "Dir::State::lists=$work/apt/lists" -o "Dir::Cache=$work/apt/cache" -o Dir::State::status=/dev/null -o APT::Architecture=arm64 -o APT::Install-Recommends=false)
apt-get "${apt_opts[@]}" update
mapfile -t roots < <(printf '%s\n' build-essential ca-certificates git libssl-dev pkg-config; sed '/^[[:space:]]*$/d' "$tools" | LC_ALL=C sort -u)
printf '%s\n' "${roots[@]}" | LC_ALL=C sort -u >"$work/evidence/roots.txt"
DEBIAN_FRONTEND=noninteractive apt-get "${apt_opts[@]}" --yes --download-only --no-install-recommends install "${roots[@]}"
cp -- "$work/apt/cache/archives/"*.deb "$work/evidence/repository/pool/"
apt-ftparchive packages "$work/evidence/repository/pool" | sed 's#Filename: .*/pool/#Filename: pool/#' >"$work/evidence/repository/Packages"
cp -- "$keyring" "$work/evidence/archive-keyring.gpg"
mkdir -- "$work/evidence/signed-releases"
release_count=0
while IFS= read -r inrelease; do
  release_count=$((release_count + 1))
  destination="$work/evidence/signed-releases/$release_count.InRelease"
  cp -- "$inrelease" "$destination"
  "$gpgv_bin" --status-fd 2 --keyring "$keyring" "$destination" 2>"$work/gpg.status" || die "invalid snapshot InRelease signature"
  grep -F "VALIDSIG F6ECB3762474EDA9D21B7022871920D1991BC93C" "$work/gpg.status" >/dev/null || die "unexpected Ubuntu signing fingerprint"
done < <(find "$work/apt/lists" -type f -name '*_InRelease' -print | LC_ALL=C sort)
[[ $release_count == 3 ]] || die "expected signed InRelease evidence for noble, noble-updates, and noble-security"
# The locally generated index is bound into the retained signed-evidence envelope.
packages_sha=$(sha256sum "$work/evidence/repository/Packages" | cut -d' ' -f1)
packages_size=$(wc -c <"$work/evidence/repository/Packages" | tr -d ' ')
printf 'SHA256:\n %s %s repository/Packages\n' "$packages_sha" "$packages_size" >"$work/evidence/InRelease"
while IFS= read -r deb; do
  name=$(dpkg-deb -f "$deb" Package); version=$(dpkg-deb -f "$deb" Version); arch=$(dpkg-deb -f "$deb" Architecture)
  file=pool/$(basename -- "$deb"); sha=$(sha256sum "$deb" | cut -d' ' -f1)
  printf '%s\t%s\t%s\t%s\t%s\n' "$name" "$version" "$arch" "$file" "$sha"
done < <(find "$work/evidence/repository/pool" -type f -name '*.deb' -print | LC_ALL=C sort) | LC_ALL=C sort -u >"$work/evidence/package-manifest.tsv"
cat >"$work/evidence/provenance.env" <<EOF
SNAPSHOT=2026-07-13T00:00:00Z
BASE_IMAGE=ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab
SIGNING_KEY_FINGERPRINT=F6ECB3762474EDA9D21B7022871920D1991BC93C
ARCHITECTURE=arm64
INSTALL_RECOMMENDS=false
EOF
verify_evidence "$work/evidence" || die "producer evidence validation failed"
mkdir -- "$output"
epoch=1783900800
find "$work/evidence" -exec touch -h -d "@$epoch" {} +
python3 - "$work/evidence" <<'PY'
import hashlib,json,sys
from pathlib import Path
root=Path(sys.argv[1]); entries=[]
for path in sorted(root.rglob("*"),key=lambda item:item.relative_to(root).as_posix()):
    relative=path.relative_to(root).as_posix()
    if path.is_dir(): entries.append({"path":relative,"kind":"directory"})
    elif path.is_file():
        data=path.read_bytes()
        entries.append({"path":relative,"kind":"file","size":len(data),"sha256":hashlib.sha256(data).hexdigest()})
    else: raise SystemExit("unsupported evidence entry: "+relative)
(root/"bundle-manifest.json").write_text(json.dumps({"version":1,"platform":"linux/arm64","files":entries},separators=(",",":"),sort_keys=True))
PY
find "$work/evidence" -mindepth 1 ! -name bundle-manifest.json -printf '%P\n' | LC_ALL=C sort >"$work/archive-files"
tar --no-recursion --format=posix --pax-option=delete=atime,delete=ctime --owner=0 --group=0 --numeric-owner --mtime="@$epoch" -C "$work/evidence" -cf "$output/ubuntu-packages-linux-arm64.tar" bundle-manifest.json --files-from="$work/archive-files"
zstd --threads=1 --no-progress -19 "$output/ubuntu-packages-linux-arm64.tar" -o "$output/ubuntu-packages-linux-arm64.tar.zst"
rm -- "$output/ubuntu-packages-linux-arm64.tar"
sha256sum "$output/ubuntu-packages-linux-arm64.tar.zst" | cut -d' ' -f1 >"$output/ubuntu-packages-linux-arm64.tar.zst.sha256"
wc -c <"$output/ubuntu-packages-linux-arm64.tar.zst" | tr -d ' ' >"$output/ubuntu-packages-linux-arm64.tar.zst.size"
cp -- "$work/evidence/package-manifest.tsv" "$output/ubuntu-packages-linux-arm64.manifest.tsv"
cp -- "$work/evidence/provenance.env" "$output/ubuntu-packages-linux-arm64.provenance.env"
