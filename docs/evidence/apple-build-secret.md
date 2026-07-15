# Apple build secret isolation evidence

Date: 2026-07-15 (America/Phoenix)

- Apple container CLI: 1.1.0 (release, commit `5973b9c`)
- Apple container builder: 0.12.0
- Host: macOS 26.5.1 (25F80), arm64
- Authoritative hardened probe image digest: `55d1bd0b041081aa8b182580ccff3cf572ddeef64d79f93ee8bc1e8cec39cc5f`
- Input: a fresh synthetic one-line secret in a current-UID, mode `0600` file outside the repository. Neither its value nor its private absolute path was retained.

The probe copied the input to `.build-secrets/gascamp_read_token` beneath a fresh mode `0700` build context, verified the staged file was a current-UID regular non-symlink with mode `0600`, and wrote an exact `.dockerignore` entry for `.build-secrets`. A separately captured pre-build transmitted-context archive contained the Dockerfile and `.dockerignore`, did not contain `.build-secrets`, and did not contain the synthetic value.

Apple BuildKit mounted the staged file only at `/run/secrets/gascamp_read_token` for the required secret-mounted `RUN`. That step compared its SHA-256 with the separately supplied expected digest. The following `RUN` proved the mount path absent. Structured image inspection succeeded. The value was absent from builder argv, the Dockerfile, captured build transcript, transmitted-context archive, image inspect/history JSON, and an exported stopped-container filesystem.

The probe reported `PASS`. Its exit trap removed only its token-owned test container, image tag, staged secret, and private context. A post-run `container list --all` showed no `gascan-build-secret-probe-*` container.

Follow-up hardening assigns the same unique `com.gascan.build-secret-probe` ownership marker to the image and container. Every stop or deletion is preceded by a bounded structural inspect that matches both the exact identity and marker; a mismatch fails cleanup without mutating the colliding resource. Every Apple CLI operation, including inspection and cleanup, runs under an explicit process-group deadline. The probe also rejects the supplied path itself when it is a symlink, before canonicalization.

The authoritative digest above comes from the fresh post-review run after exact Apple CLI 1.1.0 structural identity checks and explicit INT/TERM command-group termination/reaping were in place. That run passed retained-channel scans and token-owned cleanup; post-run listings contained no probe container or image.

The latest run also sets cleanup inspection state before build and create attempts. Thus, even if either mutating call creates its correctly marked resource and then fails or is interrupted before returning, bounded cleanup inspects and removes the exact owned resource; an explicit not-found result is accepted and any foreign identity or marker is refused without mutation.

Created-then-blocked build/TERM and create/INT fixtures verify this signal race directly: cleanup removes the exact structurally owned resources, reaps the active CLI group and watchdog within the deadline, and removes the private context. If stopping an owned container makes it disappear, the second structural inspect's explicit not-found result is treated as already clean; malformed or foreign responses still fail closed.

Compatibility note: this Apple CLI emits structured JSON from `container image inspect` and rejects `--format json`; the probe uses the reviewed `--format json` form when advertised by `--help`, otherwise the native structured output.
