use sha2::{Digest, Sha256};
use std::{fs, process::Command};

fn root() -> &'static std::path::Path {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
}

fn validate(contents: &str) -> std::process::Output {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("versions.lock");
    fs::write(&path, contents).unwrap();
    Command::new(env!("CARGO_BIN_EXE_update-image-lock"))
        .args(["--validate-workstation-lock", path.to_str().unwrap()])
        .output()
        .unwrap()
}

fn validate_npm_lock_bytes(contents: &[u8]) -> std::process::Output {
    validate_npm_lock_bytes_with_primary(contents, false)
}

fn validate_npm_lock_bytes_with_primary(
    contents: &[u8],
    refresh_aggregate_hashes: bool,
) -> std::process::Output {
    let temporary = tempfile::tempdir().unwrap();
    let npm_lock_path = temporary.path().join("package-lock.json");
    let npm_manifest_path = temporary.path().join("package.json");
    let image_lock_path = temporary.path().join("versions.lock");
    let manifest = fs::read(root().join("images/workspace/workstation-package.json")).unwrap();
    let mut image_lock: toml::Value =
        toml::from_str(&fs::read_to_string(root().join("images/workspace/versions.lock")).unwrap())
            .unwrap();
    if refresh_aggregate_hashes {
        image_lock["workstation_npm"]["package_manifest_sha256"] =
            toml::Value::String(format!("{:x}", Sha256::digest(&manifest)));
        image_lock["workstation_npm"]["package_lock_sha256"] =
            toml::Value::String(format!("{:x}", Sha256::digest(contents)));
    }
    fs::write(&npm_manifest_path, manifest).unwrap();
    fs::write(&npm_lock_path, contents).unwrap();
    fs::write(
        &image_lock_path,
        toml::to_string_pretty(&image_lock).unwrap(),
    )
    .unwrap();
    Command::new(env!("CARGO_BIN_EXE_update-image-lock"))
        .args([
            "--validate-workstation-package-lock",
            npm_manifest_path.to_str().unwrap(),
            npm_lock_path.to_str().unwrap(),
            image_lock_path.to_str().unwrap(),
        ])
        .output()
        .unwrap()
}

fn validate_npm_lock(contents: &serde_json::Value) -> std::process::Output {
    validate_npm_lock_bytes_with_primary(&serde_json::to_vec_pretty(contents).unwrap(), true)
}

fn replace_in_table(lock: &str, table: &str, from: &str, to: &str) -> String {
    let header = format!("[{table}]");
    let start = lock
        .find(&header)
        .unwrap_or_else(|| panic!("missing {header}"));
    let relative_end = lock[start + header.len()..]
        .find("\n[")
        .map(|offset| start + header.len() + offset)
        .unwrap_or(lock.len());
    let section = &lock[start..relative_end];
    assert!(section.contains(from), "{table} omitted {from}");
    format!(
        "{}{}{}",
        &lock[..start],
        section.replacen(from, to, 1),
        &lock[relative_end..]
    )
}

#[test]
fn generated_workstation_lock_passes_offline_security_validation() {
    let lock = fs::read_to_string(root().join("images/workspace/versions.lock")).unwrap();
    let output = validate(&lock);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn workstation_lock_mutations_fail_closed() {
    let lock = fs::read_to_string(root().join("images/workspace/versions.lock")).unwrap();
    let record = lock
        .split("[workstation_artifacts.claude]")
        .nth(1)
        .expect("Claude workstation record")
        .split("[workstation_artifacts.")
        .next()
        .unwrap();

    let mutations = [
        lock.replacen("registry.npmjs.org", "packages.example.invalid", 1),
        lock.replacen(
            "sha256 = \"3a434c8bcb493e9ca87315d9aa6064835c5987e8fbc85c181bb76157dd5c45d8\"",
            "sha256 = \"\"",
            1,
        ),
        lock.replacen(
            "platform = \"linux-arm64\"",
            "platform = \"linux-amd64\"",
            1,
        ),
        lock.replacen(
            "/ogulcancelik/herdr/releases/download/v",
            "/ogulcancelik/herdr/releases/latest/download/v",
            1,
        ),
        format!("{lock}\n[workstation_artifacts.claude]\n{record}"),
        lock.replacen("kind = \"npm_tgz\"", "kind = \"raw_binary\"", 1),
        lock.replacen("size = 22971", "size = 67108865", 1),
        lock.replacen("herdr-linux-aarch64", "herdr-linux-arm64", 1),
        lock.replacen(
            "github.com/ogulcancelik/herdr",
            "downloads.herdr.example/ogulcancelik/herdr",
            1,
        ),
    ];
    for mutated in mutations {
        assert!(!validate(&mutated).status.success());
    }
}

#[test]
fn lifecycle_and_native_evidence_mutations_fail_closed() {
    let lock = fs::read_to_string(root().join("images/workspace/versions.lock")).unwrap();
    let cases = [
        (
            "workstation_npm",
            "scripts = \"disabled\"",
            "scripts = \"enabled\"",
        ),
        (
            "workstation_npm",
            "npm_version = \"11.12.1\"",
            "npm_version = \"11.12.2\"",
        ),
        (
            "workstation_npm",
            "package_manifest_sha256 = \"5",
            "package_manifest_sha256 = \"0",
        ),
        (
            "workstation_npm",
            "package_lock_sha256 = \"2",
            "package_lock_sha256 = \"0",
        ),
        (
            "workstation_npm.lifecycle_exceptions.claude",
            "version = \"2.1.218\"",
            "version = \"2.1.219\"",
        ),
        (
            "workstation_npm.lifecycle_exceptions.claude",
            "command = \"node install.cjs\"",
            "command = \"node changed.cjs\"",
        ),
        (
            "workstation_npm.lifecycle_exceptions.claude",
            "integrity = \"sha512-B",
            "integrity = \"sha512-X",
        ),
        (
            "workstation_npm.lifecycle_exceptions.claude",
            "manifest_sha256 = \"e",
            "manifest_sha256 = \"0",
        ),
        (
            "workstation_npm.lifecycle_exceptions.claude",
            "script_path = \"package/install.cjs\"",
            "script_path = \"package/changed.cjs\"",
        ),
        (
            "workstation_npm.lifecycle_exceptions.claude",
            "script_sha256 = \"5",
            "script_sha256 = \"0",
        ),
        (
            "workstation_npm.lifecycle_exceptions.google_genai",
            "version = \"1.52.0\"",
            "version = \"1.52.1\"",
        ),
        (
            "workstation_npm.lifecycle_exceptions.google_genai",
            "command = \"echo 'preinstall: no-op'\"",
            "command = \"echo changed\"",
        ),
        (
            "workstation_npm.lifecycle_exceptions.google_genai",
            "integrity = \"sha512-g",
            "integrity = \"sha512-X",
        ),
        (
            "workstation_npm.lifecycle_exceptions.google_genai",
            "manifest_sha256 = \"e",
            "manifest_sha256 = \"0",
        ),
        (
            "workstation_npm.lifecycle_exceptions.protobufjs",
            "version = \"7.6.4\"",
            "version = \"7.6.5\"",
        ),
        (
            "workstation_npm.lifecycle_exceptions.protobufjs",
            "command = \"node scripts/postinstall\"",
            "command = \"node changed\"",
        ),
        (
            "workstation_npm.lifecycle_exceptions.protobufjs",
            "integrity = \"sha512-R",
            "integrity = \"sha512-X",
        ),
        (
            "workstation_npm.lifecycle_exceptions.protobufjs",
            "manifest_sha256 = \"f",
            "manifest_sha256 = \"0",
        ),
        (
            "workstation_npm.lifecycle_exceptions.protobufjs",
            "script_path = \"package/scripts/postinstall.js\"",
            "script_path = \"package/scripts/changed.js\"",
        ),
        (
            "workstation_npm.lifecycle_exceptions.protobufjs",
            "script_sha256 = \"5",
            "script_sha256 = \"0",
        ),
        (
            "workstation_npm.claude_native",
            "package = \"@anthropic-ai/claude-code-linux-arm64\"",
            "package = \"@anthropic-ai/claude-code-linux-x64\"",
        ),
        (
            "workstation_npm.claude_native",
            "version = \"2.1.218\"",
            "version = \"2.1.219\"",
        ),
        (
            "workstation_npm.claude_native",
            "url = \"https://registry.npmjs.org/",
            "url = \"https://packages.example.invalid/",
        ),
        (
            "workstation_npm.claude_native",
            "integrity = \"sha512-C",
            "integrity = \"sha512-X",
        ),
        (
            "workstation_npm.claude_native",
            "sha256 = \"1",
            "sha256 = \"0",
        ),
        (
            "workstation_npm.claude_native",
            "size = 84159749",
            "size = 84159750",
        ),
        (
            "workstation_npm.claude_native",
            "binary_path = \"package/claude\"",
            "binary_path = \"claude\"",
        ),
        (
            "workstation_npm.claude_native",
            "binary_sha256 = \"2",
            "binary_sha256 = \"0",
        ),
        (
            "workstation_npm.claude_native",
            "binary_size = 269990816",
            "binary_size = 269990817",
        ),
        (
            "workstation_npm.claude_native",
            "platform = \"linux-arm64\"",
            "platform = \"linux-amd64\"",
        ),
    ];
    for (table, from, to) in cases {
        let mutated = replace_in_table(&lock, table, from, to);
        assert!(!validate(&mutated).status.success(), "{table}: {from}");
    }

    let unknown = format!(
        "{lock}\n[workstation_npm.lifecycle_exceptions.unknown]\npackage = \"unknown\"\nversion = \"1.0.0\"\ncommand = \"true\"\nintegrity = \"sha512-unknown\"\nmanifest_sha256 = \"{}\"\n",
        "0".repeat(64)
    );
    assert!(!validate(&unknown).status.success());
}

#[test]
fn workstation_npm_manifest_and_lock_are_exact_and_agree() {
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root().join("images/workspace/workstation-package.json")).unwrap(),
    )
    .unwrap();
    let npm_lock: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root().join("images/workspace/workstation-package-lock.json")).unwrap(),
    )
    .unwrap();
    let image_lock: toml::Value =
        toml::from_str(&fs::read_to_string(root().join("images/workspace/versions.lock")).unwrap())
            .unwrap();

    for (tool, package) in [
        ("claude", "@anthropic-ai/claude-code"),
        ("codex", "@openai/codex"),
        ("pi", "@earendil-works/pi-coding-agent"),
    ] {
        let version = image_lock["workstation_artifacts"][tool]["version"]
            .as_str()
            .unwrap();
        assert_eq!(manifest["dependencies"][package].as_str(), Some(version));
        assert_eq!(
            npm_lock["packages"][""]["dependencies"][package].as_str(),
            Some(version)
        );
    }
    let lifecycle_packages = npm_lock["packages"]
        .as_object()
        .unwrap()
        .iter()
        .filter_map(|(path, package)| {
            (package["hasInstallScript"].as_bool() == Some(true)).then_some(path.as_str())
        })
        .collect::<Vec<_>>();
    assert_eq!(lifecycle_packages.len(), 3);
    for expected in [
        "node_modules/@anthropic-ai/claude-code",
        "node_modules/@earendil-works/pi-coding-agent/node_modules/@google/genai",
        "node_modules/@earendil-works/pi-coding-agent/node_modules/protobufjs",
    ] {
        assert!(lifecycle_packages.contains(&expected));
    }
    assert!(
        npm_lock["packages"]
            .as_object()
            .unwrap()
            .iter()
            .all(|(path, package)| path.is_empty()
                || package.get("resolved").is_none()
                || package.get("integrity").is_some()),
        "every resolved transitive package must carry registry integrity"
    );
}

#[test]
fn npm_lock_semantic_mutations_reach_their_intended_validation_gates() {
    let exact = fs::read(root().join("images/workspace/workstation-package-lock.json")).unwrap();
    let npm_lock: serde_json::Value = serde_json::from_slice(&exact).unwrap();
    assert!(validate_npm_lock_bytes(&exact).status.success());
    let semver = "node_modules/@earendil-works/pi-coding-agent/node_modules/semver";
    let mut mutations = Vec::new();

    let mut unknown_script = npm_lock.clone();
    unknown_script["packages"][semver]["hasInstallScript"] = true.into();
    mutations.push((
        "unknown lifecycle",
        unknown_script,
        "npm lifecycle package set differs from reviewed evidence",
    ));

    let mut extra_root = npm_lock.clone();
    extra_root["packages"][""]["dependencies"]["unexpected"] = "1.0.0".into();
    mutations.push((
        "extra root",
        extra_root,
        "must contain exactly three top-level packages",
    ));

    let mut off_host = npm_lock.clone();
    off_host["packages"][semver]["resolved"] =
        "https://packages.example.invalid/semver-7.8.0.tgz".into();
    mutations.push(("off-host URL", off_host, "unapproved npm tarball URL"));

    let mut malformed_integrity = npm_lock.clone();
    malformed_integrity["packages"][semver]["integrity"] = "sha512-not-base64".into();
    mutations.push((
        "malformed SRI",
        malformed_integrity,
        "npm integrity is malformed",
    ));

    let mut changed_version = npm_lock.clone();
    changed_version["packages"][semver]["version"] = "7.8.1".into();
    mutations.push((
        "version/canonical identity",
        changed_version,
        "unapproved npm tarball URL",
    ));

    let mut changed_identity = npm_lock.clone();
    changed_identity["packages"][semver]["resolved"] =
        "https://registry.npmjs.org/semver/-/different-7.8.0.tgz".into();
    mutations.push((
        "canonical tarball identity",
        changed_identity,
        "unapproved npm tarball URL",
    ));

    let mut replaced_lifecycle = npm_lock.clone();
    replaced_lifecycle["packages"]["node_modules/@anthropic-ai/claude-code"]["hasInstallScript"] =
        false.into();
    replaced_lifecycle["packages"][semver]["hasInstallScript"] = true.into();
    mutations.push((
        "lifecycle bijection",
        replaced_lifecycle,
        "npm lifecycle package set differs from reviewed evidence",
    ));

    let mut missing_closure = npm_lock;
    missing_closure["packages"]
        .as_object_mut()
        .unwrap()
        .remove(semver);
    mutations.push((
        "missing dependency closure",
        missing_closure,
        "dependency closure omitted semver",
    ));

    for (name, mutated, expected) in mutations {
        let output = validate_npm_lock(&mutated);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{name} unexpectedly passed");
        assert!(stderr.contains(expected), "{name}: {stderr}");
    }
}

#[test]
fn npm_lock_stale_aggregate_hash_is_a_distinct_early_rejection() {
    let exact = fs::read(root().join("images/workspace/workstation-package-lock.json")).unwrap();
    let mut npm_lock: serde_json::Value = serde_json::from_slice(&exact).unwrap();
    npm_lock["packages"]["node_modules/@anthropic-ai/claude-code"]["hasInstallScript"] =
        false.into();
    let mutated = serde_json::to_vec_pretty(&npm_lock).unwrap();
    let output = validate_npm_lock_bytes_with_primary(&mutated, false);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("generated input hash differs from image lock"),
        "{stderr}"
    );
}
