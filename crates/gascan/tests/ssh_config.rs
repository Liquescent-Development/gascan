use gascan::ssh_config::{
    INCLUDE_BLOCK_LF, IncludeChange, OfferAnswer, SshConfig, answer_first_use_offer,
    first_use_offer, managed_config_path,
};
use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn fixture() -> TestResult<(tempfile::TempDir, SshConfig)> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let xdg = temp.path().join("config");
    fs::create_dir(&home)?;
    fs::create_dir(&xdg)?;
    Ok((temp, SshConfig::for_environment(Some(&xdg), Some(&home))?))
}

#[test]
fn managed_path_always_matches_the_fixed_home_relative_include() -> TestResult {
    assert_eq!(
        managed_config_path(Some(Path::new("/tmp/xdg")), Some(Path::new("/tmp/home")))?,
        PathBuf::from("/tmp/home/.config/gascan/ssh/config")
    );
    assert_eq!(
        managed_config_path(None, Some(Path::new("/tmp/home")))?,
        PathBuf::from("/tmp/home/.config/gascan/ssh/config")
    );
    assert!(managed_config_path(Some(Path::new("/tmp/xdg")), None).is_err());
    Ok(())
}

#[test]
fn production_authority_matches_openssh_tilde_and_ignores_overridden_home() -> TestResult {
    let account_home = gascan_core::account::effective_account_home()?;
    let production = SshConfig::for_user()?;
    assert_eq!(
        production.managed_config_path(),
        account_home.join(".config/gascan/ssh/config")
    );

    let temp = tempfile::tempdir()?;
    let override_home = temp.path().join("overridden-home");
    fs::create_dir(&override_home)?;
    let openssh = std::process::Command::new("/usr/bin/ssh")
        .env("HOME", &override_home)
        .args(["-G", "-F", "/dev/null"])
        .arg("gascan-account-home-probe")
        .output()?;
    assert!(openssh.status.success(), "{openssh:?}");
    let openssh = String::from_utf8(openssh.stdout)?;
    assert!(
        openssh.lines().any(|line| {
            line == format!(
                "userknownhostsfile {0}/.ssh/known_hosts {0}/.ssh/known_hosts2",
                account_home.display()
            )
        }),
        "OpenSSH account-home expansion missing: {openssh}"
    );

    let path = std::process::Command::new(env!("CARGO_BIN_EXE_gascan"))
        .env("HOME", &override_home)
        .env("XDG_CONFIG_HOME", temp.path().join("overridden-xdg"))
        .args(["ssh-config", "path"])
        .output()?;
    assert!(path.status.success(), "{path:?}");
    assert_eq!(
        path.stdout,
        format!(
            "{}\n",
            account_home.join(".config/gascan/ssh/config").display()
        )
        .as_bytes()
    );
    Ok(())
}

#[test]
fn environment_overrides_cannot_select_production_ssh_mutation_paths() -> TestResult {
    const CHILD: &str = "GASCAN_TEST_SSH_MUTATION_AUTHORITY_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let overridden_home =
            PathBuf::from(std::env::var_os("HOME").ok_or("child HOME is missing")?);
        let account_home = gascan_core::account::effective_account_home()?;
        let production = SshConfig::for_user()?;
        assert_eq!(
            production.user_config_path(),
            account_home.join(".ssh/config")
        );
        assert_eq!(
            production.managed_config_path(),
            account_home.join(".config/gascan/ssh/config")
        );
        assert_ne!(
            production.user_config_path(),
            overridden_home.join(".ssh/config")
        );
        assert_ne!(
            production
                .managed_config_path()
                .parent()
                .ok_or("managed config has no parent")?
                .join("include-offer-v1"),
            overridden_home.join(".config/gascan/ssh/include-offer-v1")
        );

        let injected = SshConfig::for_environment(None, Some(&overridden_home))?;
        assert_eq!(
            injected.user_config_path(),
            overridden_home.join(".ssh/config")
        );
        assert_eq!(
            injected.managed_config_path(),
            overridden_home.join(".config/gascan/ssh/config")
        );
        return Ok(());
    }

    let temp = tempfile::tempdir()?;
    let overridden_home = temp.path().join("overridden-home");
    fs::create_dir(&overridden_home)?;
    let output = std::process::Command::new(std::env::current_exe()?)
        .env(CHILD, "1")
        .env("HOME", &overridden_home)
        .env("XDG_CONFIG_HOME", temp.path().join("overridden-xdg"))
        .args([
            "--exact",
            "environment_overrides_cannot_select_production_ssh_mutation_paths",
            "--nocapture",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "production mutation authority trusted environment overrides: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
fn install_and_remove_are_idempotent_and_touch_only_the_exact_block() -> TestResult {
    let (_temp, config) = fixture()?;
    let original = b"Host personal\n    HostName example.test\n";
    write_user_config(&config, original)?;

    assert_eq!(config.install()?, IncludeChange::Changed);
    assert_eq!(config.install()?, IncludeChange::Unchanged);
    let installed = fs::read(config.user_config_path())?;
    assert_eq!(installed, [INCLUDE_BLOCK_LF, original.as_slice()].concat());

    assert_eq!(config.remove()?, IncludeChange::Changed);
    assert_eq!(config.remove()?, IncludeChange::Unchanged);
    assert_eq!(fs::read(config.user_config_path())?, original);
    Ok(())
}

#[test]
fn crlf_and_missing_final_newline_round_trip_byte_for_byte() -> TestResult {
    for original in [
        b"Host personal\r\n    HostName example.test\r\n".as_slice(),
        b"Host personal\n    HostName example.test".as_slice(),
        b"".as_slice(),
    ] {
        let (_temp, config) = fixture()?;
        write_user_config(&config, original)?;
        config.install()?;
        config.remove()?;
        assert_eq!(fs::read(config.user_config_path())?, original);
    }
    Ok(())
}

#[test]
fn removal_preserves_similar_user_comments_and_includes() -> TestResult {
    let (_temp, config) = fixture()?;
    let original = concat!(
        "# >>> gascan managed ssh include >>> user note\n",
        "Include ~/.config/gascan/ssh/config.backup\n",
        "# <<< gascan managed ssh include <<< user note\n",
    )
    .as_bytes();
    write_user_config(&config, original)?;
    config.install()?;
    config.remove()?;
    assert_eq!(fs::read(config.user_config_path())?, original);
    Ok(())
}

#[test]
fn embedded_marker_text_is_not_mistaken_for_the_exact_managed_block() -> TestResult {
    let (_temp, config) = fixture()?;
    let original = concat!(
        "Match host prefix # >>> gascan managed ssh include >>>\n",
        "Include ~/.config/gascan/ssh/config\n",
        "# <<< gascan managed ssh include <<<\n",
    )
    .as_bytes();
    write_user_config(&config, original)?;
    assert!(config.install().is_err());
    assert_eq!(fs::read(config.user_config_path())?, original);
    Ok(())
}

#[test]
fn installer_creates_exact_private_modes() -> TestResult {
    let (_temp, config) = fixture()?;
    assert_eq!(config.install()?, IncludeChange::Changed);
    assert_eq!(
        fs::metadata(config.ssh_directory_path())?.mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(config.user_config_path())?.mode() & 0o777,
        0o600
    );
    Ok(())
}

#[test]
fn conventional_user_config_mode_is_accepted_and_preserved() -> TestResult {
    let (_temp, config) = fixture()?;
    write_user_config(&config, b"Host personal\n")?;
    fs::set_permissions(config.user_config_path(), fs::Permissions::from_mode(0o644))?;

    assert_eq!(config.install()?, IncludeChange::Changed);
    assert_eq!(
        fs::metadata(config.user_config_path())?.mode() & 0o777,
        0o644
    );
    assert_eq!(config.remove()?, IncludeChange::Changed);
    assert_eq!(
        fs::metadata(config.user_config_path())?.mode() & 0o777,
        0o644
    );
    Ok(())
}

#[test]
fn conventional_owner_controlled_directory_modes_are_accepted() -> TestResult {
    let (_temp, user_config) = fixture()?;
    fs::create_dir(user_config.ssh_directory_path())?;
    fs::set_permissions(
        user_config.ssh_directory_path(),
        fs::Permissions::from_mode(0o755),
    )?;
    assert_eq!(user_config.install()?, IncludeChange::Changed);

    let (_temp, managed_config) = fixture()?;
    let managed_ssh = managed_config
        .managed_config_path()
        .parent()
        .ok_or("managed SSH directory")?;
    let gascan = managed_ssh.parent().ok_or("managed Gas Can directory")?;
    fs::create_dir_all(managed_ssh)?;
    fs::set_permissions(gascan, fs::Permissions::from_mode(0o755))?;
    fs::set_permissions(managed_ssh, fs::Permissions::from_mode(0o755))?;
    managed_config.record_offer_receipt()?;
    assert!(managed_config.offer_receipt_exists()?);
    Ok(())
}

#[test]
fn installer_rejects_symlink_hard_link_fifo_owner_and_mode_attacks() -> TestResult {
    symlink_attack()?;
    hard_link_attack()?;
    fifo_attack()?;
    unsafe_mode_attack()?;
    Ok(())
}

fn symlink_attack() -> TestResult {
    let (temp, config) = fixture()?;
    let victim = temp.path().join("home/victim");
    fs::write(&victim, b"do not touch")?;
    prepare_ssh_directory(&config)?;
    std::os::unix::fs::symlink(&victim, config.user_config_path())?;
    assert!(config.install().is_err());
    assert_eq!(fs::read(victim)?, b"do not touch");
    Ok(())
}

fn hard_link_attack() -> TestResult {
    let (temp, config) = fixture()?;
    let victim = temp.path().join("home/victim");
    fs::write(&victim, b"do not touch")?;
    fs::set_permissions(&victim, fs::Permissions::from_mode(0o600))?;
    prepare_ssh_directory(&config)?;
    fs::hard_link(&victim, config.user_config_path())?;
    assert!(fs::metadata(&victim)?.nlink() > 1);
    assert!(config.install().is_err());
    assert_eq!(fs::read(victim)?, b"do not touch");
    Ok(())
}

fn fifo_attack() -> TestResult {
    let (_temp, config) = fixture()?;
    prepare_ssh_directory(&config)?;
    let result = std::process::Command::new("/usr/bin/mkfifo")
        .arg(config.user_config_path())
        .status()?;
    assert!(result.success());
    assert!(config.install().is_err());
    Ok(())
}

fn unsafe_mode_attack() -> TestResult {
    let (_temp, config) = fixture()?;
    write_user_config(&config, b"Host personal\n")?;
    fs::set_permissions(config.user_config_path(), fs::Permissions::from_mode(0o664))?;
    assert!(config.install().is_err());
    Ok(())
}

#[test]
fn unsafe_ssh_directory_is_rejected_without_repairing_it() -> TestResult {
    let (_temp, config) = fixture()?;
    fs::create_dir(config.ssh_directory_path())?;
    fs::set_permissions(
        config.ssh_directory_path(),
        fs::Permissions::from_mode(0o775),
    )?;
    assert!(config.install().is_err());
    assert_eq!(
        fs::metadata(config.ssh_directory_path())?.mode() & 0o777,
        0o775
    );
    Ok(())
}

#[test]
fn update_io_and_unsafe_path_failures_have_distinct_stable_codes() -> TestResult {
    let temp = tempfile::tempdir()?;
    let missing_home = temp.path().join("missing-home");
    let missing = SshConfig::for_environment(None, Some(&missing_home))?;
    assert_eq!(
        missing
            .install()
            .expect_err("missing HOME must fail")
            .stable_code(),
        gascan_proto::error_code::SSH_CONFIG_UPDATE_FAILED
    );

    let (_temp, unsafe_config) = fixture()?;
    fs::create_dir(unsafe_config.ssh_directory_path())?;
    fs::set_permissions(
        unsafe_config.ssh_directory_path(),
        fs::Permissions::from_mode(0o775),
    )?;
    assert_eq!(
        unsafe_config
            .install()
            .expect_err("unsafe directory must fail")
            .stable_code(),
        gascan_proto::error_code::SSH_CONFIG_UNSAFE
    );

    let symlink_temp = tempfile::tempdir()?;
    let symlink_home = symlink_temp.path().join("home");
    let symlink_target = symlink_temp.path().join("target");
    fs::create_dir(&symlink_home)?;
    fs::create_dir(&symlink_target)?;
    std::os::unix::fs::symlink(&symlink_target, symlink_home.join(".ssh"))?;
    let symlink_config = SshConfig::for_environment(None, Some(&symlink_home))?;
    assert_eq!(
        symlink_config
            .install()
            .expect_err("symlinked SSH directory must fail")
            .stable_code(),
        gascan_proto::error_code::SSH_CONFIG_UNSAFE
    );
    Ok(())
}

fn prepare_ssh_directory(config: &SshConfig) -> TestResult {
    fs::create_dir(config.ssh_directory_path())?;
    fs::set_permissions(
        config.ssh_directory_path(),
        fs::Permissions::from_mode(0o700),
    )?;
    Ok(())
}

fn write_user_config(config: &SshConfig, contents: &[u8]) -> TestResult {
    prepare_ssh_directory(config)?;
    fs::write(config.user_config_path(), contents)?;
    fs::set_permissions(config.user_config_path(), fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[test]
fn first_use_offer_is_interactive_only_and_receipt_is_recorded_after_an_answer() -> TestResult {
    let (_temp, config) = fixture()?;
    assert!(!config.offer_receipt_exists()?);
    assert!(!first_use_offer(&config, false, true)?);
    assert!(!first_use_offer(&config, true, false)?);
    assert!(!config.offer_receipt_exists()?);
    assert!(first_use_offer(&config, true, true)?);

    config.record_offer_receipt()?;
    assert!(config.offer_receipt_exists()?);
    assert!(!first_use_offer(&config, true, true)?);
    Ok(())
}

#[test]
fn existing_include_suppresses_offer_without_writing_a_receipt() -> TestResult {
    let (_temp, config) = fixture()?;
    config.install()?;
    assert!(!first_use_offer(&config, true, true)?);
    assert!(!config.offer_receipt_exists()?);
    Ok(())
}

#[test]
fn interactive_yes_installs_and_no_defers_then_both_record_the_receipt() -> TestResult {
    let (_yes_temp, yes) = fixture()?;
    assert_eq!(answer_first_use_offer(&yes, "\n")?, OfferAnswer::Installed);
    assert!(yes.contains_include()?);
    assert!(yes.offer_receipt_exists()?);

    let (_no_temp, no) = fixture()?;
    assert_eq!(answer_first_use_offer(&no, "n\n")?, OfferAnswer::Declined);
    assert!(!no.contains_include()?);
    assert!(no.offer_receipt_exists()?);
    Ok(())
}

#[test]
fn ssh_config_path_command_is_local_and_prints_the_absolute_managed_path() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let xdg = temp.path().join("xdg");
    fs::create_dir(&home)?;
    fs::create_dir(&xdg)?;
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_gascan"))
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &xdg)
        .args(["ssh-config", "path"])
        .output()?;
    assert!(output.status.success(), "{output:?}");
    let account_home = gascan_core::account::effective_account_home()?;
    assert_eq!(
        output.stdout,
        format!(
            "{}\n",
            account_home.join(".config/gascan/ssh/config").display()
        )
        .as_bytes()
    );
    assert!(output.stderr.is_empty());
    Ok(())
}

#[test]
fn custom_xdg_install_and_path_resolve_the_same_fixed_include_target() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let xdg = temp.path().join("custom-xdg");
    fs::create_dir(&home)?;
    fs::create_dir(&xdg)?;

    let config = SshConfig::for_environment(Some(&xdg), Some(&home))?;
    assert_eq!(config.install()?, IncludeChange::Changed);
    assert_eq!(
        config.managed_config_path(),
        home.join(".config/gascan/ssh/config")
    );
    assert_eq!(fs::read(home.join(".ssh/config"))?, INCLUDE_BLOCK_LF);
    assert!(!xdg.join("gascan/ssh/config").exists());
    Ok(())
}
