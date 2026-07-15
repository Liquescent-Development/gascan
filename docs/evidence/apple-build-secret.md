# Apple build secret isolation evidence

Date: 2026-07-15 (America/Phoenix)

- Apple container CLI: 1.1.0 (release, commit `5973b9c`)
- Apple container builder: 0.12.0
- Host: macOS 26.5.1 (25F80), arm64
- Probe image digest: `8b6ed7442e0bc4fe53e2a6097da1e499d2bf78e279824b707e5b1ec7061e4581`
- Input: a fresh synthetic one-line secret in a current-UID, mode `0600` file outside the repository. Neither its value nor its private absolute path was retained.

The probe copied the input to `.build-secrets/gascamp_read_token` beneath a fresh mode `0700` build context, verified the staged file was a current-UID regular non-symlink with mode `0600`, and wrote an exact `.dockerignore` entry for `.build-secrets`. A separately captured pre-build transmitted-context archive contained the Dockerfile and `.dockerignore`, did not contain `.build-secrets`, and did not contain the synthetic value.

Apple BuildKit mounted the staged file only at `/run/secrets/gascamp_read_token` for the required secret-mounted `RUN`. That step compared its SHA-256 with the separately supplied expected digest. The following `RUN` proved the mount path absent. Structured image inspection succeeded. The value was absent from builder argv, the Dockerfile, captured build transcript, transmitted-context archive, image inspect/history JSON, and an exported stopped-container filesystem.

The probe reported `PASS`. Its exit trap removed only its token-owned test container, image tag, staged secret, and private context. A post-run `container list --all` showed no `gascan-build-secret-probe-*` container.

Compatibility note: this Apple CLI emits structured JSON from `container image inspect` and rejects `--format json`; the probe uses the reviewed `--format json` form when advertised by `--help`, otherwise the native structured output.
