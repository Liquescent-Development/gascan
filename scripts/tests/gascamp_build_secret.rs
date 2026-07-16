use std::{fs, path::Path};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

fn dockerfile() -> String {
    fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap()
}

fn gascamp_run(dockerfile: &str) -> &str {
    dockerfile
        .split("\nFROM ")
        .find(|stage| stage.contains("AS gascamp-builder"))
        .and_then(|stage| stage.split("\nRUN ").nth(1))
        .and_then(|run| run.split("\nRUN ").next())
        .expect("Gascamp must be fetched and built in one ordinary RUN")
}

const REVISION: &str = "f6b248c5926240856dbea83d1d2c5c90ea1c1456";

fn assert_exact_revision_pin(dockerfile: &str) {
    let run = gascamp_run(dockerfile);
    let pin = format!("test \"$GASCAMP_REVISION\" = {REVISION}");
    let pin_offset = run.find(&pin).expect("missing exact Gascamp revision pin");
    let fetch_offset = run.find("fetch --depth=1").expect("missing Gascamp fetch");
    assert!(
        pin_offset < fetch_offset,
        "revision must be pinned before fetch"
    );
    assert!(run.contains("fetch --depth=1 origin \"$GASCAMP_REVISION\""));
    for mutable_ref in ["origin main", "origin master", "origin HEAD", "--branch"] {
        assert!(
            !run.contains(mutable_ref),
            "mutable Gascamp ref: {mutable_ref}"
        );
    }
}

fn assert_only_reviewed_outputs(dockerfile: &str) {
    let out_operations: Vec<_> = gascamp_run(dockerfile)
        .lines()
        .map(|line| line.trim().trim_end_matches(" \\").trim_end_matches(';'))
        .filter(|line| line.contains("/out"))
        .collect();
    assert_eq!(
        out_operations,
        [
            "install -D -o root -g root -m 0555 target/release/camp /out/bin/camp",
            "ln -s camp /out/bin/campd",
            "printf '%s\\n' \"$GASCAMP_REVISION\" >/out/REVISION",
            "chown -R root:root /out",
            "chmod 0444 /out/REVISION",
            "chmod -R a-w /out",
        ],
        "builder may emit only camp, relative campd, and REVISION"
    );
}

fn assert_anonymous_public_builder(dockerfile: &str) {
    for required in [
        "https://github.com/Liquescent-Development/gascamp.git",
        "git rev-parse HEAD",
        "$GASCAMP_REVISION",
        "cargo test --locked",
        "cargo build --locked --release --bin camp",
        "COPY --from=gascamp-builder /out /opt/gascan/gascamp",
    ] {
        assert!(
            dockerfile.contains(required),
            "missing Gascamp boundary: {required}"
        );
    }
    for forbidden in [
        "--mount=type=secret",
        "credential.helper",
        "http.extraHeader",
        "ARG GASCAMP_READ_TOKEN",
        "ENV GASCAMP_READ_TOKEN",
        "COPY .git",
        "COPY --from=gascamp-builder /root",
        "bundles/gascamp_source_vendor",
        "@github.com",
        "/run/secrets/",
    ] {
        assert!(
            !dockerfile.contains(forbidden),
            "authentication/source leak: {forbidden}"
        );
    }
    let builder_copies: Vec<_> = dockerfile
        .lines()
        .filter(|line| line.starts_with("COPY --from=gascamp-builder"))
        .collect();
    assert_eq!(
        builder_copies,
        ["COPY --from=gascamp-builder /out /opt/gascan/gascamp"]
    );
    let run = gascamp_run(dockerfile);
    for required in [
        "git remote add origin https://github.com/Liquescent-Development/gascamp.git",
        "git checkout --detach FETCH_HEAD",
        "rm -rf .git",
        "strip target/release/camp",
        "ln -s camp /out/bin/campd",
        "chmod -R a-w /out",
    ] {
        assert!(run.contains(required), "anonymous RUN missing: {required}");
    }
    assert!(!run.contains("--offline"));
    assert!(!run.contains("--frozen"));
}

#[test]
fn public_gascamp_build_is_anonymous_pinned_and_output_only() {
    let dockerfile = dockerfile();
    assert_anonymous_public_builder(&dockerfile);
    assert_exact_revision_pin(&dockerfile);
    assert_only_reviewed_outputs(&dockerfile);
}

#[test]
fn rejects_an_alternate_well_formed_revision() {
    let mutation = dockerfile().replace(
        &format!("test \"$GASCAMP_REVISION\" = {REVISION}"),
        "test \"$GASCAMP_REVISION\" = 0123456789abcdef0123456789abcdef01234567",
    );
    assert!(std::panic::catch_unwind(|| assert_exact_revision_pin(&mutation)).is_err());
}

#[test]
fn rejects_authentication_and_source_leak_patterns() {
    let base = dockerfile();
    for mutation in [
        base.replace("https://github.com/", "https://token@github.com/"),
        base.replace("git fetch", "git -c credential.helper=/tmp/helper fetch"),
        base.replace(
            "git fetch",
            "git -c http.extraHeader=Authorization:secret fetch",
        ),
        base.replace(
            "ARG GASCAMP_REVISION",
            "ARG GASCAMP_READ_TOKEN\nARG GASCAMP_REVISION",
        ),
        base.replace(
            "COPY --from=gascamp-builder /out",
            "COPY --from=gascamp-builder /root",
        ),
        base.replace(
            "COPY --from=gascamp-builder /out /opt/gascan/gascamp",
            "COPY --from=gascamp-builder /tmp/gascamp /opt/gascan/gascamp",
        ),
        base.replace("rm -rf .git", "cp -R .git /out/git; rm -rf .git"),
    ] {
        assert!(std::panic::catch_unwind(|| {
            assert_anonymous_public_builder(&mutation);
            assert_only_reviewed_outputs(&mutation);
        })
        .is_err());
    }
}

#[test]
fn rejects_mutable_fetch_refs_and_unlocked_cargo() {
    let base = dockerfile();
    for mutation in [
        base.replace("origin \"$GASCAMP_REVISION\"", "origin main"),
        base.replace("cargo test --locked", "cargo test"),
        base.replace("cargo build --locked", "cargo build"),
    ] {
        assert!(std::panic::catch_unwind(|| {
            assert_anonymous_public_builder(&mutation);
            assert_exact_revision_pin(&mutation);
        })
        .is_err());
    }
}

#[test]
fn rejects_extra_builder_outputs() {
    let base = dockerfile();
    for mutation in [
        base.replace(
            "    chown -R root:root /out; \\",
            "    cp Cargo.lock /out/; chown -R root:root /out; \\",
        ),
        base.replace(
            "    chown -R root:root /out; \\",
            "    cp -R /tmp/gascamp /out/source; chown -R root:root /out; \\",
        ),
    ] {
        assert!(std::panic::catch_unwind(|| assert_only_reviewed_outputs(&mutation)).is_err());
    }
}
