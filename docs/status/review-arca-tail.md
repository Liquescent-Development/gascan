<!--
Committed verbatim as written by the reviewer. Run before Arca PR #59 merged,
to close a gap: the landing review covered `5e11704..4134b54` and three commits
landed after it, one of them ~4,600 lines pinning the kernel build recipe.

M-3, M-4 and m-1 were fixed on the Arca branch. M-1 and M-2 -- pinning the
toolchain, and asserting the required config against the built artefact rather
than against a text file -- are real and are NOT fixed: they are design work,
not edits, and they belong in a follow-up.
-->

# Review: Arca `4134b54..e14be74` (the unreviewed tail of PR #59)

Repository: `/Users/kiener/code/arca`, branch `feat/milestone-4-engine`, head `e14be74`.
Range reviewed: `4134b54549a5de89cfe2c4bf567df1b0c93d7ee3..e14be740ed083fb1e5128269a48fd0631bf9ca5e`
(3 commits, 16 files, +4831/-69).

Read-only review. Nothing was modified, built, pushed, or re-tagged. `swift test` was not run.

Severity summary: **5 Major, 8 Minor, 0 Critical.** No Critical findings — I looked for one
and did not find one. The published bytes are correct; every digest in
`Documentation/RELEASE-ARTIFACTS-gascan-engine-m4.md` and in gascan's `engine/arca-pin.json`
verifies against the staged files on disk. **No finding in this review requires re-cutting the
tag or re-uploading an asset.** Two findings are baked into the tagged tree and can only be
corrected in a later commit (M-4, M-5); the rest are fixable now.

---

## Major findings

### M-1. The toolchain — the largest input to the output — is not pinned by anything

**Files:** `kernel/recipe/image/Dockerfile:1`, `:3-18`, `kernel/recipe/image/sources.list:1-15`

`recipe.sha256` pins the *text of the instructions*. It does not pin what those instructions
fetch. Three unpinned network inputs decide what compiler builds the kernel:

1. `Dockerfile:1` — `FROM ubuntu:focal`. A floating tag, not a digest. `ubuntu:focal` is
   re-pushed on every Ubuntu 20.04 point respin.
2. `Dockerfile:3-18` — `apt-get install -y autoconf bc binutils-multiarch
   binutils-aarch64-linux-gnu bison flex gcc xz-utils gcc-aarch64-linux-gnu git libncurses-dev
   make openssl python-is-python3` with no version constraint on any package.
3. `sources.list` — `http://ports.ubuntu.com/` and `http://archive.ubuntu.com/ubuntu/`, the
   live pockets including `focal-updates` and `focal-backports`. Not `snapshot.ubuntu.com`, and
   plain HTTP.

That this matters is not hypothetical — the shipped kernel names the toolchain in its own
banner:

```
$ strings -a /Applications/Arca.app/Contents/Resources/vmlinux | grep -m1 'Linux version 6\.'
Linux version 6.14.9 (root@413d46d8-...) (gcc (Ubuntu 9.4.0-1ubuntu1~20.04.2) 9.4.0,
  GNU ld (GNU Binutils for Ubuntu) 2.34) #1 SMP Wed Nov 19 18:48:14 UTC 2025
```

`gcc 9.4.0-1ubuntu1~20.04.2` and `binutils 2.34` are recorded nowhere in the tree. A rebuild
today gets whatever `focal-updates` currently serves.

**Failure scenario.** Two, and the second is the one that will actually happen. (a) A
`focal-updates` gcc respin changes codegen; a rebuild produces a kernel that differs in
behaviour, not only in build ID, and nothing in the repository can tell the two apart —
`kernel/README.md`'s "do not treat a digest mismatch after a rebuild as evidence of tampering"
then also covers a genuinely different kernel. (b) Ubuntu 20.04 is past standard support. When
focal moves from `archive.ubuntu.com` to `old-releases.ubuntu.com`, `apt-get update` starts
404-ing and the recipe stops building at all — which directly degrades the written offer in
`kernel/README.md` ("available to any third party for the lifetime of the release"), since the
"scripts used to control compilation" that the offer names will no longer run.

**Credit where due:** the documentation does not *claim* the toolchain is pinned.
`kernel/README.md` says "identical source, identical config and identical scripts" — three
things, and all three are true. `recipe.sha256`'s header is likewise exact. This is a real gap,
not a false statement. But `recipe.env:5` says "Every value here is an assertion the build
enforces; nothing falls back if an assertion fails," and a reader arriving at
`build-kernel.sh`'s header ("which source produced this kernel is answerable from a clean
checkout, offline") will over-read the guarantee.

**Suggested fix.** There is a real tension here: `recipe/` is deliberately byte-verbatim
upstream, so editing the Dockerfile breaks the property `recipe.sha256` and `kernel/README.md`
both advertise. The fix that preserves it is to record the toolchain as *observed provenance*
rather than as an edit — add to `recipe.env` the base-image digest and the gcc/binutils versions
that produced the shipped kernel (they are recoverable from the banner above), and have
`build-kernel.sh` report the actual `gcc --version` of the build image and warn loudly on
divergence. If a real pin is wanted later, it is a documented single-line deviation
(`FROM ubuntu@sha256:…`) plus a `.deviations` note, and `recipe.sha256` regenerated.
**Future-release work; does not touch published bytes.**

### M-2. `REQUIRED_CONFIGS` is asserted against the input config, never against the kernel that gets built

**File:** `scripts/build-kernel.sh:44-53`

```bash
for opt in $REQUIRED_CONFIGS; do
    if ! grep -qx -- "$opt" "$RECIPE_DIR/config-arm64"; then
```

This greps a text file. `recipe/build.sh:24` then runs `make olddefconfig`, which is free to
change what that file says, and demonstrably does. Comparing `kernel/recipe/config-arm64`
against the config the shipped kernel carries in its own `IKCONFIG` section:

```
vendored symbols: 3260   shipped symbols: 3440
vendored settings NOT reproduced in the shipped kernel: 148
  CONFIG_ARCH_FORCE_MAX_ORDER: vendored='11' shipped='10'
  CONFIG_CRC64:                vendored='n'  shipped='y'
  CONFIG_CRYPTO_SHA3:          vendored='n'  shipped='y'
  CONFIG_BPFILTER:             vendored='y'  shipped=<ABSENT>
  ... 144 more
olddefconfig additions: 321 symbols
```

(Extracted from `/Applications/Arca.app/Contents/Resources/vmlinux` between `IKCFG_ST` and
`IKCFG_ED`; the config is present because `kernel/recipe/config-arm64:138-139` set
`CONFIG_IKCONFIG=y` and `CONFIG_IKCONFIG_PROC=y`.)

So `olddefconfig` changes 148 settings, including flipping two from `n` to `y`. In *this* build
`CONFIG_TUN=y` and `CONFIG_WIREGUARD=y` survived — I confirmed both are in the shipped kernel's
embedded config, at its lines 2070 and 2053. But nothing in the script checks that.

**Failure scenario.** A future kernel bump makes `CONFIG_WIREGUARD` depend on a symbol the
vendored config does not set. `olddefconfig` silently drops it. `make` succeeds. `vmlinux`
exists. The script prints `✓ CONFIG_WIREGUARD=y` — because the *text file* still says so — and
installs a kernel with no WireGuard. Arca's networking then fails at container runtime, far from
the build, with a green build log. This is exactly the failure class the script's own comment
says it is preventing ("a config that does not carry them is not a config we know how to build").

**Suggested fix.** Keep the input check as a fast fail, and add a post-build assertion on the
artifact. `CONFIG_IKCONFIG=y` is already set, so the built kernel carries its own config and the
check is cheap and needs no container:

```bash
# after the build, before install
python3 - "$PWD/vmlinux" <<'PY' > built.config
import gzip,sys
d=open(sys.argv[1],'rb').read(); i=d.find(b'IKCFG_ST'); b=d[i+8:]
sys.stdout.buffer.write(gzip.decompress(b[:b.find(b'IKCFG_ED')]))
PY
for opt in $REQUIRED_CONFIGS; do
    grep -qx -- "$opt" built.config || { echo "ERROR: $opt is not in the BUILT kernel"; exit 1; }
done
```

Simpler alternative if you prefer no Python: have `recipe/`-adjacent driver code copy
`/kbuild/.config` back out after `olddefconfig` and assert on that. (That one needs a change to
`build.sh`, which would break the verbatim property — the `IKCONFIG` route does not.)

### M-3. A stale `vmlinux` survives the staging step and defeats the post-build guard

**File:** `scripts/build-kernel.sh:59-61` and `:81-84`

```bash
rsync -a --delete --exclude 'source.tar.xz' --exclude 'vmlinux' \
    "$RECIPE_DIR/" "$WORK_DIR/kernel/"
...
make
if [ ! -f vmlinux ]; then
    echo "ERROR: Build completed but vmlinux not found"
    exit 1
fi
```

`--exclude` under `--delete` protects the receiver-side file (only `--delete-excluded` would
remove it), so `$WORK_DIR/kernel/vmlinux` from any previous run survives every subsequent run.
The `[ ! -f vmlinux ]` check exists precisely to catch a build that returned success without
producing an artifact — and a previous run's artifact is always sitting there to satisfy it.
`~/.arca/kernel-build/kernel/vmlinux` exists on this machine right now, dated 22 Nov 2025.

**Failure scenario.** `container run` returns 0 despite `build.sh` failing inside the VM — the
one case the guard is written for, since `build.sh` itself is `set -e` and would otherwise
propagate. The script then finds the stale `vmlinux`, prints its sha256 and byte count as if
they were this build's, backs up the good installed kernel, and copies the stale one over it.
The operator gets a digest, a byte count, and `=== Build complete ===`, all describing a kernel
this run did not build. Because the message is the same shape as a real success, nothing about
the output distinguishes the two.

**Suggested fix.** One line, immediately before `make`:

```bash
rm -f vmlinux
```

That makes the existing guard actually guard, and costs nothing — the kernel object tree lives
in `/kbuild` inside the container, not here, so removing this file does not force a recompile.

### M-4. `RELEASE-ARTIFACTS-gascan-engine-m4.md` names the wrong commit for the tag

**File:** `Documentation/RELEASE-ARTIFACTS-gascan-engine-m4.md:3-5` (commit `c545612`)

> Identity of the two binary assets published with the `gascan-engine-m4` tag on Arca commit
> `4134b54549a5de89cfe2c4bf567df1b0c93d7ee3`

The tag is not on `4134b54`:

```
$ git tag -l --format='%(refname:short) %(objectname) %(*objectname)' gascan-engine-m4
gascan-engine-m4 d143a6611fdb62e46b11f76cca2627a258f1b2eb c545612b056e028d5885968a7b9f586d694f994c
```

The annotated tag dereferences to `c545612` — the commit that adds this very file. Gas Can's
`engine/arca-pin.json:6` agrees with the tag and not with the doc:
`"revision": "c545612b056e028d5885968a7b9f586d694f994c"`. And `e14be74`'s own
`EVIDENCE-layer-cache-poisoning.md:15` states it correctly — "Fixed at Arca `4134b54`, published
in `gascan-engine-m4` (`c545612b`)" — so the two documents in this range contradict each other.

`4134b54` is also materially the wrong tree: it predates `6f9e0d9`, so a reader who checks it out
to audit the release gets no `kernel/recipe/` at all and cannot find the corresponding-source
material the release doc points them to.

**Failure scenario.** Someone auditing the GPL offer or reproducing the pin checks out
`4134b54`, finds `kernel/README.md` absent, and concludes the release has no
corresponding-source record.

**Suggested fix.** Correct the line to say the tag is on `c545612` and that the artefacts were
built from the tree at `4134b54`, if that is what was meant. **This one is baked into the tagged
tree** — the copy inside `gascan-engine-m4` will keep the wrong SHA whatever you do, since fixing
it creates a new commit the tag does not contain. Fix it on the branch and, if it matters,
mention the correction in the GitHub release body, which is editable. **Do not re-cut the tag
for this** — the pin, the digests and the bytes are all correct.

### M-5. `kernel/README.md`'s provenance audit rests on a build tree that is not the one it claims

**File:** `kernel/README.md`, "Where the recipe came from" and "What rebuilding does and does not give you"

Two statements:

> The build tree that produced the shipped kernel survives at `~/.arca/kernel-build/kernel/`

> The shipped kernel and the kernel left in the build tree **by the build that produced it** are
> the same size and different bytes

Both kernels name their own build time:

| file | build stamp | build id |
|---|---|---|
| `/Applications/Arca.app/Contents/Resources/vmlinux` | `#1 SMP Wed Nov 19 18:48:14 UTC 2025` | `root@413d46d8-84e7-46f1-b187-47f9477605ea` |
| `~/.arca/kernel-build/kernel/vmlinux` | `#1 SMP Sat Nov 22 16:43:50 UTC 2025` | `root@35c8af63-5192-431e-9c92-b13d6be898b6` |

These are two different builds three days apart, not one build and its shipped copy. Every file
in `~/.arca/kernel-build/kernel/` is dated 22 Nov 2025. So the surviving tree is the *Nov 22*
tree; the tree that produced the shipped kernel on Nov 19 was overwritten by it and does not
survive. The audit therefore proves what the Nov 22 build used, and infers the Nov 19 build used
the same — a reasonable inference given the 121-commit verbatim window, but not what the document
says.

The section also offers the size-equal/bytes-differ pair as the *measurement* that the build is
not bit-reproducible. Two independently-launched builds differing is much weaker evidence for
that than one rebuild of the same tree would be, so the stated reason does not support the stated
conclusion as written.

**The conclusion survives, and can be stated much more strongly.** Both kernels carry their
config in an `IKCONFIG` section, and the two extracted configs are byte-identical (113,895 bytes
each). Same config, same compiler string, same size, different bytes, three days apart — that is
a *better* demonstration of non-reproducibility than the one in the document, and it also
independently establishes that the shipped kernel was built from `recipe/config-arm64` run
through `olddefconfig`, which the document currently treats as unverifiable.

**Suggested fix.** Say that the surviving tree is from a later build of the same recipe, cite the
two banner strings so the three-day gap is on the record rather than papered over, and replace
the reproducibility measurement with the identical-`IKCONFIG` comparison. **Baked into the tagged
tree (same situation as M-4); correct it on the branch, no re-tag.**

---

## Minor findings

### m-1. The vminit asset was not built by the command the doc says it was

**File:** `Documentation/RELEASE-ARTIFACTS-gascan-engine-m4.md:12-15` vs `Makefile:237`

The doc says both assets "were built by the same two commands `make build-assets` uses
(`Makefile:235` and `Makefile:237`), run directly". `Makefile:235` matches
(`gzip -c ~/.arca/vmlinux > assets/vmlinux-arm64.gz`). `Makefile:237` does not:

```
Makefile:237:  @cd ~/.arca && tar czf $(shell pwd)/assets/vminit-oci-arm64.tar.gz vminit/
doc:           cd ~/.arca && COPYFILE_DISABLE=1 tar czf …/vminit-oci-arm64.tar.gz vminit/
```

The published asset was built with `COPYFILE_DISABLE=1`; `make build-assets` has no such setting.
On macOS that is not cosmetic — without it, `tar` can emit AppleDouble `._*` members for extended
attributes, so `make build-assets` can produce an archive containing entries the published one
does not have.

**Suggested fix.** Add `COPYFILE_DISABLE=1` to `Makefile:237` so the target actually matches what
was shipped, and the doc's claim becomes true rather than needing correction. **Fixable now, no
re-tag** — the published bytes are the correct ones; it is the Makefile that diverged.

### m-2. The "121 commits / newest `452f354b`" count is wrong against the pinned submodule

**File:** `kernel/README.md`, "Where the recipe came from"

> 121 commits reachable from `apple/containerization` `main` carry it verbatim, the oldest
> `995a2313` (2025-10-03) and the newest `452f354b` (2026-01-05)

The 121 and the oldest are exactly right — I reproduced both against the vendored submodule
(`kernel` tree `3866bad145a050b200ffd93f2ecacdb308c94a59`, 121 matching commits reachable from
`452f354b`, oldest `995a2313`, 2025-10-03). The "newest" is not. Walking forward from `452f354b`
to the pinned submodule head `6304122`, a further **58** commits carry the identical `kernel/`
tree, the newest being `5754902` (2026-08-04). The true window is 179 commits spanning
2025-10-03 to 2026-08-04.

This does not weaken the argument — a wider verbatim window makes "whichever of them the clone
landed on, the bytes were these" *more* certain, not less. But the sentence justifying the choice
of name ("`452f354b` is the newest and is tagged") is half false, and the number is checkable
from the submodule that ships in this repository.

**Suggested fix.** State the window as 179 commits, 2025-10-03 to `5754902` (2026-08-04), and
justify the choice of `452f354b` on the tag alone — which is the real reason and a sufficient one.

### m-3. The Linux tarball digest is recorded twice; only one copy is enforced

**Files:** `kernel/recipe.env:28` and `kernel/recipe.sha256:13`

Both carry `390cdde032…`. `scripts/build-kernel.sh:71-79` checks the tarball against
`$KERNEL_SOURCE_SHA256` from `recipe.env`. The `source.tar.xz` line in `recipe.sha256` is never
read by anything — `build-kernel.sh:38-40` filters the lock file with
`grep -E '^[0-9a-f]{64}  recipe/'`, which excludes it by construction.

**Failure scenario.** A kernel bump updates `recipe.env` and misses `recipe.sha256`. Nothing
detects the drift, and a reader auditing `recipe.sha256` — the file whose header says it is
"Verified by scripts/build-kernel.sh before every build" — gets the digest of the previous
kernel.

**Suggested fix.** Either drop the line and note in `recipe.sha256`'s header that the tarball is
pinned in `recipe.env`, or verify it: after the download, `grep 'source.tar.xz$' "$LOCK_FILE" |
shasum -a 256 -c -` in the work dir, and delete `KERNEL_SOURCE_SHA256` so there is one authority.
The second is the DRY answer.

### m-4. The release doc's evidence for "byte-identical to the 2025-12-01 build" is a git-ignored file

**File:** `Documentation/RELEASE-ARTIFACTS-gascan-engine-m4.md:32-34`

> This asset is byte-identical to the one built on 2025-12-01 — `assets/SHA256SUMS` of that date
> carries the same `8a30e10d…`.

`assets/SHA256SUMS` is excluded by `assets/.gitignore:13`, and `git ls-files assets/` returns
only `.gitignore`, `ArcaLogo.png`, `README.md`. The claim is true on this machine — the local
`assets/SHA256SUMS` does carry `8a30e10d…` for the kernel — but it is unverifiable from any
checkout.

This is the same defect class `e14be74` was created to fix, in a file added one commit earlier: a
committed record citing evidence that is not committed. Worth naming because the fix commit's own
premise applies to it.

**Suggested fix.** Inline the fact rather than the citation — the kernel binary's build stamp
(`#1 SMP Wed Nov 19 18:48:14 UTC 2025`, recoverable from the shipped asset by anyone) is durable
evidence that no Milestone 4 work rebuilt it, and needs no untracked file.

### m-5. `Documentation/.gitignore`'s header comment is stale, and both commits in this range made it staler

**File:** `Documentation/.gitignore:1-10`

The header lists the public documents as `ARCHITECTURE.md`, `OVERVIEW.md`, `LIMITATIONS.md`,
`DISTRIBUTION.md`. The negation list below it has seven entries: those four plus
`!VMINIT_BUILD.md` (pre-existing), `!RELEASE-ARTIFACTS-*.md` (added by `c545612`) and
`!EVIDENCE-*.md` (added by `e14be74`). Neither commit updated the header. Separately,
`OVERVIEW.md` and `LIMITATIONS.md` do not exist in `Documentation/` — the header advertises two
documents that are not there.

**Suggested fix.** Delete the enumeration from the header and let the negations be the list —
they are already self-documenting and cannot go stale.

### m-6. The GPL "corresponding source" list omits the two files that do the pinning

**File:** `kernel/README.md`, "The scripts used to control compilation"

The list names `scripts/build-kernel.sh`, `recipe/Makefile`, `recipe/build.sh`,
`recipe/image/Dockerfile`, `recipe/image/sources.list`. It omits `kernel/recipe.env` — which
`build-kernel.sh:24` *sources*, and which supplies `KERNEL_SOURCE_URL`, `KERNEL_SOURCE_SHA256`
and `REQUIRED_CONFIGS` — and `kernel/recipe.sha256`, which the build enforces. A third party
handed only the five listed files could not run the build.

Minor nit in the same list: `scripts/build-kernel.sh` is written relative to the repository root
while the other four are relative to `kernel/`, in one list, in a file that lives in `kernel/`.

### m-7. `recipe.sha256` detects modification and deletion, not addition

**File:** `scripts/build-kernel.sh:38-40`

`shasum -c` verifies that each listed path matches; it says nothing about paths that are not
listed. A file added under `kernel/recipe/` passes verification and is then `rsync`-ed into the
work dir.

I could not construct a way to turn this into a wrong artifact — the container build context is
`image/`, its `.dockerignore` is covered, and the only files the Dockerfile and `build.sh`
consume (`sources.list`, `config-arm64`, `source.tar.xz`) are all covered — so this is hardening,
not a live hole. But the property the file's header asserts ("sha256 of every file in
kernel/recipe/") is only half-checked.

**Suggested fix.** Add a count assertion before the `shasum -c`:

```bash
[ "$(cd "$PROJECT_ROOT/kernel" && find recipe -type f | wc -l)" \
  -eq "$(grep -cE '^[0-9a-f]{64}  recipe/' "$LOCK_FILE")" ] || { echo "ERROR: recipe/ has files recipe.sha256 does not list"; exit 1; }
```

### m-8. `kernel/README.md` cites `recipe/build.sh:20` for two actions, one of which is at `:24`

**File:** `kernel/README.md`, "The configuration"

> It is copied to `.config` and run through `make olddefconfig` by `recipe/build.sh:20`.

`build.sh:20` is `cp /kernel/config-arm64 /kbuild/.config`. `make olddefconfig` is at `:24`.
Cite both.

---

## Verified correct

Everything below I checked and found accurate.

**The published bytes and the pin agree, exactly.** Every digest and byte count in
`RELEASE-ARTIFACTS-gascan-engine-m4.md` matches `engine/arca-pin.json` in
`/Users/kiener/code/gascan` field for field, and both match the staged files on disk:

| | doc / pin | measured (`shasum -a 256`, `stat -f %z`) |
|---|---|---|
| `vmlinux-arm64.gz` | 9,092,349 / `8a30e10d…597a` | matches |
| kernel inside it | 28,248,576 / `49e0f081…3394` | matches (`gzip -dc` round trip reproduces both) |
| `vminit-oci-arm64.tar.gz` | 73,739,738 / `51602e72…369b` | matches |
| OCI manifest | 478 / `cf74cd41…22c2` | not re-derived (see below) |

Staged files are at `~/.arca/release/gascan-engine-m4/`, and its `SHA256SUMS` carries the same
two asset digests. The doc's statement that each digest is over a single file, and its warning
that `tar czf` output is not reproducible so the *upload must be that file*, are both correct and
well put.

**`~/.arca/vmlinux` really is a symlink** into `/Applications/Arca.app/Contents/Resources/`, as
the release doc says — so the compressed asset is the kernel the live tier ran against, not a
rebuild. That also makes `build-kernel.sh:98-103`'s refusal to install through a symlink a real
protection on this machine and not a theoretical one.

**Provenance of `kernel/recipe/` is exactly as claimed.** All six upstream files are
byte-identical (compared by `git hash-object` against
`452f354bac52ecbfe4a40b729880435a070c5a29:kernel/*`) to the vendored copies, and the only
non-upstream file is the empty `image/.dockerignore` — precisely the one exception `recipe.env`
and `kernel/README.md` both call out. Tags `0.20.0` and `0.20.1` do both point at `452f354b`, as
stated. The same six files in the surviving build tree at `~/.arca/kernel-build/kernel/` are also
byte-identical to that tag, so the audit `kernel/README.md` describes does hold (subject to M-5's
correction about *which* build's tree it is).

**`recipe.sha256` is honest and verifies clean.** All seven `recipe/` digests check out:

```
$ cd kernel && grep -E '^[0-9a-f]{64}  recipe/' recipe.sha256 | shasum -a 256 -c -
recipe/build.sh: OK ... recipe/README.md: OK          (7/7 OK)
```

The `source.tar.xz` digest `390cdde0…` matches the tarball in the build tree, and its byte count
(149,501,424) matches `kernel/README.md`. The kernel source really is 6.14.9 — the shipped
kernel's own embedded config says `# Linux/arm64 6.14.9 Kernel Configuration`.

**`build-kernel.sh` fails closed where it says it does.** `set -euo pipefail` is set (`:15`), so
the `grep | shasum -c` pipeline at `:38-40` fails the script on a digest mismatch *and* on an
empty grep. `--fail` is on the `curl` at `:69`, which is the gap in the upstream Makefile's own
`curl` at `recipe/Makefile:32` — the script's comment explains that it pre-downloads for exactly
that reason, and the Makefile's `ifeq (,$(wildcard source.tar.xz))` guard means it then skips its
own unguarded fetch. The source digest check at `:71-79` is a plain `if` with an explicit
`exit 1` and prints expected/actual/path. The install path refuses a symlink and backs up an
existing file before overwriting. `SCRIPT_DIR`/`PROJECT_ROOT` are resolved with quoted
`cd … && pwd`. All expansions are quoted except `$REQUIRED_CONFIGS` in the `for`, where the word
splitting is intended. `grep -qx --` is the right form for exact-line matching of a `CONFIG_X=y`
string. I found no shell defect in `build-kernel.sh` beyond M-3.

**`recipe/build.sh` is `set -e` only** — no `-u`, no `pipefail` — and `make -j$((\`nproc\`-1))`
would be `-j0` on a single-CPU host. Both are upstream code, vendored verbatim on purpose, and
`recipe/Makefile:35` passes `--cpus 8` so the `-j0` case cannot arise here. I am **not** filing
these: fixing them would break the byte-verbatim property that is the whole point of the
vendoring, and the tradeoff as chosen is the right one. Noted so a later reader does not
re-discover them as new. The subshell at `:22-27` propagates `cd` failure correctly under
`set -e`.

**Line citations, all checked, all correct:** `config-arm64:1916` (`CONFIG_WIREGUARD=y`),
`config-arm64:1932` (`CONFIG_TUN=y`), `build.sh:26` (`cp arch/arm64/boot/Image /kernel/vmlinux`,
and `kernel/README.md`'s note that the shipped file is `Image` and not the ELF `vmlinux` is
correct), `Makefile:15` (`KSOURCE`), `Makefile:32` (the `curl`), `Makefile:235`/`:237` in the
Arca root Makefile (both are the lines named, modulo m-1), and in the submodule
`EXT4+Formatter.swift:645` (`public func close() throws`) and `:970-972` (the ARCA PATCH volume
label write).

**The submodule did not move.** `containerization` is `63041224e82befc1e3a825253125feabbc323da7`
at all four of `4134b54`, `6f9e0d9`, `c545612`, `e14be74` — `git ls-tree` gives the same SHA at
every one, and `git log 4134b54..e14be74 -- containerization` is empty. Nothing in the diff
depends on any other submodule state; the kernel recipe compares against `452f354b`, which is an
ancestor of `6304122` (`git merge-base --is-ancestor` confirms), so the comparison is reproducible
from the pinned tree with no network access.

**Every commit anchor cited in the tests resolves, in the right repository.**

| anchor | repo | resolves to |
|---|---|---|
| `a3e812d` | arca | `a3e812de…` 2026-08-17 fix(overlay): report the attached layer count… |
| `823201e` | arca | `823201e4…` 2026-08-17 test(engine): the layer cache is asked which layer… |
| `cc8068c` | arca | valid |
| `ca47c87` | **submodule** | `ca47c877…` 2026-08-14 feat(ext4): identify Arca's block devices by volume label… |
| `cc2ea7d` | submodule | `cc2ea7dd…` 2026-08-17 fix(boot): the host tells the guest how many layers… |
| `fb2b2f2` | submodule | `fb2b2f28…` 2026-08-17 docs(vz): the hop these comments call untestable… |

`ca47c87` at `LayerCacheRoleTests.swift:13` does not exist in Arca — it is a submodule commit,
which is correct for a claim about "the guest", but a reader running `git show ca47c87` from the
repository root gets `unknown revision`. Not filed as a finding (it is outside this range and the
anchor is real), but worth a repo-relative prefix if these are ever swept.

**Every test count is corroborated.** I did not run `swift test`, as instructed. Instead I
counted `^\s*func test` declarations at each cited commit:

| commit | claim | static count |
|---|---|---|
| `823201e` | "a reader reproducing this mutation at `823201e` sees 244 tests" (`:397`, `:464`, `:519`) | **244** |
| `a3e812d` | "all 247 tests here" (`AttachedLayerCountTests.swift:96`) | **247** |
| `4134b54`/`e14be74` | "250 tests each" (evidence doc, mutation matrix) | **250** |
| — | "243 … one test short of `823201e`" (`:462`, `:517`) | 244−1 = 243, coherent |

The evidence doc's failing-before run also reconciles exactly: it claims `Executed 14 tests` under
`--filter LayerCacheRoleTests` at Arca `cc8068c`, and that file has **11** test functions at
`cc8068c` and **14** at `e14be74`. The three added are precisely the three the doc names
(`testAnUnpackThatRefusesALayerLeavesNoCacheEntryForTheNextCreateToReuse`,
`testARefusedLayerLeavesNoPartialSlotForItsSiblings`,
`testARefusedUnpackLeavesNoScratchBesideTheCacheSlot` — each absent at `cc8068c`, present at
`e14be74`). So "the tests were written and run against unfixed code, source otherwise unmodified"
is arithmetically consistent: 11 committed + 3 new = the 14 that ran.

**The evidence file's mutation matrix matches what the tests assert.** Mutation A (formatter at
`layerPath`, `promoteStagedLayer` removed) → the two reuse tests; Mutation B (staging cleanup
removed) → `testARefusedUnpackLeavesNoScratchBesideTheCacheSlot`. Disjoint, as claimed. The
docstring at `LayerCacheRoleTests.swift:804` says the matrix shows "each one failing this test
alone or those two alone, never both" — that is exactly the matrix. The fix the doc describes is
the code that is there: `OverlayFSUnpacker.swift:352` creates the staging sibling, `:377` calls
`promoteStagedLayer`, `:379` is the `try? removeItem(at: staging)` cleanup mutation B removes, and
`:215-216` is the `rename(2)` promotion. The doc's restoration check —
`shasum -a 256 Sources/Containerization/Image/Unpacker/OverlayFSUnpacker.swift` =
`1559e9c18a91482232d0a86068c1f73bbed64c59445b2f35d2d7c6eb78a51e84` — **reproduces exactly** at
the pinned submodule commit. The doc's note that a first attempt at mutation A was discarded as
unclean, "recorded because it happened, not as a result", is the right call and the right way to
write it.

**`e14be74` does what its message says.** The two docstrings that pointed at "the fix report"
(a file in another repository's git-ignored scaffolding) now cite
`Documentation/EVIDENCE-layer-cache-poisoning.md`, which is tracked
(`git ls-files Documentation/` lists it). `Documentation/.gitignore`'s `!EVIDENCE-*.md` is what
makes that possible, and no parent directory is excluded, so the negation is effective. Same for
`!RELEASE-ARTIFACTS-*.md` in `c545612`. The root `.gitignore:92` `!kernel/recipe/image/.dockerignore`
is also effective — the global excludes file (`~/.gitignore_global:328`) has `.dockerignore`,
repo-level rules take precedence, and `git ls-files -s` confirms the file is tracked as
`e69de29b` (empty). Both commits' `.gitignore` claims are accurate, and neither adds an entry that
hides anything that should be committed.

I checked whether `e14be74`'s fix was complete by sweeping for other cross-repository citations:
four exist in `Sources/` (`ArcaIP/IP.swift:10`, `SandboxEngineService.swift:579`,
`engine.pb.swift:33-34`), all pointing at `docs/superpowers/specs/*` in gascan. I verified all
four of those files exist **and are tracked** in `/Users/kiener/code/gascan`, so they are not
instances of the defect `e14be74` fixed. Not a finding.

**Commit messages check out against the code.** `6f9e0d9` ("the recipe was whatever main held
that day, and now it is pinned") is supported — `kernel/README.md` documents the prior
`git clone --depth 1` of `main` with no pinned commit, and the diff replaces it with a vendored,
digest-verified tree. `c545612` ("state which bytes gascan-engine-m4 publishes, and which digest
means what") is supported and the digests are right, though the commit it names for the tag is
not (M-4). `e14be74` ("the tests cited a file that is not in any repository") is supported and
accurate.

---

## Could not verify, and why

1. **That the published GitHub release assets are the staged files.** I verified
   `~/.arca/release/gascan-engine-m4/{vmlinux-arm64.gz,vminit-oci-arm64.tar.gz}` byte-for-byte
   against the doc and the pin. I did not download the assets from the release to confirm the
   upload is those bytes — that needs network access I did not take. If the upload is ever in
   doubt, `gh release download gascan-engine-m4 && shasum -a 256 *` settles it in seconds and
   costs nothing.
2. **The OCI manifest digest `cf74cd41…` and the 5-file layout table.** Recomputing it means
   extracting the 74 MB tarball and hashing the manifest blob. I did not, because the staged
   tarball's own digest matches the pin and the manifest is *inside* those verified bytes — so
   the tarball digest already transitively covers it. The doc's round-trip evidence (`diff -r`
   against `~/.arca/vminit`, each blob's sha256 equalling its filename) is self-consistent. Note
   that the *local* `assets/vminit-oci-arm64.tar.gz` is a different, older archive (164 MB,
   `65b1dbfa…` per `assets/SHA256SUMS`) — that is stale build-host scaffolding, not the release,
   and is consistent with the doc's statement that the image was rebuilt after the Landing 1 fix
   wave.
3. **The "613 in the submodule" count** at `AttachedLayerCountTests.swift:96`. I did not run the
   submodule's suite. Static counting is not conclusive there because `containerization` uses
   swift-testing: at `cc2ea7d` I count 637 `@Test` declarations plus 188 XCTest `func test`s, and
   the run figure would be lower than the declaration total after skips, traits and
   platform-conditional cases — so 613 is plausible but unconfirmed. I found no occurrence of
   "616" anywhere in `Tests/ArcaEngineTests/`; if that figure is cited somewhere, it is in the
   submodule and outside this range.
4. **That the shipped kernel was built from *this* recipe, cryptographically.** The build is not
   bit-reproducible, so no digest can close this. What I *can* say is stronger than the document
   currently claims: the shipped kernel's embedded `IKCONFIG` is byte-identical (113,895 bytes) to
   the build-tree kernel's, reports `Linux/arm64 6.14.9`, carries `CONFIG_TUN=y` and
   `CONFIG_WIREGUARD=y`, and names a gcc consistent with `FROM ubuntu:focal`. That places the
   shipped binary on this config and this kernel version beyond reasonable doubt. It does not, and
   cannot, place it on a specific *toolchain build* — see M-1.
5. **The three-day gap in M-5.** I established that the two kernels are different builds. I could
   not establish what happened between Nov 19 and Nov 22, or why the Nov 19 tree did not survive.
   Whoever ran those builds may know; the tree cannot tell me.
6. **Anything requiring `swift test`.** Not run, per instruction — every run relinks
   `arca-engine` and strips its `virtualization` entitlement, which gascan's live tier depends on.
   All test-related conclusions above are from static counting and source reading.

---

## Bottom line

The three commits do what they claim, and the reproducibility work in `6f9e0d9` is a genuine
improvement over `git clone --depth 1 main` — the recipe, the config and the kernel source are all
now pinned and verified fail-closed, and the vendoring is exactly verbatim as advertised. What is
*not* pinned is the toolchain (M-1), and the required-config assertion checks a text file rather
than the artifact (M-2), so the guarantee is narrower than a fast reader will take it to be. M-3 is
a one-line fix to a guard that currently cannot fire. M-4 and M-5 are factual errors in durable
documents whose in-tag copies cannot be corrected — but neither touches a published byte, and
nothing here justifies re-cutting `gascan-engine-m4`.

If only three things get fixed before merge, make them M-3 (one line), m-1 (one Makefile edit, and
it makes a currently-false claim true), and M-4 (the pin and the doc must not disagree about which
commit was released).
