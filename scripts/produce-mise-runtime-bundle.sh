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
expected={"elixir":"1.20.2-otp-29","erlang":"29.0.3","go":"1.26.5","java":"25.0.2","node":"24.18.0","python":"3.14.6","ruby":"3.4.10","rust":"1.97.0"}
p=env(root/"provenance.env")
required={"PLATFORM","MISE_VERSION","MISE_SHA256","CONFIG_SHA256","BASE_IMAGE","BASE_ATTESTATION_SHA256"}
if set(p)!=required: fail("provenance fields differ from exact schema")
if p["PLATFORM"]!="linux/arm64": fail("wrong platform")
if p["MISE_VERSION"]!="2026.5.0": fail("wrong mise version")
if p["MISE_SHA256"]!="fba7c8a383cf3c59eb5a9995d5299fd2c78eba7eb1daace48d75fe491362f79a": fail("wrong mise digest")
if p["CONFIG_SHA256"]!="b72f66102d09e065b3778c0d6dd52c77a3ef404c2687d910c943d5682cb3063f": fail("wrong config digest")
if p["BASE_IMAGE"]!="ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab": fail("wrong base image digest")
attestation=read(root/"base-attestation.env")
if hashlib.sha256(attestation.encode()).hexdigest()!=p["BASE_ATTESTATION_SHA256"]: fail("base attestation digest mismatch")
base=env(root/"base-attestation.env")
if base.get("IMAGE_DIGEST")!=p["BASE_IMAGE"] or base.get("PLATFORM")!="linux/arm64" or base.get("INVOCATION")!="docker-run-read-only-attestation-v1": fail("invalid producer-independent base attestation")
if not re.fullmatch("[0-9a-f]{40}",base.get("WORKFLOW_COMMIT","")) or not re.fullmatch("sha256:[0-9a-f]{64}",base.get("IMAGE_ID","")) or not re.fullmatch("[0-9a-f]{64}",base.get("UBUNTU_BUNDLE_SHA256","")): fail("invalid base attestation receipt")
try: current=json.loads(read(root/"mise-current.json"))
except json.JSONDecodeError: fail("invalid mise current JSON")
if set(current)!=set(expected): fail("mise current does not contain the exact seven runtimes and Erlang dependency")
for tool,version in expected.items():
 value=current[tool]
 actual=value.get("version") if isinstance(value,dict) else value
 if actual!=version: fail("wrong tool version for "+tool)
downloads=[line for line in read(root/"upstream-artifacts.tsv").splitlines() if line]
if downloads!=sorted(set(downloads)): fail("upstream provenance is not in canonical order")
seen=set()
for line in downloads:
 cols=line.split("\t")
 if len(cols)!=8: fail("invalid upstream artifact provenance")
 tool,version,backend,url,sha,size,path,event_path=cols
 if expected.get(tool)!=version: fail("upstream provenance has wrong tool/version")
 if not backend or not url.startswith("https://"): fail("upstream artifact URL/backend provenance missing")
 if not path.startswith("downloads/") or PurePosixPath(path).is_absolute() or ".." in PurePosixPath(path).parts: fail("unsafe downloaded artifact path")
 if not event_path.startswith("/opt/gascan/mise/downloads/") or ".." in PurePosixPath(event_path).parts: fail("unsafe actual download event path")
 artifact=root/path
 if not re.fullmatch("[0-9a-f]{64}",sha) or not size.isdigit() or not artifact.is_file() or artifact.stat().st_size!=int(size) or hashlib.sha256(artifact.read_bytes()).hexdigest()!=sha: fail("downloaded artifact checksum/size provenance invalid")
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
if not isinstance(lock_tools,dict) or set(lock_tools)!=set(expected): fail("mise lock does not contain the exact seven runtimes and Erlang dependency")
for tool,version in expected.items():
 entries=lock_tools[tool] if isinstance(lock_tools[tool],list) else [lock_tools[tool]]
 if not any(isinstance(entry,dict) and entry.get("version")==version for entry in entries): fail("mise lock has wrong tool version")
 records={(backend,url,sha) for backend,url,sha in lock_records(lock_tools[tool]) if backend and url.startswith("https://") and re.fullmatch("[0-9a-f]{64}",sha)}
 if not records: fail("mise lock provenance missing for "+tool)
 for backend,url,sha in records:
  matches=[line for line in downloads if line.split("\t")[:5]==[tool,version,backend,url,sha]]
  if len(matches)!=1: fail("upstream artifacts differ from mise lock provenance")
  locked_rows.extend(matches)
if downloads!=sorted(set(locked_rows)): fail("upstream artifacts differ from mise lock provenance")
event_re=re.compile(r'^DEBUG GET Downloading (https://\S+) to (/\S+?)(?: checksum=([0-9a-f]{64}) size=([0-9]+))?$')
events=[]
logs=root/"mise-install-logs"
if not logs.is_dir() or sorted(p.name for p in logs.glob("*.log"))!=[tool+".log" for tool in sorted(expected)]: fail("actual mise install logs are missing or extra")
for tool in sorted(expected):
 for line in read(logs/(tool+".log")).splitlines():
  match=event_re.fullmatch(line)
  if not match: fail("invalid sanitized actual download event")
  events.append((tool,match.group(1),match.group(2),match.group(3),match.group(4)))
for line in downloads:
 tool,version,backend,url,sha,size,path,event_path=line.split("\t")
 matches=[event for event in events if event[:3]==(tool,url,event_path)]
 if len(matches)!=1: fail("actual download event is absent or mismatched")
 emitted_sha,emitted_size=matches[0][3:]
 if emitted_sha is not None and (emitted_sha!=sha or emitted_size!=size): fail("actual download event checksum/size mismatch")
if len(events)!=len(downloads): fail("actual download event has no locked retained artifact")
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
entrypoints={"elixir":"bin/elixir","erlang":"bin/erl","go":"bin/go","java":"bin/java","node":"bin/node","python":"bin/python","ruby":"bin/ruby","rust":"bin/rustc"}
for tool,version in expected.items():
 path=f"opt/gascan/mise/installs/{tool}/{version}/{entrypoints[tool]}"
 rec=actual.get(path)
 for _ in range(16):
  if rec is None or rec[0]!="symlink": break
  path=posixpath.normpath(posixpath.join(posixpath.dirname(path),rec[6])); rec=actual.get(path)
 if rec is None or rec[0]!="file" or rec[1]!=0o755: fail("missing executable for "+tool)
 body=tar.extractfile if False else None
 data=None
 # Validate native AArch64 ELF entrypoints, or a narrowly reviewed shebang.
 with tarfile.open(fileobj=io.BytesIO(raw),mode="r:") as check_tar:
  extracted=check_tar.extractfile(path)
  if extracted is not None: data=extracted.read(256)
 if data is None: fail("missing executable format for "+tool)
 if data.startswith(b"\x7fELF"):
  if len(data)<20 or data[4:7]!=b"\x02\x01\x01" or int.from_bytes(data[18:20],"little")!=183: fail("wrong executable format/platform for "+tool)
 elif len(data)<128 or not any(data.startswith(line) for line in (b"#!/bin/sh\n",b"#!/usr/bin/env sh\n",b"#!/usr/bin/env bash\n")): fail("unreviewed executable format for "+tool)
sha_file=root/"mise-runtimes-linux-arm64.tar.zst.sha256"; size_file=root/"mise-runtimes-linux-arm64.tar.zst.size"
if read(sha_file).strip()!=hashlib.sha256(archive.read_bytes()).hexdigest(): fail("archive checksum sidecar mismatch")
if read(size_file).strip()!=str(archive.stat().st_size): fail("archive size sidecar mismatch")
if sys.platform.startswith("linux") and os.uname().machine=="aarch64":
 import tempfile
 with tempfile.TemporaryDirectory() as directory:
  with tarfile.open(fileobj=io.BytesIO(raw),mode="r:") as run_tar: run_tar.extractall(directory,filter="data")
  commands={"elixir":["--version"],"erlang":["-noshell","-eval",'io:format("~s", [erlang:system_info(otp_release)]), halt().'],"go":["version"],"java":["-version"],"node":["--version"],"python":["--version"],"ruby":["--version"],"rust":["--version"]}
  runtime_path=os.pathsep.join(str(Path(directory)/f"opt/gascan/mise/installs/{tool}/{version}/bin") for tool,version in expected.items())
  runtime_env={**os.environ,"PATH":runtime_path+os.pathsep+os.environ.get("PATH","")}
  for tool,version in expected.items():
   executable=Path(directory)/f"opt/gascan/mise/installs/{tool}/{version}"/entrypoints[tool]
   result=subprocess.run([str(executable),*commands[tool]],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,check=False,timeout=30,env=runtime_env)
   output=result.stdout.strip()
   valid={
    "node":output=="v"+version,
    "python":output=="Python "+version,
    "go":output=="go version go"+version+" linux/arm64",
    "rust":output.startswith("rustc "+version+" "),
    "ruby":output.startswith("ruby "+version+" "),
    "java":('version "'+version+'"') in output,
    "elixir":"Elixir 1.20.2" in output and "Erlang/OTP 29" in output,
    "erlang":output=="29",
   }[tool]
   if result.returncode!=0 or not valid: fail("runtime version execution mismatch for "+tool)
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
attestation=${GASCAN_BASE_ATTESTATION:-/run/gascan/base-attestation.env}
[[ -f $attestation ]] || die "producer-independent base attestation is required"
python3 - "$attestation" <<'PY'
import re,sys
p={}
for line in open(sys.argv[1]):
 key,value=line.rstrip("\n").split("=",1); p[key]=value
if p.get("IMAGE_DIGEST")!="ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab" or p.get("PLATFORM")!="linux/arm64" or p.get("INVOCATION")!="docker-run-read-only-attestation-v1" or not re.fullmatch(r"[0-9a-f]{40}",p.get("WORKFLOW_COMMIT","")) or not re.fullmatch(r"sha256:[0-9a-f]{64}",p.get("IMAGE_ID","")) or not re.fullmatch(r"[0-9a-f]{64}",p.get("UBUNTU_BUNDLE_SHA256","")): raise SystemExit("mise runtime bundle: invalid producer-independent base attestation")
PY
awk -v target="$attestation" '$5 == target && $6 ~ /(^|,)ro(,|$)/ {found=1} END {exit !found}' /proc/self/mountinfo || die "base attestation must be a separate read-only mount"
for command in curl find jq python3 sha256sum tar zstd; do command -v "$command" >/dev/null || die "exact pinned base lacks required producer command: $command"; done
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
expected={"elixir":"1.20.2-otp-29","erlang":"29.0.3","go":"1.26.5","java":"25.0.2","node":"24.18.0","python":"3.14.6","ruby":"3.4.10","rust":"1.97.0"}
assert lock["mise"]=={"version":"2026.5.0","url":"https://github.com/jdx/mise/releases/download/v2026.5.0/mise-v2026.5.0-linux-arm64","sha256":"fba7c8a383cf3c59eb5a9995d5299fd2c78eba7eb1daace48d75fe491362f79a"}
assert lock["tools"]==expected and config["tools"]==expected
assert hashlib.sha256(open(sys.argv[2],"rb").read()).hexdigest()=="b72f66102d09e065b3778c0d6dd52c77a3ef404c2687d910c943d5682cb3063f"
assert lock["base_image"]=="ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab"
PY

mise="$work/mise"
curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 'https://github.com/jdx/mise/releases/download/v2026.5.0/mise-v2026.5.0-linux-arm64' --output "$mise"
test "$(sha256sum "$mise" | cut -d' ' -f1)" = fba7c8a383cf3c59eb5a9995d5299fd2c78eba7eb1daace48d75fe491362f79a || die "mise checksum mismatch"
chmod 0555 "$mise"
cp -- "$config" "$work/config.toml"
touch "$work/config.lock"
export MISE_DATA_DIR=/opt/gascan/mise MISE_CACHE_DIR="$work/cache" MISE_GLOBAL_CONFIG_FILE="$work/config.toml" MISE_CONFIG_DIR="$work/empty-config" MISE_YES=1 MISE_LOG_LEVEL=trace MISE_LOCKFILE=1 MISE_LOCKFILE_PLATFORMS=linux-arm64 MISE_ALWAYS_KEEP_DOWNLOAD=1 NO_COLOR=1
mkdir -p "$MISE_CONFIG_DIR"
"$mise" install --yes erlang@29.0.3 2>"$work/logs/erlang.log" || die "mise failed to install Erlang dependency"
"$mise" exec erlang@29.0.3 -- "$mise" install --yes elixir@1.20.2-otp-29 2>"$work/logs/elixir.log" || die "mise failed to install Elixir with exact Erlang dependency"
tools=(go java node python ruby rust)
for tool in "${tools[@]}"; do
  rm -rf -- "$work/cache"
  mkdir -p "$work/cache"
  "$mise" install --yes "$tool" 2>"$work/logs/$tool.log" || die "mise failed to install $tool"
done
"$mise" lock --platform linux-arm64 >/dev/null || die "mise failed to finalize upstream lock provenance"
"$mise" ls --current --installed --json \
  | jq --exit-status --compact-output --sort-keys 'if ((keys|sort) != ["elixir","erlang","go","java","node","python","ruby","rust"]) then error("unexpected mise tool set") else to_entries | map(if ((.value|type)!="array") or ((.value|length)!=1) or (.value[0].installed != true) or (.value[0].active != true) or ((.value[0].version|type)!="string") or (.value[0].version=="") then error("invalid mise ls record") else {key:.key,value:.value[0].version} end) | from_entries end' \
  >"$output/mise-current.json" || die "mise ls normalization failed"

python3 - "$work/config.lock" "$lock" "$work/logs" /opt/gascan/mise/downloads "$output" <<'PY'
import hashlib,re,shutil,sys,tomllib,urllib.parse
from pathlib import Path
mise_lock=tomllib.loads(Path(sys.argv[1]).read_text()); versions=tomllib.loads(Path(sys.argv[2]).read_text())["tools"]; logs=Path(sys.argv[3]); downloads=Path(sys.argv[4]); output=Path(sys.argv[5]); rows=[]
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
 raw_lines=(logs/(tool+".log")).read_text(errors="strict").splitlines()
 event_re=re.compile(r'^.*DEBUG GET Downloading (?P<url>https://\S+) to (?P<path>/\S+?)(?: checksum=(?P<sha>[0-9a-f]{64}) size=(?P<size>[0-9]+))?$')
 events=[]
 for line in raw_lines:
  match=event_re.fullmatch(line)
  if match:
   parsed=urllib.parse.urlsplit(match.group("url"))
   if parsed.username or parsed.password or parsed.query or parsed.fragment: raise SystemExit("mise runtime bundle: unsafe credentials/query in download trace")
   events.append((line,match.group("url"),Path(match.group("path")),match.group("sha"),match.group("size")))
 (output/"mise-install-logs").mkdir(exist_ok=True)
 canonical_events=[]
 for _,event_url,event_path,event_sha,event_size in events:
  canonical=f"DEBUG GET Downloading {event_url} to {event_path}"
  if event_sha is not None: canonical+=f" checksum={event_sha} size={event_size}"
  canonical_events.append(canonical)
 (output/"mise-install-logs"/(tool+".log")).write_text("\n".join(canonical_events)+("\n" if canonical_events else ""))
 for backend,url,sha in sorted(normalized):
  matching=[event for event in events if event[1]==url]
  if len(matching)!=1: raise SystemExit("mise runtime bundle: locked URL lacks one unambiguous actual download event for "+tool)
  _,_,path,emitted_sha,emitted_size=matching[0]
  try: path.resolve().relative_to(downloads.resolve())
  except ValueError: raise SystemExit("mise runtime bundle: actual download path escapes retained download root")
  if not path.is_file(): raise SystemExit("mise runtime bundle: actual downloaded path was not retained for "+tool)
  body=path.read_bytes()
  if hashlib.sha256(body).hexdigest()!=sha: raise SystemExit("mise runtime bundle: actual downloaded bytes differ from backend lock checksum for "+tool)
  if emitted_sha is not None and (emitted_sha!=sha or emitted_size!=str(len(body))): raise SystemExit("mise runtime bundle: emitted download checksum/size differs from retained bytes")
  relative=Path("downloads")/(tool+"-"+sha)
  (output/"downloads").mkdir(exist_ok=True); shutil.copyfile(path,output/relative)
  rows.append("\t".join((tool,versions[tool],backend,url,sha,str(len(body)),relative.as_posix(),str(path))))
 if len(events)!=len(normalized): raise SystemExit("mise runtime bundle: actual download event has no backend lock record for "+tool)
canonical="\n".join(sorted(set(rows)))+"\n"
(output/"upstream-artifacts.tsv").write_text(canonical)
PY
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
CONFIG_SHA256=b72f66102d09e065b3778c0d6dd52c77a3ef404c2687d910c943d5682cb3063f
BASE_IMAGE=ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab
EOF
printf 'BASE_ATTESTATION_SHA256=%s\n' "$(sha256sum "$attestation" | cut -d' ' -f1)" >>"$output/provenance.env"
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
