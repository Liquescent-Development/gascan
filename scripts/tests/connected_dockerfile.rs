use std::{
    collections::BTreeSet, fs, os::unix::fs::PermissionsExt, os::unix::fs::symlink, path::Path,
    process::Command,
};

use sha2::{Digest, Sha256};

const MISE_LS_FILTER: &str = r#"if ((keys|sort) != ["elixir","erlang","go","java","node","python","ruby","rust"]) then error("unexpected mise tool set") else to_entries | map(if ((.value|type)!="array") or ((.value|length)!=1) or (.value[0].installed != true) or (.value[0].active != true) or ((.value[0].version|type)!="string") or (.value[0].version=="") then error("invalid mise ls record") else {key:.key,value:.value[0].version} end) | from_entries end"#;
const EXPECTED_SYSTEM_TOOLS: &str = "\
autoconf
bind9-dnsutils
bison
build-essential
ca-certificates
curl
emacs-nox
fd-find
file
fonts-liberation
fzf
gh
git
iproute2
iputils-ping
jq
less
libasound2t64
libatk-bridge2.0-0
libatk1.0-0
libcups2
libdbus-1-3
libdrm2
libffi-dev
libgbm1
libgdbm-dev
libglib2.0-0t64
libgtk-3-0t64
libncurses-dev
libnspr4
libnss3
libpango-1.0-0
libreadline-dev
libssl-dev
libx11-6
libxcb1
libxcomposite1
libxdamage1
libxext6
libxfixes3
libxkbcommon0
libxrandr2
libyaml-dev
lsof
nano
net-tools
netcat-openbsd
openssh-client
openssh-server
patch
pkg-config
procps
psmisc
python3
ripgrep
rsync
sudo
tini
tmux
traceroute
tree
unzip
vim
wget
xz-utils
zlib1g-dev
zstd
";

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

#[test]
fn dockerfile_assembles_workstation_only_from_the_verified_context() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let installer =
        fs::read_to_string(root().join("images/workspace/bin/install-workstation-artifacts"))
            .unwrap();
    for required in [
        "COPY --chmod=0555 images/workspace/bin/install-workstation-artifacts /usr/local/bin/install-workstation-artifacts",
        "COPY workstation /tmp/workstation",
        "RUN --network=none test -x /opt/gascan/gascamp/bin/camp",
        "/usr/local/bin/install-workstation-artifacts",
        "/opt/gascan/workstation",
        "chown -R root:root /opt/gascan/workstation",
        "find /opt/gascan/workstation ! -type l \\( -perm -020 -o -perm -002 \\) -print -quit",
        "find /opt/gascan/workstation \\( ! -user root -o ! -group root \\) -print -quit",
    ] {
        assert!(
            dockerfile.contains(required),
            "missing offline workstation assembly contract: {required}"
        );
    }
    for required in [
        "\"ci\"",
        "\"--offline\"",
        "\"--ignore-scripts\"",
        "\"npm_config_logs_dir\": str(npm_root / \".home\" / \"logs\")",
        "verify_npm_inventory(npm_root, source / \"package-lock.json\", target_lock_path)",
        "checked_link(command_dir / \"fd\", Path(\"/usr/bin/fdfind\"))",
        "checked_link(command_dir / \"pico\", Path(\"/usr/bin/nano\"))",
    ] {
        assert!(
            installer.contains(required),
            "installer omits required npm argument: {required}"
        );
    }
    let workstation_assembly = dockerfile
        .split_once("COPY workstation /tmp/workstation")
        .unwrap()
        .1;
    for forbidden in [
        "curl https://",
        "wget https://",
        "npm install",
        "npm view",
        "registry.npmjs",
        "github.com/",
        "gitlab.com/",
    ] {
        assert!(
            !workstation_assembly.contains(forbidden) && !installer.contains(forbidden),
            "network-capable workstation assembly path: {forbidden}"
        );
    }
}

#[test]
fn workstation_step_diagnostics_name_only_the_failing_boundary() {
    let wrapper = root().join("images/workspace/bin/run-workstation-step");
    let output = Command::new(&wrapper)
        .args([
            "immutable-owner",
            "sh",
            "-c",
            "printf 'private-command-output\\n' >&2; exit 7",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "private-command-output\nworkstation assembly: immutable-owner failed\n"
    );

    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    for boundary in [
        "installer",
        "immutable-owner",
        "immutable-mode",
        "immutable-ownership",
        "home-directories",
        "home-configuration",
        "home-owner",
        "home-link-owner",
        "sudoers-mode",
        "sudoers-validation",
        "chromium-mode",
        "chromium-command",
        "chromium-seal",
        "temporary-cleanup",
    ] {
        assert!(
            dockerfile.contains(&format!("run-workstation-step {boundary} ")),
            "missing named workstation boundary {boundary}"
        );
    }
    assert!(dockerfile.contains("workstation assembly: installer complete"));
}

#[test]
fn immutable_mode_check_excludes_links_but_rejects_writable_content() {
    let temporary = tempfile::tempdir().unwrap();
    let tree = temporary.path().join("immutable");
    fs::create_dir(&tree).unwrap();
    let target = tree.join("target");
    fs::write(&target, b"locked\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o444)).unwrap();
    std::os::unix::fs::symlink("target", tree.join("command")).unwrap();
    let check = r#"test -z "$(find "$1" ! -type l \( -perm -020 -o -perm -002 \) -print -quit)""#;
    assert!(
        Command::new("sh")
            .args(["-c", check, "sh", tree.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );
    fs::set_permissions(&target, fs::Permissions::from_mode(0o464)).unwrap();
    assert!(
        !Command::new("sh")
            .args(["-c", check, "sh", tree.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );

    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    assert!(dockerfile.contains(
        r#"find /opt/gascan/workstation ! -type l \( -perm -020 -o -perm -002 \) -print -quit"#
    ));
}

#[test]
fn workstation_installer_rejects_unsafe_archives_behaviorally() {
    let installer = root().join("images/workspace/bin/install-workstation-artifacts");
    let temp = tempfile::tempdir().unwrap();
    let make_archive = |name: &str, python_body: &str| {
        let archive = temp.path().join(name);
        let status = Command::new("python3")
            .args(["-c", python_body])
            .arg(&archive)
            .status()
            .unwrap();
        assert!(status.success());
        archive
    };

    let safe = make_archive(
        "safe.tar.gz",
        "import io,sys,tarfile\np=sys.argv[1]\nwith tarfile.open(p,'w:gz') as t:\n i=tarfile.TarInfo('tree/bin/tool'); i.mode=0o755; d=b'ok'; i.size=len(d); t.addfile(i,io.BytesIO(d))",
    );
    assert!(
        Command::new(&installer)
            .args(["validate-tar", safe.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );

    for (name, body) in [
        (
            "traversal.tar.gz",
            "import io,sys,tarfile\nwith tarfile.open(sys.argv[1],'w:gz') as t:\n i=tarfile.TarInfo('../escape'); d=b'x'; i.size=1; t.addfile(i,io.BytesIO(d))",
        ),
        (
            "absolute.tar.gz",
            "import io,sys,tarfile\nwith tarfile.open(sys.argv[1],'w:gz') as t:\n i=tarfile.TarInfo('/escape'); d=b'x'; i.size=1; t.addfile(i,io.BytesIO(d))",
        ),
        (
            "device.tar.gz",
            "import sys,tarfile\nwith tarfile.open(sys.argv[1],'w:gz') as t:\n i=tarfile.TarInfo('dev'); i.type=tarfile.CHRTYPE; t.addfile(i)",
        ),
        (
            "escaping-link.tar.gz",
            "import sys,tarfile\nwith tarfile.open(sys.argv[1],'w:gz') as t:\n i=tarfile.TarInfo('tree/link'); i.type=tarfile.SYMTYPE; i.linkname='../../escape'; t.addfile(i)",
        ),
    ] {
        let archive = make_archive(name, body);
        assert!(
            !Command::new(&installer)
                .args(["validate-tar", archive.to_str().unwrap()])
                .status()
                .unwrap()
                .success(),
            "unsafe archive passed: {name}"
        );
    }
}

#[test]
fn workstation_installer_enforces_exact_target_npm_inventory_behaviorally() {
    let installer = root().join("images/workspace/bin/install-workstation-artifacts");
    let temporary = tempfile::tempdir_in("/tmp").unwrap();
    let npm_root = temporary.path().join("npm");
    let alpha = npm_root.join("node_modules/alpha");
    fs::create_dir_all(&alpha).unwrap();
    fs::write(
        alpha.join("package.json"),
        "{\"name\":\"alpha\",\"version\":\"1.0.0\"}\n",
    )
    .unwrap();
    let package_lock = temporary.path().join("package-lock.json");
    fs::write(
        &package_lock,
        "{\"lockfileVersion\":3,\"packages\":{\"\":{},\"node_modules/alpha\":{\"name\":\"alpha\",\"version\":\"1.0.0\"},\"node_modules/excluded\":{\"name\":\"excluded\",\"version\":\"1.0.0\"}}}\n",
    )
    .unwrap();
    let target_lock = temporary.path().join("target-lock.toml");
    fs::write(
        &target_lock,
        "schema_version = 1\nnpm_version = \"11.12.1\"\nos = \"linux\"\ncpu = \"arm64\"\nlibc = \"glibc\"\nrecord_count = 1\nexcluded_paths = [\"node_modules/excluded\"]\n",
    )
    .unwrap();
    let verify = || {
        Command::new(&installer)
            .args([
                "verify-inventory",
                npm_root.to_str().unwrap(),
                package_lock.to_str().unwrap(),
                target_lock.to_str().unwrap(),
            ])
            .status()
            .unwrap()
    };
    assert!(verify().success(), "exact target inventory was rejected");

    let extra = npm_root.join("node_modules/extra");
    fs::create_dir(&extra).unwrap();
    fs::write(
        extra.join("package.json"),
        "{\"name\":\"extra\",\"version\":\"1.0.0\"}\n",
    )
    .unwrap();
    assert!(!verify().success(), "extra package was accepted");
    fs::remove_dir_all(extra).unwrap();

    fs::write(
        alpha.join("package.json"),
        "{\"name\":\"alpha\",\"version\":\"2.0.0\"}\n",
    )
    .unwrap();
    assert!(!verify().success(), "wrong package identity was accepted");
    fs::remove_file(alpha.join("package.json")).unwrap();
    assert!(!verify().success(), "missing package manifest was accepted");

    fs::remove_dir_all(&alpha).unwrap();
    symlink(temporary.path(), &alpha).unwrap();
    assert!(!verify().success(), "symlink package root was accepted");
}

#[test]
fn workstation_installer_enforces_file_npm_version_and_mode_boundaries_behaviorally() {
    let installer = root().join("images/workspace/bin/install-workstation-artifacts");
    let temp = tempfile::tempdir().unwrap();

    let elf = temp.path().join("tool");
    let mut elf_bytes = vec![0_u8; 20];
    elf_bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
    elf_bytes[18..20].copy_from_slice(&183_u16.to_le_bytes());
    fs::write(&elf, &elf_bytes).unwrap();
    let sha = format!("{:x}", Sha256::digest(&elf_bytes));
    let validate = |size: &str, digest: &str, path: &Path| {
        Command::new(&installer)
            .args([
                "validate-file",
                path.to_str().unwrap(),
                size,
                digest,
                "arm64-elf",
            ])
            .status()
            .unwrap()
    };
    assert!(validate("20", &sha, &elf).success());
    assert!(!validate("19", &sha, &elf).success());
    assert!(!validate("20", &"0".repeat(64), &elf).success());
    fs::write(&elf, vec![0_u8; 20]).unwrap();
    let non_elf_sha = format!("{:x}", Sha256::digest(vec![0_u8; 20]));
    assert!(!validate("20", &non_elf_sha, &elf).success());

    let source = temp.path().join("npm-source");
    let destination = temp.path().join("npm-destination");
    let fake_bin = temp.path().join("fake-bin");
    fs::create_dir(&source).unwrap();
    fs::create_dir(source.join("npm-cache")).unwrap();
    fs::create_dir(&fake_bin).unwrap();
    fs::write(source.join("package.json"), "{}\n").unwrap();
    fs::write(source.join("package-lock.json"), "{}\n").unwrap();
    let bootstrap = temp.path().join("npm-cli.tgz");
    let bootstrap_status = Command::new("python3")
        .args([
            "-c",
            "import io,json,sys,tarfile\nwith tarfile.open(sys.argv[1],'w:gz') as t:\n files={'package/package.json':json.dumps({'name':'npm','version':'11.12.1','bin':{'npm':'bin/npm-cli.js','npx':'bin/npx-cli.js'}}).encode(),'package/bin/npm-cli.js':b'require(\"../lib/cli.js\");\\n','package/lib/cli.js':b''}\n for name,data in files.items():\n  i=tarfile.TarInfo(name); i.mode=0o755 if name.endswith('.js') else 0o644; i.size=len(data); t.addfile(i,io.BytesIO(data))",
        ])
        .arg(&bootstrap)
        .status()
        .unwrap();
    assert!(bootstrap_status.success());
    let args_file = temp.path().join("npm-args");
    let fake_node = fake_bin.join("node");
    fs::write(
        &fake_node,
        "#!/bin/sh\nprintf '%s\\n' CALL \"$@\" >>\"$NPM_ARGS\"\nif [ \"$1\" = --version ]; then printf '%s\\n' \"$FAKE_NODE_VERSION\"; exit 0; fi\nif [ \"$2\" = --version ]; then printf '%s\\n' \"$FAKE_NPM_VERSION\"; exit 0; fi\nmkdir -p node_modules\n",
    )
    .unwrap();
    fs::set_permissions(&fake_node, fs::Permissions::from_mode(0o755)).unwrap();
    let status = Command::new(&installer)
        .args([
            "npm-ci",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
            bootstrap.to_str().unwrap(),
            "11.12.1",
            "24.18.0",
        ])
        .env(
            "PATH",
            format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("NPM_ARGS", &args_file)
        .env("FAKE_NPM_VERSION", "11.12.1")
        .env("FAKE_NODE_VERSION", "v24.18.0")
        .status()
        .unwrap();
    assert!(status.success());
    let calls = fs::read_to_string(&args_file).unwrap();
    assert!(calls.starts_with("CALL\n--version\nCALL\n"));
    assert!(calls.contains("/package/bin/npm-cli.js\n--version\nCALL\n"));
    assert!(
        calls.ends_with(&format!(
            "/package/bin/npm-cli.js\nci\n--offline\n--ignore-scripts\n--cache\n{}\n",
            source.join("npm-cache").display()
        )),
        "{calls}"
    );
    fs::write(&args_file, "").unwrap();
    let mismatch_destination = temp.path().join("npm-mismatch");
    let mismatch = Command::new(&installer)
        .args([
            "npm-ci",
            source.to_str().unwrap(),
            mismatch_destination.to_str().unwrap(),
            bootstrap.to_str().unwrap(),
            "11.12.1",
            "24.18.0",
        ])
        .env(
            "PATH",
            format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("NPM_ARGS", &args_file)
        .env("FAKE_NPM_VERSION", "11.12.2")
        .env("FAKE_NODE_VERSION", "v24.18.0")
        .status()
        .unwrap();
    assert!(!mismatch.success(), "accepted the wrong npm version");
    assert!(!mismatch_destination.exists());
    let calls = fs::read_to_string(&args_file).unwrap();
    assert!(calls.starts_with("CALL\n--version\nCALL\n"));
    assert!(calls.ends_with("/package/bin/npm-cli.js\n--version\n"));

    let version = temp.path().join("version-tool");
    fs::write(&version, "#!/bin/sh\nexit 99\n").unwrap();
    fs::set_permissions(&version, fs::Permissions::from_mode(0o755)).unwrap();
    let version_status = |tool: &str, expected: &str, output: &str| {
        fs::write(
            &version,
            format!(
                "#!/bin/sh\nprintf '%s' '{}'\n",
                output.replace('\'', "'\\''")
            ),
        )
        .unwrap();
        Command::new(&installer)
            .args(["verify-version", version.to_str().unwrap(), tool, expected])
            .status()
            .unwrap()
    };
    fs::write(
        &version,
        "#!/bin/sh\n\
         set -eu\n\
         test -d \"$HOME\"\n\
         test \"$HOME\" != /tmp\n\
         printf '%s\\n' 'codex-cli 0.145.0'\n",
    )
    .unwrap();
    assert!(
        Command::new(&installer)
            .args([
                "verify-version",
                version.to_str().unwrap(),
                "codex",
                "0.145.0",
            ])
            .status()
            .unwrap()
            .success(),
        "version verification did not provide a safe private home"
    );
    for (tool, expected, output) in [
        ("claude", "2.1.218", "2.1.218 (Claude Code)\n"),
        ("codex", "0.145.0", "codex-cli 0.145.0\n"),
        ("pi", "0.81.1", "0.81.1\n"),
        ("herdr", "0.7.5", "herdr 0.7.5\n"),
        ("glab", "1.109.0", "glab 1.109.0 (abcdef)\n"),
        (
            "nvim",
            "0.11.7",
            "NVIM v0.11.7\nBuild type: Release\nLuaJIT 2.1\n",
        ),
    ] {
        assert!(
            version_status(tool, expected, output).success(),
            "real-shaped {tool} output was rejected"
        );
    }
    for (tool, expected, output) in [
        ("pi", "0.81.1", "pi 0.81.1\n"),
        ("pi", "0.81.1", "0.81.1beta\n"),
        ("codex", "0.145.0", "codex-cli 0.145.0-rc1\n"),
        ("claude", "2.1.218", "2.1.218+other (Claude Code)\n"),
        ("herdr", "0.7.5", "other 0.7.5\n"),
        ("glab", "1.109.0", "glab 1.109.0beta (abcdef)\n"),
        ("nvim", "0.11.7", "NVIM v0.11.70\n"),
    ] {
        assert!(
            !version_status(tool, expected, output).success(),
            "spoofed/suffixed {tool} output was accepted: {output:?}"
        );
    }

    let tree = temp.path().join("tree");
    fs::create_dir(&tree).unwrap();
    fs::write(tree.join("metadata"), "locked").unwrap();
    fs::write(tree.join("executable"), "#!/bin/sh\n").unwrap();
    fs::set_permissions(tree.join("metadata"), fs::Permissions::from_mode(0o666)).unwrap();
    fs::set_permissions(tree.join("executable"), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(
        Command::new(&installer)
            .args(["seal-tree", tree.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        fs::metadata(&tree).unwrap().permissions().mode() & 0o777,
        0o555
    );
    assert_eq!(
        fs::metadata(tree.join("metadata"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o444
    );
    assert_eq!(
        fs::metadata(tree.join("executable"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o555
    );

    let escaping_tree = temp.path().join("escaping-tree");
    fs::create_dir(&escaping_tree).unwrap();
    symlink(temp.path(), escaping_tree.join("escape")).unwrap();
    assert!(
        !Command::new(&installer)
            .args(["seal-tree", escaping_tree.to_str().unwrap()])
            .status()
            .unwrap()
            .success(),
        "immutable tree accepted an escaping symlink"
    );
}

fn assert_exact_system_tools(package_text: &str) -> Result<(), &'static str> {
    if package_text != EXPECTED_SYSTEM_TOOLS {
        return Err("reviewed Ubuntu root package set changed");
    }
    Ok(())
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
    assert_exact_system_tools(&package_text).unwrap();
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
fn exact_system_tool_contract_rejects_addition_removal_and_substitution() {
    let exact = fs::read_to_string(root().join("tests/image/system-tools.txt")).unwrap();
    assert_exact_system_tools(&(exact.clone() + "unreviewed-extra\n")).unwrap_err();
    assert_exact_system_tools(&exact.replacen("bind9-dnsutils\n", "", 1)).unwrap_err();
    assert_exact_system_tools(&exact.replacen("nano\n", "nano-tiny\n", 1)).unwrap_err();
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
