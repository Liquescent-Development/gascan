# Polished CLI Output Design

## Purpose

Gascan's default command output must communicate outcomes and useful progress in
language operators understand. It must not expose internal operation phases,
protocol identifiers, release evidence, or implementation details unless the
operator explicitly requests JSON.

This change covers the human output of `up`, `apply`, `down`, `destroy`,
`doctor`, `status`, and `list`. It also establishes consistent human error
presentation for those commands. It does not change daemon behavior, protocol
schemas, JSON schemas, or exit codes.

## Output Modes

The renderer has three explicit modes:

1. Interactive human output is selected when the relevant output stream is a
   terminal. It may use animation, color, and Unicode symbols.
2. Static human output is selected for redirected output. It prints stable,
   non-animated, uncolored, ASCII-safe lines with no cursor-control sequences.
3. JSON output is selected only by `--json`. It bypasses the human renderer and
   preserves the existing JSON or JSON Lines schema exactly.

Interactive color respects terminal capability and the `NO_COLOR` environment
variable. `NO_COLOR` disables color but does not disable animation or Unicode
when the stream is otherwise interactive.

Operation progress remains on stderr so stdout can continue to be redirected
independently. Inspection output from `doctor`, `status`, and `list` remains on
stdout. Errors remain on stderr.

## Lifecycle Commands

`up`, `apply`, `down`, and `destroy` use one Gascan-owned operation presenter.
The presenter wraps a small terminal-progress library rather than implementing
cursor management directly.

For an interactive terminal, it shows one in-place spinner. Each meaningful
protocol phase updates the spinner with a command-appropriate, sentence-case
message. Examples include:

- `Validating configuration`
- `Creating sandbox`
- `Starting sandbox`
- `Writing safe mise configuration`
- `Installing project tools`
- `Running project setup`
- `Verifying Gascamp`
- `Checking sandbox health`
- `Stopping sandbox`
- `Destroying sandbox`

The renderer maps structured phases and provision-step enum values to these
phrases. It never derives human output from opaque payload data. Plumbing-only
phases such as `operation`, `before_provision`, `after_provision`,
`before_health`, and `after_health` are suppressed. Consecutive duplicate
messages are suppressed.

On success, the spinner becomes one concise completion line. The exact message
is command-specific and includes a sandbox identifier only when the command
handler already has one from its selector, for example:

```text
✓ Sandbox code-3fd063e3b68e is running
```

`up` and `apply` do not parse opaque event payloads or issue an extra API call
solely to discover an identifier. When no identifier is already available, the
renderer reports the command outcome, for example `Sandbox is running`.

For static human output, the same meaningful messages are printed once as
plain lines, followed by the plain completion line. Static output contains no
spinner frames, ANSI escapes, or Unicode-only symbols.

The progress object is guarded so every return path finishes or clears it.
Daemon-reported operation errors, RPC stream errors, and local I/O errors clear
the active spinner before the error is printed. A failed command never leaves a
partial progress line or corrupts the next shell prompt.

## Doctor Output

Human `doctor` output leads with the overall result and groups checks by the
prefix before the first dot. Known groups are titled `Host`, `Runtime`,
`Storage`, and `Workspace`; unknown future groups are converted to sentence
case rather than omitted.

When every check passes, the output is compact:

```text
✓ Gascan is ready
  Host       2/2 checks passed
  Runtime    11/11 checks passed
  Storage    2/2 checks passed
  Workspace  1/1 check passed
```

Passing checks do not print their detail strings. This specifically keeps
release commits, report hashes, fixture hashes, and backend evidence out of
ordinary human output. All existing detail remains available in
`doctor --json`.

When checks fail, the heading becomes `Gascan needs attention`. Each failed
check expands beneath its group with:

- a humanized name derived from the portion after the group prefix;
- its useful detail text, if non-empty; and
- a `Fix:` line containing its remedy, if non-empty.

Passing groups still show totals, so the user can see the scope of the report
without reading every successful check. The command retains its existing exit
status behavior.

## Status and List Output

Human `status` output is a compact labeled summary:

```text
Sandbox: code-3fd063e3b68e
State:   Running
```

Human `list` output is an aligned table with `SANDBOX` and `STATE` headings.
States are title-cased for people while JSON keeps its existing lowercase
values. When no sandboxes exist, `list` prints `No sandboxes found.` rather
than producing empty output.

## Human Error Presentation

Human errors use a single presentation path and start with `Error:`. When the
condition has a known recovery action, the renderer adds a separate `Try:`
line. Existing daemon-provided cause text remains the primary explanation.
Stable internal error codes are shown only as a fallback when no cause or
specific human explanation is available.

The first targeted recovery mappings are the errors already encountered in the
normal lifecycle:

- no available sandbox: suggest `gascan up <project-root>`;
- multiple available sandboxes: suggest `gascan list` and
  `--sandbox <sandbox-id>`;
- sandbox not found: suggest `gascan list` and use of the Gascan sandbox ID;
- resource conflict: explain that a managed runtime resource already exists
  and retain the daemon's concrete resource name for diagnosis.

Error presentation does not change error classification or exit codes. JSON
operation streams preserve their existing error event and do not receive human
decoration.

## Components and Data Flow

A new focused presentation module owns:

- terminal capability and output-mode selection;
- styles and symbols;
- lifecycle phase-to-message mapping;
- progress lifecycle and cleanup;
- doctor grouping and rendering;
- status and list rendering; and
- human error formatting.

CLI command handlers continue to parse arguments, invoke the API, and select
exit codes. They pass structured protobuf values to the presenter. JSON
branches continue to serialize directly and never pass through the human
presenter. The daemon and protobuf API remain unchanged.

Writer and terminal-capability inputs are injectable at the presentation
boundary. Production uses stdout/stderr and real terminal detection; tests use
captured buffers and explicit capabilities. Animation timing is owned by the
progress library and does not enter deterministic rendering tests.

## Verification

Implementation follows test-driven development. Renderer tests cover:

- semantic mapping for every known lifecycle phase and provision step;
- suppression of plumbing phases and consecutive duplicates;
- interactive success, failure, and cleanup behavior;
- static output without ANSI, cursor controls, animation frames, or Unicode;
- color capability and `NO_COLOR` behavior;
- compact passing doctor output;
- grouped doctor failures, details, and remedies;
- humanized unknown doctor groups and checks;
- status, list, and empty-list layouts;
- consistent human error headings and recovery suggestions; and
- byte-for-byte unchanged JSON structures.

Focused CLI integration tests exercise a real pseudo-terminal to verify that
progress updates in place and restores the prompt cleanly. Non-TTY integration
tests verify deterministic output and confirm that raw strings such as
`operation`, `before_provision`, and `provision_step` do not leak from lifecycle
commands. Existing command and end-to-end suites must remain green.

## Non-Goals

- Changing daemon operation phases or protobuf messages.
- Adding a verbose human mode or changing the `--json` contract.
- Restyling `run`, `shell`, or `logs`, whose output belongs to the process or
  log stream being transported.
- Hiding concrete causes that are necessary to resolve an error.
