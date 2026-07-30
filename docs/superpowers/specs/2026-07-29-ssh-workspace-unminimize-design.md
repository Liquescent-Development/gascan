# SSH Workspace and Full Ubuntu Image Design

## Goal

Make the interactive SSH experience match `gascan shell`: start in the
bind-mounted `/workspace` with no Ubuntu minimized-image warning, while
preserving SSH automation, SFTP, and editor bootstrap behavior.

## Design

The workspace image will run Ubuntu's `unminimize` operation during the
connected image build. This restores the content intentionally removed from
Ubuntu's minimized base instead of merely suppressing its login warning.

The managed interactive Bash hook will change to `/workspace` only when all
of these conditions hold:

- the shell is interactive;
- `SSH_CONNECTION` is set;
- the shell started in `$HOME`;
- `/workspace` is a directory.

The user's home remains `/home/workspace`, so durable tool configuration and
managed volumes keep their existing paths. Noninteractive SSH commands do not
source the interactive hook, SFTP does not invoke Bash, and sessions started
outside `$HOME` are not redirected.

## Verification

Contract tests will prove the Dockerfile performs `unminimize`, the minimized
login marker is absent, interactive SSH lands in `/workspace`, and
noninteractive SSH still starts in `/home/workspace`. The connected image
suite and release gates must pass before publishing version `0.1.15`.
