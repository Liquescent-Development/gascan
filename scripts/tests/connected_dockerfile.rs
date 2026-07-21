use std::{collections::BTreeSet, fs, path::Path, process::Command};

const MISE_LS_FILTER: &str = r#"if ((keys|sort) != ["elixir","erlang","go","java","node","python","ruby","rust"]) then error("unexpected mise tool set") else to_entries | map(if ((.value|type)!="array") or ((.value|length)!=1) or (.value[0].installed != true) or (.value[0].active != true) or ((.value[0].version|type)!="string") or (.value[0].version=="") then error("invalid mise ls record") else {key:.key,value:.value[0].version} end) | from_entries end"#;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

fn assert_sole_reviewed_package_install(dockerfile: &str) -> Result<(), &'static str> {
    if dockerfile.lines().any(|line| {
        line.split_whitespace()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|tokens| tokens == ["apt", "install"])
    }) {
        return Err("direct apt install bypasses the reviewed package file");
    }
    let apt_get_lines: Vec<_> = dockerfile
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("apt-get"))
        .collect();
    if apt_get_lines
        != [
            "&& apt-get -o Acquire::Retries=0 update \\",
            "&& DEBIAN_FRONTEND=noninteractive xargs apt-get \\",
            "&& apt-get clean \\",
        ]
    {
        return Err("apt-get must only update, install from the reviewed file, and clean");
    }
    if !dockerfile.contains(
        "&& DEBIAN_FRONTEND=noninteractive xargs apt-get \\\n         -o Acquire::Retries=0 install --yes --no-install-recommends </tmp/system-tools.txt \\",
    ) {
        return Err("package install must consume only the reviewed file");
    }
    Ok(())
}

fn assert_correct_otp_release_term_check(script: &str) -> Result<(), &'static str> {
    let exact = r#"erlang:system_info(otp_release) =:= "29""#;
    if !script.contains(exact) {
        return Err("OTP release must use strict equality with Erlang's string/list result");
    }
    if script.contains(r#"otp_release) =:= <<"29">>"#) {
        return Err("OTP release list must not be compared with an Erlang binary");
    }
    Ok(())
}

fn effective_env_value<'a>(dockerfile: &'a str, variable: &str) -> Option<&'a str> {
    dockerfile
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("ENV "))
        .flat_map(str::split_whitespace)
        .filter_map(|assignment| assignment.split_once('='))
        .filter_map(|(name, value)| (name == variable).then_some(value))
        .next_back()
}

fn assert_persistent_rustup_homes(dockerfile: &str) -> Result<(), &'static str> {
    let first_install = dockerfile
        .find("mise install --yes")
        .ok_or("missing mise install")?;
    for (variable, value) in [
        ("CARGO_HOME", "/opt/gascan/mise/cargo"),
        ("RUSTUP_HOME", "/opt/gascan/mise/rustup"),
    ] {
        let declaration = format!("ENV {variable}={value}");
        let position = dockerfile
            .find(&declaration)
            .ok_or("missing persistent Rustup home")?;
        if position >= first_install {
            return Err("Rustup homes must be set before mise installs tools");
        }
        if effective_env_value(dockerfile, variable) != Some(value) {
            return Err("effective Rustup homes must remain persistent");
        }
    }
    Ok(())
}

#[test]
fn dockerfile_assembles_the_connected_workspace_base() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    for required in [
        "FROM ${BASE_IMAGE} AS workspace-base",
        "apt-get -o Acquire::Retries=0 update",
        "install --yes --no-install-recommends",
        "rm -rf /var/lib/apt/lists/*",
        "COPY --chmod=0555 .artifacts/mise-linux-arm64 /usr/local/bin/mise",
        "mise install --yes",
        "mise ls --current --installed --json",
        "cmp --silent /tmp/resolved-tool-versions.json /tmp/expected-tool-versions.json",
    ] {
        assert!(
            dockerfile.contains(required),
            "missing connected contract: {required}"
        );
    }
    for forbidden in [
        "bundles/ubuntu_packages",
        "bundles/mise_runtimes",
        "Dir::Bin::methods=/nonexistent",
        "apt-get upgrade",
        "latest",
    ] {
        assert!(
            !dockerfile.contains(forbidden),
            "deferred/unlocked path: {forbidden}"
        );
    }
}

#[test]
fn dockerfile_creates_traversable_mise_config_directory_before_copying_config() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let directory = dockerfile
        .find("RUN install -d -o root -g root -m 0555 /etc/mise")
        .expect("missing explicit root-owned mode 0555 /etc/mise creation");
    let config = dockerfile
        .find("COPY --chmod=0444 images/workspace/etc/mise/config.toml /etc/mise/config.toml")
        .unwrap();
    assert!(
        directory < config,
        "/etc/mise must be created before config.toml is copied"
    );
}

#[test]
fn dockerfile_sets_persistent_rustup_homes_before_mise_installs_tools() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    assert_persistent_rustup_homes(&dockerfile).unwrap();
}

#[test]
fn rustup_home_contract_rejects_later_overrides() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    for later_override in ["ENV CARGO_HOME=/tmp/cargo", "ENV RUSTUP_HOME=/tmp/rustup"] {
        let mutated = format!("{dockerfile}\n{later_override}\n");
        assert!(
            assert_persistent_rustup_homes(&mutated).is_err(),
            "accepted later override: {later_override}"
        );
    }
}

fn normalize_mise_ls(input: &str) -> std::process::Output {
    Command::new("jq")
        .args([
            "--exit-status",
            "--compact-output",
            "--sort-keys",
            MISE_LS_FILTER,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(input.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap()
}

#[test]
fn mise_ls_schema_requires_one_active_installed_record_per_preserved_key() {
    let record =
        |version: &str| format!(r#"[{{"version":"{version}","installed":true,"active":true}}]"#);
    let valid = format!(
        r#"{{"elixir":{},"erlang":{},"go":{},"java":{},"node":{},"python":{},"ruby":{},"rust":{}}}"#,
        record("1.20.2-otp-29"),
        record("29.0.3"),
        record("1.26.5"),
        record("25.0.2"),
        record("24.18.0"),
        record("3.14.6"),
        record("3.4.10"),
        record("1.97.0")
    );
    let output = normalize_mise_ls(&valid);
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), r#"{"elixir":"1.20.2-otp-29","erlang":"29.0.3","go":"1.26.5","java":"25.0.2","node":"24.18.0","python":"3.14.6","ruby":"3.4.10","rust":"1.97.0"}"#.to_owned() + "\n");
    for invalid in [
        valid.replace(&record("29.0.3"), "[]"),
        valid.replace(
            &record("29.0.3"),
            &format!(
                "[{},{}]",
                &record("29.0.3")[1..record("29.0.3").len() - 1],
                &record("29.0.3")[1..record("29.0.3").len() - 1]
            ),
        ),
        valid.replace(r#""installed":true"#, r#""installed":false"#),
        valid.replace(r#""active":true"#, r#""active":false"#),
    ] {
        assert!(
            !normalize_mise_ls(&invalid).status.success(),
            "accepted {invalid}"
        );
    }
    let extra = valid.replacen(
        '{',
        r#"{"unexpected":[{"version":"1","installed":true,"active":true}],"#,
        1,
    );
    assert!(!normalize_mise_ls(&extra).status.success());
}

#[test]
fn dockerfile_uses_supported_mise_ls_schema_and_exact_filter() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    assert!(dockerfile.contains("mise ls --current --installed --json"));
    assert!(!dockerfile.contains("mise current --json"));
    assert!(dockerfile.contains(MISE_LS_FILTER));
}

#[test]
fn dockerfile_installs_pinned_erlang_before_elixir_and_validates_otp_29() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let erlang = dockerfile.find("mise install --yes erlang@29.0.3").unwrap();
    let otp = dockerfile
        .find("mise exec erlang@29.0.3 -- erl -noshell -eval")
        .unwrap();
    let elixir = dockerfile
        .find("mise exec erlang@29.0.3 -- mise install --yes elixir@1.20.2-otp-29")
        .unwrap();
    let remaining = dockerfile.find("mise install --yes go@1.26.5").unwrap();
    assert!(erlang < otp && otp < elixir && elixir < remaining);
    assert!(!dockerfile.contains("&& erl -noshell"));
    assert!(dockerfile.contains("otp_release"));
    assert_correct_otp_release_term_check(&dockerfile).unwrap();
    assert!(dockerfile.contains(r#"test "$(mise current elixir)" = "1.20.2-otp-29""#));
}

#[test]
fn otp_release_contract_rejects_binary_type_and_wrong_major() {
    let valid = r#"true = (erlang:system_info(otp_release) =:= "29"), halt()."#;
    assert!(assert_correct_otp_release_term_check(valid).is_ok());
    assert!(
        assert_correct_otp_release_term_check(&valid.replace(r#""29""#, r#"<<"29">>"#)).is_err()
    );
    assert!(assert_correct_otp_release_term_check(&valid.replace("29", "28")).is_err());
}

#[test]
fn dockerfile_prints_safe_mise_version_metadata_only_when_the_lock_comparison_fails() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    assert!(dockerfile.contains("if ! cmp --silent"));
    assert!(!dockerfile.contains("mise version metadata mismatch"));
    assert!(!dockerfile.contains("actual resolved versions:"));
    assert!(!dockerfile.contains("expected resolved versions:"));
}

#[test]
fn mise_comparison_is_quiet_on_match_and_emits_only_both_json_documents_on_mismatch() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let block = dockerfile
        .split("if ! cmp --silent")
        .nth(1)
        .unwrap()
        .split("       fi \\")
        .next()
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let actual = temp.path().join("actual.json");
    let expected = temp.path().join("expected.json");
    let script = format!(
        "if ! cmp --silent{} fi",
        block
            .replace("/tmp/resolved-tool-versions.json", actual.to_str().unwrap())
            .replace(
                "/tmp/expected-tool-versions.json",
                expected.to_str().unwrap()
            )
            .replace("\\\n", "\n")
    );
    fs::write(&actual, "{\"node\":\"20\"}\n").unwrap();
    fs::write(&expected, "{\"node\":\"20\"}\n").unwrap();
    let equal = Command::new("bash").args(["-c", &script]).output().unwrap();
    assert!(equal.status.success());
    assert!(equal.stdout.is_empty());
    assert!(equal.stderr.is_empty());
    fs::write(&expected, "{\"node\":\"22\"}\n").unwrap();
    let mismatch = Command::new("bash").args(["-c", &script]).output().unwrap();
    assert!(!mismatch.status.success());
    assert_eq!(mismatch.stdout, b"{\"node\":\"20\"}\n{\"node\":\"22\"}\n");
    assert!(mismatch.stderr.is_empty());
}

#[test]
fn dockerfile_installs_exactly_the_sorted_unique_reviewed_package_list() {
    let package_text = fs::read_to_string(root().join("tests/image/system-tools.txt")).unwrap();
    assert!(package_text.ends_with('\n'));
    assert!(package_text.lines().all(|line| !line.is_empty()));
    let packages: Vec<_> = package_text.lines().collect();
    let sorted_unique: BTreeSet<_> = packages.iter().copied().collect();
    assert_eq!(packages, sorted_unique.into_iter().collect::<Vec<_>>());

    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    for required in [
        "COPY --chmod=0444 tests/image/system-tools.txt /tmp/system-tools.txt",
        "xargs apt-get \\",
        "--no-install-recommends </tmp/system-tools.txt",
        "done </tmp/system-tools.txt",
        "rm -rf /var/lib/apt/lists/* /tmp/system-tools.txt",
    ] {
        assert!(
            dockerfile.contains(required),
            "missing package contract: {required}"
        );
    }
    assert_sole_reviewed_package_install(&dockerfile).unwrap();
}

#[test]
fn package_contract_rejects_an_inline_unreviewed_install() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let mutated = format!("{dockerfile}\nRUN apt-get install arbitrary-package\n");
    assert!(assert_sole_reviewed_package_install(&mutated).is_err());
}

#[test]
fn package_contract_rejects_an_inline_unreviewed_apt_install() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let mutated = format!("{dockerfile}\nRUN apt install arbitrary-package\n");
    assert!(assert_sole_reviewed_package_install(&mutated).is_err());
}
