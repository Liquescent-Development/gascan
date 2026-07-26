use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tests/image/workstation-state-persistence.sh")
}

fn write_fake_tools(directory: &Path) {
    fs::create_dir_all(directory).unwrap();
    let tool = directory.join("tool");
    if tool.exists() {
        return;
    }
    fs::write(
        &tool,
        r#"#!/bin/sh
set -eu
printf '%s %s offline=%s telemetry=%s\n' \
    "${0##*/}" "$*" "${PI_OFFLINE:-}" "${PI_TELEMETRY:-}" >>"$GASCAN_INVOCATIONS"
if test "${0##*/}" = pi && test "$1" = --offline; then shift; fi
case "${0##*/}:$1" in
    claude:doctor)
        mkdir -p "$CLAUDE_CONFIG_DIR"
        printf '{"native":"claude"}\n' >"$CLAUDE_CONFIG_DIR/.claude.json"
        ;;
    codex:mcp)
        case "$2" in
            remove) ;;
            add)
                mkdir -p "$CODEX_HOME"
                printf '[mcp_servers.gascan-persistence]\ncommand = "/bin/true"\n' >"$CODEX_HOME/config.toml"
                ;;
            get) printf 'gascan-persistence\n' ;;
        esac
        ;;
    pi:install)
        mkdir -p "$PI_CODING_AGENT_DIR"
        printf '{"packages":["gascan-pi-extension.js"]}\n' >"$PI_CODING_AGENT_DIR/settings.json"
        ;;
    pi:list) printf 'gascan-pi-extension.js\n' ;;
    herdr:--help) printf 'HERDR_CONFIG_PATH=%s\n' "$HERDR_CONFIG_PATH" ;;
    gh:config)
        mkdir -p "$GH_CONFIG_DIR"
        if test "$2" = set; then printf 'editor: vim\n' >"$GH_CONFIG_DIR/config.yml"; else printf 'vim\n'; fi
        ;;
    glab:config)
        mkdir -p "$GLAB_CONFIG_DIR"
        if test "$2" = set; then printf 'editor: vim\n' >"$GLAB_CONFIG_DIR/config.yml"; else printf 'vim\n'; fi
        ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    for name in ["claude", "codex", "pi", "herdr", "gh", "glab"] {
        std::os::unix::fs::symlink(&tool, directory.join(name)).unwrap();
    }
    let timeout = directory.join("timeout");
    fs::write(&timeout, "#!/bin/sh\nshift\nexec \"$@\"\n").unwrap();
    fs::set_permissions(&timeout, fs::Permissions::from_mode(0o755)).unwrap();
}

fn invoke(temp: &Path, mode: &str) -> Output {
    let bin = temp.join("bin");
    write_fake_tools(&bin);
    let config = temp.join("config");
    let cache = temp.join("cache");
    Command::new(script())
        .arg(mode)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("GASCAN_CONFIG_ROOT", &config)
        .env("GASCAN_CACHE_ROOT", &cache)
        .env("CLAUDE_CONFIG_DIR", config.join("agents/claude"))
        .env("CODEX_HOME", config.join("agents/codex"))
        .env("PI_CODING_AGENT_DIR", config.join("agents/pi"))
        .env("HERDR_CONFIG_PATH", config.join("herdr/config.toml"))
        .env("GH_CONFIG_DIR", config.join("gh"))
        .env("GLAB_CONFIG_DIR", config.join("glab"))
        .env("GASCAN_PI_EXTENSION", temp.join("gascan-pi-extension.js"))
        .env("GASCAN_INVOCATIONS", temp.join("invocations"))
        .output()
        .unwrap()
}

#[test]
fn credentials_are_rejected_before_any_tool_is_invoked() {
    for name in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_OAUTH_TOKEN",
        "OPENAI_API_KEY",
        "AZURE_OPENAI_API_KEY",
        "GITHUB_TOKEN",
        "GH_TOKEN",
        "GITLAB_TOKEN",
        "GLAB_TOKEN",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "AWS_BEARER_TOKEN_BEDROCK",
        "GOOGLE_API_KEY",
        "GEMINI_API_KEY",
    ] {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("bin")).unwrap();
        let output = Command::new(script())
            .arg("seed")
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", temp.path().join("bin").display()),
            )
            .env("GASCAN_CONFIG_ROOT", temp.path().join("config"))
            .env("GASCAN_CACHE_ROOT", temp.path().join("cache"))
            .env("GASCAN_INVOCATIONS", temp.path().join("invocations"))
            .env(name, "must-not-be-used")
            .output()
            .unwrap();
        assert!(!output.status.success(), "{name} was accepted");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(name),
            "unexpected error for {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!temp.path().join("invocations").exists());
    }
}

#[test]
fn seed_and_probe_use_only_native_cli_state() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("gascan-pi-extension.js"),
        "export default function fixture() {}\n",
    )
    .unwrap();

    let seed = invoke(temp.path(), "seed");
    assert!(
        seed.status.success(),
        "stdout: {}\nstderr: {}\ninvocations: {}",
        String::from_utf8_lossy(&seed.stdout),
        String::from_utf8_lossy(&seed.stderr),
        fs::read_to_string(temp.path().join("invocations")).unwrap_or_default()
    );
    let probe = invoke(temp.path(), "probe");
    assert!(
        probe.status.success(),
        "stdout: {}\nstderr: {}\ninvocations: {}",
        String::from_utf8_lossy(&probe.stdout),
        String::from_utf8_lossy(&probe.stderr),
        fs::read_to_string(temp.path().join("invocations")).unwrap_or_default()
    );

    let config = temp.path().join("config");
    for relative in [
        "agents/claude/.claude.json",
        "agents/codex/config.toml",
        "agents/pi/settings.json",
        "gh/config.yml",
        "glab/config.yml",
    ] {
        assert!(
            config.join(relative).is_file(),
            "missing native state {relative}"
        );
    }
    assert!(
        !temp.path().join("cache").exists(),
        "the probe must not manufacture cache or log evidence"
    );
    assert!(
        !config.join("herdr/config.toml").exists(),
        "the probe must not manufacture Herdr configuration"
    );
    for invocation in fs::read_to_string(temp.path().join("invocations"))
        .unwrap()
        .lines()
        .filter(|line| line.starts_with("pi "))
    {
        assert!(
            invocation.ends_with("offline=1 telemetry=0"),
            "Pi command was not explicitly offline: {invocation}"
        );
    }
}
