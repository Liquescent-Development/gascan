# SSH Permission Compatibility and Onboarding Design

## Summary

Gas Can currently rejects conventional, OpenSSH-safe host configuration
permissions because it requires exact modes: `0700` for selected directories
and `0600` for every SSH configuration file. This prevents automatic SSH
setup for users whose `~/.ssh/config` is the common `0644`, and it can reject
an existing Gas Can configuration root created by an older release as `0755`.

Gas Can will accept existing paths based on whether another user can modify
them, rather than requiring one exact mode. It will retain strict path-type,
ownership, and link checks, preserve the safe mode of an existing user SSH
configuration during atomic updates, and continue using private modes for
newly created paths.

The README will also make the first-use path complete enough to start a
networked sandbox and immediately use Claude Code or Herdr. It will document
how a manifest `[tools]` entry can override an immutable workstation tool,
including Claude Code, without adding a new Gas Can command.

## Goals

- Accept conventional safe modes such as `0755` on owner-controlled
  directories and `0644` on `~/.ssh/config`.
- Preserve the mode of an existing safe `~/.ssh/config` when installing or
  removing Gas Can's managed include block.
- Keep new Gas Can-managed directories at `0700` and new files at `0600`.
- Continue failing closed for paths that another user can modify or substitute.
- Give a new user a concise install-to-agent workflow near the beginning of
  the README.
- Show how to install or pin a newer Claude Code release through `[tools]` and
  `gascan apply`.

## Non-goals

- No new `gascan tools` or Claude-specific CLI command.
- No change to provisioning hashes, floating-version refresh behavior, or mise
  itself.
- No automatic upgrade of Claude Code or any other workstation tool.
- No weakening of ownership, regular-file/directory, symlink, hard-link, size,
  race, or atomic-replacement checks.
- No release-image rebuild solely to refresh the bundled Claude Code version.

## SSH Path Safety

### Existing directories

An existing directory traversed for SSH configuration is safe when all of the
following hold:

- it is a directory opened without following a final symlink;
- it is owned by the effective user;
- neither group nor other has write permission.

Read and execute permissions for group or other are not treated as an
integrity failure. Modes including `0700`, `0750`, and `0755` are therefore
accepted. Modes including `0770`, `0702`, and `0777` remain rejected.

This rule applies consistently to the existing `~/.ssh`,
`~/.config/gascan`, and managed SSH directories. Parent directories already
validated with the no-group-or-other-write rule retain that behavior.

### Existing files

An existing SSH configuration file is safe when all of the following hold:

- it is a regular file opened without following a symlink;
- it is owned by the effective user;
- it has exactly one hard link;
- neither group nor other has write permission.

Owner permissions and group/other read permission may vary. Conventional
`0600`, `0640`, and `0644` files are accepted. Any group- or world-writable
file remains rejected.

The existing maximum-size and before/during/after identity checks remain
unchanged.

### Creation and replacement

New directories are created as `0700`. New files are created as `0600`.

When atomically replacing an existing safe file, Gas Can records its mode and
applies that mode to the staging file before publication. All validation and
concurrent-update recovery paths use the recorded mode. Consequently,
installing and removing the managed include from an existing `0644`
`~/.ssh/config` leaves it `0644`; Gas Can does not silently tighten or broaden
the user's chosen safe mode.

Managed files that Gas Can creates itself remain `0600`.

## README Onboarding

The opening material will retain the short product and security description,
then provide a practical Quickstart that can be followed without reading the
reference sections:

1. Install Gas Can and run human-readable `gascan doctor`.
2. Create a networked `gascan.toml` with representative resources, persistent
   storage, Node, and a current Claude Code override.
3. Run `gascan up .` and enter with `gascan shell`.
4. Verify and launch Claude Code with `claude --version` and `claude`.
5. Verify and launch Herdr with `herdr --version` and `herdr`.
6. Explain that agent authentication and configuration are sandbox-local and
   persist until `gascan destroy`.
7. Show the edit-and-apply loop and the essential `status`, `shell`, `down`,
   and `destroy` commands.

The full schema example will remain the authoritative copyable manifest and
will show the Claude Code mise backend syntax:

```toml
[tools]
node = "lts"
"npm:@anthropic-ai/claude-code" = "latest"
```

The tools reference will explain:

- immutable image tools are fallbacks;
- a `[tools]` declaration installs into the persistent managed tools volume
  and takes precedence in `PATH`;
- `latest` installs the latest release when that declaration is first applied;
- changing the value to a specific newer version and running `gascan apply`
  provides a controlled upgrade;
- a networked sandbox is required for versions not already cached or bundled.

The documentation will not promise that an unchanged `latest` declaration is
re-resolved by every `gascan apply`.

## Testing and Verification

Automated tests will demonstrate that:

- safe `0755` managed directories no longer block SSH setup;
- a `0644` user SSH config can be inspected, installed into, and removed from;
- install and removal preserve an existing safe file mode;
- group- or world-writable directories and files remain rejected;
- symlink, hard-link, ownership, type, race, and recovery coverage remains
  passing.

Documentation verification will include checking every command and manifest
key against the current CLI and schema, Markdown formatting, and repository
link validity. The complete Rust workspace test suite and formatting/lint
checks will run before integration.

## Release

The implementation and documentation will be reviewed and merged through a
pull request. After merge, the normal repository release driver will bump the
patch version, create and push the signed tag, and publish the release using
the existing release workflow. The temporary feature worktree will be removed
after the release completes.
