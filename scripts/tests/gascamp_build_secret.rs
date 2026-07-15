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
        .and_then(|stage| {
            stage.split("\nRUN ").find(|run| {
                run.starts_with("--mount=type=secret,id=gascamp_read_token,required=true")
            })
        })
        .and_then(|run| run.split("\nRUN ").next())
        .expect("Gascamp must be fetched and built in one secret-mounted RUN")
}

fn assert_secure_gascamp_builder(dockerfile: &str) {
    for forbidden in [
        "ARG GASCAMP_READ_TOKEN",
        "ENV GASCAMP_READ_TOKEN",
        "COPY .git",
        "COPY --from=gascamp-builder /root",
        "bundles/gascamp_source_vendor",
        "@github.com",
        "--mount=type=cache",
    ] {
        assert!(
            !dockerfile.contains(forbidden),
            "credential/source leak: {forbidden}"
        );
    }
    for required in [
        "RUN --mount=type=secret,id=gascamp_read_token,required=true",
        "https://github.com/Liquescent-Development/gascamp.git",
        "git rev-parse HEAD",
        "$GASCAMP_REVISION",
        "cargo test --locked",
        "cargo build --locked --release --bin camp",
        "COPY --from=gascamp-builder /out /opt/gascan/gascamp",
        "ARG GASCAMP_REVISION=f6b248c5926240856dbea83d1d2c5c90ea1c1456",
        "test \"${#GASCAMP_REVISION}\" -eq 40",
    ] {
        assert!(
            dockerfile.contains(required),
            "missing Gascamp boundary: {required}"
        );
    }
    let builder_copies: Vec<_> = dockerfile
        .lines()
        .filter(|line| line.starts_with("COPY --from=gascamp-builder"))
        .collect();
    assert_eq!(
        builder_copies,
        ["COPY --from=gascamp-builder /out /opt/gascan/gascamp"],
        "the final stage may copy only /out from the private builder"
    );
    let run = gascamp_run(dockerfile);
    for required in [
        "/run/secrets/gascamp_read_token",
        "username=x-access-token",
        "credential.helper=/tmp/gascamp-credential",
        "git remote add origin https://github.com/Liquescent-Development/gascamp.git",
        "fetch --depth=1 origin \"$GASCAMP_REVISION\"",
        "rm -rf .git /tmp/gascamp-credential",
        "cargo test --locked",
        "cargo build --locked --release --bin camp",
        "strip target/release/camp",
        "ln -s camp /out/bin/campd",
        "chmod -R a-w /out",
    ] {
        assert!(
            run.contains(required),
            "secret-mounted RUN missing: {required}"
        );
    }
    assert!(
        !run.contains("--offline"),
        "private checkout must not use the old vendor build"
    );
    assert!(
        !run.contains("--frozen"),
        "required Cargo commands must have the exact locked shape"
    );
}

#[test]
fn private_gascamp_build_has_a_single_secret_and_output_boundary() {
    assert_secure_gascamp_builder(&dockerfile());
}

#[test]
fn rejects_credential_and_source_leak_patterns() {
    let base = dockerfile();
    for mutation in [
        base.replace("https://github.com/", "https://token@github.com/"),
        base.replace(
            "ARG GASCAMP_REVISION",
            "ARG GASCAMP_READ_TOKEN\nARG GASCAMP_REVISION",
        ),
        base.replace(
            "ARG GASCAMP_REVISION",
            "ENV GASCAMP_READ_TOKEN=secret\nARG GASCAMP_REVISION",
        ),
        base.replace(
            "COPY --from=gascamp-builder /out",
            "COPY --from=gascamp-builder /root",
        ),
        base.replace(
            "COPY --from=gascamp-builder /out /opt/gascan/gascamp",
            "COPY --from=gascamp-builder /tmp/gascamp /opt/gascan/gascamp",
        ),
        base.replace(
            "COPY --from=gascamp-builder /out /opt/gascan/gascamp",
            "COPY --from=gascamp-builder /opt/gascan/mise /opt/gascan/gascamp",
        ),
        base.replace(
            "RUN --mount=type=secret,id=gascamp_read_token,required=true",
            "RUN --mount=type=secret,id=gascamp_read_token,required=true --mount=type=cache,target=/root/.cargo",
        ),
        base.replace("cargo test --locked", "cargo test"),
        base.replace("cargo build --locked", "cargo build"),
    ] {
        assert!(std::panic::catch_unwind(|| assert_secure_gascamp_builder(&mutation)).is_err());
    }
}

#[test]
fn rejects_a_secret_mount_that_ends_before_fetch() {
    let mutation = dockerfile().replace(
        "    git -c credential.helper=/tmp/gascamp-credential",
        "RUN git -c credential.helper=/tmp/gascamp-credential",
    );
    assert!(std::panic::catch_unwind(|| assert_secure_gascamp_builder(&mutation)).is_err());
}
