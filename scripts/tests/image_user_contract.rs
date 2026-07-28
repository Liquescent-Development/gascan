#[cfg(target_os = "macos")]
use std::ffi::OsString;
use std::{
    fs,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Command, Output},
};

const RUNTIME_PATH: &str = concat!(
    "/home/workspace/.local/bin:",
    "/home/workspace/.local/share/cargo/bin:",
    "/home/workspace/.local/share/go/bin:",
    "/home/workspace/.local/share/gem/bin:",
    "/home/workspace/.local/share/mise/shims:",
    "/opt/gascan/mise/shims:",
    "/usr/local/sbin:/usr/local/bin:",
    "/opt/gascan/workstation/bin:",
    "/usr/sbin:/usr/bin:/sbin:/bin"
);

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

struct ShellFixture {
    _temporary: tempfile::TempDir,
    home: PathBuf,
    script: PathBuf,
}

impl ShellFixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        fs::create_dir(&home).unwrap();
        let script = temporary.path().join("configure-shell-home");
        fs::copy(
            root().join("images/workspace/bin/configure-shell-home"),
            &script,
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o555)).unwrap();
        fs::create_dir_all(home.join(".config/gascan")).unwrap();
        Self {
            _temporary: temporary,
            home,
            script,
        }
    }

    fn run(&self, prompt: &str) -> Output {
        Command::new(&self.script)
            .arg(prompt)
            .env_clear()
            .env("HOME", &self.home)
            .output()
            .unwrap()
    }
}

fn write_mode(path: &Path, contents: &str, mode: u32) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn hook_fixture(temporary: &tempfile::TempDir) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let shell_dir = temporary.path().join("managed-shell");
    let immutable_root = temporary.path().join("opt/gascan/shell");
    let starship = immutable_root.join("bin/starship");
    let starship_target = temporary.path().join("opt/gascan/workstation/bin/starship");
    let presets = immutable_root.join("presets");
    let log = temporary.path().join("starship.log");
    fs::create_dir(&shell_dir).unwrap();
    fs::create_dir_all(starship.parent().unwrap()).unwrap();
    fs::create_dir_all(starship_target.parent().unwrap()).unwrap();
    fs::create_dir(&presets).unwrap();
    symlink("../../workstation/bin/starship", &starship).unwrap();
    for name in ["starship.toml", "starship-nerd-font.toml"] {
        write_mode(&presets.join(name), "format = \"$character\"\n", 0o444);
    }
    let source = fs::read_to_string(root().join("images/workspace/etc/gascan/bashrc")).unwrap();
    let fixture_uid = fs::metadata(temporary.path()).unwrap().uid();
    let fixture_gid = fs::metadata(temporary.path()).unwrap().gid();
    let fixture_link_mode = fs::symlink_metadata(&starship)
        .unwrap()
        .permissions()
        .mode()
        & 0o7777;
    let rebound = source
        .replace(
            "/home/workspace/.config/gascan/shell",
            shell_dir.to_str().unwrap(),
        )
        .replace(
            "/opt/gascan/workstation/bin/starship",
            starship_target.to_str().unwrap(),
        )
        .replace("/opt/gascan/shell/bin/starship", starship.to_str().unwrap())
        .replace("/opt/gascan/shell/presets", presets.to_str().unwrap())
        .replace("managed_owner=0", &format!("managed_owner={fixture_uid}"))
        .replace(
            "workspace_uid=1000",
            &format!("workspace_uid={fixture_uid}"),
        )
        .replace(
            "workspace_gid=1000",
            &format!("workspace_gid={fixture_gid}"),
        )
        .replace(
            "immutable_owner=0",
            &format!("immutable_owner={fixture_uid}"),
        )
        .replace("immutable_gid=0", &format!("immutable_gid={fixture_gid}"))
        .replace(
            "immutable_link_mode=777",
            &format!("immutable_link_mode={fixture_link_mode:o}"),
        );
    assert_ne!(
        source, rebound,
        "fixture did not rebind managed shell paths"
    );
    let hook = temporary.path().join("gascan-bashrc");
    fs::write(&hook, rebound).unwrap();
    fs::write(
        &starship_target,
        "#!/bin/sh\n\
         printf '%s|%s|%s\\n' \"$*\" \"${STARSHIP_CONFIG-}\" \"${STARSHIP_EXECUTABLE-}\" >>\"$GASCAN_TEST_LOG\"\n\
         if [ \"$*\" = 'init bash' ]; then\n\
           printf '%s\\n' 'eval \"$(starship init bash --print-full-init)\"'\n\
         elif [ \"$*\" = 'init bash --print-full-init' ]; then\n\
           test \"${GASCAN_TEST_GENERATION_FAIL-0}\" != 1 || exit 23\n\
           if [ \"${GASCAN_TEST_EVAL_FAIL-0}\" = 1 ]; then\n\
             printf '%s\\n' \
               'PS1=broken' \
               'PS2=broken-continuation' \
               'PROMPT_COMMAND=broken-prompt-command' \
               'STARSHIP_PARTIAL=leaked-variable' \
               'starship_partial_leak() { printf leaked; }' \
               \"trap 'STARSHIP_TRAP_LEAK=1' DEBUG\" \
               'false' \
               'true'\n\
           else\n\
             if [ \"${GASCAN_TEST_SIGNAL_COLLISION-0}\" = 1 ]; then\n\
               printf '%s\\n' 'kill -USR1 \"$GASCAN_TEST_PARENT_PID\"'\n\
             fi\n\
             printf '%s\\n' \
               '_starship_set_return() { return \"${1:-0}\"; }' \
               'starship_preexec() { :; }' \
               'starship_preexec_all() { :; }' \
               'starship_preexec_ps0() { :; }' \
               'starship_precmd() { if [ -n \"${STARSHIP_PROMPT_COMMAND-}\" ]; then eval \"$STARSHIP_PROMPT_COMMAND\"; fi; \"$STARSHIP_EXECUTABLE\" prompt; }' \
               'STARSHIP_START_TIME=1' \
               'STARSHIP_SHELL=bash' \
               'STARSHIP_SESSION_KEY=1234567890123456' \
               'PS0=managed-ps0' \
               'PS1=managed-starship' \
               'PS2=managed-continuation' \
               'if [ -n \"${PROMPT_COMMAND-}\" ] && [[ \"$PROMPT_COMMAND\" != *\"starship_precmd\"* ]]; then STARSHIP_PROMPT_COMMAND=$PROMPT_COMMAND; fi' \
               'PROMPT_COMMAND=starship_precmd' \
               'shopt -s checkwinsize' \
               \"trap ':' DEBUG\"\n\
           fi\n\
         elif [ \"$*\" = prompt ]; then\n\
           printf '%s\\n' stable-runtime-prompt\n\
         else\n\
           exit 24\n\
         fi\n",
    )
    .unwrap();
    fs::set_permissions(&starship_target, fs::Permissions::from_mode(0o555)).unwrap();
    (hook, shell_dir, starship, log)
}

fn run_hook(
    hook: &Path,
    command: &str,
    log: &Path,
    generation_fail: bool,
    eval_fail: bool,
    interactive: bool,
) -> Output {
    let mut process = Command::new("/bin/bash");
    process.args(["--noprofile", "--norc"]);
    if interactive {
        process.arg("-i");
    }
    process
        .args(["-c", command])
        .env("GASCAN_TEST_LOG", log)
        .env(
            "GASCAN_TEST_GENERATION_FAIL",
            if generation_fail { "1" } else { "0" },
        )
        .env("GASCAN_TEST_EVAL_FAIL", if eval_fail { "1" } else { "0" })
        .env("GASCAN_TEST_HOOK", hook)
        .env_remove("STARSHIP_SHELL")
        .env_remove("STARSHIP_SESSION_KEY")
        .output()
        .unwrap()
}

fn rust_seed_command(
    script: &Path,
    source: &Path,
    destination: &Path,
    test_root: &Path,
) -> Command {
    let mut command = Command::new(script);
    command
        .arg(source)
        .arg(destination)
        .arg(source.join("cargo-bin"))
        .arg(destination.join("cargo-bin"));
    let test_bin = test_root.join("test-bin");
    fs::create_dir_all(&test_bin).unwrap();
    let mut path = vec![test_bin];
    #[cfg(target_os = "macos")]
    {
        let bin = test_root.join("gnu-bin");
        fs::create_dir_all(&bin).unwrap();
        let mv = bin.join("mv");
        if !mv.exists() {
            let gmv = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|directory| directory.join("gmv"))
                .find(|candidate| candidate.is_file())
                .expect("GNU mv is required to exercise Linux publication semantics on macOS");
            symlink(gmv, &mv).unwrap();
        }
        path.push(bin);
    }
    path.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    command.env(
        "PATH",
        std::env::join_paths(path).unwrap_or(OsString::new()),
    );
    command
}

fn gnu_mv_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        return std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|directory| directory.join("gmv"))
            .find(|candidate| candidate.is_file())
            .expect("GNU mv is required to exercise Linux publication semantics on macOS");
    }
    #[cfg(not(target_os = "macos"))]
    {
        Path::new("/bin/mv").to_path_buf()
    }
}

const RUST_PROXIES: [&str; 14] = [
    "cargo",
    "cargo-clippy",
    "cargo-fmt",
    "cargo-miri",
    "clippy-driver",
    "rls",
    "rust-analyzer",
    "rust-gdb",
    "rust-gdbgui",
    "rust-lldb",
    "rustc",
    "rustdoc",
    "rustfmt",
    "rustup",
];

fn write_test_rust_proxies(source_root: &Path) {
    let bin = source_root.join("cargo-bin");
    fs::create_dir_all(&bin).unwrap();
    let rustup = bin.join("rustup");
    if !rustup.exists() {
        fs::write(&rustup, "#!/bin/sh\nprintf 'rustup proxy\\n'\n").unwrap();
        fs::set_permissions(&rustup, fs::Permissions::from_mode(0o555)).unwrap();
    }
    for proxy in RUST_PROXIES {
        if proxy == "rustup" {
            continue;
        }
        let path = bin.join(proxy);
        if !path.symlink_metadata().is_ok() {
            symlink("rustup", path).unwrap();
        }
    }
}

fn write_test_toolchain(source_root: &Path, name: &str, cargo: &str) {
    write_test_rust_proxies(source_root);
    let bin = source_root.join("toolchains").join(name).join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("cargo"), cargo).unwrap();
    fs::write(bin.join("rustc"), format!("rustc for {name}\n")).unwrap();
    for command in ["cargo", "rustc"] {
        fs::set_permissions(bin.join(command), fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::create_dir_all(source_root.join("update-hashes")).unwrap();
    fs::write(
        source_root.join("update-hashes").join(name),
        format!("hash for {name}\n"),
    )
    .unwrap();
    let settings = source_root.join("settings.toml");
    if !settings.exists() {
        fs::write(
            &settings,
            format!(
                "version = \"12\"\ndefault_toolchain = \"{name}\"\nprofile = \"default\"\n\n[overrides]\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&settings, fs::Permissions::from_mode(0o444)).unwrap();
    }
}

fn rust_staging_residue(destination_root: &Path) -> Vec<String> {
    if !destination_root.is_dir() {
        return Vec::new();
    }
    fs::read_dir(destination_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with(".gascan-rust-"))
        .collect()
}

fn rust_proxy_staging_residue(destination_root: &Path) -> Vec<String> {
    let bin = destination_root.join("cargo-bin");
    if !bin.is_dir() {
        return Vec::new();
    }
    fs::read_dir(bin)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with(".gascan-rust-proxy."))
        .collect()
}

fn rust_settings_staging_residue(destination_root: &Path) -> Vec<String> {
    if !destination_root.is_dir() {
        return Vec::new();
    }
    fs::read_dir(destination_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with(".gascan-rust-settings."))
        .collect()
}

fn assert_rejected_update_hash_collision(case: &str) {
    let script = root().join("images/workspace/bin/initialize-rust-home");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join(format!("destination-{case}"));
    let bundled = "1.97.0-aarch64-unknown-linux-gnu";
    write_test_toolchain(&source, bundled, "bundled cargo\n");
    assert!(
        rust_seed_command(&script, &source, &destination, temp.path())
            .status()
            .unwrap()
            .success()
    );
    let collision = destination.join("update-hashes").join(bundled);
    fs::remove_file(&collision).unwrap();
    let outside = temp.path().join(format!("outside-{case}"));
    match case {
        "symlink" => {
            fs::write(&outside, "outside survives\n").unwrap();
            symlink(&outside, &collision).unwrap();
        }
        "directory" => {
            fs::create_dir(&collision).unwrap();
            fs::write(collision.join("sentinel"), "directory survives\n").unwrap();
        }
        "fifo" => {
            assert!(
                Command::new("mkfifo")
                    .arg(&collision)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        _ => unreachable!(),
    }
    let inode = fs::symlink_metadata(&collision).unwrap().ino();

    let rejected = rust_seed_command(&script, &source, &destination, temp.path())
        .status()
        .unwrap();
    assert!(
        !rejected.success(),
        "accepted {case} update-hash destination"
    );
    let metadata = fs::symlink_metadata(&collision).unwrap();
    assert_eq!(metadata.ino(), inode, "mutated {case} collision");
    match case {
        "symlink" => {
            assert_eq!(fs::read_link(&collision).unwrap(), outside);
            assert_eq!(fs::read_to_string(&outside).unwrap(), "outside survives\n");
        }
        "directory" => {
            assert_eq!(
                fs::read_to_string(collision.join("sentinel")).unwrap(),
                "directory survives\n"
            );
        }
        "fifo" => assert!(metadata.file_type().is_fifo()),
        _ => unreachable!(),
    }
    assert!(rust_staging_residue(&destination).is_empty());
}

#[test]
fn writable_rust_bootstrap_rejects_symlink_update_hash_collision() {
    assert_rejected_update_hash_collision("symlink");
}

#[test]
fn writable_rust_bootstrap_rejects_directory_update_hash_collision() {
    assert_rejected_update_hash_collision("directory");
}

#[test]
fn writable_rust_bootstrap_rejects_fifo_update_hash_collision() {
    assert_rejected_update_hash_collision("fifo");
}

#[test]
fn writable_rust_bootstrap_preserves_regular_update_hash_collision() {
    let script = root().join("images/workspace/bin/initialize-rust-home");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    let bundled = "1.97.0-aarch64-unknown-linux-gnu";
    write_test_toolchain(&source, bundled, "bundled cargo\n");
    assert!(
        rust_seed_command(&script, &source, &destination, temp.path())
            .status()
            .unwrap()
            .success()
    );
    let collision = destination.join("update-hashes").join(bundled);
    fs::remove_file(&collision).unwrap();
    fs::write(&collision, "user-owned hash\n").unwrap();
    fs::set_permissions(&collision, fs::Permissions::from_mode(0o600)).unwrap();
    let inode = fs::metadata(&collision).unwrap().ino();

    assert!(
        rust_seed_command(&script, &source, &destination, temp.path())
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(fs::metadata(&collision).unwrap().ino(), inode);
    assert_eq!(fs::read_to_string(&collision).unwrap(), "user-owned hash\n");
    assert!(rust_staging_residue(&destination).is_empty());
}

#[test]
fn writable_rust_bootstrap_is_idempotent_fail_closed_and_never_mutates_source() {
    let script = root().join("images/workspace/bin/initialize-rust-home");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    let bundled = "1.97.0-aarch64-unknown-linux-gnu";
    write_test_toolchain(&source, bundled, "bundled cargo\n");

    let source_cargo = source.join("toolchains").join(bundled).join("bin/cargo");
    let source_inode = fs::metadata(&source_cargo).unwrap().ino();
    let source_contents = fs::read(&source_cargo).unwrap();
    let user_toolchain = destination.join("toolchains/user-installed");
    fs::create_dir_all(&user_toolchain).unwrap();
    fs::write(user_toolchain.join("sentinel"), "keep me\n").unwrap();
    let user_cargo = destination.join("cargo-bin/cargo");
    fs::create_dir_all(user_cargo.parent().unwrap()).unwrap();
    fs::write(&user_cargo, "user cargo\n").unwrap();
    fs::set_permissions(&user_cargo, fs::Permissions::from_mode(0o700)).unwrap();
    let user_cargo_inode = fs::metadata(&user_cargo).unwrap().ino();
    let source_rustup = source.join("cargo-bin/rustup");
    let source_rustup_inode = fs::metadata(&source_rustup).unwrap().ino();
    let source_rustup_contents = fs::read(&source_rustup).unwrap();
    let source_settings = source.join("settings.toml");
    let source_settings_inode = fs::metadata(&source_settings).unwrap().ino();
    let source_settings_contents = fs::read(&source_settings).unwrap();
    let immutable_data_directory = source
        .join("toolchains")
        .join(bundled)
        .join("lib/rustlib/src");
    fs::create_dir_all(&immutable_data_directory).unwrap();
    let immutable_data = immutable_data_directory.join("library.txt");
    fs::write(&immutable_data, "immutable library source\n").unwrap();
    fs::set_permissions(&immutable_data, fs::Permissions::from_mode(0o444)).unwrap();
    fs::set_permissions(&immutable_data_directory, fs::Permissions::from_mode(0o555)).unwrap();
    let immutable_data_inode = fs::metadata(&immutable_data).unwrap().ino();
    let immutable_data_contents = fs::read(&immutable_data).unwrap();
    let outside_target = temp.path().join("outside-toolchain-target");
    fs::write(&outside_target, "outside target\n").unwrap();
    fs::set_permissions(&outside_target, fs::Permissions::from_mode(0o444)).unwrap();
    let outside_mode = fs::metadata(&outside_target).unwrap().permissions().mode() & 0o777;
    symlink(
        &outside_target,
        source.join("toolchains").join(bundled).join("outside-link"),
    )
    .unwrap();

    let run = || rust_seed_command(&script, &source, &destination, temp.path()).status();
    assert!(run().unwrap().success());
    let published = destination
        .join("toolchains")
        .join(bundled)
        .join("bin/cargo");
    let first_inode = fs::metadata(&published).unwrap().ino();
    let first_contents = fs::read(&published).unwrap();
    assert_eq!(first_contents, b"bundled cargo\n");
    let copied_data_directory = destination
        .join("toolchains")
        .join(bundled)
        .join("lib/rustlib/src");
    let copied_data = copied_data_directory.join("library.txt");
    assert_eq!(
        fs::metadata(&copied_data_directory)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&copied_data).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&published).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::read_link(
            destination
                .join("toolchains")
                .join(bundled)
                .join("outside-link")
        )
        .unwrap(),
        outside_target
    );
    assert_eq!(
        fs::metadata(&outside_target).unwrap().permissions().mode() & 0o777,
        outside_mode,
        "stage normalization followed an internal symlink"
    );
    assert_eq!(
        fs::metadata(&published).unwrap().uid(),
        fs::metadata(&destination).unwrap().uid(),
        "published files must belong to the invoking user"
    );
    assert_eq!(
        fs::read_to_string(destination.join("update-hashes").join(bundled)).unwrap(),
        "hash for 1.97.0-aarch64-unknown-linux-gnu\n"
    );
    let marker = destination.join(".gascan-bundled-toolchains-v1");
    let marker_inode = fs::metadata(&marker).unwrap().ino();
    let marker_contents = fs::read(&marker).unwrap();
    let published_rustup = destination.join("cargo-bin/rustup");
    assert!(published_rustup.is_file());
    assert!(
        !published_rustup
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::metadata(&published_rustup)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(fs::read(&published_rustup).unwrap(), source_rustup_contents);
    for proxy in RUST_PROXIES {
        let published = destination.join("cargo-bin").join(proxy);
        if proxy == "rustup" || proxy == "cargo" {
            continue;
        }
        assert!(
            published
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink(),
            "{proxy} was not published as an exact rustup proxy"
        );
        assert_eq!(fs::read_link(&published).unwrap(), Path::new("rustup"));
    }
    assert_eq!(fs::metadata(&user_cargo).unwrap().ino(), user_cargo_inode);
    assert_eq!(fs::read(&user_cargo).unwrap(), b"user cargo\n");
    let rustup_inode = fs::metadata(&published_rustup).unwrap().ino();
    let published_settings = destination.join("settings.toml");
    let expected_settings = "version = \"12\"\ndefault_toolchain = \"1.97.0-aarch64-unknown-linux-gnu\"\nprofile = \"default\"\n\n[overrides]\n";
    assert_eq!(
        fs::read_to_string(&published_settings).unwrap(),
        expected_settings
    );
    assert_eq!(
        fs::metadata(&published_settings)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let settings_inode = fs::metadata(&published_settings).unwrap().ino();
    let rustc_inode = published_rustup
        .parent()
        .unwrap()
        .join("rustc")
        .symlink_metadata()
        .unwrap()
        .ino();

    assert!(run().unwrap().success());
    assert_eq!(fs::metadata(&published).unwrap().ino(), first_inode);
    assert_eq!(fs::read(&published).unwrap(), first_contents);
    assert_eq!(fs::metadata(&marker).unwrap().ino(), marker_inode);
    assert_eq!(fs::read(&marker).unwrap(), marker_contents);
    assert_eq!(fs::metadata(&published_rustup).unwrap().ino(), rustup_inode);
    assert_eq!(
        fs::metadata(&published_settings).unwrap().ino(),
        settings_inode
    );
    assert_eq!(
        published_rustup
            .parent()
            .unwrap()
            .join("rustc")
            .symlink_metadata()
            .unwrap()
            .ino(),
        rustc_inode
    );
    assert_eq!(fs::metadata(&user_cargo).unwrap().ino(), user_cargo_inode);
    assert_eq!(fs::read(&user_cargo).unwrap(), b"user cargo\n");
    assert_eq!(
        fs::read_to_string(user_toolchain.join("sentinel")).unwrap(),
        "keep me\n"
    );

    let additional = "1.98.0-aarch64-unknown-linux-gnu";
    write_test_toolchain(&source, additional, "new cargo\n");
    assert!(run().unwrap().success());
    assert_eq!(fs::metadata(&published).unwrap().ino(), first_inode);
    assert_eq!(fs::read(&published).unwrap(), first_contents);
    assert_eq!(
        fs::read_to_string(
            destination
                .join("toolchains")
                .join(additional)
                .join("bin/cargo")
        )
        .unwrap(),
        "new cargo\n"
    );
    assert_eq!(fs::metadata(&marker).unwrap().ino(), marker_inode);
    assert_eq!(
        fs::read_to_string(&marker).unwrap(),
        "1.97.0-aarch64-unknown-linux-gnu\n"
    );
    assert_eq!(
        fs::metadata(&marker).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(fs::metadata(&source_cargo).unwrap().ino(), source_inode);
    assert_eq!(fs::read(&source_cargo).unwrap(), source_contents);
    assert_eq!(
        fs::metadata(&immutable_data).unwrap().ino(),
        immutable_data_inode
    );
    assert_eq!(fs::read(&immutable_data).unwrap(), immutable_data_contents);
    assert_eq!(
        fs::metadata(&immutable_data).unwrap().permissions().mode() & 0o777,
        0o444
    );
    assert_eq!(
        fs::metadata(&immutable_data_directory)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o555
    );
    assert_eq!(
        fs::metadata(&source_rustup).unwrap().ino(),
        source_rustup_inode
    );
    assert_eq!(fs::read(&source_rustup).unwrap(), source_rustup_contents);
    assert_eq!(
        fs::metadata(&source_settings).unwrap().ino(),
        source_settings_inode
    );
    assert_eq!(
        fs::read(&source_settings).unwrap(),
        source_settings_contents
    );
    assert_eq!(
        fs::metadata(&published_settings).unwrap().ino(),
        settings_inode
    );
    assert_eq!(
        fs::read_to_string(&published_settings).unwrap(),
        expected_settings,
        "adding a future bundled toolchain changed the immutable-source default"
    );
    assert!(rust_proxy_staging_residue(&destination).is_empty());
    assert!(rust_settings_staging_residue(&destination).is_empty());
}

#[test]
fn writable_rust_bootstrap_rerun_does_not_move_over_an_existing_marker() {
    let script = root().join("images/workspace/bin/initialize-rust-home");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    write_test_toolchain(
        &source,
        "1.97.0-aarch64-unknown-linux-gnu",
        "bundled cargo\n",
    );
    assert!(
        rust_seed_command(&script, &source, &destination, temp.path())
            .status()
            .unwrap()
            .success()
    );

    let marker = destination.join(".gascan-bundled-toolchains-v1");
    let marker_inode = fs::metadata(&marker).unwrap().ino();
    let marker_contents = fs::read(&marker).unwrap();
    let fake_mv = temp.path().join("test-bin/mv");
    fs::write(
        &fake_mv,
        "#!/bin/sh\nlast=\nfor argument in \"$@\"; do last=$argument; done\ncase \"$last\" in */.gascan-bundled-toolchains-v1) printf 'existing marker move attempted\\n' >&2; exit 91 ;; esac\nexec \"$GASCAN_TEST_REAL_MV\" \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&fake_mv, fs::Permissions::from_mode(0o700)).unwrap();

    let rerun = rust_seed_command(&script, &source, &destination, temp.path())
        .env("GASCAN_TEST_REAL_MV", gnu_mv_path())
        .status()
        .unwrap();
    assert!(
        rerun.success(),
        "rerun attempted Linux no-clobber marker move"
    );
    assert_eq!(fs::metadata(&marker).unwrap().ino(), marker_inode);
    assert_eq!(fs::read(&marker).unwrap(), marker_contents);
    assert!(rust_staging_residue(&destination).is_empty());
}

#[test]
fn writable_rust_bootstrap_revalidates_a_marker_publication_race() {
    for collision in ["safe-subset", "wrong-content"] {
        let script = root().join("images/workspace/bin/initialize-rust-home");
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        write_test_toolchain(
            &source,
            "1.97.0-aarch64-unknown-linux-gnu",
            "bundled cargo\n",
        );
        write_test_toolchain(
            &source,
            "1.98.0-aarch64-unknown-linux-gnu",
            "future bundled cargo\n",
        );
        let fake_mv = temp.path().join("test-bin/mv");
        fs::create_dir_all(fake_mv.parent().unwrap()).unwrap();
        fs::write(
            &fake_mv,
            format!(
                "#!/bin/sh\nprevious=\nlast=\nfor argument in \"$@\"; do previous=$last; last=$argument; done\ncase \"$last\" in\n  */.gascan-bundled-toolchains-v1)\n    case {collision} in\n      safe-subset) printf '1.97.0-aarch64-unknown-linux-gnu\\n' >\"$last\" ;;\n      wrong-content) printf 'attacker-controlled\\n' >\"$last\" ;;\n    esac\n    chmod 0600 \"$last\"\n    printf 'mv: not replacing existing marker\\n' >&2\n    exit 1\n    ;;\nesac\nexec \"$GASCAN_TEST_REAL_MV\" \"$@\"\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&fake_mv, fs::Permissions::from_mode(0o700)).unwrap();

        let result = rust_seed_command(&script, &source, &destination, temp.path())
            .env("GASCAN_TEST_REAL_MV", gnu_mv_path())
            .status()
            .unwrap();
        assert_eq!(
            result.success(),
            collision == "safe-subset",
            "unexpected marker race result for {collision}"
        );
        assert!(rust_staging_residue(&destination).is_empty());
    }
}

#[test]
fn writable_rust_bootstrap_recovers_every_valid_gnu_no_clobber_race() {
    let script = root().join("images/workspace/bin/initialize-rust-home");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    write_test_toolchain(
        &source,
        "1.97.0-aarch64-unknown-linux-gnu",
        "bundled cargo\n",
    );
    let fake_mv = temp.path().join("test-bin/mv");
    fs::create_dir_all(fake_mv.parent().unwrap()).unwrap();
    fs::write(
        &fake_mv,
        "#!/bin/sh\nprevious=\nlast=\nfor argument in \"$@\"; do previous=$last; last=$argument; done\nkind=\ncase \"$last\" in\n  */toolchains/*) kind=toolchain; cp -R \"$previous\" \"$last\" ;;\n  */update-hashes/*) kind=hash; cp \"$previous\" \"$last\" ;;\n  */cargo-bin/rustup) kind=rustup; cp \"$previous\" \"$last\"; chmod 0700 \"$last\" ;;\n  */cargo-bin/*) kind=proxy; ln -s \"$(readlink \"$previous\")\" \"$last\" ;;\n  */settings.toml) kind=settings; cp \"$previous\" \"$last\" ;;\n  */.gascan-bundled-toolchains-v1) kind=marker; cp \"$previous\" \"$last\" ;;\nesac\nif [ -n \"$kind\" ]; then printf '%s\\n' \"$kind\" >>\"$GASCAN_TEST_COLLISIONS\"; exit 1; fi\nexec \"$GASCAN_TEST_REAL_MV\" \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&fake_mv, fs::Permissions::from_mode(0o700)).unwrap();
    let collisions = temp.path().join("collisions");

    let result = rust_seed_command(&script, &source, &destination, temp.path())
        .env("GASCAN_TEST_REAL_MV", gnu_mv_path())
        .env("GASCAN_TEST_COLLISIONS", &collisions)
        .status()
        .unwrap();
    assert!(result.success());
    let collision_kinds = fs::read_to_string(collisions).unwrap();
    for kind in ["toolchain", "hash", "rustup", "proxy", "settings", "marker"] {
        assert!(
            collision_kinds.lines().any(|actual| actual == kind),
            "missing GNU no-clobber race: {kind}"
        );
    }
    assert!(rust_staging_residue(&destination).is_empty());
    assert!(rust_proxy_staging_residue(&destination).is_empty());
    assert!(rust_settings_staging_residue(&destination).is_empty());
}

#[test]
fn writable_rust_bootstrap_rejects_every_unsafe_gnu_no_clobber_race() {
    for unsafe_kind in ["toolchain", "hash", "rustup", "proxy", "settings", "marker"] {
        let script = root().join("images/workspace/bin/initialize-rust-home");
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        write_test_toolchain(
            &source,
            "1.97.0-aarch64-unknown-linux-gnu",
            "bundled cargo\n",
        );
        let fake_mv = temp.path().join("test-bin/mv");
        fs::create_dir_all(fake_mv.parent().unwrap()).unwrap();
        fs::write(
            &fake_mv,
            "#!/bin/sh\nprevious=\nlast=\nfor argument in \"$@\"; do previous=$last; last=$argument; done\nkind=\ncase \"$last\" in\n  */toolchains/*) kind=toolchain ;;\n  */update-hashes/*) kind=hash ;;\n  */cargo-bin/rustup) kind=rustup ;;\n  */cargo-bin/*) kind=proxy ;;\n  */settings.toml) kind=settings ;;\n  */.gascan-bundled-toolchains-v1) kind=marker ;;\nesac\nif [ \"$kind\" = \"$GASCAN_TEST_UNSAFE_KIND\" ]; then\n  case \"$kind\" in\n    toolchain|settings) mkdir \"$last\" ;;\n    hash) printf 'wrong hash\\n' >\"$last\" ;;\n    rustup) printf '#!/bin/sh\\nexit 99\\n' >\"$last\"; chmod 0700 \"$last\" ;;\n    proxy) ln -s wrong-target \"$last\" ;;\n    marker) printf 'unknown-toolchain\\n' >\"$last\"; chmod 0600 \"$last\" ;;\n  esac\n  exit 1\nfi\nexec \"$GASCAN_TEST_REAL_MV\" \"$@\"\n",
        )
        .unwrap();
        fs::set_permissions(&fake_mv, fs::Permissions::from_mode(0o700)).unwrap();

        let result = rust_seed_command(&script, &source, &destination, temp.path())
            .env("GASCAN_TEST_REAL_MV", gnu_mv_path())
            .env("GASCAN_TEST_UNSAFE_KIND", unsafe_kind)
            .status()
            .unwrap();
        assert!(!result.success(), "accepted unsafe {unsafe_kind} race");
        assert!(rust_staging_residue(&destination).is_empty());
        assert!(rust_proxy_staging_residue(&destination).is_empty());
        assert!(rust_settings_staging_residue(&destination).is_empty());
    }
}

#[test]
fn writable_rust_bootstrap_rejects_an_unsafe_existing_marker() {
    for case in [
        "unknown",
        "unsorted",
        "duplicate",
        "writable-mode",
        "symlink",
    ] {
        let script = root().join("images/workspace/bin/initialize-rust-home");
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join(format!("destination-{case}"));
        let bundled = "1.97.0-aarch64-unknown-linux-gnu";
        write_test_toolchain(&source, bundled, "bundled cargo\n");
        if case == "unsorted" {
            write_test_toolchain(&source, "0.0.1-older", "older cargo\n");
        }
        fs::create_dir_all(&destination).unwrap();
        let marker = destination.join(".gascan-bundled-toolchains-v1");
        match case {
            "unknown" => {
                fs::write(&marker, "9.99.0-unknown\n").unwrap();
                fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
            }
            "unsorted" => {
                fs::write(&marker, format!("{bundled}\n0.0.1-older\n")).unwrap();
                fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
            }
            "duplicate" => {
                fs::write(&marker, format!("{bundled}\n{bundled}\n")).unwrap();
                fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
            }
            "writable-mode" => {
                fs::write(&marker, format!("{bundled}\n")).unwrap();
                fs::set_permissions(&marker, fs::Permissions::from_mode(0o644)).unwrap();
            }
            "symlink" => symlink(source.join("settings.toml"), &marker).unwrap(),
            _ => unreachable!(),
        }
        let inode = fs::symlink_metadata(&marker).unwrap().ino();

        let rejected = rust_seed_command(&script, &source, &destination, temp.path())
            .status()
            .unwrap();
        assert!(!rejected.success(), "accepted unsafe marker: {case}");
        assert_eq!(fs::symlink_metadata(&marker).unwrap().ino(), inode);
    }
}

#[test]
fn writable_rust_bootstrap_rejects_unsafe_source_root_components() {
    let script = root().join("images/workspace/bin/initialize-rust-home");
    let temp = tempfile::tempdir().unwrap();
    let valid_source = temp.path().join("valid-source");
    write_test_toolchain(&valid_source, "1.97.0-aarch64-unknown-linux-gnu", "cargo\n");

    let source_link = temp.path().join("source-link");
    symlink(&valid_source, &source_link).unwrap();
    let rejected = rust_seed_command(
        &script,
        &source_link,
        &temp.path().join("source-link-destination"),
        temp.path(),
    )
    .status()
    .unwrap();
    assert!(!rejected.success(), "accepted symlink source root");

    let source_file = temp.path().join("source-file");
    fs::write(&source_file, "not a directory").unwrap();
    let rejected = rust_seed_command(
        &script,
        &source_file,
        &temp.path().join("source-file-destination"),
        temp.path(),
    )
    .status()
    .unwrap();
    assert!(!rejected.success(), "accepted non-directory source root");

    for collision in ["symlink", "file"] {
        let unsafe_source = temp.path().join(format!("update-hashes-{collision}"));
        fs::create_dir_all(unsafe_source.join("toolchains")).unwrap();
        fs::create_dir_all(
            unsafe_source
                .join("toolchains")
                .join("1.97.0-aarch64-unknown-linux-gnu")
                .join("bin"),
        )
        .unwrap();
        for command in ["cargo", "rustc"] {
            let path = unsafe_source
                .join("toolchains")
                .join("1.97.0-aarch64-unknown-linux-gnu")
                .join("bin")
                .join(command);
            fs::write(&path, command).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let update_hashes = unsafe_source.join("update-hashes");
        if collision == "symlink" {
            symlink(valid_source.join("update-hashes"), &update_hashes).unwrap();
        } else {
            fs::write(&update_hashes, "not a directory").unwrap();
        }
        let rejected = rust_seed_command(
            &script,
            &unsafe_source,
            &temp
                .path()
                .join(format!("update-hashes-{collision}-destination")),
            temp.path(),
        )
        .status()
        .unwrap();
        assert!(
            !rejected.success(),
            "accepted {collision} update-hashes root"
        );
    }
}

#[test]
fn writable_rust_bootstrap_requires_the_exact_reviewed_proxy_source_layout() {
    let script = root().join("images/workspace/bin/initialize-rust-home");
    let temp = tempfile::tempdir().unwrap();
    let bundled = "1.97.0-aarch64-unknown-linux-gnu";

    for case in [
        "cargo-bin-symlink",
        "cargo-bin-file",
        "rustup-symlink",
        "missing-proxy",
        "regular-proxy",
        "alternate-target",
        "unexpected-entry",
    ] {
        let source = temp.path().join(format!("source-{case}"));
        let destination = temp.path().join(format!("destination-{case}"));
        write_test_toolchain(&source, bundled, "cargo\n");
        let cargo_bin = source.join("cargo-bin");
        match case {
            "cargo-bin-symlink" => {
                fs::remove_dir_all(&cargo_bin).unwrap();
                symlink(
                    temp.path().join("source-missing-proxy/cargo-bin"),
                    &cargo_bin,
                )
                .unwrap();
            }
            "cargo-bin-file" => {
                fs::remove_dir_all(&cargo_bin).unwrap();
                fs::write(&cargo_bin, "not a directory\n").unwrap();
            }
            "rustup-symlink" => {
                fs::remove_file(cargo_bin.join("rustup")).unwrap();
                symlink("/bin/true", cargo_bin.join("rustup")).unwrap();
            }
            "missing-proxy" => fs::remove_file(cargo_bin.join("rustc")).unwrap(),
            "regular-proxy" => {
                fs::remove_file(cargo_bin.join("rustc")).unwrap();
                fs::write(cargo_bin.join("rustc"), "not the reviewed symlink\n").unwrap();
                fs::set_permissions(cargo_bin.join("rustc"), fs::Permissions::from_mode(0o755))
                    .unwrap();
            }
            "alternate-target" => {
                fs::remove_file(cargo_bin.join("rustc")).unwrap();
                symlink("./rustup", cargo_bin.join("rustc")).unwrap();
            }
            "unexpected-entry" => fs::write(cargo_bin.join("rustup-init"), "unexpected\n").unwrap(),
            _ => unreachable!(),
        }
        let rejected = rust_seed_command(&script, &source, &destination, temp.path())
            .status()
            .unwrap();
        assert!(!rejected.success(), "accepted unsafe proxy source: {case}");
        assert!(
            rust_proxy_staging_residue(&destination).is_empty(),
            "left proxy staging residue for {case}"
        );
    }
}

#[test]
fn writable_rust_bootstrap_rejects_unsafe_proxy_destinations_without_overwriting_them() {
    let script = root().join("images/workspace/bin/initialize-rust-home");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    write_test_toolchain(&source, "1.97.0-aarch64-unknown-linux-gnu", "cargo\n");

    for case in [
        "cargo-bin-symlink",
        "cargo-bin-file",
        "rustup-symlink",
        "proxy-alternate-target",
        "proxy-directory",
        "proxy-nonexecutable-file",
    ] {
        let destination = temp.path().join(format!("destination-{case}"));
        fs::create_dir_all(&destination).unwrap();
        let cargo_bin = destination.join("cargo-bin");
        match case {
            "cargo-bin-symlink" => symlink(temp.path(), &cargo_bin).unwrap(),
            "cargo-bin-file" => fs::write(&cargo_bin, "not a directory\n").unwrap(),
            _ => {
                fs::create_dir(&cargo_bin).unwrap();
                match case {
                    "rustup-symlink" => symlink("/bin/true", cargo_bin.join("rustup")).unwrap(),
                    "proxy-alternate-target" => {
                        symlink("./rustup", cargo_bin.join("rustc")).unwrap()
                    }
                    "proxy-directory" => fs::create_dir(cargo_bin.join("rustc")).unwrap(),
                    "proxy-nonexecutable-file" => {
                        fs::write(cargo_bin.join("rustc"), "user rustc\n").unwrap();
                        fs::set_permissions(
                            cargo_bin.join("rustc"),
                            fs::Permissions::from_mode(0o600),
                        )
                        .unwrap();
                    }
                    _ => unreachable!(),
                }
            }
        }
        let before = fs::symlink_metadata(if case.starts_with("cargo-bin") {
            cargo_bin.clone()
        } else if case == "rustup-symlink" {
            cargo_bin.join("rustup")
        } else {
            cargo_bin.join("rustc")
        })
        .unwrap()
        .ino();
        let rejected = rust_seed_command(&script, &source, &destination, temp.path())
            .status()
            .unwrap();
        assert!(
            !rejected.success(),
            "accepted unsafe proxy destination: {case}"
        );
        let collision = if case.starts_with("cargo-bin") {
            cargo_bin
        } else if case == "rustup-symlink" {
            cargo_bin.join("rustup")
        } else {
            cargo_bin.join("rustc")
        };
        assert_eq!(
            fs::symlink_metadata(collision).unwrap().ino(),
            before,
            "overwrote user entry for {case}"
        );
        assert!(rust_proxy_staging_residue(&destination).is_empty());
    }
}

#[test]
fn writable_rust_bootstrap_cleans_an_incomplete_proxy_publication_and_retries() {
    let script = root().join("images/workspace/bin/initialize-rust-home");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    write_test_toolchain(&source, "1.97.0-aarch64-unknown-linux-gnu", "cargo\n");
    let test_bin = temp.path().join("test-bin");
    fs::create_dir(&test_bin).unwrap();
    let fake_cp = test_bin.join("cp");
    fs::write(
        &fake_cp,
        "#!/bin/sh\ncase \"$1\" in\n  */cargo-bin/rustup) : >\"$2\"; exit 23 ;;\nesac\nexec /bin/cp \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&fake_cp, fs::Permissions::from_mode(0o700)).unwrap();

    let failed = rust_seed_command(&script, &source, &destination, temp.path())
        .status()
        .unwrap();
    assert!(!failed.success());
    assert!(rust_proxy_staging_residue(&destination).is_empty());
    assert!(
        !destination.join("cargo-bin/rustup").exists(),
        "published an incomplete rustup"
    );

    fs::remove_file(fake_cp).unwrap();
    assert!(
        rust_seed_command(&script, &source, &destination, temp.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(destination.join("cargo-bin/rustup").is_file());
    assert!(rust_proxy_staging_residue(&destination).is_empty());
}

#[test]
fn writable_rust_bootstrap_rejects_unsafe_or_ambiguous_default_toolchain_settings() {
    let script = root().join("images/workspace/bin/initialize-rust-home");
    let temp = tempfile::tempdir().unwrap();
    let bundled = "1.97.0-aarch64-unknown-linux-gnu";

    for case in [
        "missing",
        "symlink",
        "directory",
        "oversized",
        "missing-default",
        "missing-version",
        "missing-profile",
        "duplicate-default",
        "duplicate-version",
        "duplicate-profile",
        "unsafe-default",
        "unknown-default",
        "malformed-default",
        "trailing-default-junk",
        "unterminated-default",
        "unknown-key",
        "overrides-entry",
        "repeated-overrides-table",
        "nested-table",
        "key-after-overrides",
        "noncanonical-version",
        "noncanonical-profile",
        "selected-cargo-symlink",
        "selected-rustc-directory",
        "selected-rustc-nonexecutable",
    ] {
        let source = temp.path().join(format!("settings-source-{case}"));
        let destination = temp.path().join(format!("settings-destination-{case}"));
        write_test_toolchain(&source, bundled, "cargo\n");
        let settings = source.join("settings.toml");
        fs::remove_file(&settings).unwrap();
        match case {
            "missing" => {}
            "symlink" => {
                let target = temp.path().join("settings-target.toml");
                if !target.exists() {
                    fs::write(&target, format!("default_toolchain = \"{bundled}\"\n")).unwrap();
                }
                symlink(target, &settings).unwrap();
            }
            "directory" => fs::create_dir(&settings).unwrap(),
            "oversized" => fs::write(&settings, vec![b'x'; 4097]).unwrap(),
            "missing-default" => fs::write(&settings, "version = \"12\"\n").unwrap(),
            "missing-version" => fs::write(
                &settings,
                format!("default_toolchain = \"{bundled}\"\nprofile = \"default\"\n"),
            )
            .unwrap(),
            "missing-profile" => fs::write(
                &settings,
                format!("version = \"12\"\ndefault_toolchain = \"{bundled}\"\n"),
            )
            .unwrap(),
            "duplicate-default" => fs::write(
                &settings,
                format!(
                    "version = \"12\"\ndefault_toolchain = \"{bundled}\"\ndefault_toolchain = \"{bundled}\"\nprofile = \"default\"\n"
                ),
            )
            .unwrap(),
            "duplicate-version" => fs::write(
                &settings,
                format!(
                    "version = \"12\"\nversion = \"12\"\ndefault_toolchain = \"{bundled}\"\nprofile = \"default\"\n"
                ),
            )
            .unwrap(),
            "duplicate-profile" => fs::write(
                &settings,
                format!(
                    "version = \"12\"\ndefault_toolchain = \"{bundled}\"\nprofile = \"default\"\nprofile = \"default\"\n"
                ),
            )
            .unwrap(),
            "unsafe-default" => {
                fs::write(
                    &settings,
                    "version = \"12\"\ndefault_toolchain = \"../outside\"\nprofile = \"default\"\n",
                )
                .unwrap()
            }
            "unknown-default" => fs::write(
                &settings,
                "version = \"12\"\ndefault_toolchain = \"1.98.0-aarch64-unknown-linux-gnu\"\nprofile = \"default\"\n",
            )
            .unwrap(),
            "malformed-default" => fs::write(
                &settings,
                format!(
                    "version = \"12\"\ndefault_toolchain=\"{bundled}\"\nprofile = \"default\"\n"
                ),
            )
            .unwrap(),
            "trailing-default-junk" => fs::write(
                &settings,
                format!(
                    "version = \"12\"\ndefault_toolchain = \"{bundled}\" # junk\nprofile = \"default\"\n"
                ),
            )
            .unwrap(),
            "unterminated-default" => fs::write(
                &settings,
                format!(
                    "version = \"12\"\ndefault_toolchain = \"{bundled}\nprofile = \"default\"\n"
                ),
            )
            .unwrap(),
            "unknown-key" => fs::write(
                &settings,
                format!(
                    "version = \"12\"\ndefault_toolchain = \"{bundled}\"\nprofile = \"default\"\ntelemetry = true\n"
                ),
            )
            .unwrap(),
            "overrides-entry" => fs::write(
                &settings,
                format!(
                    "version = \"12\"\ndefault_toolchain = \"{bundled}\"\nprofile = \"default\"\n\n[overrides]\n\"/workspace\" = \"nightly\"\n"
                ),
            )
            .unwrap(),
            "repeated-overrides-table" => fs::write(
                &settings,
                format!(
                    "version = \"12\"\ndefault_toolchain = \"{bundled}\"\nprofile = \"default\"\n\n[overrides]\n[overrides]\n"
                ),
            )
            .unwrap(),
            "nested-table" => fs::write(
                &settings,
                format!(
                    "version = \"12\"\n[nested]\ndefault_toolchain = \"{bundled}\"\nprofile = \"default\"\n"
                ),
            )
            .unwrap(),
            "key-after-overrides" => fs::write(
                &settings,
                format!(
                    "version = \"12\"\nprofile = \"default\"\n[overrides]\ndefault_toolchain = \"{bundled}\"\n"
                ),
            )
            .unwrap(),
            "noncanonical-version" => fs::write(
                &settings,
                format!(
                    "version = \"11\"\ndefault_toolchain = \"{bundled}\"\nprofile = \"default\"\n"
                ),
            )
            .unwrap(),
            "noncanonical-profile" => fs::write(
                &settings,
                format!(
                    "version = \"12\"\ndefault_toolchain = \"{bundled}\"\nprofile = \"minimal\"\n"
                ),
            )
            .unwrap(),
            "selected-cargo-symlink"
            | "selected-rustc-directory"
            | "selected-rustc-nonexecutable" => {
                fs::write(
                    &settings,
                    format!(
                        "version = \"12\"\ndefault_toolchain = \"{bundled}\"\nprofile = \"default\"\n\n[overrides]\n"
                    ),
                )
                .unwrap();
                let selected = source.join("toolchains").join(bundled).join("bin");
                match case {
                    "selected-cargo-symlink" => {
                        fs::remove_file(selected.join("cargo")).unwrap();
                        symlink("/bin/true", selected.join("cargo")).unwrap();
                    }
                    "selected-rustc-directory" => {
                        fs::remove_file(selected.join("rustc")).unwrap();
                        fs::create_dir(selected.join("rustc")).unwrap();
                    }
                    "selected-rustc-nonexecutable" => {
                        fs::set_permissions(
                            selected.join("rustc"),
                            fs::Permissions::from_mode(0o644),
                        )
                        .unwrap();
                    }
                    _ => unreachable!(),
                }
            }
            _ => unreachable!(),
        }
        let rejected = rust_seed_command(&script, &source, &destination, temp.path())
            .status()
            .unwrap();
        assert!(
            !rejected.success(),
            "accepted unsafe immutable settings: {case}"
        );
        assert!(rust_settings_staging_residue(&destination).is_empty());
    }
}

#[test]
fn writable_rust_bootstrap_accepts_canonical_settings_without_overrides_table() {
    let script = root().join("images/workspace/bin/initialize-rust-home");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    let bundled = "1.97.0-aarch64-unknown-linux-gnu";
    write_test_toolchain(&source, bundled, "cargo\n");
    let source_settings = source.join("settings.toml");
    fs::remove_file(&source_settings).unwrap();
    fs::write(
        &source_settings,
        format!("version = \"12\"\ndefault_toolchain = \"{bundled}\"\nprofile = \"default\"\n"),
    )
    .unwrap();
    fs::set_permissions(&source_settings, fs::Permissions::from_mode(0o444)).unwrap();

    assert!(
        rust_seed_command(&script, &source, &destination, temp.path())
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        fs::read_to_string(destination.join("settings.toml")).unwrap(),
        format!(
            "version = \"12\"\ndefault_toolchain = \"{bundled}\"\nprofile = \"default\"\n\n[overrides]\n"
        )
    );
}

#[test]
fn writable_rust_bootstrap_preserves_user_settings_and_rejects_unsafe_collisions() {
    let script = root().join("images/workspace/bin/initialize-rust-home");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    write_test_toolchain(&source, "1.97.0-aarch64-unknown-linux-gnu", "cargo\n");

    let user_destination = temp.path().join("user-destination");
    fs::create_dir(&user_destination).unwrap();
    let user_settings = user_destination.join("settings.toml");
    fs::write(&user_settings, "user-owned settings\n").unwrap();
    fs::set_permissions(&user_settings, fs::Permissions::from_mode(0o600)).unwrap();
    let user_inode = fs::metadata(&user_settings).unwrap().ino();
    assert!(
        rust_seed_command(&script, &source, &user_destination, temp.path())
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(fs::metadata(&user_settings).unwrap().ino(), user_inode);
    assert_eq!(
        fs::read_to_string(&user_settings).unwrap(),
        "user-owned settings\n"
    );

    for case in ["symlink", "directory"] {
        let destination = temp.path().join(format!("unsafe-settings-{case}"));
        fs::create_dir(&destination).unwrap();
        let settings = destination.join("settings.toml");
        if case == "symlink" {
            symlink(&user_settings, &settings).unwrap();
        } else {
            fs::create_dir(&settings).unwrap();
        }
        let inode = fs::symlink_metadata(&settings).unwrap().ino();
        let rejected = rust_seed_command(&script, &source, &destination, temp.path())
            .status()
            .unwrap();
        assert!(!rejected.success(), "accepted {case} user settings");
        assert_eq!(fs::symlink_metadata(&settings).unwrap().ino(), inode);
        assert!(rust_settings_staging_residue(&destination).is_empty());
    }
}

#[test]
fn writable_rust_bootstrap_cleans_interrupted_settings_publication_and_retries() {
    let script = root().join("images/workspace/bin/initialize-rust-home");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    write_test_toolchain(&source, "1.97.0-aarch64-unknown-linux-gnu", "cargo\n");
    let test_bin = temp.path().join("test-bin");
    fs::create_dir(&test_bin).unwrap();
    let fake_mv = test_bin.join("mv");
    fs::write(
        &fake_mv,
        "#!/bin/sh\nfor arg in \"$@\"; do\n  case \"$arg\" in */.gascan-rust-settings.*) exit 23 ;; esac\ndone\nexec \"$GASCAN_TEST_REAL_MV\" \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&fake_mv, fs::Permissions::from_mode(0o700)).unwrap();
    #[cfg(target_os = "macos")]
    let real_mv = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join("gmv"))
        .find(|candidate| candidate.is_file())
        .unwrap();
    #[cfg(not(target_os = "macos"))]
    let real_mv = Path::new("/bin/mv").to_path_buf();

    let failed = rust_seed_command(&script, &source, &destination, temp.path())
        .env("GASCAN_TEST_REAL_MV", real_mv)
        .status()
        .unwrap();
    assert!(!failed.success());
    assert!(!destination.join("settings.toml").exists());
    assert!(rust_settings_staging_residue(&destination).is_empty());

    fs::remove_file(fake_mv).unwrap();
    assert!(
        rust_seed_command(&script, &source, &destination, temp.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(destination.join("settings.toml").is_file());
    assert!(rust_settings_staging_residue(&destination).is_empty());
}

#[test]
fn writable_rust_bootstrap_reclaims_only_confined_crash_staging_and_retries() {
    let script = root().join("images/workspace/bin/initialize-rust-home");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    let cargo_bin = destination.join("cargo-bin");
    write_test_toolchain(&source, "1.97.0-aarch64-unknown-linux-gnu", "cargo\n");
    fs::create_dir_all(&cargo_bin).unwrap();

    let unrelated_target = temp.path().join("unrelated-target");
    fs::create_dir(&unrelated_target).unwrap();
    fs::write(unrelated_target.join("sentinel"), "outside survives\n").unwrap();
    let seed_stage = destination.join(".gascan-rust-seed.crash");
    fs::create_dir(&seed_stage).unwrap();
    fs::write(seed_stage.join("partial"), "partial\n").unwrap();
    symlink(&unrelated_target, seed_stage.join("outside-link")).unwrap();
    fs::write(destination.join(".gascan-rust-hash.crash"), "partial\n").unwrap();
    fs::write(destination.join(".gascan-rust-settings.crash"), "partial\n").unwrap();
    fs::create_dir(destination.join(".gascan-rust-marker.crash")).unwrap();
    fs::write(
        cargo_bin.join(".gascan-rust-proxy.rustup.crash"),
        "partial\n",
    )
    .unwrap();
    fs::create_dir(cargo_bin.join(".gascan-rust-proxy.rustc.crash")).unwrap();

    let unrelated_root = destination.join(".user-bootstrap-state");
    let unrelated_cargo = cargo_bin.join(".user-command-state");
    let similar_reserved = destination.join(".gascan-rust-seed-user");
    fs::write(&unrelated_root, "keep root\n").unwrap();
    fs::write(&unrelated_cargo, "keep cargo\n").unwrap();
    fs::write(&similar_reserved, "keep similar\n").unwrap();

    assert!(
        rust_seed_command(&script, &source, &destination, temp.path())
            .status()
            .unwrap()
            .success()
    );
    for reclaimed in [
        destination.join(".gascan-rust-seed.crash"),
        destination.join(".gascan-rust-hash.crash"),
        destination.join(".gascan-rust-settings.crash"),
        destination.join(".gascan-rust-marker.crash"),
        cargo_bin.join(".gascan-rust-proxy.rustup.crash"),
        cargo_bin.join(".gascan-rust-proxy.rustc.crash"),
    ] {
        assert!(
            !reclaimed.symlink_metadata().is_ok(),
            "stale crash stage survived: {}",
            reclaimed.display()
        );
    }
    assert_eq!(
        fs::read_to_string(unrelated_target.join("sentinel")).unwrap(),
        "outside survives\n"
    );
    assert_eq!(fs::read_to_string(unrelated_root).unwrap(), "keep root\n");
    assert_eq!(fs::read_to_string(unrelated_cargo).unwrap(), "keep cargo\n");
    assert_eq!(
        fs::read_to_string(similar_reserved).unwrap(),
        "keep similar\n"
    );
}

#[test]
fn writable_rust_bootstrap_rejects_unsafe_crash_staging_without_following_or_removing_it() {
    let script = root().join("images/workspace/bin/initialize-rust-home");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    write_test_toolchain(&source, "1.97.0-aarch64-unknown-linux-gnu", "cargo\n");
    let target_file = temp.path().join("outside-file");
    let target_directory = temp.path().join("outside-directory");
    fs::write(&target_file, "outside file survives\n").unwrap();
    fs::create_dir(&target_directory).unwrap();
    fs::write(target_directory.join("sentinel"), "outside dir survives\n").unwrap();

    for case in [
        "seed-symlink",
        "hash-symlink",
        "settings-symlink",
        "marker-symlink",
        "proxy-symlink",
        "seed-file",
        "hash-directory",
        "settings-directory",
        "marker-file",
    ] {
        let destination = temp.path().join(format!("unsafe-crash-{case}"));
        let cargo_bin = destination.join("cargo-bin");
        fs::create_dir_all(&cargo_bin).unwrap();
        let collision = match case {
            "seed-symlink" => destination.join(".gascan-rust-seed.crash"),
            "hash-symlink" => destination.join(".gascan-rust-hash.crash"),
            "settings-symlink" => destination.join(".gascan-rust-settings.crash"),
            "marker-symlink" => destination.join(".gascan-rust-marker.crash"),
            "proxy-symlink" => cargo_bin.join(".gascan-rust-proxy.rustc.crash"),
            "seed-file" => destination.join(".gascan-rust-seed.crash"),
            "hash-directory" => destination.join(".gascan-rust-hash.crash"),
            "settings-directory" => destination.join(".gascan-rust-settings.crash"),
            "marker-file" => destination.join(".gascan-rust-marker.crash"),
            _ => unreachable!(),
        };
        match case {
            "seed-symlink" | "marker-symlink" | "proxy-symlink" => {
                symlink(&target_directory, &collision).unwrap()
            }
            "hash-symlink" | "settings-symlink" => symlink(&target_file, &collision).unwrap(),
            "seed-file" | "marker-file" => fs::write(&collision, "wrong type\n").unwrap(),
            "hash-directory" | "settings-directory" => fs::create_dir(&collision).unwrap(),
            _ => unreachable!(),
        }
        let collision_inode = fs::symlink_metadata(&collision).unwrap().ino();
        let rejected = rust_seed_command(&script, &source, &destination, temp.path())
            .status()
            .unwrap();
        assert!(!rejected.success(), "accepted unsafe crash stage: {case}");
        assert_eq!(
            fs::symlink_metadata(&collision).unwrap().ino(),
            collision_inode,
            "mutated unsafe crash stage: {case}"
        );
        assert_eq!(
            fs::read_to_string(&target_file).unwrap(),
            "outside file survives\n"
        );
        assert_eq!(
            fs::read_to_string(target_directory.join("sentinel")).unwrap(),
            "outside dir survives\n"
        );
    }
}

#[test]
fn writable_rust_bootstrap_cleans_staging_and_rejects_unsafe_paths() {
    let script = root().join("images/workspace/bin/initialize-rust-home");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let incomplete = "1.97.0-aarch64-unknown-linux-gnu";
    write_test_toolchain(&source, incomplete, "cargo\n");
    fs::remove_file(source.join("toolchains").join(incomplete).join("bin/rustc")).unwrap();
    let retry_destination = temp.path().join("retry-destination");
    let failed = rust_seed_command(&script, &source, &retry_destination, temp.path())
        .status()
        .unwrap();
    assert!(!failed.success());
    assert!(rust_staging_residue(&retry_destination).is_empty());
    fs::write(
        source.join("toolchains").join(incomplete).join("bin/rustc"),
        "rustc\n",
    )
    .unwrap();
    fs::set_permissions(
        source.join("toolchains").join(incomplete).join("bin/rustc"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(
        rust_seed_command(&script, &source, &retry_destination, temp.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(rust_staging_residue(&retry_destination).is_empty());

    for collision in ["symlink", "file"] {
        let destination = temp.path().join(format!("{collision}-destination"));
        fs::create_dir_all(destination.join("toolchains")).unwrap();
        let final_path = destination.join("toolchains").join(incomplete);
        if collision == "symlink" {
            symlink(temp.path(), &final_path).unwrap();
        } else {
            fs::write(&final_path, "unmanaged").unwrap();
        }
        let rejected = rust_seed_command(&script, &source, &destination, temp.path())
            .status()
            .unwrap();
        assert!(
            !rejected.success(),
            "accepted {collision} destination collision"
        );
        assert!(rust_staging_residue(&destination).is_empty());
    }

    for source_collision in ["symlink", "file"] {
        let unsafe_source = temp.path().join(format!("{source_collision}-source"));
        fs::create_dir_all(unsafe_source.join("toolchains")).unwrap();
        fs::create_dir_all(unsafe_source.join("update-hashes")).unwrap();
        let toolchain = unsafe_source.join("toolchains/unsafe");
        if source_collision == "symlink" {
            symlink(source.join("toolchains").join(incomplete), &toolchain).unwrap();
        } else {
            fs::write(&toolchain, "not a directory").unwrap();
        }
        let destination = temp
            .path()
            .join(format!("{source_collision}-source-destination"));
        let rejected = rust_seed_command(&script, &unsafe_source, &destination, temp.path())
            .status()
            .unwrap();
        assert!(
            !rejected.success(),
            "accepted {source_collision} source collision"
        );
    }
}

#[test]
fn workstation_contract_uses_exact_locked_command_versions() {
    let contract =
        fs::read_to_string(root().join("images/workspace/tests/workstation-contract.sh")).unwrap();
    assert!(
        !contract.contains("expect_pattern"),
        "guaranteed commands must not pass through broad version regexes"
    );
    assert!(
        contract.contains("first_line ip -Version | cut -d, -f1,2"),
        "ip normalization must retain the utility and iproute2 version fields"
    );

    let lock: toml::Value =
        toml::from_str(&fs::read_to_string(root().join("images/workspace/versions.lock")).unwrap())
            .unwrap();
    let commands = lock["workstation_commands"].as_table().unwrap();
    let expected = [
        ("cargo", "cargo 1.97.0"),
        ("rustc", "rustc 1.97.0"),
        ("vim", "VIM - Vi IMproved 9.1"),
        ("emacs", "GNU Emacs 29.3"),
        ("pico", "GNU nano, version 7.2"),
        ("gh", "gh version 2.45.0"),
        ("git", "git version 2.43.0"),
        ("ip", "ip utility, iproute2-6.1.0"),
        ("ss", "ss utility, iproute2-6.1.0"),
        ("ping", "ping from iputils 20240117"),
        ("ifconfig", "net-tools 2.10"),
        ("netstat", "net-tools 2.10"),
        ("dig", "DiG 9.18.39-0ubuntu0.24.04.5-Ubuntu"),
        ("traceroute", "Modern traceroute for Linux, version 2.1.5"),
        ("nc", "OpenBSD netcat (Debian patchlevel 1.226-1ubuntu2)"),
        ("rg", "ripgrep 14.1.0"),
        ("fd", "fdfind 9.0.0"),
        ("fzf", "0.44.1 (debian)"),
        ("tmux", "tmux 3.4"),
    ];
    assert_eq!(commands.len(), expected.len());
    for (name, version) in expected {
        assert_eq!(commands[name].as_str(), Some(version), "{name}");
        assert!(
            contract.contains(&format!("locked_version {name}")),
            "{name} is not checked against locked workstation evidence"
        );
    }
}

#[test]
fn workstation_contract_requires_a_traversable_managed_hook_directory() {
    let contract =
        fs::read_to_string(root().join("images/workspace/tests/workstation-contract.sh")).unwrap();
    assert!(
        contract.contains(r#"test "$(stat -c %U:%G:%a /etc/gascan)" = root:root:555"#),
        "workstation smoke does not pin the managed hook directory boundary"
    );
    assert!(
        contract.contains("test -x /etc/gascan"),
        "workspace smoke does not prove it can traverse the managed hook directory"
    );
    assert!(
        contract.contains("test -r /etc/gascan/bashrc"),
        "workspace smoke does not prove it can read the immutable hook"
    );
}

#[test]
fn workstation_contract_accepts_only_the_locked_starship_first_line_behaviorally() {
    let contract =
        fs::read_to_string(root().join("images/workspace/tests/workstation-contract.sh")).unwrap();
    let helper_prefix = contract
        .split_once("locked=/opt/gascan/workstation/versions.json")
        .unwrap()
        .0;
    let lines = contract.lines().collect::<Vec<_>>();
    let check_index = lines
        .iter()
        .position(|line| line.contains("/opt/gascan/shell/bin/starship --version"))
        .unwrap();
    let mut check = lines[check_index].to_owned();
    if check.ends_with("||") {
        check.push('\n');
        check.push_str(lines[check_index + 1]);
    }
    let check = check.replace("/opt/gascan/shell/bin/starship", "\"$1\"");

    let temporary = tempfile::tempdir().unwrap();
    let starship = temporary.path().join("starship");
    fs::write(
        &starship,
        "#!/bin/sh\nset -eu\ntest \"$1\" = --version\nprintf '%s' \"$STARSHIP_VERSION_OUTPUT\"\n",
    )
    .unwrap();
    fs::set_permissions(&starship, fs::Permissions::from_mode(0o755)).unwrap();
    let probe = format!(
        "{helper_prefix}\n\
         locked_version()\n\
         {{\n\
             test \"$1\" = starship\n\
             printf '%s\\n' 1.25.1\n\
         }}\n\
         {check}\n"
    );
    let run = |output: &str| {
        Command::new("sh")
            .args(["-c", &probe, "sh", starship.to_str().unwrap()])
            .env("STARSHIP_VERSION_OUTPUT", output)
            .status()
            .unwrap()
    };

    assert!(
        run("starship 1.25.1\n\
             branch:master\n\
             commit_hash:8758daa\n\
             build_time:2026-04-30 19:35:31 +00:00\n\
             build_env:rustc 1.95.0 (59807616e 2026-04-14),\n")
        .success(),
        "real trailing Starship build metadata was rejected"
    );
    assert!(
        !run("starship 1.25.10\n\
              branch:master\n\
              commit_hash:8758daa\n\
              build_time:2026-04-30 19:35:31 +00:00\n\
              build_env:rustc 1.95.0 (59807616e 2026-04-14),\n")
        .success(),
        "wrong first-line Starship version was accepted"
    );
}

#[test]
fn workstation_home_configuration_is_idempotent_and_refuses_unmanaged_paths() {
    let script = root().join("images/workspace/bin/configure-workstation-home");
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let run = || Command::new(&script).env("HOME", &home).status().unwrap();
    assert!(run().success());
    assert!(run().success());
    assert!(
        !home.join(".config/gascan/.gascan-managed").exists(),
        "broad Gas Can config boundary must not be claimed as an application directory"
    );
    for agent in ["claude", "codex", "pi"] {
        let link = home.join(format!(".{agent}"));
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(
            fs::read_link(link).unwrap(),
            Path::new(".config/gascan/agents").join(agent)
        );
        assert!(home.join(".config/gascan/agents").join(agent).is_dir());
        assert_eq!(
            fs::read_to_string(
                home.join(".config/gascan/agents")
                    .join(agent)
                    .join(".gascan-managed")
            )
            .unwrap(),
            "gascan-workstation-home-v1\n"
        );
    }
    for tool in ["mise", "claude", "codex", "pi", "herdr", "gh", "glab"] {
        assert!(home.join(".cache").join(tool).is_dir());
    }
    let herdr = home.join(".config/herdr");
    assert!(herdr.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(fs::read_link(herdr).unwrap(), Path::new("../.cache/herdr"));

    let blocked_home = temp.path().join("blocked");
    fs::create_dir(&blocked_home).unwrap();
    fs::write(blocked_home.join(".claude"), "user data").unwrap();
    let blocked = Command::new(&script)
        .env("HOME", &blocked_home)
        .status()
        .unwrap();
    assert!(!blocked.success());
    assert_eq!(
        fs::read_to_string(blocked_home.join(".claude")).unwrap(),
        "user data"
    );
    assert!(
        !blocked_home.join(".config/gascan/agents/claude").exists(),
        "refusal must occur before managed targets are created"
    );

    let blocked_herdr_home = temp.path().join("blocked-herdr");
    fs::create_dir_all(blocked_herdr_home.join(".config/herdr")).unwrap();
    let blocked_herdr = Command::new(&script)
        .env("HOME", &blocked_herdr_home)
        .status()
        .unwrap();
    assert!(!blocked_herdr.success());
    assert!(
        !blocked_herdr_home.join(".config/gascan/agents").exists(),
        "Herdr refusal must occur before managed targets are created"
    );

    for case in [
        "later-agent",
        "config",
        "cache",
        "cache-mise",
        "cache-file",
        "cache-link",
    ] {
        let adversarial_home = temp.path().join(case);
        fs::create_dir(&adversarial_home).unwrap();
        match case {
            "later-agent" => {
                let agents = adversarial_home.join(".config/gascan/agents");
                fs::create_dir_all(agents.join("codex")).unwrap();
                fs::write(
                    agents.join(".gascan-managed"),
                    "gascan-workstation-home-v1\n",
                )
                .unwrap();
            }
            "config" => fs::create_dir_all(adversarial_home.join(".config/gascan/glab")).unwrap(),
            "cache" => fs::create_dir_all(adversarial_home.join(".cache/glab")).unwrap(),
            "cache-mise" => fs::create_dir_all(adversarial_home.join(".cache/mise")).unwrap(),
            "cache-file" => {
                fs::create_dir_all(adversarial_home.join(".cache")).unwrap();
                fs::write(adversarial_home.join(".cache/pi"), "unmanaged").unwrap();
            }
            "cache-link" => {
                fs::create_dir_all(adversarial_home.join(".cache")).unwrap();
                symlink(temp.path(), adversarial_home.join(".cache/gh")).unwrap();
            }
            _ => unreachable!(),
        }
        let rejected = Command::new(&script)
            .env("HOME", &adversarial_home)
            .status()
            .unwrap();
        assert!(!rejected.success(), "accepted unmanaged {case} destination");
        assert!(
            !adversarial_home.join(".claude").exists()
                && !adversarial_home
                    .join(".config/gascan/agents/claude")
                    .exists(),
            "preflight rejection for {case} left partial Claude state"
        );
    }
}

#[test]
fn profile_defaults_are_exact_and_idempotent() {
    let profile = root().join("images/workspace/etc/profile.d/mise.sh");
    let script = format!(". '{}'; . '{}'; env", profile.display(), profile.display());
    let output = Command::new("/bin/sh")
        .args(["-c", &script])
        .env_clear()
        .env("HOME", "/home/workspace")
        .env("PATH", "/tmp/duplicate:/opt/gascan/mise/shims")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let environment = stdout
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect::<std::collections::BTreeMap<_, _>>();
    for (name, value) in [
        ("XDG_DATA_HOME", "/home/workspace/.local/share"),
        ("XDG_CACHE_HOME", "/home/workspace/.cache"),
        ("XDG_CONFIG_HOME", "/home/workspace/.config"),
        ("MISE_DATA_DIR", "/home/workspace/.local/share/mise"),
        ("MISE_SYSTEM_DATA_DIR", "/opt/gascan/mise"),
        ("MISE_CACHE_DIR", "/home/workspace/.cache/mise"),
        (
            "MISE_GLOBAL_CONFIG_FILE",
            "/home/workspace/.config/gascan/mise.toml",
        ),
        ("MISE_SYSTEM_CONFIG_FILE", "/etc/mise/config.toml"),
        (
            "MISE_STATE_DIR",
            "/home/workspace/.config/gascan/mise-state",
        ),
        ("CARGO_HOME", "/home/workspace/.local/share/cargo"),
        ("MISE_CARGO_HOME", "/home/workspace/.local/share/cargo"),
        ("RUSTUP_HOME", "/home/workspace/.local/share/rustup"),
        ("MISE_RUSTUP_HOME", "/home/workspace/.local/share/rustup"),
        ("NPM_CONFIG_PREFIX", "/home/workspace/.local"),
        ("NPM_CONFIG_CACHE", "/home/workspace/.cache/npm"),
        ("GOPATH", "/home/workspace/.local/share/go"),
        ("GOBIN", "/home/workspace/.local/bin"),
        ("GOCACHE", "/home/workspace/.cache/go-build"),
        ("GOMODCACHE", "/home/workspace/.cache/go-mod"),
        ("PYTHONUSERBASE", "/home/workspace/.local"),
        ("GEM_HOME", "/home/workspace/.local/share/gem"),
        ("MIX_HOME", "/home/workspace/.local/share/mix"),
        ("HEX_HOME", "/home/workspace/.local/share/hex"),
        ("REBAR_CACHE_DIR", "/home/workspace/.cache/rebar3"),
        ("PATH", RUNTIME_PATH),
    ] {
        assert_eq!(environment.get(name), Some(&value), "{name}");
    }
}

#[test]
fn workstation_home_configuration_contains_no_credentials() {
    let script =
        fs::read_to_string(root().join("images/workspace/bin/configure-workstation-home")).unwrap();
    for forbidden in ["token=", "TOKEN=", "api_key", "API_KEY", "credential"] {
        assert!(
            !script.contains(forbidden),
            "home setup must not materialize credentials: {forbidden}"
        );
    }
}

#[test]
fn shell_configurator_rejects_non_root_and_extra_input_without_mutation() {
    let fixture = ShellFixture::new();
    for prompt in ["standard", "starship", "starship-nerd-font"] {
        let output = fixture.run(prompt);
        assert!(
            !output.status.success(),
            "non-root configurator accepted {prompt}"
        );
    }
    let extra = Command::new(&fixture.script)
        .args(["standard", "ignored"])
        .env_clear()
        .output()
        .unwrap();
    assert!(!extra.status.success(), "configurator accepted extra input");
    assert!(
        !fixture.home.join(".config/gascan/shell").exists(),
        "rejected invocation mutated managed state"
    );
}

#[test]
fn shell_hook_is_interactive_only_and_standard_is_a_true_prompt_no_op() {
    let temporary = tempfile::tempdir().unwrap();
    let (hook, shell_dir, _starship, log) = hook_fixture(&temporary);

    let noninteractive = run_hook(
        &hook,
        r#". "$GASCAN_TEST_HOOK"; printf 'noninteractive-ok\n'"#,
        &log,
        false,
        false,
        false,
    );
    assert!(noninteractive.status.success());
    assert_eq!(noninteractive.stdout, b"noninteractive-ok\n");
    assert!(!log.exists(), "non-interactive hook invoked Starship");

    write_mode(&shell_dir.join("prompt"), "standard\n", 0o640);
    let standard = run_hook(
        &hook,
        r#"PS1='native-prompt'; STARSHIP_CONFIG='user-value'; . "$GASCAN_TEST_HOOK"; printf '%s|%s\n' "$PS1" "$STARSHIP_CONFIG""#,
        &log,
        false,
        false,
        true,
    );
    assert!(
        standard.status.success(),
        "{}",
        String::from_utf8_lossy(&standard.stderr)
    );
    assert_eq!(standard.stdout, b"native-prompt|user-value\n");
    assert!(
        !String::from_utf8_lossy(&standard.stderr).contains("gascan:"),
        "{}",
        String::from_utf8_lossy(&standard.stderr)
    );
    assert!(!log.exists(), "standard mode invoked Starship");
}

#[test]
fn shell_hook_uses_only_the_pinned_binary_and_managed_config() {
    let temporary = tempfile::tempdir().unwrap();
    let (hook, shell_dir, _starship, log) = hook_fixture(&temporary);
    write_mode(&shell_dir.join("prompt"), "starship-nerd-font\n", 0o640);
    write_mode(
        &shell_dir.join("starship.toml"),
        "format = \"$character\"\n",
        0o640,
    );
    let fake_path = temporary.path().join("path");
    fs::create_dir(&fake_path).unwrap();
    fs::write(
        fake_path.join("starship"),
        "#!/bin/sh\nprintf 'PATH starship invoked\\n' >&2\nexit 99\n",
    )
    .unwrap();
    fs::set_permissions(
        fake_path.join("starship"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    let output = Command::new("/bin/bash")
        .args([
            "--noprofile",
            "--norc",
            "-i",
            "-c",
            r#"PS1='native-prompt'; . "$GASCAN_TEST_HOOK"; printf '%s|' "$PS1"; starship_precmd"#,
        ])
        .env("PATH", &fake_path)
        .env("GASCAN_TEST_HOOK", &hook)
        .env("GASCAN_TEST_LOG", &log)
        .env("GASCAN_TEST_GENERATION_FAIL", "0")
        .env("GASCAN_TEST_EVAL_FAIL", "0")
        .env_remove("STARSHIP_SHELL")
        .env_remove("STARSHIP_SESSION_KEY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        b"managed-starship|stable-runtime-prompt\n",
        "stderr: {}\nlog: {}",
        String::from_utf8_lossy(&output.stderr),
        fs::read_to_string(&log).unwrap_or_default()
    );
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        format!(
            "init bash --print-full-init|{}|{}\nprompt|{}|{}\n",
            shell_dir.join("starship.toml").display(),
            _starship.display(),
            shell_dir.join("starship.toml").display(),
            _starship.display(),
        )
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("PATH starship"),
        "hook resolved Starship through PATH"
    );
}

#[test]
fn shell_hook_preserves_supported_prompt_command_customization() {
    let temporary = tempfile::tempdir().unwrap();
    let (hook, shell_dir, _starship, log) = hook_fixture(&temporary);
    write_mode(&shell_dir.join("prompt"), "starship\n", 0o640);
    write_mode(
        &shell_dir.join("starship.toml"),
        "format = \"$character\"\n",
        0o640,
    );

    let output = run_hook(
        &hook,
        r#"PS1=native-prompt; caller_prompt_command() { printf 'caller-customization\n'; }; PROMPT_COMMAND=caller_prompt_command; . "$GASCAN_TEST_HOOK"; printf 'COMMAND=%s\nPRESERVED=%s\n' "$PROMPT_COMMAND" "$STARSHIP_PROMPT_COMMAND"; starship_precmd"#,
        &log,
        false,
        false,
        true,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        b"COMMAND=starship_precmd\nPRESERVED=caller_prompt_command\ncaller-customization\nstable-runtime-prompt\n"
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("gascan:"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn shell_hook_warns_once_and_returns_success_when_starship_is_unavailable() {
    let temporary = tempfile::tempdir().unwrap();
    let (hook, shell_dir, _starship, log) = hook_fixture(&temporary);
    write_mode(&shell_dir.join("prompt"), "starship\n", 0o640);
    write_mode(
        &shell_dir.join("starship.toml"),
        "format = \"$character\"\n",
        0o640,
    );
    let output = run_hook(
        &hook,
        r#"PS1='native-prompt'; . "$GASCAN_TEST_HOOK"; . "$GASCAN_TEST_HOOK"; if declare -p __gascan_starship_euid >/dev/null 2>&1; then helper=set; else helper=unset; fi; printf '%s|%s\n' "$PS1" "$helper""#,
        &log,
        true,
        false,
        true,
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"native-prompt|unset\n");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr)
            .matches("gascan: Starship prompt unavailable; using standard Bash prompt.")
            .count(),
        1,
        "stderr: {}\nlog: {}",
        String::from_utf8_lossy(&output.stderr),
        fs::read_to_string(&log).unwrap_or_default()
    );

    let before = fs::read_to_string(&log).unwrap_or_default();
    let debug_trap = run_hook(
        &hook,
        r#"PS1=native-prompt; set -T; trap 'if [ "$BASH_SUBSHELL" -gt 0 ]; then starship_precmd() { printf attacker; }; STARSHIP_START_TIME=attacker; fi' DEBUG; before=$(trap -p DEBUG); . "$GASCAN_TEST_HOOK"; after=$(trap -p DEBUG); if declare -F starship_precmd >/dev/null; then function=present; else function=absent; fi; printf 'PS1=%s\nFUNCTION=%s\nSTART=%s\nBEFORE=%s\nAFTER=%s\n' "$PS1" "$function" "${STARSHIP_START_TIME-unset}" "$before" "$after""#,
        &log,
        false,
        false,
        true,
    );
    assert!(debug_trap.status.success());
    let debug_stdout = String::from_utf8_lossy(&debug_trap.stdout);
    assert!(
        debug_stdout.contains("PS1=native-prompt\n"),
        "{debug_stdout}"
    );
    assert!(debug_stdout.contains("FUNCTION=absent\n"), "{debug_stdout}");
    assert!(debug_stdout.contains("START=unset\n"), "{debug_stdout}");
    let trap_line = "trap -- 'if [ \"$BASH_SUBSHELL\" -gt 0 ]; then starship_precmd() { printf attacker; }; STARSHIP_START_TIME=attacker; fi' DEBUG";
    assert!(
        debug_stdout.contains(&format!("BEFORE={trap_line}\nAFTER={trap_line}\n")),
        "{debug_stdout}"
    );
    assert_eq!(
        String::from_utf8_lossy(&debug_trap.stderr)
            .matches("gascan: Starship prompt unavailable; using standard Bash prompt.")
            .count(),
        1,
        "{}",
        String::from_utf8_lossy(&debug_trap.stderr)
    );
    assert_eq!(
        fs::read_to_string(&log).unwrap_or_default(),
        before,
        "inherited DEBUG trap reached Starship"
    );

    let before = fs::read_to_string(&log).unwrap_or_default();
    let spoofed_debug_trap = run_hook(
        &hook,
        r#"PS1=native-prompt; set -T; builtin trap 'trap() { :; }' DEBUG; before=$(builtin trap -p DEBUG); . "$GASCAN_TEST_HOOK"; after=$(builtin trap -p DEBUG); if declare -F trap >/dev/null; then spoof=present; else spoof=absent; fi; printf 'PS1=%s\nSPOOF=%s\nBEFORE=%s\nAFTER=%s\n' "$PS1" "$spoof" "$before" "$after""#,
        &log,
        false,
        false,
        true,
    );
    assert!(spoofed_debug_trap.status.success());
    let spoofed_stdout = String::from_utf8_lossy(&spoofed_debug_trap.stdout);
    assert!(
        spoofed_stdout.contains("PS1=native-prompt\n"),
        "{spoofed_stdout}"
    );
    assert!(
        spoofed_stdout.contains("SPOOF=present\n"),
        "{spoofed_stdout}"
    );
    let spoofed_trap_line = "trap -- 'trap() { :; }' DEBUG";
    assert!(
        spoofed_stdout.contains(&format!(
            "BEFORE={spoofed_trap_line}\nAFTER={spoofed_trap_line}\n"
        )),
        "{spoofed_stdout}"
    );
    assert_eq!(
        String::from_utf8_lossy(&spoofed_debug_trap.stderr)
            .matches("gascan: Starship prompt unavailable; using standard Bash prompt.")
            .count(),
        1,
        "{}",
        String::from_utf8_lossy(&spoofed_debug_trap.stderr)
    );
    assert_eq!(
        fs::read_to_string(&log).unwrap_or_default(),
        before,
        "spoofed DEBUG trap reached Starship"
    );

    let before = fs::read_to_string(&log).unwrap_or_default();
    let self_clearing_debug_trap = run_hook(
        &hook,
        r#"PS1=native-prompt; set -T; builtin trap 'builtin trap - DEBUG; STARSHIP_START_TIME=attacker' DEBUG; . "$GASCAN_TEST_HOOK"; printf 'PS1=%s\nSTART=%s\nTRAP=%s\n' "$PS1" "${STARSHIP_START_TIME-unset}" "$(builtin trap -p DEBUG)""#,
        &log,
        false,
        false,
        true,
    );
    assert!(self_clearing_debug_trap.status.success());
    assert_eq!(
        self_clearing_debug_trap.stdout,
        b"PS1=native-prompt\nSTART=attacker\nTRAP=\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&self_clearing_debug_trap.stderr)
            .matches("gascan: Starship prompt unavailable; using standard Bash prompt.")
            .count(),
        1,
        "{}",
        String::from_utf8_lossy(&self_clearing_debug_trap.stderr)
    );
    assert_eq!(
        fs::read_to_string(&log).unwrap_or_default(),
        before,
        "self-clearing DEBUG mutation reached Starship"
    );

    let before = fs::read_to_string(&log).unwrap_or_default();
    let signal_collision = Command::new("/bin/bash")
        .args([
            "--noprofile",
            "--norc",
            "-i",
            "-c",
            r#"PS1=native-prompt; trap 'starship_precmd() { printf attacker; }; readonly -f starship_precmd' USR1; export GASCAN_TEST_PARENT_PID=$$; . "$GASCAN_TEST_HOOK"; printf 'PS1=%s\nFUNCTION=' "$PS1"; starship_precmd; printf '\n'"#,
        ])
        .env("GASCAN_TEST_HOOK", &hook)
        .env("GASCAN_TEST_LOG", &log)
        .env("GASCAN_TEST_GENERATION_FAIL", "0")
        .env("GASCAN_TEST_EVAL_FAIL", "0")
        .env("GASCAN_TEST_SIGNAL_COLLISION", "1")
        .env_remove("STARSHIP_SHELL")
        .env_remove("STARSHIP_SESSION_KEY")
        .output()
        .unwrap();
    assert!(signal_collision.status.success());
    assert_eq!(
        signal_collision.stdout,
        b"PS1=native-prompt\nFUNCTION=attacker\n",
        "stderr: {}",
        String::from_utf8_lossy(&signal_collision.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&signal_collision.stderr)
            .matches("gascan: Starship prompt unavailable; using standard Bash prompt.")
            .count(),
        1,
        "{}",
        String::from_utf8_lossy(&signal_collision.stderr)
    );
    assert_eq!(
        fs::read_to_string(&log)
            .unwrap_or_default()
            .matches("init bash --print-full-init")
            .count(),
        before.matches("init bash --print-full-init").count() + 1,
        "readonly function race did not stop after one generated init"
    );

    let before = fs::read_to_string(&log).unwrap_or_default();
    let writable_signal_collision = Command::new("/bin/bash")
        .args([
            "--noprofile",
            "--norc",
            "-i",
            "-c",
            r#"PS1=native-prompt; trap 'starship_precmd() { printf attacker; }' USR1; export GASCAN_TEST_PARENT_PID=$$; . "$GASCAN_TEST_HOOK"; printf 'PS1=%s\nFUNCTION=' "$PS1"; starship_precmd; printf '\n'"#,
        ])
        .env("GASCAN_TEST_HOOK", &hook)
        .env("GASCAN_TEST_LOG", &log)
        .env("GASCAN_TEST_GENERATION_FAIL", "0")
        .env("GASCAN_TEST_EVAL_FAIL", "0")
        .env("GASCAN_TEST_SIGNAL_COLLISION", "1")
        .env_remove("STARSHIP_SHELL")
        .env_remove("STARSHIP_SESSION_KEY")
        .output()
        .unwrap();
    assert!(writable_signal_collision.status.success());
    assert_eq!(
        writable_signal_collision.stdout,
        b"PS1=native-prompt\nFUNCTION=attacker\n",
        "stderr: {}",
        String::from_utf8_lossy(&writable_signal_collision.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&writable_signal_collision.stderr)
            .matches("gascan: Starship prompt unavailable; using standard Bash prompt.")
            .count(),
        1,
        "{}",
        String::from_utf8_lossy(&writable_signal_collision.stderr)
    );
    assert_eq!(
        fs::read_to_string(&log)
            .unwrap_or_default()
            .matches("init bash --print-full-init")
            .count(),
        before.matches("init bash --print-full-init").count() + 1,
        "writable function race did not stop after one generated init"
    );

    for (name, setup) in [
        (
            "STARSHIP_CONFIG",
            "readonly STARSHIP_CONFIG=readonly-config",
        ),
        (
            "STARSHIP_EXECUTABLE",
            "readonly STARSHIP_EXECUTABLE=readonly-executable",
        ),
        ("PATH", "readonly PATH"),
    ] {
        let before = fs::read_to_string(&log).unwrap_or_default();
        let command = format!(
            "PS1=native-prompt; {setup}; . \"$GASCAN_TEST_HOOK\"; \
             printf 'survived|%s\\n' \"$PS1\""
        );
        let readonly = run_hook(&hook, &command, &log, false, false, true);
        assert!(
            readonly.status.success(),
            "readonly {name} aborted startup: {}",
            String::from_utf8_lossy(&readonly.stderr)
        );
        assert_eq!(readonly.stdout, b"survived|native-prompt\n");
        assert_eq!(
            String::from_utf8_lossy(&readonly.stderr)
                .matches("gascan: Starship prompt unavailable; using standard Bash prompt.")
                .count(),
            1,
            "readonly {name}: {}",
            String::from_utf8_lossy(&readonly.stderr)
        );
        assert_eq!(
            fs::read_to_string(&log).unwrap_or_default(),
            before,
            "readonly {name} reached Starship"
        );
    }

    for (name, setup, expected) in [
        (
            "_starship_set_return function",
            "_starship_set_return() { printf attacker; }",
            "function=attacker",
        ),
        (
            "starship_preexec function",
            "starship_preexec() { printf attacker; }",
            "function=attacker",
        ),
        (
            "starship_preexec_all function",
            "starship_preexec_all() { printf attacker; }",
            "function=attacker",
        ),
        (
            "starship_preexec_ps0 function",
            "starship_preexec_ps0() { printf attacker; }",
            "function=attacker",
        ),
        (
            "starship_precmd function",
            "starship_precmd() { printf attacker; }",
            "function=attacker",
        ),
        (
            "readonly function",
            "starship_precmd() { printf attacker; }; readonly -f starship_precmd",
            "function=attacker",
        ),
        (
            "STARSHIP_PREEXEC_READY variable",
            "STARSHIP_PREEXEC_READY=attacker",
            "variable=attacker",
        ),
        (
            "STARSHIP_START_TIME variable",
            "STARSHIP_START_TIME=attacker",
            "variable=attacker",
        ),
        (
            "STARSHIP_CMD_STATUS variable",
            "STARSHIP_CMD_STATUS=attacker",
            "variable=attacker",
        ),
        (
            "STARSHIP_PIPE_STATUS variable",
            "STARSHIP_PIPE_STATUS=attacker",
            "variable=attacker",
        ),
        (
            "STARSHIP_END_TIME variable",
            "STARSHIP_END_TIME=attacker",
            "variable=attacker",
        ),
        (
            "STARSHIP_DURATION variable",
            "STARSHIP_DURATION=attacker",
            "variable=attacker",
        ),
        (
            "STARSHIP_PROMPT_COMMAND variable",
            "STARSHIP_PROMPT_COMMAND=attacker",
            "variable=attacker",
        ),
        (
            "STARSHIP_DEBUG_TRAP variable",
            "STARSHIP_DEBUG_TRAP=attacker",
            "variable=attacker",
        ),
        (
            "STARSHIP_SHELL variable",
            "STARSHIP_SHELL=attacker",
            "variable=attacker",
        ),
        (
            "STARSHIP_SESSION_KEY variable",
            "STARSHIP_SESSION_KEY=attacker",
            "variable=attacker",
        ),
    ] {
        let before = fs::read_to_string(&log).unwrap_or_default();
        let variable_name = setup.split('=').next().unwrap();
        let inspection = if name.contains("variable") {
            format!("printf 'variable=%s\\n' \"${{{variable_name}}}\"")
        } else if name.starts_with("_starship_set_return") {
            "printf 'function='; _starship_set_return; printf '\\n'".to_owned()
        } else if name.starts_with("starship_preexec_all") {
            "printf 'function='; starship_preexec_all; printf '\\n'".to_owned()
        } else if name.starts_with("starship_preexec_ps0") {
            "printf 'function='; starship_preexec_ps0; printf '\\n'".to_owned()
        } else if name.starts_with("starship_preexec ") {
            "printf 'function='; starship_preexec; printf '\\n'".to_owned()
        } else {
            "printf 'function='; starship_precmd; printf '\\n'".to_owned()
        };
        let command = format!(
            "PS1=native-prompt; {setup}; . \"$GASCAN_TEST_HOOK\"; \
             {inspection}; printf 'PS1=%s\\n' \"$PS1\""
        );
        let collision = run_hook(&hook, &command, &log, false, false, true);
        assert!(
            collision.status.success(),
            "{name} collision aborted startup: {}",
            String::from_utf8_lossy(&collision.stderr)
        );
        assert!(
            String::from_utf8_lossy(&collision.stdout).contains(expected),
            "{name}: {}",
            String::from_utf8_lossy(&collision.stdout)
        );
        assert!(
            String::from_utf8_lossy(&collision.stdout).contains("PS1=native-prompt"),
            "{name}: {}",
            String::from_utf8_lossy(&collision.stdout)
        );
        assert_eq!(
            String::from_utf8_lossy(&collision.stderr)
                .matches("gascan: Starship prompt unavailable; using standard Bash prompt.")
                .count(),
            1,
            "{name}: {}",
            String::from_utf8_lossy(&collision.stderr)
        );
        assert_eq!(
            fs::read_to_string(&log).unwrap_or_default(),
            before,
            "{name} collision reached Starship"
        );
    }

    let partial = run_hook(
        &hook,
        r#"PS1='native-prompt'; PS2='native-continuation'; PROMPT_COMMAND='native-command'; STARSHIP_CONFIG='old-config'; STARSHIP_EXECUTABLE='old-executable'; trap 'STARSHIP_NATIVE_TRAP=1' DEBUG; . "$GASCAN_TEST_HOOK"; declare -F starship_partial_leak >/dev/null && printf 'function=leaked\n'; if declare -p __gascan_starship_euid >/dev/null 2>&1; then helper=set; else helper=unset; fi; printf '%s|%s|%s|%s|%s|%s|%s|%s|%s|%s\n' "$PS1" "$PS2" "$PROMPT_COMMAND" "$STARSHIP_CONFIG" "$STARSHIP_EXECUTABLE" "${STARSHIP_PARTIAL-unset}" "$(trap -p DEBUG)" "$(declare -p STARSHIP_CONFIG)" "$(declare -p STARSHIP_EXECUTABLE)" "$helper""#,
        &log,
        false,
        true,
        true,
    );
    assert!(partial.status.success());
    assert_eq!(
        partial.stdout,
        b"native-prompt|native-continuation|native-command|old-config|old-executable|unset|trap -- 'STARSHIP_NATIVE_TRAP=1' DEBUG|declare -- STARSHIP_CONFIG=\"old-config\"|declare -- STARSHIP_EXECUTABLE=\"old-executable\"|unset\n",
        "stderr: {}",
        String::from_utf8_lossy(&partial.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&partial.stdout).contains("function=leaked"),
        "partial full init leaked a function"
    );

    fs::write(shell_dir.join("prompt"), "starship\nextra\n").unwrap();
    let malformed = run_hook(
        &hook,
        r#"PS1='native-prompt'; . "$GASCAN_TEST_HOOK"; printf '%s\n' "$PS1""#,
        &log,
        false,
        false,
        true,
    );
    assert!(malformed.status.success());
    assert_eq!(malformed.stdout, b"native-prompt\n");
    assert_eq!(
        String::from_utf8_lossy(&malformed.stderr)
            .matches("gascan: Starship prompt unavailable; using standard Bash prompt.")
            .count(),
        1
    );

    write_mode(&shell_dir.join("prompt"), "starship\n", 0o640);
    write_mode(
        &shell_dir.join("starship.toml"),
        "format = \"tampered\"\n",
        0o640,
    );
    let log_before = fs::read_to_string(&log).unwrap();
    let changed_config = run_hook(
        &hook,
        r#"PS1='native-prompt'; . "$GASCAN_TEST_HOOK"; if declare -p __gascan_starship_euid >/dev/null 2>&1; then helper=set; else helper=unset; fi; printf '%s|%s\n' "$PS1" "$helper""#,
        &log,
        false,
        false,
        true,
    );
    assert!(changed_config.status.success());
    assert_eq!(changed_config.stdout, b"native-prompt|unset\n");
    assert_eq!(
        String::from_utf8_lossy(&changed_config.stderr)
            .matches("gascan: Starship prompt unavailable; using standard Bash prompt.")
            .count(),
        1
    );
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        log_before,
        "hook invoked Starship with a config that differs from the immutable preset"
    );
}

#[test]
fn managed_starship_presets_have_exact_order_and_font_boundaries() {
    let compatible =
        fs::read_to_string(root().join("images/workspace/etc/gascan/starship.toml")).unwrap();
    let nerd =
        fs::read_to_string(root().join("images/workspace/etc/gascan/starship-nerd-font.toml"))
            .unwrap();
    let expected_order = concat!(
        "$username",
        "$hostname",
        "$directory",
        "$git_branch",
        "$git_status",
        "$elixir",
        "$erlang",
        "$golang",
        "$java",
        "$nodejs",
        "$python",
        "$ruby",
        "$rust",
        "$status",
        "$cmd_duration",
        "$line_break",
        "$character"
    );
    for (name, source) in [("compatible", &compatible), ("nerd", &nerd)] {
        let preset: toml::Value = toml::from_str(source).unwrap();
        assert_eq!(preset["format"].as_str(), Some(expected_order), "{name}");
        for module in [
            "username",
            "hostname",
            "directory",
            "git_branch",
            "git_status",
            "elixir",
            "erlang",
            "golang",
            "java",
            "nodejs",
            "python",
            "ruby",
            "rust",
            "status",
            "cmd_duration",
            "character",
        ] {
            assert!(preset.get(module).is_some(), "{name} omits {module}");
        }
    }
    assert!(
        !compatible
            .chars()
            .any(|c| ('\u{e000}'..='\u{f8ff}').contains(&c))
    );
    assert!(nerd.chars().any(|c| ('\u{e000}'..='\u{f8ff}').contains(&c)));
}

#[test]
fn reviewed_workstation_packages_require_no_extra_privileges() {
    let system_tools = fs::read_to_string(root().join("tests/image/system-tools.txt")).unwrap();
    let packages = system_tools
        .lines()
        .collect::<std::collections::BTreeSet<_>>();
    for forbidden_package in [
        "libcap2-bin",
        "nmap",
        "tcpdump",
        "tshark",
        "wireshark",
        "wireshark-common",
    ] {
        assert!(
            !packages.contains(forbidden_package),
            "unreviewed capability or packet-capture package: {forbidden_package}"
        );
    }

    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    for forbidden in [
        "CAP_NET_ADMIN",
        "CAP_NET_RAW",
        "--cap-add",
        "--device",
        "--privileged",
        "/dev/net/tun",
    ] {
        assert!(
            !dockerfile.contains(forbidden),
            "Dockerfile adds forbidden privilege or device access: {forbidden}"
        );
    }
}

#[test]
fn dockerfile_declares_workspace_user_init_and_persistent_layout() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let system_tools = fs::read_to_string(root().join("tests/image/system-tools.txt")).unwrap();
    for required in ["sudo", "tini"] {
        assert!(
            system_tools.lines().any(|package| package == required),
            "missing image package: {required}"
        );
    }
    for required in [
        "COPY --chmod=0440 images/workspace/etc/sudoers.d/workspace /etc/sudoers.d/workspace",
        "COPY --chmod=0555 images/workspace/bin/migrate-workspace-identity /usr/local/bin/migrate-workspace-identity",
        "COPY --chmod=0555 images/workspace/bin/initialize-rust-home /usr/local/bin/initialize-rust-home",
        "COPY --chmod=0555 images/workspace/libexec/migrate-workspace-identity-core /usr/local/libexec/gascan/migrate-workspace-identity-core",
        "/usr/local/bin/migrate-workspace-identity",
        "chown -R root:root /opt/gascan/mise",
        "chmod -R a-w /opt/gascan/mise",
        "/opt/gascan/mise",
        "/home/workspace/.cache",
        "/home/workspace/.local",
        "/home/workspace/.config",
        "visudo -cf /etc/sudoers.d/workspace",
        "USER workspace:workspace",
        "WORKDIR /workspace",
        "ENTRYPOINT [\"/usr/local/bin/gascan-entrypoint\"]",
        "VOLUME [\"/home/workspace/.local\", \"/home/workspace/.cache\", \"/home/workspace/.config\"]",
    ] {
        assert!(
            dockerfile.contains(required),
            "missing image contract: {required}"
        );
    }
    assert!(
        !dockerfile.contains(
            "ENTRYPOINT [\"/usr/bin/tini\", \"--\", \"/usr/local/bin/gascan-entrypoint\"]"
        ),
        "Apple --init must be the sole init boundary"
    );
}

#[test]
fn workstation_contract_audits_reported_writable_destinations() {
    let contract =
        fs::read_to_string(root().join("images/workspace/tests/workstation-contract.sh")).unwrap();
    for required in [
        "rustup show home",
        "npm config get prefix",
        "npm config get cache",
        "go env GOPATH",
        "go env GOBIN",
        "go env GOCACHE",
        "go env GOMODCACHE",
        "python -m site --user-base",
        "gem env home",
        "realpath -m",
        "nearest_existing_parent",
        "/home/workspace/.local",
        "/home/workspace/.cache",
        "/home/workspace/.config",
    ] {
        assert!(
            contract.contains(required),
            "workstation destination audit omits: {required}"
        );
    }
    for install_bin in [
        "/home/workspace/.local/bin",
        "/home/workspace/.local/share/cargo/bin",
        "/home/workspace/.local/share/go/bin",
        "/home/workspace/.local/share/gem/bin",
    ] {
        assert!(
            contract.contains(install_bin),
            "PATH audit omits install bin: {install_bin}"
        );
    }
}

#[test]
fn identity_migration_is_exact_and_fail_closed() {
    let wrapper =
        fs::read_to_string(root().join("images/workspace/bin/migrate-workspace-identity")).unwrap();
    let migration =
        fs::read_to_string(root().join("images/workspace/libexec/migrate-workspace-identity-core"))
            .unwrap();

    assert!(wrapper.contains("/etc/passwd /etc/group /home"));
    assert!(wrapper.contains("/usr/sbin/usermod /usr/sbin/groupmod /usr/bin/stat"));

    for required in [
        "ubuntu:x:1000:1000:Ubuntu:$old_home:/bin/bash",
        "ubuntu:x:1000:",
        "--login workspace --home \"$new_home\" --move-home ubuntu",
        "--new-name workspace ubuntu",
        "workspace:x:1000:1000:Ubuntu:$new_home:/bin/bash",
        "workspace:x:1000:",
        "test ! -e \"$old_home\"",
    ] {
        assert!(
            migration.contains(required),
            "missing exact identity contract: {required}"
        );
    }
    for forbidden in ["--non-unique", "userdel", "groupdel", "useradd", "groupadd"] {
        assert!(
            !migration.contains(forbidden),
            "unsafe identity migration: {forbidden}"
        );
    }
}

#[test]
fn identity_migration_executes_exact_transition_and_rejects_before_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let bin = temp.path().join("bin");
    fs::create_dir_all(home.join("ubuntu")).unwrap();
    fs::create_dir(&bin).unwrap();
    let passwd = temp.path().join("passwd");
    let group = temp.path().join("group");
    fs::write(
        &passwd,
        format!(
            "ubuntu:x:1000:1000:Ubuntu:{}/ubuntu:/bin/bash\n",
            home.display()
        ),
    )
    .unwrap();
    fs::write(&group, "ubuntu:x:1000:\n").unwrap();
    let calls = temp.path().join("calls");
    let fake = |name: &str, body: &str| {
        let path = bin.join(name);
        fs::write(&path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    };
    fake("stat", "#!/bin/sh\nprintf 'directory:1000:1000\n'\n");
    fake(
        "usermod",
        "#!/bin/sh\nprintf 'usermod\n' >>\"$CALLS\"\ntest \"${BAD_POST:-0}\" = 0 || exit 0\nsed 's/^ubuntu:/workspace:/; s#/ubuntu:/bin/bash#/workspace:/bin/bash#' \"$PASSWD\" >\"$PASSWD.new\"\nmv \"$PASSWD.new\" \"$PASSWD\"\nmv \"$HOME_ROOT/ubuntu\" \"$HOME_ROOT/workspace\"\n",
    );
    fake(
        "groupmod",
        "#!/bin/sh\nprintf 'groupmod\n' >>\"$CALLS\"\ntest \"${BAD_POST:-0}\" = 0 || exit 0\nsed 's/^ubuntu:/workspace:/' \"$GROUP\" >\"$GROUP.new\"\nmv \"$GROUP.new\" \"$GROUP\"\n",
    );

    let run = || {
        Command::new("bash")
            .arg(root().join("images/workspace/libexec/migrate-workspace-identity-core"))
            .args([
                &passwd,
                &group,
                &home,
                &bin.join("usermod"),
                &bin.join("groupmod"),
                &bin.join("stat"),
            ])
            .env("CALLS", &calls)
            .env("PASSWD", &passwd)
            .env("GROUP", &group)
            .env("HOME_ROOT", &home)
            .status()
            .unwrap()
    };
    let bad_post = Command::new("bash")
        .arg(root().join("images/workspace/libexec/migrate-workspace-identity-core"))
        .args([
            &passwd,
            &group,
            &home,
            &bin.join("usermod"),
            &bin.join("groupmod"),
            &bin.join("stat"),
        ])
        .env("CALLS", &calls)
        .env("PASSWD", &passwd)
        .env("GROUP", &group)
        .env("HOME_ROOT", &home)
        .env("BAD_POST", "1")
        .status()
        .unwrap();
    assert!(!bad_post.success(), "invalid post-mutation state passed");
    assert_eq!(fs::read_to_string(&calls).unwrap(), "usermod\ngroupmod\n");
    fs::remove_file(&calls).unwrap();

    assert!(run().success());
    assert_eq!(fs::read_to_string(&calls).unwrap(), "usermod\ngroupmod\n");

    fs::remove_file(&calls).unwrap();
    fs::write(
        &passwd,
        format!(
            "ubuntu:x:1001:1000:Ubuntu:{}/ubuntu:/bin/bash\n",
            home.display()
        ),
    )
    .unwrap();
    assert!(!run().success());
    assert!(!calls.exists(), "prevalidation failure invoked mutation");
}

#[test]
fn identity_migration_prevalidation_rejects_unsafe_fixtures_without_mutation() {
    use std::os::unix::fs::symlink;

    for case in [
        "passwd-fields",
        "group-fields",
        "duplicate-uid",
        "duplicate-gid",
        "workspace-user",
        "workspace-group",
        "missing-home",
        "symlink-home",
        "file-home",
        "wrong-owner",
        "destination-exists",
        "destination-link",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let bin = temp.path().join("bin");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir(&bin).unwrap();
        let passwd = temp.path().join("passwd");
        let group = temp.path().join("group");
        let mut passwd_text = format!(
            "ubuntu:x:1000:1000:Ubuntu:{}/ubuntu:/bin/bash\n",
            home.display()
        );
        let mut group_text = "ubuntu:x:1000:\n".to_string();
        match case {
            "passwd-fields" => passwd_text = passwd_text.replace("Ubuntu:", "Wrong:"),
            "group-fields" => group_text = "ubuntu:x:1000:member\n".into(),
            "duplicate-uid" => passwd_text.push_str("alias:x:1000:2000::/tmp:/bin/false\n"),
            "duplicate-gid" => group_text.push_str("alias:x:1000:\n"),
            "workspace-user" => passwd_text.push_str("workspace:x:2000:2000::/tmp:/bin/false\n"),
            "workspace-group" => group_text.push_str("workspace:x:2000:\n"),
            _ => {}
        }
        fs::write(&passwd, passwd_text).unwrap();
        fs::write(&group, group_text).unwrap();
        match case {
            "missing-home" => {}
            "symlink-home" => symlink(temp.path(), home.join("ubuntu")).unwrap(),
            "file-home" => fs::write(home.join("ubuntu"), "not a directory").unwrap(),
            _ => fs::create_dir(home.join("ubuntu")).unwrap(),
        }
        match case {
            "destination-exists" => fs::create_dir(home.join("workspace")).unwrap(),
            "destination-link" => symlink(temp.path(), home.join("workspace")).unwrap(),
            _ => {}
        }
        let calls = temp.path().join("calls");
        for command in ["usermod", "groupmod"] {
            let path = bin.join(command);
            fs::write(&path, "#!/bin/sh\ntouch \"$CALLS\"\nexit 99\n").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let stat = bin.join("stat");
        let value = if case == "wrong-owner" {
            "directory:501:20"
        } else {
            "directory:1000:1000"
        };
        fs::write(&stat, format!("#!/bin/sh\nprintf '%s\\n' '{value}'\n")).unwrap();
        fs::set_permissions(&stat, fs::Permissions::from_mode(0o755)).unwrap();
        let status = Command::new("bash")
            .arg(root().join("images/workspace/libexec/migrate-workspace-identity-core"))
            .args([
                &passwd,
                &group,
                &home,
                &bin.join("usermod"),
                &bin.join("groupmod"),
                &stat,
            ])
            .env("CALLS", &calls)
            .status()
            .unwrap();
        assert!(!status.success(), "unsafe fixture passed: {case}");
        assert!(!calls.exists(), "unsafe fixture mutated identity: {case}");
    }
}

#[test]
fn sudoers_and_entrypoint_are_exact_and_non_bootstrapping() {
    let sudoers = root().join("images/workspace/etc/sudoers.d/workspace");
    assert_eq!(
        fs::read_to_string(&sudoers).unwrap(),
        "workspace ALL=(ALL:ALL) NOPASSWD: ALL\n"
    );

    let entrypoint =
        fs::read_to_string(root().join("images/workspace/bin/gascan-entrypoint")).unwrap();
    assert!(entrypoint.contains("exec \"$@\""));
    assert!(entrypoint.contains("exec sleep infinity"));
    for forbidden in [
        "curl",
        "wget",
        "http://",
        "https://",
        "mise install",
        "git clone",
    ] {
        assert!(
            !entrypoint.contains(forbidden),
            "entrypoint contains bootstrap behavior: {forbidden}"
        );
    }
}

#[test]
fn ssh_entrypoint_preserves_commands_and_has_fail_closed_default_dispatch() {
    let entrypoint_path = root().join("images/workspace/bin/gascan-entrypoint");
    let temporary = tempfile::tempdir().unwrap();
    let marker = temporary.path().join("marker");
    let command = temporary.path().join("explicit-command");
    fs::write(
        &command,
        "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" >\"$MARKER\"\n",
    )
    .unwrap();
    fs::set_permissions(&command, fs::Permissions::from_mode(0o755)).unwrap();
    let status = Command::new(&entrypoint_path)
        .arg(&command)
        .args(["one", "two words"])
        .env("MARKER", &marker)
        .env("GASCAN_SSH_ENABLED", "1")
        .env("GASCAN_SSH_AUTHORIZED_KEY", "not-a-key")
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(fs::read_to_string(&marker).unwrap(), "one two words\n");

    let sleep = temporary.path().join("sleep");
    fs::write(
        &sleep,
        "#!/bin/sh\nset -eu\nprintf 'sleep:%s\\n' \"$*\" >\"$MARKER\"\n",
    )
    .unwrap();
    fs::set_permissions(&sleep, fs::Permissions::from_mode(0o755)).unwrap();
    let disabled = Command::new(&entrypoint_path)
        .env("MARKER", &marker)
        .env("PATH", temporary.path())
        .env("GASCAN_SSH_ENABLED", "0")
        .status()
        .unwrap();
    assert!(disabled.success());
    assert_eq!(fs::read_to_string(&marker).unwrap(), "sleep:infinity\n");

    let invalid = Command::new(&entrypoint_path)
        .env("GASCAN_SSH_ENABLED", "yes")
        .status()
        .unwrap();
    assert!(!invalid.success());

    let entrypoint = fs::read_to_string(&entrypoint_path).unwrap();
    assert!(
        entrypoint.find("exec \"$@\"").unwrap() < entrypoint.find("GASCAN_SSH_ENABLED").unwrap(),
        "explicit image commands must dispatch before managed SSH startup"
    );
    for required in [
        "GASCAN_SSH_ENABLED:-0",
        "0)",
        "1)",
        "--preserve-env=GASCAN_SSH_AUTHORIZED_KEY",
        "/usr/local/bin/start-gascan-sshd",
        "exec sleep infinity",
    ] {
        assert!(
            entrypoint.contains(required),
            "missing SSH dispatch: {required}"
        );
    }
}

#[test]
fn smoke_fixture_uses_built_ref_and_checks_signal_and_zombies() {
    let smoke = fs::read_to_string(root().join("tests/image/user-and-volumes.sh")).unwrap();
    for required in [
        ".artifacts/workspace-image-ref",
        "\"$container_bin\" create",
        "--init",
        "--label dev.gascan.test=true",
        "dev.gascan.test.owner=$owner_token",
        "--mount \"type=bind,source=$root,target=/workspace\"",
        "--bin validate-owned-container",
        "\"$container_bin\" start",
        "\"$container_bin\" exec",
        "/proc/[0-9]*/status",
        "bounded_container stop --time 5",
        "test \"$elapsed\" -le 5",
    ] {
        assert!(
            smoke.contains(required),
            "missing live smoke contract: {required}"
        );
    }
    assert_eq!(smoke.matches("--mount ").count(), 1);
    assert!(!smoke.contains("container run"));
}

#[test]
fn smoke_fixture_distinguishes_immutable_and_writable_runtime_homes() {
    let smoke = fs::read_to_string(root().join("tests/image/user-and-volumes.sh")).unwrap();
    for required in [
        r#"test "$(stat -c %U:%G "$immutable")" = root:root"#,
        r#"test ! -w "$immutable""#,
        r#"test "$(stat -c %U:%G "$directory")" = workspace:workspace"#,
        r#"test "$(stat -c %U:%G "$directory")" = root:workspace"#,
        r#"test -w "$directory""#,
    ] {
        assert!(
            smoke.contains(required),
            "missing runtime-home ownership contract: {required}"
        );
    }
}

#[test]
fn smoke_fixture_restarts_and_rechecks_the_process_contract_after_stop() {
    let smoke = fs::read_to_string(root().join("tests/image/user-and-volumes.sh")).unwrap();
    assert_eq!(
        smoke.matches("\"$container_bin\" start \"$name\"").count(),
        2,
        "live smoke must exercise initial start and restart"
    );
    assert_eq!(
        smoke
            .matches(
                "\"$container_bin\" exec \"$name\" bash /workspace/tests/image/user-and-volumes.sh --inside",
            )
            .count(),
        2,
        "live smoke must recheck identity, signal, and zombie contracts after restart"
    );
}

#[test]
fn every_live_image_smoke_models_the_runtime_init_boundary() {
    for fixture in [
        "user-and-volumes.sh",
        "polyglot-smoke.sh",
        "gascamp-smoke.sh",
        "workstation-smoke.sh",
    ] {
        let smoke = fs::read_to_string(root().join("tests/image").join(fixture)).unwrap();
        assert!(
            smoke.contains("--init"),
            "{fixture} does not model the production Apple runtime init boundary"
        );
    }
}

#[test]
fn gascamp_smoke_fails_closed_without_a_built_image_reference() {
    let missing = root().join(".artifacts/definitely-missing-gascamp-image-ref");
    let output = Command::new("bash")
        .arg(root().join("tests/image/gascamp-smoke.sh"))
        .env("GASCAN_IMAGE_REF_FILE", &missing)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!("missing Gascamp image reference: {}\n", missing.display())
    );
}
