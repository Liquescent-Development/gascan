#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
lock="$root/images/workspace/versions.lock"
config="$root/images/workspace/etc/mise/config.toml"
archive_name=mise-runtimes-linux-arm64.tar.zst

die() { printf 'mise runtime bundle: %s\n' "$*" >&2; exit 1; }

verify_evidence() {
  python3 - "$1" <<'PY'
import hashlib,io,json,os,posixpath,re,stat,subprocess,sys,tarfile,tomllib
from pathlib import Path,PurePosixPath
root=Path(sys.argv[1])
def fail(message): raise SystemExit("mise runtime bundle: "+message)
def read(path):
 try: return path.read_text(encoding="utf-8")
 except (OSError,UnicodeError): fail("missing or invalid "+path.name)
def env(path):
 out={}
 for line in read(path).splitlines():
  if not line or "=" not in line: fail("invalid provenance")
  key,value=line.split("=",1)
  if key in out: fail("duplicate provenance field")
  out[key]=value
 return out
expected={"elixir":"1.20.2-otp-29","go":"1.26.5","java":"25.0.2","node":"24.18.0","python":"3.14.6","ruby":"3.4.10","rust":"1.97.0"}
p=env(root/"provenance.env")
required={"PLATFORM","MISE_VERSION","MISE_SHA256","CONFIG_SHA256","BASE_IMAGE"}
if set(p)!=required: fail("provenance fields differ from exact schema")
if p["PLATFORM"]!="linux/arm64": fail("wrong platform")
if p["MISE_VERSION"]!="2026.5.0": fail("wrong mise version")
if p["MISE_SHA256"]!="fba7c8a383cf3c59eb5a9995d5299fd2c78eba7eb1daace48d75fe491362f79a": fail("wrong mise digest")
if p["CONFIG_SHA256"]!="687b22340b2f0e48d07bc5521fbaa39749f2ac1554e1bebc6848f92296ac663b": fail("wrong config digest")
if p["BASE_IMAGE"]!="ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab": fail("wrong base image digest")
try: current=json.loads(read(root/"mise-current.json"))
except json.JSONDecodeError: fail("invalid mise current JSON")
if set(current)!=set(expected): fail("mise current does not contain exact seven tools")
for tool,version in expected.items():
 value=current[tool]
 actual=value.get("version") if isinstance(value,dict) else value
 if actual!=version: fail("wrong tool version for "+tool)
downloads=[line for line in read(root/"upstream-artifacts.tsv").splitlines() if line]
if downloads!=sorted(set(downloads)): fail("upstream provenance is not in canonical order")
seen=set()
for line in downloads:
 cols=line.split("\t")
 if len(cols)!=5: fail("invalid upstream artifact provenance")
 tool,version,backend,url,sha=cols
 if expected.get(tool)!=version: fail("upstream provenance has wrong tool/version")
 if not backend or not url.startswith("https://"): fail("upstream artifact URL/backend provenance missing")
 if not re.fullmatch("[0-9a-f]{64}",sha): fail("upstream artifact checksum provenance invalid")
 seen.add(tool)
if seen!=set(expected): fail("upstream provenance missing locked runtime")
try: mise_lock=tomllib.loads(read(root/"mise.lock"))
except (tomllib.TOMLDecodeError,TypeError): fail("invalid mise lock provenance")
def lock_records(value,backend=""):
 if isinstance(value,list):
  for item in value: yield from lock_records(item,backend)
 elif isinstance(value,dict):
  backend=str(value.get("backend",backend))
  url=value.get("url"); checksum=value.get("checksum")
  if isinstance(url,str) and isinstance(checksum,str): yield backend,url,checksum.removeprefix("sha256:")
  for child in value.values(): yield from lock_records(child,backend)
locked_rows=[]
lock_tools=mise_lock.get("tools",{})
if not isinstance(lock_tools,dict) or set(lock_tools)!=set(expected): fail("mise lock does not contain exact seven tools")
for tool,version in expected.items():
 entries=lock_tools[tool] if isinstance(lock_tools[tool],list) else [lock_tools[tool]]
 if not any(isinstance(entry,dict) and entry.get("version")==version for entry in entries): fail("mise lock has wrong tool version")
 records={(backend,url,sha) for backend,url,sha in lock_records(lock_tools[tool]) if backend and url.startswith("https://") and re.fullmatch("[0-9a-f]{64}",sha)}
 if not records: fail("mise lock provenance missing for "+tool)
 for backend,url,sha in records: locked_rows.append("\t".join((tool,version,backend,url,sha)))
if downloads!=sorted(set(locked_rows)): fail("upstream artifacts differ from mise lock provenance")
manifest=[line for line in read(root/"mise-runtimes-linux-arm64.manifest.tsv").splitlines() if line]
if manifest!=sorted(set(manifest)): fail("tree manifest is not in canonical order")
declared={}
for line in manifest:
 cols=line.split("\t")
 if len(cols)!=8: fail("invalid tree manifest")
 path,kind,mode,uid,gid,size,sha,target=cols
 pure=PurePosixPath(path)
 if pure.is_absolute() or ".." in pure.parts or not (path in ("opt","opt/gascan","opt/gascan/mise") or path.startswith("opt/gascan/mise/")): fail("unsafe manifest path")
 try: mode_i=int(mode,8); uid_i=int(uid); gid_i=int(gid); size_i=int(size)
 except ValueError: fail("invalid tree metadata")
 if uid_i!=0 or gid_i!=0: fail("root ownership evidence mismatch")
 if kind != "symlink" and mode_i & 0o022: fail("group/world writable tree entry")
 if kind == "directory" and mode_i != 0o755: fail("non-canonical directory mode")
 if kind == "file" and mode_i not in (0o644,0o755): fail("non-canonical file mode")
 if kind not in ("file","directory","symlink"): fail("unsupported tree entry")
 declared[path]=(kind,mode_i,uid_i,gid_i,size_i,sha,target)
archive=root/"mise-runtimes-linux-arm64.tar.zst"
try: raw=subprocess.run(["zstd","--decompress","--stdout",str(archive)],stdout=subprocess.PIPE,stderr=subprocess.PIPE,check=True).stdout
except (OSError,subprocess.CalledProcessError): fail("invalid runtime archive")
actual={}
try:
 with tarfile.open(fileobj=io.BytesIO(raw),mode="r:") as tar:
  for member in tar:
   name=member.name.rstrip("/")
   pure=PurePosixPath(name)
   if pure.is_absolute() or ".." in pure.parts or not (name in ("opt","opt/gascan","opt/gascan/mise") or name.startswith("opt/gascan/mise/")):
    fail("unsafe archive entry")
   if member.isreg():
    body=tar.extractfile(member).read(); record=("file",member.mode,member.uid,member.gid,len(body),hashlib.sha256(body).hexdigest(),"-")
   elif member.isdir(): record=("directory",member.mode,member.uid,member.gid,0,"-","-")
   elif member.issym():
    target=member.linkname
    if target.startswith("/") or ".." in PurePosixPath(target).parts: fail("unsafe archive symlink")
    record=("symlink",member.mode,member.uid,member.gid,0,"-",target)
   else: fail("unsafe archive entry type")
   if name in actual: fail("duplicate archive entry")
   actual[name]=record
except (tarfile.TarError,OSError): fail("invalid runtime archive")
if actual!=declared: fail("archive tree differs from canonical manifest")
entrypoints={"elixir":"bin/elixir","go":"bin/go","java":"bin/java","node":"bin/node","python":"bin/python","ruby":"bin/ruby","rust":"bin/rustc"}
for tool,version in expected.items():
 path=f"opt/gascan/mise/installs/{tool}/{version}/{entrypoints[tool]}"
 rec=actual.get(path)
 for _ in range(16):
  if rec is None or rec[0]!="symlink": break
  path=posixpath.normpath(posixpath.join(posixpath.dirname(path),rec[6])); rec=actual.get(path)
 if rec is None or rec[0]!="file" or rec[1]!=0o755: fail("missing executable for "+tool)
sha_file=root/"mise-runtimes-linux-arm64.tar.zst.sha256"; size_file=root/"mise-runtimes-linux-arm64.tar.zst.size"
if read(sha_file).strip()!=hashlib.sha256(archive.read_bytes()).hexdigest(): fail("archive checksum sidecar mismatch")
if read(size_file).strip()!=str(archive.stat().st_size): fail("archive size sidecar mismatch")
PY
}

if [[ ${1:-} == --verify-evidence ]]; then
  [[ $# == 2 ]] || die "usage: $0 --verify-evidence DIRECTORY"
  verify_evidence "$2"
  exit
fi

[[ $# == 1 ]] || die "usage: $0 OUTPUT_DIRECTORY"
[[ $(uname -s) == Linux && $(uname -m) == aarch64 ]] || die "producer requires connected Linux ARM64"
[[ $(id -u) == 0 ]] || die "producer must run as root; validation must run unprivileged"
for command in curl find python3 sha256sum tar zstd; do command -v "$command" >/dev/null || die "missing command: $command"; done
tar --help 2>&1 | grep -q -- --sort || die "GNU tar with deterministic sorting is required"
output=$1
mkdir -p "$output"
[[ -d $output && -z $(find "$output" -mindepth 1 -print -quit) ]] || die "output must be an empty directory"
work=$(mktemp -d)
trap 'rm -rf -- "$work" /opt/gascan/mise' EXIT
[[ ! -e /opt/gascan/mise ]] || die "/opt/gascan/mise must not already exist"
mkdir -p /opt/gascan/mise "$work/cache" "$work/logs"

python3 - "$lock" "$config" <<'PY'
import hashlib,sys,tomllib
lock=tomllib.loads(open(sys.argv[1],"rb").read().decode()); config=tomllib.loads(open(sys.argv[2],"rb").read().decode())
expected={"elixir":"1.20.2-otp-29","go":"1.26.5","java":"25.0.2","node":"24.18.0","python":"3.14.6","ruby":"3.4.10","rust":"1.97.0"}
assert lock["mise"]=={"version":"2026.5.0","url":"https://github.com/jdx/mise/releases/download/v2026.5.0/mise-v2026.5.0-linux-arm64","sha256":"fba7c8a383cf3c59eb5a9995d5299fd2c78eba7eb1daace48d75fe491362f79a"}
assert lock["tools"]==expected and config["tools"]==expected
assert hashlib.sha256(open(sys.argv[2],"rb").read()).hexdigest()=="687b22340b2f0e48d07bc5521fbaa39749f2ac1554e1bebc6848f92296ac663b"
assert lock["base_image"]=="ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab"
PY

mise="$work/mise"
curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 'https://github.com/jdx/mise/releases/download/v2026.5.0/mise-v2026.5.0-linux-arm64' --output "$mise"
test "$(sha256sum "$mise" | cut -d' ' -f1)" = fba7c8a383cf3c59eb5a9995d5299fd2c78eba7eb1daace48d75fe491362f79a || die "mise checksum mismatch"
chmod 0555 "$mise"
cp -- "$config" "$work/config.toml"
touch "$work/config.lock"
export MISE_DATA_DIR=/opt/gascan/mise MISE_CACHE_DIR="$work/cache" MISE_GLOBAL_CONFIG_FILE="$work/config.toml" MISE_CONFIG_DIR="$work/empty-config" MISE_YES=1 MISE_LOG_LEVEL=trace MISE_LOCKFILE=1 MISE_LOCKFILE_PLATFORMS=linux-arm64
mkdir -p "$MISE_CONFIG_DIR"
tools=(elixir go java node python ruby rust)
for tool in "${tools[@]}"; do
  rm -rf -- "$work/cache"
  mkdir -p "$work/cache"
  "$mise" install --yes "$tool" 2>"$work/logs/$tool.log" || die "mise failed to install $tool"
done
"$mise" lock --platform linux-arm64 >/dev/null || die "mise failed to finalize upstream lock provenance"
"$mise" current --json >"$output/mise-current.json"

python3 - "$work/config.lock" "$lock" <<'PY'
import re,sys,tomllib
from pathlib import Path
mise_lock=tomllib.loads(Path(sys.argv[1]).read_text()); versions=tomllib.loads(Path(sys.argv[2]).read_text())["tools"]; rows=[]
def records(value,backend=""):
 if isinstance(value,list):
  for item in value: yield from records(item,backend)
 elif isinstance(value,dict):
  backend=str(value.get("backend",backend))
  url=value.get("url"); checksum=value.get("checksum")
  if isinstance(url,str) and isinstance(checksum,str): yield backend,url,checksum
  for child in value.values(): yield from records(child,backend)
for tool in sorted(versions):
 found=set(records(mise_lock.get("tools",{}).get(tool,{})))
 normalized=set()
 for backend,url,checksum in found:
  sha=checksum.removeprefix("sha256:")
  if backend and url.startswith("https://") and re.fullmatch(r"[0-9a-f]{64}",sha): normalized.add((backend,url,sha))
 if not normalized: raise SystemExit("mise runtime bundle: mise lock lacks upstream URL/checksum/backend for "+tool)
 for backend,url,sha in sorted(normalized): rows.append("\t".join((tool,versions[tool],backend,url,sha)))
Path(sys.argv[1]).with_name("upstream-artifacts.tsv").write_text("\n".join(sorted(set(rows)))+"\n")
PY
cp "$work/upstream-artifacts.tsv" "$output/upstream-artifacts.tsv"
cp "$work/config.lock" "$output/mise.lock"
rm -rf -- "$work/cache" /opt/gascan/mise/downloads
find /opt/gascan/mise -exec chown -h 0:0 {} +
find /opt/gascan/mise -type d -exec chmod 0755 {} +
find /opt/gascan/mise -type f -perm /111 -exec chmod 0755 {} +
find /opt/gascan/mise -type f ! -perm /111 -exec chmod 0644 {} +
find /opt/gascan/mise -exec touch -h -d @0 {} +

cat >"$output/provenance.env" <<'EOF'
PLATFORM=linux/arm64
MISE_VERSION=2026.5.0
MISE_SHA256=fba7c8a383cf3c59eb5a9995d5299fd2c78eba7eb1daace48d75fe491362f79a
CONFIG_SHA256=687b22340b2f0e48d07bc5521fbaa39749f2ac1554e1bebc6848f92296ac663b
BASE_IMAGE=ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab
EOF
python3 - / "$output/mise-runtimes-linux-arm64.manifest.tsv" <<'PY'
import hashlib,os,stat,sys
from pathlib import Path
root=Path(sys.argv[1]); rows=[]
for path in sorted((root/"opt/gascan/mise").rglob("*")):
 rel=path.relative_to(root).as_posix(); info=path.lstat(); mode=stat.S_IMODE(info.st_mode)
 if path.is_symlink(): rows.append(f"{rel}\tsymlink\t{mode:04o}\t{info.st_uid}\t{info.st_gid}\t0\t-\t{os.readlink(path)}")
 elif path.is_dir(): rows.append(f"{rel}\tdirectory\t{mode:04o}\t{info.st_uid}\t{info.st_gid}\t0\t-\t-")
 elif path.is_file():
  body=path.read_bytes(); rows.append(f"{rel}\tfile\t{mode:04o}\t{info.st_uid}\t{info.st_gid}\t{len(body)}\t{hashlib.sha256(body).hexdigest()}\t-")
 else: raise SystemExit("mise runtime bundle: unsupported tree entry")
Path(sys.argv[2]).write_text("\n".join(rows)+"\n")
PY
for prefix in /opt /opt/gascan /opt/gascan/mise; do
  mode=$(stat -c '%a' "$prefix"); printf '%s\tdirectory\t%04o\t0\t0\t0\t-\t-\n' "${prefix#/}" "$((8#$mode))"
done >>"$output/mise-runtimes-linux-arm64.manifest.tsv"
LC_ALL=C sort -o "$output/mise-runtimes-linux-arm64.manifest.tsv" "$output/mise-runtimes-linux-arm64.manifest.tsv"
tar --sort=name --format=posix --pax-option=delete=atime,delete=ctime --owner=0 --group=0 --numeric-owner --mtime=@0 -C / -cf "$work/bundle.tar" opt
zstd --threads=1 --no-progress -19 "$work/bundle.tar" -o "$output/$archive_name"
sha256sum "$output/$archive_name" | cut -d' ' -f1 >"$output/$archive_name.sha256"
wc -c <"$output/$archive_name" | tr -d ' ' >"$output/$archive_name.size"
verify_evidence "$output"
