# Native Shell and Managed Starship Design

## Summary

`gascan shell` currently opens an interactive TTY but the daemon substitutes
the literal command `sh` when the user supplies no argv. The released workspace
account is configured with `/bin/bash`, so SSH starts Bash while
`gascan shell` starts a minimal POSIX shell. A live session confirmed the
difference:

- `ARGV0=sh`;
- `SHELL` was unset;
- the prompt was the bare `$`;
- the workspace account's login shell was `/bin/bash`.

The image also omits `bash-completion`. Its existing `.bashrc` is prepared to
load the conventional completion framework when present, but there is nothing
to load.

Gas Can will make its default interactive shell match SSH by starting Bash as a
login shell, add Bash completion to the workspace image, and add an optional
Gas Can-managed Starship experience. Starship will offer a polished
font-compatible preset and a richer Nerd Font preset. Both `gascan shell` and
SSH will use the same selected prompt.

## Goals

- Make a default `gascan shell` feel like a normal interactive login on the
  sandbox.
- Give `gascan shell` and SSH the same Bash initialization, Mise activation,
  completion framework, and prompt selection.
- Preserve explicit `gascan shell -- <argv>` behavior exactly.
- Provide a useful colored Bash prompt and Bash completion without requiring
  Starship.
- Let a manifest opt into either a font-compatible or Nerd Font Starship
  preset.
- Install, pin, configure, and update the Starship used by managed prompts as
  part of the Gas Can workspace image and release process.
- Apply prompt changes to an existing compatible sandbox without recreating
  it.
- Keep prompt initialization out of `gascan run`, setup scripts, and all other
  non-interactive execution.
- Document the default shell, both prompt presets, and the host-side Nerd Font
  prerequisite in the quick start and complete manifest reference.

## Non-goals

- No general-purpose dotfile manager.
- No arbitrary shell executable or shell startup command in `gascan.toml`.
- No first-class Zsh, Fish, or other shell configuration in this change.
- No user-supplied Starship TOML through the Gas Can manifest.
- No host font installation or terminal font detection.
- No automatic modification of host shell files.
- No persistence guarantee for arbitrary home-directory files or shell
  history.
- No isolation from code or state already running in the same interactive
  Bash process.
- No change to explicit shell commands such as
  `gascan shell -- zsh` or `gascan shell -- bash --noprofile --norc`.

## Manifest Configuration

The top-level manifest gains an optional `shell` table:

```toml
[shell]
prompt = "standard"
```

`prompt` is a closed, kebab-case enum:

| Value | Behavior |
| --- | --- |
| `standard` | Native colored Bash prompt with Bash completion; Starship is not activated |
| `starship` | Gas Can's managed Starship preset using symbols that do not require a Nerd Font |
| `starship-nerd-font` | Gas Can's richer managed Starship preset for terminals configured with a Nerd Font |

The table and field both default to `standard`. Existing manifests therefore
retain a non-Starship prompt without modification. Unknown table fields or
prompt values fail manifest loading and identify the accepted values.

The selected prompt participates in the applied provisioning identity. A
change makes status report that configuration must be applied, and
`gascan apply` updates the managed shell files in place.

## Default Interactive Shell

When `ShellRequest.command.argv` is empty, the daemon records the exact argv:

```text
/bin/bash --login
```

The daemon no longer substitutes `sh`. An explicit argv remains opaque and is
forwarded byte-for-byte after the existing wire validation. `RunRequest`
behavior is unchanged.

The default session remains a TTY and retains the existing resize, signal,
stdin, stdout, stderr, and exit-status attachment behavior. Gas Can continues
to forward the host's validated `TERM`, `COLORTERM`, `LANG`, and `LC_*`
variables. The image explicitly defines `SHELL=/bin/bash` for the interactive
runtime environment so Bash-aware programs and prompt initialization observe
the account shell consistently.

Starting Bash with `--login` makes the default Gas Can entry path follow the
same system and workspace login initialization used by SSH. No shell command
is constructed or interpreted by the host.

## Workspace Image

The reviewed system package set adds `bash-completion`. Image contracts prove
that its conventional initialization file is readable and that the
image-provided workspace Bash startup path loads it for interactive Bash.

The image also contains a release-pinned Starship binary. The version and
source artifact are locked and verified through the existing reviewed image
input process. The final binary is exposed to managed prompt initialization
through a stable root-owned path:

```text
/opt/gascan/shell/bin/starship
```

Managed initialization invokes that exact path rather than resolving
`starship` through user-controlled `PATH` or Mise configuration. Users remain
free to declare or install their own `starship` command for direct use, but it
does not silently replace the binary backing Gas Can's managed prompt.

The pinned binary is present in the image for all sandboxes but inactive under
the default `standard` mode. This makes either opt-in preset available without
a network download, including in an offline sandbox, and updates it only
through a reviewed Gas Can image release.

## Trust Boundary

Gas Can's security boundary covers the inputs and state it owns: the
release-pinned Starship binary and presets, their exact root-owned immutable
paths, the root/workspace provisioning boundary, managed selector and
configuration ownership and modes, and failure handling for Gas Can's own
initialization transaction. User-controlled `PATH` cannot replace the managed
binary, and the workspace account cannot publish or race root-managed prompt
configuration.

The existing interactive Bash process is the trusted caller, not a security
isolation boundary. Its pre-existing or concurrently same-user-mutated
functions, variables, traps, signals, and prompt customization are caller
state. A sourced hook cannot authenticate shell mutations that occur before
its first instruction; in particular, a self-clearing DEBUG trap can run
before the hook can observe it. Collision checks, authoritative builtin use,
compare-before-write checks, isolated evaluation, and rollback remain useful
defense in depth against accidental incompatibility and failures in Gas Can's
own initialization, but they do not protect against adversarial code already
executing with equal authority in the same shell.

## Managed Shell Files

The image-provided Bash startup files for the supported workspace and root
interactive paths source one immutable Gas Can hook after the normal Bash
prompt setup. The hook does nothing in non-interactive shells.

Provisioning atomically maintains prompt state below:

```text
/home/workspace/.config/gascan/shell/
```

The state includes a closed prompt selector and, for a Starship mode, the
corresponding generated Starship configuration. It contains no shell text
copied from the manifest. The shell directory is `root:workspace` mode `0750`;
the selector, generated configuration, and staging files are regular,
non-symlinked `root:workspace` mode `0640` files. A root-owned mode `0600`
advisory lock serializes the complete transaction. This lets the workspace
account read applied state but prevents it from mutating or racing the
configurator. Staging names are reserved and bounded; unsafe types, links,
ownership, modes, link counts, or unexpected entries fail provisioning rather
than being followed or overwritten.

On an applied `standard` selection, the hook leaves the existing Bash prompt
unchanged. Selector validation compares exact bytes, including rejecting
embedded NUL bytes. On either Starship selection, a workspace shell exports
the exact root-owned managed configuration path, while a root shell uses the
matching immutable preset directly and never retains the generated home
configuration. The hook invokes the pinned binary directly with
`init bash --print-full-init` under an immutable-only `PATH`, exports the
pinned `STARSHIP_EXECUTABLE` for prompt runtime, and evaluates the resulting
full initialization exactly once in an inherited subshell. Before generation,
the hook records any DEBUG trap visible at hook entry with `builtin trap` and
fails closed if one remains visible, preserving it byte-for-byte. This is a
compatibility check, not authentication of prior same-shell activity. The hook
also rejects readonly live variables and pre-existing function or
internal-variable collisions on the reviewed Starship 1.25.1 mutation surface.
Generation and isolated evaluation receive config, executable, and
immutable-only `PATH` through their child environment; no live shell variable
is changed before success.

The isolated evaluator uses effective `errexit` outside conditional context,
so any failed init command aborts even if a later command would succeed. Only
after evaluation succeeds does it emit an allowlisted declaration-only commit
of the reviewed Bash state (managed Starship functions and variables, prompt
variables, `PROMPT_COMMAND`, supported preexec arrays, the DEBUG trap, and
`checkwinsize`). It cannot serialize an inherited user Starship definition.
The hook snapshots that same surface, syntax-checks and dry-runs the commit,
then applies guarded operations. Immediately before each managed function
declaration it confirms that the function remains absent; immediately before
each managed variable write it compares the exact set/unset declaration and
attributes captured at preflight. All DEBUG-trap reads and manipulation use
`builtin trap`. Any unexpected apply failure is reported and rolls back the
snapshot, including set/unset and exported/unexported variable attributes.
These checks keep a partially failing managed init or an observed writable or
readonly collision from leaving partial managed prompt state; they do not
isolate the shell from same-user code running concurrently. No second
full-init evaluation is needed. The pinned Starship Bash initialization
preserves a normal existing `PROMPT_COMMAND` in `STARSHIP_PROMPT_COMMAND` and
executes it from `starship_precmd`, so compatible caller customization remains
active. Unsupported BLE integration fails closed to standard Bash. The hook
warns once on visible inherited DEBUG state, collision, readonly state,
generation failure, isolated evaluation failure, or guarded apply failure.
Switching back to `standard`
disables Starship on the next interactive login. Obsolete generated preset
state may be removed only after its exact managed identity and type have been
verified.

The stable executable path is the image-created, root-owned relative symlink
`/opt/gascan/shell/bin/starship -> ../../workstation/bin/starship`. The hook
validates that exact link identity and validates the resolved
`/opt/gascan/workstation/bin/starship` as a non-symlinked, root-owned mode
`0555` regular file before execution. It never searches `PATH`.

Both default `gascan shell` and SSH login sessions reach the same final hook.
Gas Can does not edit user-created shell files at runtime. A user who
deliberately replaces the image-provided startup chain also assumes control of
prompt activation.

## Presets

Both presets are owned and versioned by Gas Can. They show the sandbox
identity, current directory, Git branch and working-tree state, relevant active
language/runtime context, prior command status, and useful command duration
without turning every prompt into a wall of metadata.

The `starship` preset uses text and broadly supported Unicode symbols only. It
must remain legible with an ordinary terminal monospace font.

The `starship-nerd-font` preset uses the same information architecture but may
use Nerd Font icons and separators. Gas Can does not claim that the guest can
detect the font selected by a macOS terminal. Selecting this preset is the
user's assertion that the host terminal already uses a Nerd Font.

Preset output must remain readable in both light and dark terminal themes.
Neither preset embeds project paths, usernames, host secrets, or untrusted
manifest content.

## Provisioning and Apply Flow

The manifest prompt selection is carried through the validated sandbox spec
and provisioning plan. It is included in the plan fingerprint used to
determine whether an apply is required.

During initial provisioning or apply, the daemon invokes exactly:

```text
/usr/bin/sudo -n /usr/local/bin/configure-shell-home <validated-prompt>
```

The configurator accepts only effective root, one validated prompt argument,
and the fixed workspace identity. Its target home is compiled as
`/home/workspace`; inherited `HOME` is ignored because production `sudo`
resets it to `/root`. It never derives authority or a writable path from the
invoking user or environment. It then:

1. Validate the managed configuration root without following links.
2. Open and exclusively lock the root-owned transaction lock.
3. Validate or create the root-owned shell directory and `fsync` each newly
   created directory's parent.
4. Validate the pinned Starship executable and immutable preset inputs.
5. Generate the exact selector and selected preset into restrictive,
   root-owned staging files.
6. Publish them atomically and durably without replacing unsafe collisions.
7. Remove only verified obsolete Gas Can prompt artifacts.
8. Record successful provisioning through the existing apply lifecycle.

No prompt initialization runs as part of provisioning. The new selection takes
effect when the user opens the next interactive shell or SSH session.

## Failure Behavior

Manifest errors are reported before runtime mutation. Invalid prompt values
include the accepted values in the error.

Unsafe managed shell paths fail the relevant provisioning step with a concise
actionable error. Gas Can never repairs an unsafe symlink or non-regular
collision by deleting arbitrary user data.

Interactive shell access must remain available if the applied Starship binary
or generated configuration is unexpectedly missing or unreadable. The startup
hook prints one concise warning for that session and falls back to the
standard Bash prompt. It does not retry installation, access the network, or
abort the shell. This fallback covers Gas Can-owned inputs and initialization
failures; it is not a containment guarantee for arbitrary code already
executing in the interactive shell.

Starship initialization errors cannot affect non-interactive commands.
`gascan run`, explicit non-interactive shell argv, setup scripts, health
checks, and provisioning commands do not source the interactive hook.

## Compatibility

Existing manifests parse as `prompt = "standard"`. The manifest schema version
remains `1`.

Existing compatible sandbox records can apply the prompt state in place.
Sandboxes must use a workspace image containing the managed hook, pinned
Starship input, and Bash completion before Gas Can reports the feature ready.
Normal image compatibility and recreation rules remain authoritative; this
design does not add an independent volume-layout migration.

The public RPC shape does not change. The behavioral change is limited to the
server-side default argv for an empty Shell command and the manifest-driven
provisioning state.

## Verification

Automated coverage includes:

- manifest parsing for the omitted table, omitted field, all three prompt
  values, unknown fields, and invalid values;
- serialization and accessor coverage for the validated prompt enum;
- provisioning-plan fingerprints that change with the prompt selection;
- daemon API tests proving an empty Shell argv becomes
  `["/bin/bash", "--login"]`;
- protocol and API tests proving explicit Shell argv remains unchanged;
- existing attachment tests for resize, signals, EOF, cancellation, and exit
  status under the new default;
- image package contracts for `bash-completion`;
- image input, checksum, version, ownership, mode, and stable-path contracts
  for the pinned Starship binary;
- Bash startup contract tests proving interactive-only loading and standard
  fallback;
- Bash startup contracts proving pinned Starship-compatible caller
  `PROMPT_COMMAND` customization remains active;
- preset snapshots for compatible and Nerd Font modes;
- unsafe-file, symlink, interrupted-publication, retry, enable, switch, and
  disable provisioning tests;
- apply tests proving a prompt change is detected and updates an existing
  compatible sandbox;
- PTY tests proving the default session is interactive login Bash, receives
  terminal metadata, loads Bash completion, and exits cleanly;
- live Apple tests proving `gascan shell` and SSH activate the same selected
  prompt;
- offline verification proving an enabled managed preset performs no download;
- regression tests proving `gascan run` and explicit shell argv do not load
  prompt initialization.

The installed-release smoke checks the standard interactive shell and one
managed Starship selection without depending on visual terminal rendering.
The full live matrix remains opt-in under the existing Apple runtime gates.

## Documentation

The README quick start shows:

```bash
gascan shell
```

and explains that it opens the sandbox's interactive Bash login environment
with colors and completion.

The complete `gascan.toml` example includes the default shell table and
commented examples for both Starship presets. The shell reference explains:

- the three accepted values;
- that Gas Can pins and manages the Starship used by the prompt;
- that the standard and compatible Starship modes need no special font;
- that the Nerd Font preset requires configuring a Nerd Font in the host
  terminal;
- that the same selection applies to `gascan shell` and SSH;
- that explicit shell argv bypasses the managed default shell choice.
