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
