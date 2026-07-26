use std::{
    io::Write,
    process::{Command, Stdio},
};

fn validate(json: &str, name: &str, token: &str) -> std::process::Output {
    validate_with_image(json, name, token, None)
}

fn validate_with_image(
    json: &str,
    name: &str,
    token: &str,
    image: Option<(&str, &str)>,
) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_validate-owned-container"))
        .args([name, token])
        .args(
            image
                .into_iter()
                .flat_map(|(digest, reference)| [digest, reference]),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
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

#[test]
fn optional_created_image_binding_requires_exact_digest_and_local_reference() {
    let name = "gascan-image-user-test-owner";
    let token = "00112233445566778899aabbccddeeff";
    let digest = format!("sha256:{}", "a".repeat(64));
    let reference = "gascan-workspace:0011223344556677";
    let exact = format!(
        r#"[{{"id":"{name}","configuration":{{"id":"{name}","labels":{{"dev.gascan.test":"true","dev.gascan.test.owner":"{token}"}},"image":{{"descriptor":{{"digest":"{digest}"}},"reference":"{reference}"}}}}}}]"#
    );
    assert!(
        validate_with_image(&exact, name, token, Some((&digest, reference)))
            .status
            .success()
    );
    for malformed in [
        exact.replace(&digest, &format!("sha256:{}", "b".repeat(64))),
        exact.replace(reference, "gascan-workspace:ffeeddccbbaa9988"),
        exact.replace(r#","image":"#, r#","other":"#),
    ] {
        assert!(
            !validate_with_image(&malformed, name, token, Some((&digest, reference)))
                .status
                .success()
        );
    }
}

#[test]
fn optional_created_image_binding_accepts_only_exact_digest_qualified_ghcr_reference() {
    let name = "gascan-image-user-test-owner";
    let token = "00112233445566778899aabbccddeeff";
    let digest = format!("sha256:{}", "a".repeat(64));
    let reference = format!("ghcr.io/liquescent-development/gascan/workspace:v1.2.3@{digest}");
    let exact = format!(
        r#"[{{"id":"{name}","configuration":{{"id":"{name}","labels":{{"dev.gascan.test":"true","dev.gascan.test.owner":"{token}"}},"image":{{"descriptor":{{"digest":"{digest}"}},"reference":"{reference}"}}}}}}]"#
    );
    assert!(
        validate_with_image(&exact, name, token, Some((&digest, &reference)))
            .status
            .success()
    );
    for malformed_reference in [
        format!("ghcr.io/other/workspace:v1.2.3@{digest}"),
        "ghcr.io/liquescent-development/gascan/workspace:v1.2.3".to_owned(),
        format!(
            "ghcr.io/liquescent-development/gascan/workspace:v1.2.3@sha256:{}",
            "b".repeat(64)
        ),
        format!("ghcr.io/liquescent-development/gascan/workspace:bad/tag@{digest}"),
    ] {
        assert!(
            !validate_with_image(&exact, name, token, Some((&digest, &malformed_reference)))
                .status
                .success()
        );
    }
}

#[test]
fn apple_canonical_ghcr_binding_is_equivalent_only_at_the_exact_expected_digest() {
    let name = "gascan-image-user-test-owner";
    let token = "00112233445566778899aabbccddeeff";
    let digest = format!("sha256:{}", "a".repeat(64));
    let expected = format!("ghcr.io/liquescent-development/gascan/workspace:v1.2.3@{digest}");
    let observed = format!("ghcr.io/liquescent-development/gascan/workspace@{digest}");
    let exact = format!(
        r#"[{{"id":"{name}","configuration":{{"id":"{name}","labels":{{"dev.gascan.test":"true","dev.gascan.test.owner":"{token}"}},"image":{{"descriptor":{{"digest":"{digest}"}},"reference":"{observed}"}}}}}}]"#
    );
    assert!(
        validate_with_image(&exact, name, token, Some((&digest, &expected)))
            .status
            .success()
    );
    assert!(
        !validate_with_image(
            &exact.replace(
                &observed,
                &format!(
                    "ghcr.io/liquescent-development/gascan/workspace@sha256:{}",
                    "b".repeat(64)
                )
            ),
            name,
            token,
            Some((&digest, &expected))
        )
        .status
        .success()
    );
}

#[test]
fn exact_name_and_owner_label_are_required() {
    let name = "gascan-image-user-test-owner";
    let token = "00112233445566778899aabbccddeeff";
    let exact = format!(
        r#"[{{"id":"{name}","configuration":{{"id":"{name}","labels":{{"dev.gascan.test":"true","dev.gascan.test.owner":"{token}"}}}}}}]"#
    );
    assert!(validate(&exact, name, token).status.success());

    for malformed in [
        exact.replace(token, "ffeeddccbbaa99887766554433221100"),
        exact.replace(name, "somebody-elses-container"),
        "[]".to_owned(),
        format!("[{},{0}]", &exact[1..exact.len() - 1]),
    ] {
        assert!(!validate(&malformed, name, token).status.success());
    }
}

#[test]
fn native_apple_identity_shape_is_accepted_without_configuration_name() {
    let name = "gascan-image-user-test-owner";
    let token = "00112233445566778899aabbccddeeff";
    let native = format!(
        r#"[{{"id":"{name}","configuration":{{"id":"{name}","labels":{{"dev.gascan.test":"true","dev.gascan.test.owner":"{token}"}}}}}}]"#
    );
    assert!(validate(&native, name, token).status.success());
    assert!(
        !validate(
            &native.replacen(
                &format!(r#""id":"{name}""#),
                r#""id":"somebody-elses-container""#,
                1,
            ),
            name,
            token
        )
        .status
        .success()
    );
    let configuration_only_mismatch = native
        .replacen(
            &format!(r#""id":"{name}""#),
            r#""id":"somebody-elses-container""#,
            2,
        )
        .replacen(
            r#""id":"somebody-elses-container""#,
            &format!(r#""id":"{name}""#),
            1,
        );
    assert!(
        !validate(&configuration_only_mismatch, name, token)
            .status
            .success()
    );
}
