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
import hashlib, lzma, re, subprocess, sys
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
for key in ("SNAPSHOT","BASE_IMAGE","SIGNING_KEY_FINGERPRINT","ARCHITECTURE","INSTALL_RECOMMENDS","SYSTEM_PACKAGES_PATH","SYSTEM_PACKAGES_SHA256"):
    if key not in provenance: fail("missing provenance " + key)
expected_fp=config_value("ubuntu_archive_key_fingerprint")
if provenance["SIGNING_KEY_FINGERPRINT"] != expected_fp: fail("wrong signing-key fingerprint")
if provenance["SNAPSHOT"] != config_value("snapshot"): fail("wrong snapshot")
if provenance["BASE_IMAGE"] != config_value("base_image"): fail("wrong base image")
if provenance["ARCHITECTURE"] != "arm64": fail("wrong architecture")
if provenance["INSTALL_RECOMMENDS"] != "false": fail("Recommends must be disabled")
if provenance["SYSTEM_PACKAGES_PATH"] != config_value("system_packages_file"): fail("wrong system package path")
if provenance["SYSTEM_PACKAGES_SHA256"] != config_value("system_packages_sha256"): fail("wrong system package digest")
signed_releases=sorted((root/"signed-releases").rglob("InRelease"))
if not signed_releases: fail("signed InRelease evidence is missing")
release_hashes={}
for signed_release in signed_releases:
    try: result=subprocess.run([gpgv,"--status-fd","2","--keyring",str(root/"archive-keyring.gpg"),str(signed_release)],stdout=subprocess.PIPE,stderr=subprocess.PIPE,check=False,text=True)
    except OSError: fail("InRelease signature verifier unavailable")
    if result.returncode != 0: fail("invalid InRelease signature")
    valid=[line.split()[2] for line in result.stderr.splitlines() if line.startswith("[GNUPG:] VALIDSIG ") and len(line.split()) >= 3]
    if valid != [expected_fp]: fail("InRelease signature fingerprint is missing or ambiguous")
    suite=signed_release.parent.name
    hashes={}
    in_sha=False
    for line in signed_release.read_text(errors="strict").splitlines():
        if line == "SHA256:": in_sha=True; continue
        if in_sha and line.startswith(" "):
            parts=line.split()
            if len(parts)==3: hashes[parts[2]]=(parts[0],int(parts[1]))
        elif in_sha and line and not line.startswith(" "): in_sha=False
    release_hashes[suite]=hashes

package_text=[]
indexes=sorted((root/"signed-indexes").rglob("Packages.xz"))
if not indexes: fail("signed Packages indexes are missing")
for index in indexes:
    relative=index.relative_to(root/"signed-indexes")
    suite=relative.parts[0]
    release_path="/".join(relative.parts[1:])
    expected=release_hashes.get(suite,{}).get(release_path)
    if expected != (digest(index),index.stat().st_size): fail("compressed Packages hash/size is not covered by signed InRelease")
    try: unpacked=lzma.decompress(index.read_bytes())
    except lzma.LZMAError: fail("invalid compressed Packages index")
    plain_path=release_path.removesuffix(".xz")
    expected_plain=release_hashes[suite].get(plain_path)
    actual_plain=(hashlib.sha256(unpacked).hexdigest(),len(unpacked))
    if expected_plain != actual_plain: fail("uncompressed Packages hash/size is not covered by signed InRelease")
    package_text.append(unpacked.decode("utf-8","strict"))
stanzas=[]
for raw in "\n".join(package_text).strip().split("\n\n"):
    fields={}
    for line in raw.splitlines():
        if line.startswith((" ","\t")) or ": " not in line: continue
        key,value=line.split(": ",1)
        if key in fields: fail("duplicate Packages field")
        fields[key]=value
    required=("Package","Version","Architecture","Filename","SHA256","Size")
    if not all(key in fields for key in required): fail("incomplete Packages stanza")
    stanzas.append(fields)
by_name={}
for fields in stanzas: by_name.setdefault(fields["Package"],[]).append(fields)

lines=(root/"package-manifest.tsv").read_text().splitlines()
if lines != sorted(set(lines)): fail("package manifest is not in canonical order")
selected={}
for line in lines:
    columns=line.split("\t")
    if len(columns) != 6: fail("invalid package manifest")
    name,version,arch,filename,sha,size=columns
    key=(name,version,arch)
    if key in selected: fail("duplicate selected package")
    matches=[item for item in by_name.get(name,[]) if (item["Version"],item["Architecture"],item["Filename"],item["SHA256"],item["Size"]) == (version,arch,filename,sha,size)]
    if not matches: fail("manifest package is absent from Packages metadata")
    if len(matches) != 1: fail("manifest package has ambiguous signed metadata")
    if arch not in ("arm64","all"): fail("non-ARM64 package architecture")
    payload=root/"repository"/filename
    if not payload.is_file() or digest(payload) != sha or payload.stat().st_size != int(size): fail("package payload hash/size mismatch against signed Packages")
    selected[key]=matches[0]

roots=[line for line in (root/"roots.txt").read_text().splitlines() if line]
if roots != sorted(set(roots)): fail("roots are not in canonical order")
root_keys={key for key in selected if key[0] in roots}
if {key[0] for key in root_keys} != set(roots): fail("missing root package")
edge_lines=[line for line in (root/"dependency-edges.tsv").read_text().splitlines() if line]
if edge_lines != sorted(set(edge_lines)): fail("dependency edges are not in canonical order")
requirement_lines=[line for line in (root/"dependency-requirements.tsv").read_text().splitlines() if line]
if requirement_lines != sorted(set(requirement_lines)): fail("dependency requirements are not in canonical order")
requirements={tuple(line.split("\t")) for line in requirement_lines}
if any(len(item) != 6 or item[3] not in ("Depends","Pre-Depends") or not item[5] for item in requirements): fail("invalid normalized dependency requirement")
incoming=set(); outgoing={key:[] for key in selected}
chosen=set()
for line in edge_lines:
    columns=line.split("\t")
    if len(columns) != 9: fail("invalid normalized dependency edge")
    source=(columns[0],columns[1],columns[2]); relation=columns[3]; expression=columns[5]; target=(columns[6],columns[7],columns[8])
    if source not in selected or target not in selected: fail("dependency edge names an unselected package")
    if relation not in ("Depends","Pre-Depends") or not expression: fail("invalid normalized dependency relation")
    chosen.add(tuple(columns[:6]))
    outgoing[source].append(target); incoming.add(target)
if chosen != requirements: fail("missing or extra chosen dependency edge")
if set(selected)-root_keys-incoming: fail("selected package lacks a chosen dependency edge")
reached=set(root_keys); queue=list(root_keys)
while queue:
    for target in outgoing[queue.pop()]:
        if target not in reached: reached.add(target); queue.append(target)
if reached != set(selected): fail("Recommends or unrelated package is outside chosen dependency closure")
PY
  debian_verifier=${DEBIAN_EVIDENCE_VERIFIER:-$root/scripts/verify-ubuntu-debian-evidence.py}
  "$debian_verifier" --verify "$evidence"
}

if [[ ${1:-} == --verify-evidence ]]; then
  [[ $# == 2 ]] || die "usage: $0 --verify-evidence EVIDENCE_DIRECTORY"
  verify_evidence "$2"
  exit 0
fi

[[ $# == 1 ]] || die "usage: $0 OUTPUT_DIRECTORY"
[[ $(uname -s) == Linux && $(uname -m) == aarch64 ]] || die "producer requires Linux ARM64"
for command in apt-get curl dpkg-deb gpgv python3 sha256sum tar zstd; do command -v "$command" >/dev/null || die "missing command: $command"; done
python3 -c 'import apt_pkg' >/dev/null 2>&1 || die "python3-apt is required for canonical Debian dependency semantics"
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
mkdir -p "$work/evidence/repository" "$work/evidence/signed-releases" "$work/evidence/signed-indexes" "$work/apt/lists/partial" "$work/apt/cache/archives/partial"
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
cp -- "$keyring" "$work/evidence/archive-keyring.gpg"
release_count=0
for suite in noble noble-updates noble-security; do
  release_count=$((release_count + 1))
  mkdir -p "$work/evidence/signed-releases/$suite"
  destination="$work/evidence/signed-releases/$suite/InRelease"
  curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 "https://snapshot.ubuntu.com/ubuntu/$snapshot/dists/$suite/InRelease" --output "$destination"
  "$gpgv_bin" --status-fd 2 --keyring "$keyring" "$destination" 2>"$work/gpg.status" || die "invalid snapshot InRelease signature"
  grep -F "VALIDSIG F6ECB3762474EDA9D21B7022871920D1991BC93C" "$work/gpg.status" >/dev/null || die "unexpected Ubuntu signing fingerprint"
  for component in main universe; do
    mkdir -p "$work/evidence/signed-indexes/$suite/$component/binary-arm64"
    curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 "https://snapshot.ubuntu.com/ubuntu/$snapshot/dists/$suite/$component/binary-arm64/Packages.xz" --output "$work/evidence/signed-indexes/$suite/$component/binary-arm64/Packages.xz"
  done
done
[[ $release_count == 3 ]] || die "expected signed InRelease evidence for noble, noble-updates, and noble-security"
python3 - "$work/evidence" "$work/apt/cache/archives" <<'PY'
import apt_pkg,hashlib,lzma,shutil,subprocess,sys
from pathlib import Path
evidence,archives=map(Path,sys.argv[1:]); apt_pkg.init_system()
def fields(raw):
    out={}; current=None
    for line in raw.splitlines():
        if line.startswith((' ','\t')) and current: out[current]+="\n"+line
        elif ': ' in line: current,value=line.split(': ',1); out[current]=value
    return out
upstream=[]
for index in sorted((evidence/'signed-indexes').rglob('Packages.xz')):
    upstream.extend(fields(raw) for raw in lzma.decompress(index.read_bytes()).decode().strip().split('\n\n'))
selected={}; selected_raw={}
for deb in sorted(archives.glob('*.deb')):
    values=subprocess.check_output(['dpkg-deb','-f',str(deb),'Package','Version','Architecture'],text=True).splitlines()
    if len(values)!=3: raise SystemExit('invalid downloaded deb control metadata')
    name,version,arch=values; data=deb.read_bytes(); sha=hashlib.sha256(data).hexdigest(); size=str(len(data))
    matches=[item for item in upstream if (item.get('Package'),item.get('Version'),item.get('Architecture'),item.get('SHA256'),item.get('Size'))==(name,version,arch,sha,size)]
    if len(matches)!=1: raise SystemExit('downloaded deb is not uniquely bound to signed Packages metadata: '+name)
    item=matches[0]; key=(name,version,arch); selected[key]=item
    destination=evidence/'repository'/item['Filename']; destination.parent.mkdir(parents=True,exist_ok=True); shutil.copyfile(deb,destination)
manifest=['\t'.join((*key,item['Filename'],item['SHA256'],item['Size'])) for key,item in selected.items()]
(evidence/'package-manifest.tsv').write_text('\n'.join(sorted(manifest))+'\n')

local=evidence/'repository/dists/gascan/main/binary-arm64'; local.mkdir(parents=True,exist_ok=True)
paragraphs=[]
for key,item in sorted(selected.items()): paragraphs.append('\n'.join(f'{field}: {value}' for field,value in item.items())+'\n')
(local/'Packages').write_text('\n'.join(paragraphs))
PY
"$root/scripts/verify-ubuntu-debian-evidence.py" --write "$work/evidence"
cat >"$work/evidence/provenance.env" <<EOF
SNAPSHOT=2026-07-13T00:00:00Z
BASE_IMAGE=ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab
SIGNING_KEY_FINGERPRINT=F6ECB3762474EDA9D21B7022871920D1991BC93C
ARCHITECTURE=arm64
INSTALL_RECOMMENDS=false
SYSTEM_PACKAGES_PATH=tests/image/system-tools.txt
SYSTEM_PACKAGES_SHA256=d17faf2df1d118f9d7f741c21f77adc4b56e2b89ecabeebde17003bc470742e6
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
