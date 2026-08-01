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
fn release_smoke_runs_every_up_without_interactive_stdin() {
    let smoke =
        fs::read_to_string(repository_root().join("packaging/macos/release-smoke.sh")).unwrap();

    let direct_up = "\"$gascan_bin\" up \"$root\"";
    assert!(smoke.contains(
        "gascan_release_up() {\n  \"$gascan_bin\" up \"$root\" </dev/null\n}"
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
        "exec env -i",
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
