# Plan 4 Task 3 report

Status: **READY FOR REVIEW — live image build/smoke pending controller**

## Scope

Implemented only the locked polyglot image layer, reviewed system-tool manifest, mise defaults/profile, and image-local smoke fixtures. Task 1 artifact verification and Task 2 user/volume/cleanup behavior remain unchanged. Task 4 and later work was not started.

## TDD evidence

- The image-local polyglot matrix was written first and failed against the host's mismatched environment before reaching the full runtime matrix.
- The focused Rust contract then failed because the mise global config and Docker installation layer did not exist.
- After the minimal image-layer implementation, the focused contract passes 3/3: exact lock/config agreement, reviewed package and verified-artifact installation paths, and complete runtime/native/browser smoke coverage.
- Controller-review TDD first failed compilation because the archive and version validators were absent. The focused review suite now passes 7/7, including malicious Chromium archives and resolved-version mismatch, missing-key, and extra-key cases.

## Delivered contract

- Exact defaults for Node, Python, Go, Rust, Java, Ruby, and Elixir are duplicated from `versions.lock` into a global mise config containing only `[tools]`; there are no environment, hook, task, alias, or floating-version entries.
- Mise uses `/opt/gascan/mise`, `/home/workspace/.cache/mise`, and `/etc/mise/config.toml`; its shims are available in noninteractive sessions, while interactive Bash activation is isolated to the profile fragment.
- The image installs apt packages only from the reviewed `tests/image/system-tools.txt` manifest through the locked Ubuntu snapshot and `--no-install-recommends`, then removes apt metadata.
- The mise binary and Playwright Chromium archive remain sourced only from Task 1's checksum-verified `.artifacts` handoff. No `curl`, `wget`, npm install, or floating image-layer fetch was introduced.
- Before the container build, the verified Chromium ZIP is validated in full and atomically extracted. Absolute, parent/backslash traversal, symlink, normalized-duplicate, special-type, unexpected-layout, and missing-browser entries fail closed before the reviewed `chrome-linux` tree enters the image.
- The lock and mise config must contain the same exact seven-key version map. The build handoff contains its canonical JSON form.
- Default runtimes install as `workspace`; resolved versions are serialized as valid JSON only into an owned cache staging directory. A later root layer compares that map exactly with the validated lock/config handoff, installs `/opt/gascan/image-tool-versions.json` as root-owned mode 0444, and removes staging.
- The live smoke regenerates the exact seven-key installed-version map, compares it with the immutable image record, and checks root ownership/mode.
- The browser smoke launches the locked Chromium artifact headlessly using Node and verifies rendered DOM output without adding an unpinned package download.

## Non-live verification

```text
cargo test --manifest-path scripts/Cargo.toml
24 passed; 0 failed

cargo clippy --manifest-path scripts/Cargo.toml --all-targets -- -D warnings
exit 0

bash -n tests/image/polyglot-smoke.sh
exit 0

sh -n images/workspace/etc/profile.d/mise.sh
exit 0

node --check images/workspace/tests/playwright-smoke.mjs
exit 0

git diff --check
exit 0
```

Per controller instruction, `scripts/build-workspace-image.sh`, live runtime smoke, and named-volume cache reuse were not executed. Package availability in the locked Ubuntu snapshot, the real locked archive layout, runtime installation, Chromium launch, and second-volume no-redownload evidence remain pending an authorized live build/smoke review.
