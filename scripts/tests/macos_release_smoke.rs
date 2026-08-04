use std::{fs, path::PathBuf};

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("scripts has repository parent")
        .to_path_buf()
}

#[test]
fn polyglot_release_smoke_allocates_one_gibibyte() {
    let smoke =
        fs::read_to_string(repository_root().join("packaging/macos/release-smoke.sh")).unwrap();

    assert!(
        smoke.contains(r#"memory = "1GiB""#),
        "polyglot release smoke must allocate 1 GiB"
    );
    assert!(
        smoke.contains(r#"(cd "$fixture" && go install ./go-bin)"#),
        "memory contract must remain coupled to the real Go compiler workload"
    );
}

#[test]
fn readme_documents_complete_developer_onboarding_contract() {
    let readme = fs::read_to_string(repository_root().join("README.md")).unwrap();
    let readme = readme.split_whitespace().collect::<Vec<_>>().join(" ");

    for required in [
        "gascan up .",
        "gascan configure",
        "gascan configure git",
        "gascan configure gh",
        "gascan configure glab",
        "--hostname HOST",
        "--token-stdin",
        "--git-protocol ssh|https",
        "GitHub Enterprise",
        "GitLab Dedicated",
        "self-managed GitLab",
        "git config --global",
        "hidden terminal input",
        "does not provide a credential vault",
        "$GH_CONFIG_DIR/hosts.yml",
        "$GLAB_CONFIG_DIR/config.yml",
        "authentication key and a signing key",
        "auth_and_signing",
        "network = \"networked\"",
        "git log --show-signature -1",
        "gascan destroy --yes",
    ] {
        assert!(readme.contains(required), "README omits {required:?}");
    }
}

#[test]
fn release_smoke_uses_matching_branch_daemon_for_launch_and_shutdown() {
    let smoke =
        fs::read_to_string(repository_root().join("packaging/macos/release-smoke.sh")).unwrap();

    assert!(smoke.contains("GASCAN_RELEASE_GASCAND:-/usr/local/bin/gascand"));
    assert!(smoke.contains("export GASCAN_DAEMON=$gascand_bin"));
    assert!(smoke.contains(
        "GASCAN_RELEASE_APPLE_ATTACH_HELPER:-/usr/local/bin/gascan-apple-attach"
    ));
    assert!(smoke.contains(
        "GASCAN_RELEASE_APPLE_ATTACH_HELPER=\"${GASCAN_RELEASE_APPLE_ATTACH_HELPER:-}\""
    ));
    assert!(smoke.contains("[[ -x $apple_attach_bin ]]"));
    assert!(smoke.contains("apple_attach_bin=$(realpath \"$apple_attach_bin\")"));
    assert!(smoke.contains("export GASCAN_APPLE_ATTACH_HELPER=$apple_attach_bin"));
    assert!(smoke.contains("gascan_stop_attested_daemon \"$gascan_bin\" \"$gascand_bin\""));
    assert!(!smoke.contains("gascan_stop_attested_daemon \"$gascan_bin\" /usr/local/bin/gascand"));
}

#[test]
fn release_smoke_isolates_durable_controller_state_and_checks_destroyed_tombstones() {
    let smoke =
        fs::read_to_string(repository_root().join("packaging/macos/release-smoke.sh")).unwrap();

    for required in [
        "GASCAN_STATE_PATH|",
        "GASCAN_STATE_PATH=\"${GASCAN_STATE_PATH:-}\"",
        "state_path=$root/controller-state/state.sqlite3",
        "export GASCAN_STATE_PATH=$state_path",
        "rm -rf \"$root\"",
        "gascan_release_volume_marker=durable-controller-marker",
        "persistence_check controller.volume_marker",
        "gascan_release_recreate_runtime_root()",
        "runtime_root=$(gascan_user_runtime_root)",
        "gascan_release_recreate_runtime_root\n\"$gascan_bin\" --sandbox \"$sandbox_id\" status --json",
        "normal_controller_inventory=$(\"$gascan_bin\" list --json)",
        "No sandboxes found.",
        "controller_inventory=$(\"$gascan_bin\" list --all --json)",
    ] {
        assert!(
            smoke.contains(required),
            "release smoke omits durable controller-state contract {required:?}"
        );
    }

    assert!(
        !smoke.contains("\ncontroller_inventory=$(\"$gascan_bin\" list --json)"),
        "retained tombstone assertions must use list --all --json"
    );

    let daemon_stop = smoke
        .rfind("gascan_stop_attested_daemon \"$gascan_bin\" \"$gascand_bin\"")
        .expect("release smoke omits tested daemon replacement");
    let runtime_recreation = smoke[daemon_stop..]
        .find("\ngascan_release_recreate_runtime_root\n")
        .map(|index| daemon_stop + index)
        .expect("release smoke omits runtime recreation after daemon stop");
    let daemon_restart = smoke[runtime_recreation..]
        .find("status --json >/dev/null")
        .map(|index| runtime_recreation + index)
        .expect("release smoke omits daemon restart after runtime recreation");
    let marker_check = smoke[daemon_restart..]
        .find("post_recreation_marker=")
        .map(|index| daemon_restart + index)
        .expect("release smoke omits marker check after runtime recreation");
    assert!(daemon_stop < runtime_recreation);
    assert!(runtime_recreation < daemon_restart);
    assert!(daemon_restart < marker_check);
    assert!(smoke[marker_check..].contains(
        "[[ $post_recreation_marker == \"$gascan_release_volume_marker\" ]]"
    ));

    let marker_directory = smoke
        .find("mkdir -p \"$XDG_CONFIG_HOME/gascan-release-smoke\"")
        .expect("release smoke must create the marker directory");
    let marker_write = smoke
        .find(">\"$XDG_CONFIG_HOME/gascan-release-smoke/controller-state-marker\"")
        .expect("release smoke must write the persistence marker");
    assert!(
        marker_directory < marker_write,
        "release smoke must create the marker directory before writing the marker"
    );
}

#[test]
fn readme_documents_durable_controller_recovery_contract() {
    let readme = fs::read_to_string(repository_root().join("README.md")).unwrap();

    for required in [
        "~/Library/Application Support/dev.gascan/controller/state.sqlite3",
        "automatically migrates",
        "refuses to choose",
        "Package upgrades and ordinary uninstalls preserve this durable controller state.",
        "./packaging/macos/uninstall.sh --remove-data",
        "gascan list --all",
    ] {
        assert!(readme.contains(required), "README omits {required:?}");
    }
}

#[test]
fn release_smoke_runs_every_up_without_interactive_stdin() {
    let smoke =
        fs::read_to_string(repository_root().join("packaging/macos/release-smoke.sh")).unwrap();

    let direct_up = "\"$gascan_bin\" up \"$root\"";
    assert!(smoke.contains(
        "gascan_release_up() {\n  CI=1 \"$gascan_bin\" up \"$root\" </dev/null\n}"
    ));
    assert_eq!(
        smoke.matches(direct_up).count(),
        1,
        "only the noninteractive helper may invoke gascan up directly"
    );
    assert_eq!(
        smoke.matches("\ngascan_release_up\n").count(),
        4,
        "every initial, existing, restart, and offline up must use the helper"
    );
}

#[test]
fn release_smoke_proves_fake_forge_signed_git_and_persistence_without_host_tokens() {
    let smoke =
        fs::read_to_string(repository_root().join("packaging/macos/release-smoke.sh")).unwrap();

    for required in [
        "/usr/bin/env -i",
        "GASCAN_RELEASE_ENV_SANITIZED=1",
        "GIT_CONFIG_GLOBAL",
        "configure\", \"git",
        "configure gh --hostname github.enterprise.test --token-stdin --git-protocol https",
        "configure glab --hostname gitlab.enterprise.test --token-stdin --git-protocol https",
        "gascan-release-fake-gh",
        "gascan-release-fake-glab",
        "git commit",
        "git tag -s",
        "git verify-commit HEAD",
        "git verify-tag gascan-release-signed",
        "git cat-file tag gascan-release-signed | grep -F -- \"-----BEGIN SSH SIGNATURE-----\"",
        "NESTED_STARSHIP_FUNCTION=function",
        "developer-key.sha256",
    ] {
        assert!(smoke.contains(required), "release smoke omits {required:?}");
    }
    assert!(!smoke.contains("| grep -F \"-----BEGIN SSH SIGNATURE-----\""));
    for forbidden in [
        "gh auth token",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GLAB_TOKEN",
        "GITLAB_TOKEN",
    ] {
        assert!(
            !smoke.contains(forbidden),
            "release smoke could consume host token source {forbidden:?}"
        );
    }
}

#[test]
fn release_smoke_shell_assertions_name_failed_fields_and_bound_diagnostics() {
    let smoke =
        fs::read_to_string(repository_root().join("packaging/macos/release-smoke.sh")).unwrap();

    for required in [
        "shell probe field mismatch: selector=%s field=%s expected=%s",
        "shell probe pattern mismatch: selector=%s field=%s expected=%s",
        "captured shell output (last 4096 characters):",
        "${captured: -4096}",
        "gascan_assert_shell_field standard \"$required\" \"$standard_shell\"",
        "gascan_assert_shell_pattern standard BASH_VERSION '^BASH_VERSION=.+$' \"$standard_shell\"",
        "gascan_assert_shell_field starship \"$required\" \"$starship_shell\"",
        "gascan_assert_shell_field starship-nerd-font \"$required\" \"$nerd_shell\"",
    ] {
        assert!(
            smoke.contains(required),
            "release smoke omits diagnostic shell assertion contract {required:?}"
        );
    }

    for silent_assertion in [
        "grep -Fx \"$required\" <<<\"$standard_shell\" >/dev/null",
        "grep -E '^BASH_VERSION=.+$' <<<\"$standard_shell\" >/dev/null",
        "grep -Fx \"$required\" <<<\"$starship_shell\" >/dev/null",
        "grep -Fx \"$required\" <<<\"$nerd_shell\" >/dev/null",
    ] {
        assert!(
            !smoke.contains(silent_assertion),
            "release smoke retains silent shell assertion {silent_assertion:?}"
        );
    }
}

#[test]
fn release_smoke_quiesces_outer_prompt_before_framing_shell_fields() {
    let smoke =
        fs::read_to_string(repository_root().join("packaging/macos/release-smoke.sh")).unwrap();

    assert!(
        smoke.contains(
            "PROMPT_COMMAND=; PS1= PS2=\nprintf 'GASCAN_RELEASE_SHELL_BEGIN\\\\n'"
        ),
        "outer PTY probe must stop precmd from prefixing framed field output"
    );
    for required in [
        "printf 'STARSHIP_FUNCTION=%s\\\\n' \"$(type -t starship_precmd || true)\"",
        "/bin/bash --login -i -c 'printf \"NESTED_STARSHIP_CONFIG=%s\\\\n\"",
        "'NESTED_STARSHIP_FUNCTION=function'",
    ] {
        assert!(
            smoke.contains(required),
            "probe-only framing change must retain Starship assertion {required:?}"
        );
    }
}

#[test]
fn release_smoke_persistence_checks_name_fields_and_never_print_fixture_token() {
    let smoke =
        fs::read_to_string(repository_root().join("packaging/macos/release-smoke.sh")).unwrap();

    for required in [
        "credential persistence check failed: field=%s exit=%s",
        "credential persistence safe output (last 4096 characters):",
        "output=${output//gascan-release-fake-token/[REDACTED]}",
        "${output: -4096}",
        "else\n      status=$?\n    fi",
        "persistence_check setup.result",
        "persistence_check git.private_key_checksum",
        "persistence_check git.user_name",
        "persistence_check git.user_email",
        "persistence_check git.verify_commit",
        "persistence_check git.verify_tag",
        "persistence_check forge.github.config_mode",
        "persistence_check forge.gitlab.config_mode",
        "persistence_check forge.github.auth_status",
        "persistence_check forge.gitlab.auth_status",
        "credential_persistence_fail forge.config_token_absence",
        "grep -R -F gascan-release-fake-token \"$GH_CONFIG_DIR\" \"$GLAB_CONFIG_DIR\" >/dev/null 2>&1",
    ] {
        assert!(
            smoke.contains(required),
            "release smoke omits safe persistence diagnostic contract {required:?}"
        );
    }
}

#[test]
fn release_smoke_rejects_spoofed_sanitizer_markers_and_preflights_daemon() {
    let smoke =
        fs::read_to_string(repository_root().join("packaging/macos/release-smoke.sh")).unwrap();

    for required in [
        "gascan_release_environment_is_sanitized()",
        "#!/bin/bash -p",
        "builtin export -pf",
        "builtin compgen -e",
        "GASCAN_RELEASE_ENV_SANITIZED|",
        "PWD|SHLVL)",
        "if [[ $- != *p* ]] || ! gascan_release_environment_is_sanitized; then",
        "/bin/bash --noprofile --norc -p \"$0\" \"$@\"",
        "gascan_release_preflight_daemon()",
        "release smoke refused unsafe or mismatched pre-existing Gas Can daemon",
        "release smoke could not prove the selected daemon is stopped",
    ] {
        assert!(
            smoke.contains(required),
            "release smoke omits sanitizer/preflight contract {required:?}"
        );
    }

    assert!(!smoke.contains("exec env -i"));
    let daemon_export = smoke.find("export GASCAN_DAEMON=$gascand_bin").unwrap();
    let preflight = smoke
        .find("\ngascan_release_preflight_daemon\n")
        .expect("release smoke omits daemon preflight call");
    let helper_export = smoke
        .find("export GASCAN_APPLE_ATTACH_HELPER=$apple_attach_bin")
        .unwrap();
    assert!(daemon_export < preflight);
    assert!(preflight < helper_export);
}

#[test]
fn release_smoke_fake_forges_require_the_managed_public_key() {
    let smoke =
        fs::read_to_string(repository_root().join("packaging/macos/release-smoke.sh")).unwrap();
    let key_check =
        r#"[[ $key == "$(< /home/workspace/.config/gascan/git/ssh/id_ed25519.pub)" ]]"#;

    assert_eq!(
        smoke.matches(key_check).count(),
        2,
        "GitHub and GitLab fake registration must both require the managed public key"
    );
}

#[test]
fn macos_release_smoke_proves_portable_github_token_login_and_safe_compact_summaries() {
    let smoke =
        fs::read_to_string(repository_root().join("packaging/macos/release-smoke.sh")).unwrap();

    for required in [
        "gh auth login rejected unsupported --skip-ssh-key",
        "gh argv:",
        "--with-token",
        "gh argv: <auth> <login> <--hostname> <github.enterprise.test> <--git-protocol> <https> <--with-token>",
        "github_configure_output=$(printf '%s' \"$fake_forge_token\" |",
        "github_configure_retry_output=$(printf '%s' \"$fake_forge_token\" |",
        "GitHub: gascan-release-fake-gh at github.enterprise.test; protocol https; authentication configured; authentication key added; signing key added",
        "GitHub: gascan-release-fake-gh at github.enterprise.test; protocol https; authentication configured; authentication key existing; signing key existing",
        "! grep -F -- \"$fake_forge_token\" <<<\"$transcript\" >/dev/null",
        "! grep -F gascan-release-fake-token \"$log\" >/dev/null",
        "release smoke GitHub configure transcript leaked fixture token",
        "release smoke fake forge log leaked fixture token",
    ] {
        assert!(
            smoke.contains(required),
            "release smoke omits portable GitHub login contract {required:?}"
        );
    }

    assert!(
        !smoke.contains("--skip-ssh-key --with-token"),
        "release smoke must not invoke GitHub CLI with its unsupported skip-key flag"
    );
}

#[test]
fn macos_release_smoke_readme_documents_the_compact_first_run_flow() {
    let readme = fs::read_to_string(repository_root().join("README.md")).unwrap();

    for required in [
        "Use this identity with SSH transport and signed commits? [Y/n]",
        "Import <account> at <hostname>? [Y/n]",
        "m for manual token, or s to skip",
        "automatic color and falls back when `NO_COLOR` is set",
        "completed work is retained",
        "gascan configure git",
        "gascan configure gh",
        "gascan configure glab",
        "does not upgrade `gh` or `glab`",
        "tools shipped in the workspace image",
    ] {
        assert!(readme.contains(required), "README omits {required:?}");
    }
}
