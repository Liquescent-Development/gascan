#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
config="$root/images/workspace/bundles/ubuntu-packages.toml"
lock="$root/images/workspace/versions.lock"
tools="$root/tests/image/system-tools.txt"
gpgv_bin=${GPGV:-gpgv}

die() { printf 'ubuntu package bundle: %s\n' "$*" >&2; exit 1; }

fetch_signed_snapshot() {
  curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
    --retry 4 --retry-delay 2 --retry-max-time 60 --connect-timeout 20 \
    "$1" --output "$2"
}

verify_signed_metadata() {
  python3 - "$1" <<'PY'
import hashlib,lzma,sys
from pathlib import Path
root=Path(sys.argv[1]); releases={}
for signed in sorted((root/'signed-releases').rglob('InRelease')):
    hashes={}; in_sha=False
    for line in signed.read_text(errors='strict').splitlines():
        if line == 'SHA256:': in_sha=True; continue
        if in_sha and line.startswith(' '):
            parts=line.split()
            if len(parts)==3: hashes[parts[2]]=(parts[0],int(parts[1]))
        elif in_sha and line and not line.startswith(' '): in_sha=False
    releases[signed.parent.name]=hashes
for index in sorted((root/'signed-indexes').rglob('Packages.xz')):
    relative=index.relative_to(root/'signed-indexes'); suite=relative.parts[0]
    release_path='/'.join(relative.parts[1:]); data=index.read_bytes()
    actual=(hashlib.sha256(data).hexdigest(),len(data))
    if releases.get(suite,{}).get(release_path) != actual:
        raise SystemExit('ubuntu package bundle: compressed Packages hash/size is not covered by signed InRelease')
    unpacked=lzma.decompress(data); plain=release_path.removesuffix('.xz')
    actual_plain=(hashlib.sha256(unpacked).hexdigest(),len(unpacked))
    if releases[suite].get(plain) != actual_plain:
        raise SystemExit('ubuntu package bundle: uncompressed Packages hash/size is not covered by signed InRelease')
PY
}

configure_command_rootfs() {
  evidence=$1
  command_rootfs=$2
  expected_status_sha=$3
  [[ $command_rootfs == /* && $command_rootfs != / && -f $command_rootfs/var/lib/dpkg/status ]] ||
    die "UBUNTU_COMMAND_ROOTFS must name a pristine absolute root filesystem"
  actual_status_sha=$(sha256sum "$command_rootfs/var/lib/dpkg/status" | cut -d' ' -f1)
  [[ $expected_status_sha =~ ^[0-9a-f]{64}$ && $actual_status_sha == "$expected_status_sha" ]] ||
    die "pristine command root package status differs from pre-bootstrap attestation"
  command_evidence=/tmp/gascan-command-evidence
  staging="$command_rootfs$command_evidence"
  [[ ! -e $staging ]] || die "command root staging already exists"
  mkdir -p "$staging"
  cp -a -- "$evidence/repository" "$staging/repository"
  cp -- "$evidence/package-manifest.tsv" "$staging/package-manifest.tsv"
  cp -- "$root/scripts/write-ubuntu-command-evidence.sh" "$staging/write-ubuntu-command-evidence.sh"
  cp -- "$root/scripts/verify-ubuntu-command-evidence.sh" "$staging/verify-ubuntu-command-evidence.sh"
  cat >"$command_rootfs/usr/sbin/policy-rc.d" <<'EOF'
#!/bin/sh
exit 101
EOF
  chmod 0555 "$command_rootfs/usr/sbin/policy-rc.d"
  if timeout --signal=KILL 300s chroot "$command_rootfs" /bin/bash -ceu '
    evidence=/tmp/gascan-command-evidence
    printf "deb [trusted=yes] file:%s/repository gascan main\n" "$evidence" >/tmp/gascan-local.list
    apt_options=(
      -o Dir::Etc::sourcelist=/tmp/gascan-local.list
      -o Dir::Etc::sourceparts=-
      -o APT::Architecture=arm64
      -o APT::Install-Recommends=false
      -o Acquire::Retries=0
      -o Acquire::http::Proxy=false
      -o Acquire::https::Proxy=false
      -o Dir::Bin::Methods::http=/bin/false
      -o Dir::Bin::Methods::https=/bin/false
    )
    apt-get "${apt_options[@]}" update
    awk -F "\t" '\''{print $3 == "all" ? $1 "=" $2 : $1 ":" $3 "=" $2}'\'' "$evidence/package-manifest.tsv" >/tmp/gascan-exact-packages
    mapfile -t exact </tmp/gascan-exact-packages
    DEBIAN_FRONTEND=noninteractive apt-get "${apt_options[@]}" --yes --no-install-recommends install "${exact[@]}"
    test -z "$(dpkg --audit)"
    while IFS=$'\''\t'\'' read -r package version architecture _; do
      test "$(dpkg-query -W -f='\''${Version}\t${Architecture}'\'' "$package")" = "$version	$architecture"
    done <"$evidence/package-manifest.tsv"
    "$evidence/write-ubuntu-command-evidence.sh" "$evidence"
    "$evidence/verify-ubuntu-command-evidence.sh" "$evidence"
  '; then
    cp -- "$staging/command-providers.tsv" "$evidence/command-providers.tsv"
  else
    status=$?
    rm -rf -- "$staging"
    die "pristine offline command root validation failed with status $status"
  fi
  rm -rf -- "$staging"
}

verify_evidence_structure() {
  evidence=$1
  python3 - "$evidence" "$config" "$gpgv_bin" <<'PY' || return 1
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

package_indexes=[]
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
    package_indexes.append((str(relative),unpacked.decode("utf-8","strict")))
stanzas_by_group={}; same_index_conflicts=set(); cross_index_conflicts=set()
for source,text in package_indexes:
    source_groups=set()
    for raw in text.strip().split("\n\n"):
        fields={}; current=None
        for line in raw.splitlines():
            if line.startswith((" ","\t")):
                if current is None: fail("invalid Packages continuation")
                fields[current]+="\n"+line
                continue
            if ":" not in line: fail("invalid Packages field")
            current,value=line.split(":",1)
            if value.startswith(" "): value=value[1:]
            elif value: fail("invalid Packages field")
            if current in fields: fail("duplicate Packages field")
            fields[current]=value
        required=("Package","Version","Architecture","Filename","SHA256","Size")
        if not all(key in fields for key in required): fail("incomplete Packages stanza")
        group=tuple(fields[key] for key in ("Package","Version","Architecture"))
        if group in source_groups:
            same_index_conflicts.add(group)
            continue
        source_groups.add(group)
        if group in stanzas_by_group:
            if stanzas_by_group[group][0] != raw: cross_index_conflicts.add(group)
        else:
            stanzas_by_group[group]=(raw,fields)

lines=(root/"package-manifest.tsv").read_text().splitlines()
if lines != sorted(set(lines)): fail("package manifest is not in canonical order")
selected={}
for line in lines:
    columns=line.split("\t")
    if len(columns) != 6: fail("invalid package manifest")
    name,version,arch,filename,sha,size=columns
    key=(name,version,arch)
    if key in selected: fail("duplicate selected package")
    if key in same_index_conflicts: fail("duplicate package group in same signed index")
    if key in cross_index_conflicts: fail("conflicting signed package metadata across indexes")
    if key not in stanzas_by_group: fail("manifest package is absent from Packages metadata")
    item=stanzas_by_group[key][1]
    if (item["Filename"],item["SHA256"],item["Size"]) != (filename,sha,size):
        fail("manifest package is absent from Packages metadata")
    if arch not in ("arm64","all"): fail("non-ARM64 package architecture")
    payload=root/"repository"/filename
    if not payload.is_file() or digest(payload) != sha or payload.stat().st_size != int(size): fail("package payload hash/size mismatch against signed Packages")
    selected[key]=item

expected_commands={"dig":"bind9-dnsutils","ifconfig":"net-tools","ip":"iproute2","nano":"nano","netstat":"net-tools","nslookup":"bind9-dnsutils","pico":"nano","ping":"iputils-ping","ps":"procps","pstree":"psmisc","ss":"iproute2","top":"procps"}
command_lines=(root/"command-providers.tsv").read_text().splitlines()
if command_lines != sorted(set(command_lines)): fail("command provider evidence is not in canonical order")
seen_commands=set()
for line in command_lines:
    columns=line.split("\t")
    if len(columns) != 3: fail("invalid command provider evidence")
    command,provider,path=columns
    if command in seen_commands or expected_commands.get(command) != provider or not Path(path).is_absolute():
        fail("invalid command provider evidence")
    seen_commands.add(command)
if seen_commands != set(expected_commands): fail("missing command provider evidence")

roots=[line for line in (root/"roots.txt").read_text().splitlines() if line]
if roots != sorted(set(roots)): fail("roots are not in canonical order")
reviewed_providers={"libatk-bridge2.0-0":"libatk-bridge2.0-0t64","libatk1.0-0":"libatk1.0-0t64","libcups2":"libcups2t64"}
binding_lines=(root/"root-bindings.tsv").read_text().splitlines()
if binding_lines != sorted(set(binding_lines)): fail("root bindings are not in canonical order")
bound_roots=set(); root_keys=set()
for line in binding_lines:
    columns=line.split("\t")
    if len(columns) != 4: fail("invalid requested root binding against roots")
    requested,name,version,arch=columns; key=(name,version,arch)
    if requested not in roots or requested in bound_roots or key not in selected: fail("invalid requested root binding against roots")
    if name != requested:
        if reviewed_providers.get(requested) != name: fail("invalid requested root provider")
        provided=selected[key].get("Provides","").split(",")
        exact=f"{requested} (= {version})"
        if exact not in [item.strip() for item in provided]: fail("invalid requested root provider")
    bound_roots.add(requested); root_keys.add(key)
if bound_roots != set(roots): fail("missing root package")
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
  "$debian_verifier" --verify "$evidence" || return 1
}

verify_evidence() {
  evidence=$1
  verify_evidence_structure "$evidence" || return 1
  command_verifier=${COMMAND_EVIDENCE_VERIFIER:-$root/scripts/verify-ubuntu-command-evidence.sh}
  "$command_verifier" "$evidence" || return 1
}

if [[ ${1:-} == --verify-evidence ]]; then
  [[ $# == 2 ]] || die "usage: $0 --verify-evidence EVIDENCE_DIRECTORY"
  verify_evidence "$2"
  exit 0
fi
if [[ ${1:-} == --verify-evidence-structure ]]; then
  [[ $# == 2 ]] || die "usage: $0 --verify-evidence-structure EVIDENCE_DIRECTORY"
  verify_evidence_structure "$2"
  exit 0
fi

[[ $# == 1 ]] || die "usage: $0 OUTPUT_DIRECTORY"
[[ $(uname -s) == Linux && $(uname -m) == aarch64 ]] || die "producer requires Linux ARM64"
for command in apt-get curl dpkg-deb gpgv python3 sha256sum tar zstd; do command -v "$command" >/dev/null || die "missing command: $command"; done
python3 -c 'import apt_pkg' >/dev/null 2>&1 || die "python3-apt is required for canonical Debian dependency semantics"
output=$1
[[ ! -e $output ]] || die "output already exists: $output"
command_rootfs=${UBUNTU_COMMAND_ROOTFS:-}
command_rootfs_status_sha256=${UBUNTU_COMMAND_ROOTFS_STATUS_SHA256:-}
[[ -n $command_rootfs && -n $command_rootfs_status_sha256 ]] ||
  die "UBUNTU_COMMAND_ROOTFS and UBUNTU_COMMAND_ROOTFS_STATUS_SHA256 are required"

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
system_packages_sha256=$(sha256sum "$tools" | cut -d' ' -f1)
package_cache_root=${UBUNTU_PACKAGE_CACHE:-}
package_cache=
if [[ -n $package_cache_root ]]; then
  package_cache="$package_cache_root/$snapshot-arm64-$system_packages_sha256"
  mkdir -p -- "$package_cache"
fi
keyring=/usr/share/keyrings/ubuntu-archive-keyring.gpg
cp -- "$keyring" "$work/evidence/archive-keyring.gpg"
release_count=0
for suite in noble noble-updates noble-security; do
  release_count=$((release_count + 1))
  mkdir -p "$work/evidence/signed-releases/$suite"
  destination="$work/evidence/signed-releases/$suite/InRelease"
  fetch_signed_snapshot "https://snapshot.ubuntu.com/ubuntu/$snapshot/dists/$suite/InRelease" "$destination"
  "$gpgv_bin" --status-fd 2 --keyring "$keyring" "$destination" 2>"$work/gpg.status" || die "invalid snapshot InRelease signature"
  grep -F "VALIDSIG F6ECB3762474EDA9D21B7022871920D1991BC93C" "$work/gpg.status" >/dev/null || die "unexpected Ubuntu signing fingerprint"
  for component in main universe; do
    mkdir -p "$work/evidence/signed-indexes/$suite/$component/binary-arm64"
    fetch_signed_snapshot "https://snapshot.ubuntu.com/ubuntu/$snapshot/dists/$suite/$component/binary-arm64/Packages.xz" "$work/evidence/signed-indexes/$suite/$component/binary-arm64/Packages.xz"
  done
done
[[ $release_count == 3 ]] || die "expected signed InRelease evidence for noble, noble-updates, and noble-security"
verify_signed_metadata "$work/evidence"
if [[ -n $package_cache ]]; then
  "$root/scripts/ubuntu-package-cache.py" stage "$work/evidence" "$package_cache" "$work/apt/cache/archives"
fi
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
if [[ -n $package_cache ]]; then
  "$root/scripts/ubuntu-package-cache.py" publish "$work/evidence" "$package_cache" "$work/apt/cache/archives"
fi
python3 - "$work/evidence" "$work/apt/cache/archives" <<'PY'
import apt_pkg,hashlib,lzma,shutil,subprocess,sys,urllib.parse
from pathlib import Path
evidence,archives=map(Path,sys.argv[1:]); apt_pkg.init_system()
def fields(raw):
    out={}; current=None
    for line in raw.splitlines():
        if line.startswith((' ','\t')) and current: out[current]+="\n"+line
        elif ':' in line:
            current,value=line.split(':',1)
            if value.startswith(' '): value=value[1:]
            elif value: raise SystemExit('invalid signed Packages field')
            if current in out: raise SystemExit('duplicate signed Packages field')
            out[current]=value
        else: raise SystemExit('invalid signed Packages field')
    return out
upstream_by_group={}; same_index_conflicts=set(); cross_index_conflicts=set()
for index in sorted((evidence/'signed-indexes').rglob('Packages.xz')):
    source_groups=set()
    for raw in lzma.decompress(index.read_bytes()).decode().strip().split('\n\n'):
        item=fields(raw)
        required=('Package','Version','Architecture','Filename','SHA256','Size')
        if not all(key in item for key in required): raise SystemExit('incomplete signed Packages stanza')
        group=tuple(item[key] for key in ('Package','Version','Architecture'))
        if group in source_groups:
            same_index_conflicts.add(group)
            continue
        source_groups.add(group)
        if group in upstream_by_group:
            if upstream_by_group[group][0] != raw: cross_index_conflicts.add(group)
        else:
            upstream_by_group[group]=(raw,item)
selected={}; selected_raw={}
for deb in sorted(archives.glob('*.deb')):
    raw=subprocess.check_output(['dpkg-deb','--show','--showformat=${Package}\t${Version}\t${Architecture}\n',str(deb)],text=True)
    if raw.count('\n') != 1: raise SystemExit('invalid downloaded deb control metadata')
    columns=raw.removesuffix('\n').split('\t')
    if len(columns) != 3 or not all(columns) or any(ord(character) < 32 or ord(character) == 127 for column in columns for character in column): raise SystemExit('invalid downloaded deb control metadata')
    name,version,arch=columns; data=deb.read_bytes(); sha=hashlib.sha256(data).hexdigest(); size=str(len(data))
    key=(name,version,arch)
    if key in same_index_conflicts: raise SystemExit('duplicate package group in same signed index: '+name)
    if key in cross_index_conflicts: raise SystemExit('conflicting signed package metadata across indexes: '+name)
    if key not in upstream_by_group: raise SystemExit('downloaded deb is not uniquely bound to signed Packages metadata: '+name)
    item=upstream_by_group[key][1]
    expected_cache_name=f'{name}_{version}_{arch}.deb'
    if (item.get('SHA256'),item.get('Size')) != (sha,size) or urllib.parse.unquote(deb.name) != expected_cache_name or not Path(item['Filename']).name: raise SystemExit('downloaded deb is not uniquely bound to signed Packages metadata: '+name)
    selected[key]=item
    destination=evidence/'repository'/item['Filename']; destination.parent.mkdir(parents=True,exist_ok=True); shutil.copyfile(deb,destination)
manifest=['\t'.join((*key,item['Filename'],item['SHA256'],item['Size'])) for key,item in selected.items()]
(evidence/'package-manifest.tsv').write_text('\n'.join(sorted(manifest))+'\n')

local=evidence/'repository/dists/gascan/main/binary-arm64'; local.mkdir(parents=True,exist_ok=True)
paragraphs=[]
for key,item in sorted(selected.items()): paragraphs.append('\n'.join(f'{field}: {value}' for field,value in item.items())+'\n')
(local/'Packages').write_text('\n'.join(paragraphs))
PY
"$root/scripts/verify-ubuntu-debian-evidence.py" --write "$work/evidence"
configure_command_rootfs "$work/evidence" "$command_rootfs" "$command_rootfs_status_sha256"
cat >"$work/evidence/provenance.env" <<EOF
SNAPSHOT=2026-07-13T00:00:00Z
BASE_IMAGE=ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab
SIGNING_KEY_FINGERPRINT=F6ECB3762474EDA9D21B7022871920D1991BC93C
ARCHITECTURE=arm64
INSTALL_RECOMMENDS=false
SYSTEM_PACKAGES_PATH=tests/image/system-tools.txt
SYSTEM_PACKAGES_SHA256=b68046c4450d7ec11362905551a793d0e4884e20b63f82b26335d2e7610acce8
EOF
verify_evidence_structure "$work/evidence" || die "producer evidence validation failed"
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
