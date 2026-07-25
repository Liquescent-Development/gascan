use std::{
    io::Write,
    process::{Command, Stdio},
};

fn validate(json: &str, name: &str, token: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_validate-owned-volume"))
        .args([name, token])
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
fn exact_volume_identity_and_owner_labels_are_required() {
    let name = "gascan-image-workstation-tools-00112233445566778899aabbccddeeff";
    let token = "00112233445566778899aabbccddeeff";
    let exact = format!(
        r#"[{{"id":"{name}","configuration":{{"name":"{name}","labels":{{"dev.gascan.test":"true","dev.gascan.test.owner":"{token}"}}}}}}]"#
    );
    assert!(validate(&exact, name, token).status.success());
    for malformed in [
        exact.replace(token, "ffeeddccbbaa99887766554433221100"),
        exact.replace(name, "somebody-elses-volume"),
        "[]".to_owned(),
        format!("[{},{0}]", &exact[1..exact.len() - 1]),
    ] {
        assert!(!validate(&malformed, name, token).status.success());
    }
}
