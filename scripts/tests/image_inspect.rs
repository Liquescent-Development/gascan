use std::{
    io::Write,
    process::{Command, Stdio},
};

fn validate(json: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_validate-image-inspect"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(json.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn inspect(os: &str, architecture: &str, variant_digest: &str) -> String {
    format!(
        r#"[{{"id":"{id}","configuration":{{"name":"docker.io/library/ubuntu@sha256:{locked}","descriptor":{{"digest":"sha256:{id}"}}}},"variants":[{{"platform":{{"os":"{os}","architecture":"{architecture}"}},"digest":"{variant_digest}"}}]}}]"#,
        id = "b".repeat(64),
        locked = "a".repeat(64),
    )
}

#[test]
fn matching_linux_arm64_native_inspect_prints_variant_digest() {
    let digest = format!("sha256:{}", "a".repeat(64));
    let output = validate(&inspect("linux", "arm64", &digest));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), digest);
}

#[test]
fn mismatched_platform_fails_closed() {
    let digest = format!("sha256:{}", "a".repeat(64));
    for json in [
        inspect("linux", "amd64", &digest),
        inspect("darwin", "arm64", &digest),
    ] {
        assert!(!validate(&json).status.success());
    }
}

#[test]
fn malformed_or_ambiguous_inspect_fails_closed() {
    let digest = format!("sha256:{}", "a".repeat(64));
    let duplicate = format!(
        "[{},{}]",
        &inspect("linux", "arm64", &digest)[1..inspect("linux", "arm64", &digest).len() - 1],
        &inspect("linux", "arm64", &digest)[1..inspect("linux", "arm64", &digest).len() - 1]
    );
    for json in ["not-json".to_owned(), "[]".to_owned(), duplicate] {
        assert!(!validate(&json).status.success());
    }
}

#[test]
fn obsolete_schema_without_variant_digest_fails_closed() {
    let digest = format!("sha256:{}", "a".repeat(64));
    let obsolete = format!(
        r#"[{{"id":"{digest}","configuration":{{"descriptor":{{"digest":"{digest}"}}}},"variants":[{{"platform":{{"os":"linux","architecture":"arm64"}}}}]}}]"#
    );
    assert!(!validate(&obsolete).status.success());
}

#[test]
fn invalid_variant_digest_fails_closed() {
    for digest in [
        format!("sha512:{}", "a".repeat(64)),
        format!("sha256:{}", "A".repeat(64)),
        format!("sha256:{}", "a".repeat(63)),
    ] {
        assert!(!validate(&inspect("linux", "arm64", &digest)).status.success());
    }
}
