#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

revision=f6b248c5926240856dbea83d1d2c5c90ea1c1456
git_tree_expected=71e706057023049b8d15839cedd1fcd0b4a85968
archive_name=gascamp-source-vendor-linux-arm64.tar.zst
die() { printf 'gascamp bundle: %s\n' "$*" >&2; exit 1; }

verify_evidence() {
  python3 - "$1" "$revision" "$git_tree_expected" <<'PY'
import hashlib,json,re,stat,sys,tomllib
from pathlib import Path,PurePosixPath
root=Path(sys.argv[1]); expected=sys.argv[2]; expected_tree=sys.argv[3]
def fail(s): raise SystemExit("gascamp bundle: "+s)
def read(p):
 try: return p.read_text(encoding="utf-8")
 except (OSError,UnicodeError): fail("missing or invalid "+p.name)
def sha(p): return hashlib.sha256(p.read_bytes()).hexdigest()
def env(p):
 out={}
 for line in read(p).splitlines():
  if not line or "=" not in line: fail("invalid provenance")
  k,v=line.split("=",1)
  if k in out: fail("duplicate provenance field")
  out[k]=v
 return out
def actual_manifest(base):
 rows=[]
 if not base.is_dir(): fail("missing tree directory")
 for p in sorted(base.rglob("*")):
  q=p.relative_to(base).as_posix()
  if p.is_symlink():
   target=p.readlink().as_posix(); body=target.encode(); rows.append(f"{q}\tsymlink\t0777\t{len(body)}\t{hashlib.sha256(body).hexdigest()}\t{target}")
  elif p.is_file(): rows.append(f"{q}\tfile\t{stat.S_IMODE(p.stat().st_mode):04o}\t{p.stat().st_size}\t{sha(p)}\t-")
  elif not p.is_dir(): fail("unsupported tree entry")
 return "\n".join(rows)+"\n"
p=env(root/"provenance.env")
required={"REVISION","FETCHED_HEAD","GIT_TREE","SOURCE_MANIFEST_SHA256","VENDOR_MANIFEST_SHA256","CONFIG_SHA256","CARGO_VENDOR_LOCKED","PLATFORM","SUBMODULES"}
if set(p)!=required: fail("provenance fields differ from exact schema")
if p["REVISION"]!=expected or p["FETCHED_HEAD"]!=expected: fail("revision mismatch")
if p["GIT_TREE"]!=expected_tree: fail("git tree mismatch")
if p["SUBMODULES"]!="absent": fail("submodule ambiguity")
if p["CARGO_VENDOR_LOCKED"]!="true": fail("cargo vendor was not locked")
if p["PLATFORM"]!="linux/arm64": fail("wrong platform")
for name,base,key,label in (("source-tree.tsv",root/"tree/source","SOURCE_MANIFEST_SHA256","source tree"),("vendor-tree.tsv",root/"tree/vendor","VENDOR_MANIFEST_SHA256","vendor tree")):
 text=read(root/name)
 if text!=actual_manifest(base) or sha(root/name)!=p[key]: fail(label+" differs from canonical manifest")
config=root/"tree/.cargo/config.toml"
if not config.is_file() or sha(config)!=p["CONFIG_SHA256"]: fail("Cargo config digest mismatch")
try: cfg=tomllib.loads(read(config))
except tomllib.TOMLDecodeError: fail("invalid Cargo config")
if cfg!={"net":{"offline":True},"source":{"crates-io":{"replace-with":"vendored-sources"},"vendored-sources":{"directory":"vendor"}}}: fail("Cargo config permits registry or network access")
source=root/"tree/source"; vendor=root/"tree/vendor"
try: lock=tomllib.loads(read(source/"Cargo.lock"))
except tomllib.TOMLDecodeError: fail("invalid Cargo.lock")
locked=lock.get("package",[])
if not isinstance(locked,list): fail("invalid Cargo.lock packages")
git_locks={x.get("source") for x in locked if isinstance(x,dict) and str(x.get("source","")).startswith("git+")}
for cargo in source.rglob("Cargo.toml"):
 try: doc=tomllib.loads(read(cargo))
 except tomllib.TOMLDecodeError: fail("invalid Cargo.toml")
 for section in ("dependencies","dev-dependencies","build-dependencies"):
  for value in doc.get(section,{}).values():
   if isinstance(value,dict) and "git" in value:
    rev=value.get("rev")
    if not isinstance(rev,str) or not re.fullmatch(r"[0-9a-f]{40}",rev) or not any(s and s.endswith("#"+rev) and s.startswith("git+"+value["git"]) for s in git_locks): fail("unlocked git dependency")
registry={(x.get("name"),x.get("version")):x.get("checksum") for x in locked if isinstance(x,dict) and str(x.get("source","")).startswith("registry+")}
seen=set()
for crate in sorted(vendor.iterdir()):
 if not crate.is_dir(): fail("vendor tree has non-crate entry")
 checksum=crate/".cargo-checksum.json"
 if not checksum.is_file(): fail("absent cargo checksum")
 try: data=json.loads(read(checksum)); manifest=tomllib.loads(read(crate/"Cargo.toml"))
 except (json.JSONDecodeError,tomllib.TOMLDecodeError): fail("invalid cargo checksum or manifest")
 package=manifest.get("package",{}); key=(package.get("name"),package.get("version"))
 if key not in registry or data.get("package")!=registry[key]: fail("vendored crate is not lock-bound")
 files=data.get("files")
 if not isinstance(files,dict): fail("invalid cargo checksum files")
 actual={x.relative_to(crate).as_posix() for x in crate.rglob("*") if x.is_file() and x.name!=".cargo-checksum.json"}
 if set(files)!=actual: fail("cargo checksum file set mismatch")
 for name,digest in files.items():
  pure=PurePosixPath(name)
  if pure.is_absolute() or ".." in pure.parts or not re.fullmatch(r"[0-9a-f]{64}",str(digest)) or sha(crate/name)!=digest: fail("cargo checksum content mismatch")
 seen.add(key)
if seen!=set(registry): fail("missing or extra vendored crate")
PY
}

if [[ ${1:-} == --verify-evidence ]]; then
  [[ $# == 2 ]] || die "usage: $0 --verify-evidence DIRECTORY"
  verify_evidence "$2"
  exit
fi

[[ $# == 1 ]] || die "usage: $0 OUTPUT_DIRECTORY"
[[ $(uname -s) == Linux && $(uname -m) == aarch64 ]] || die "production requires connected Linux ARM64"
for command in cargo git python3 sha256sum tar zstd; do command -v "$command" >/dev/null || die "missing required command: $command"; done
output=$1
mkdir -p "$output"
[[ -z $(find "$output" -mindepth 1 -print -quit) ]] || die "output must be empty"
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
repo="$work/repository"; tree="$output/tree"
git init -q "$repo"
git -C "$repo" remote add origin https://github.com/Liquescent-Development/gascamp.git
if [[ -n ${GASCAMP_READ_TOKEN:-} ]]; then
  auth=$(printf 'x-access-token:%s' "$GASCAMP_READ_TOKEN" | base64 | tr -d '\n')
  git -C "$repo" -c "http.https://github.com/.extraheader=AUTHORIZATION: basic $auth" fetch --depth 1 origin "$revision"
  unset auth GASCAMP_READ_TOKEN
else
  git -C "$repo" fetch --depth 1 origin "$revision"
fi
test "$(git -C "$repo" rev-parse FETCH_HEAD)" = "$revision" || die "fetched revision mismatch"
git -C "$repo" checkout -q --detach "$revision"
test "$(git -C "$repo" rev-parse HEAD)" = "$revision" || die "HEAD revision mismatch"
test -z "$(git -C "$repo" status --porcelain=v1 --untracked-files=all)" || die "dirty or extra source checkout"
test ! -e "$repo/.gitmodules" || die "submodules require individual immutable locks"
test -z "$(git -C "$repo" ls-files -s | awk '$1 == 160000')" || die "submodule gitlink ambiguity"
git_tree=$(git -C "$repo" rev-parse 'HEAD^{tree}')
test "$git_tree" = "$git_tree_expected" || die "pinned commit tree mismatch"
mkdir -p "$tree/source" "$tree/.cargo"
git -C "$repo" archive --format=tar HEAD | tar -xf - -C "$tree/source"
python3 - "$tree/source" <<'PY'
import re,sys,tomllib
from pathlib import Path
root=Path(sys.argv[1]); lock=tomllib.loads((root/'Cargo.lock').read_text()); sources={p.get('source') for p in lock.get('package',[]) if isinstance(p,dict)}
for cargo in root.rglob('Cargo.toml'):
 doc=tomllib.loads(cargo.read_text())
 for section in ('dependencies','dev-dependencies','build-dependencies'):
  for value in doc.get(section,{}).values():
   if isinstance(value,dict) and 'git' in value:
    rev=value.get('rev')
    if not isinstance(rev,str) or not re.fullmatch('[0-9a-f]{40}',rev) or not any(s and s.startswith('git+'+value['git']) and s.endswith('#'+rev) for s in sources): raise SystemExit('gascamp bundle: unlocked git dependency')
PY
(cd "$tree/source" && cargo vendor --locked "$tree/vendor" >"$work/vendor-config")
cat >"$tree/.cargo/config.toml" <<'EOF'
[net]
offline = true

[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF
python3 - "$tree/source" "$output/source-tree.tsv" "$tree/vendor" "$output/vendor-tree.tsv" <<'PY'
import hashlib,stat,sys
from pathlib import Path
for base,out in ((Path(sys.argv[1]),Path(sys.argv[2])),(Path(sys.argv[3]),Path(sys.argv[4]))):
 rows=[]
 for p in sorted(base.rglob('*')):
  q=p.relative_to(base).as_posix()
  if p.is_symlink():
   target=p.readlink().as_posix(); b=target.encode(); rows.append(f'{q}\tsymlink\t0777\t{len(b)}\t{hashlib.sha256(b).hexdigest()}\t{target}')
  elif p.is_file():
   b=p.read_bytes(); rows.append(f'{q}\tfile\t{stat.S_IMODE(p.stat().st_mode):04o}\t{len(b)}\t{hashlib.sha256(b).hexdigest()}\t-')
 out.write_text('\n'.join(rows)+'\n')
PY
cat >"$output/provenance.env" <<EOF
REVISION=$revision
FETCHED_HEAD=$revision
GIT_TREE=$git_tree
SOURCE_MANIFEST_SHA256=$(sha256sum "$output/source-tree.tsv" | cut -d' ' -f1)
VENDOR_MANIFEST_SHA256=$(sha256sum "$output/vendor-tree.tsv" | cut -d' ' -f1)
CONFIG_SHA256=$(sha256sum "$tree/.cargo/config.toml" | cut -d' ' -f1)
CARGO_VENDOR_LOCKED=true
PLATFORM=linux/arm64
SUBMODULES=absent
EOF
verify_evidence "$output"
python3 - "$output" <<'PY'
import hashlib,json,sys
from pathlib import Path
r=Path(sys.argv[1]); files=[]
for p in sorted(r.rglob('*')):
 if p.name in ('bundle-manifest.json','gascamp-source-vendor-linux-arm64.tar.zst','gascamp-source-vendor-linux-arm64.tar.zst.sha256','gascamp-source-vendor-linux-arm64.tar.zst.size'): continue
 relative=p.relative_to(r).as_posix()
 if p.is_symlink(): files.append({'kind':'symlink','path':relative,'target':p.readlink().as_posix()})
 elif p.is_dir(): files.append({'kind':'directory','path':relative})
 elif p.is_file():
  b=p.read_bytes(); files.append({'kind':'file','path':relative,'size':len(b),'sha256':hashlib.sha256(b).hexdigest()})
 else: raise SystemExit('gascamp bundle: unsupported archive entry')
(r/'bundle-manifest.json').write_text(json.dumps({'version':1,'platform':'linux/arm64','files':files},sort_keys=True,separators=(',',':'))+'\n')
PY
find "$output" -mindepth 1 ! -name bundle-manifest.json -printf '%P\n' | LC_ALL=C sort >"$work/archive-files"
tar --no-recursion --format=posix --pax-option=delete=atime,delete=ctime --owner=0 --group=0 --numeric-owner --mtime=@0 -C "$output" -cf "$work/bundle.tar" bundle-manifest.json --files-from="$work/archive-files"
zstd --threads=1 --no-progress -19 "$work/bundle.tar" -o "$output/$archive_name"
sha256sum "$output/$archive_name" | cut -d' ' -f1 >"$output/$archive_name.sha256"
wc -c <"$output/$archive_name" | tr -d ' ' >"$output/$archive_name.size"
