use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

const SECRET: &str = "synthetic-apple-build-secret-0011223344556677";

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

#[test]
fn probe_requires_private_external_file_and_checks_non_retention() {
    let probe = include_str!("../probe-apple-build-secret.sh");
    for required in [
        "test \"$uid\" = \"$(stat -f %u \"$secret\")\"",
        "test \"600\" = \"$(stat -f %Lp \"$secret\")\"",
        "printf '%s\\n' '.build-secrets' >\"$context/.dockerignore\"",
        "--secret \"id=gascamp_read_token,src=$staged_secret\"",
        "RUN --mount=type=secret,id=gascamp_read_token,required=true",
        "test ! -e /run/secrets/gascamp_read_token",
        "container image inspect --format json",
    ] {
        assert!(probe.contains(required), "missing secret safeguard: {required}");
    }
    assert!(probe.contains("EXPECTED_SECRET_SHA256"));
    assert!(probe.contains("build failed; sanitized transcript follows"));
    for forbidden in ["GASCAMP_READ_TOKEN=", "ENV GASCAMP", "ARG GASCAMP"] {
        assert!(!probe.contains(forbidden), "secret-bearing channel: {forbidden}");
    }
}

#[test]
fn fake_container_proves_secret_stays_out_of_observable_channels() {
    let fixture = tempfile::tempdir_in("/tmp").unwrap();
    let bin_dir = fixture.path().join("bin");
    fs::create_dir(&bin_dir).unwrap();
    let calls = fixture.path().join("calls");
    let retained = fixture.path().join("retained-context");
    let staged_path = fixture.path().join("staged-path");
    let fake = bin_dir.join("container");
    fs::write(
        &fake,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >>"$CALLS"
case "$1" in
  build)
    context=""
    previous=""
    for argument in "$@"; do
      case "$previous" in --secret) printf '%s\n' "${argument#*,src=}" >"$STAGED_PATH" ;; esac
      previous="$argument"
      context="$argument"
    done
    test "$(cat "$context/.dockerignore")" = .build-secrets || { printf '%s\n' bad-dockerignore >&2; exit 1; }
    test -f "$(cat "$STAGED_PATH")" || { printf '%s\n' missing-staged-secret >&2; exit 1; }
    test "$(cat "$(cat "$STAGED_PATH")")" = "$SECRET_VALUE" || { printf '%s\n' wrong-staged-secret >&2; exit 1; }
    mkdir "$RETAINED_CONTEXT"
    cp "$context/Dockerfile" "$context/.dockerignore" "$RETAINED_CONTEXT/"
    test ! -e "$RETAINED_CONTEXT/.build-secrets"
    ;;
  image)
    case "$2" in
      inspect) printf '%s\n' '{"id":"sha256:fixture","config":{"env":[]},"history":[{"created_by":"RUN /bin/sh -c #(nop) secret fixture"}]}' ;;
      delete) : ;;
    esac
    ;;
  create) printf '%s\n' fixture-container ;;
  export)
    test "$3" = --output
    printf '%s\n' synthetic-export-without-token >"$4"
    ;;
  delete) : ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let secret = fixture.path().join("secret");
    fs::write(&secret, format!("{SECRET}\n")).unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    let path = format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap());
    let output = Command::new("bash")
        .arg(repository_root().join("scripts/probe-apple-build-secret.sh"))
        .env("PATH", path)
        .env("GASCAN_TEST_SECRET_FILE", &secret)
        .env("CALLS", &calls)
        .env("RETAINED_CONTEXT", &retained)
        .env("STAGED_PATH", &staged_path)
        .env("SECRET_VALUE", SECRET)
        .env("TMPDIR", format!("{}/", fixture.path().display()))
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr={}", String::from_utf8_lossy(&output.stderr));
    for (name, bytes) in [
        ("argv", fs::read(&calls).unwrap()),
        ("stdout", output.stdout),
        ("stderr", output.stderr),
        ("Dockerfile", fs::read(retained.join("Dockerfile")).unwrap()),
        ("transmitted context", fs::read(retained.join(".dockerignore")).unwrap()),
    ] {
        assert!(!bytes.windows(SECRET.len()).any(|window| window == SECRET.as_bytes()), "secret retained in {name}");
    }
    let calls = fs::read_to_string(calls).unwrap();
    assert!(calls.contains("build --secret id=gascamp_read_token,src="));
    assert!(calls.contains("delete gascan-build-secret-probe-"));
    assert!(calls.contains("image delete"));
    let staged = fs::read_to_string(staged_path).unwrap();
    let staged = Path::new(staged.trim());
    assert!(!staged.to_string_lossy().contains("//"), "staged path was not normalized");
    assert!(staged.ends_with(".build-secrets/gascamp_read_token"));
    assert!(!staged.exists(), "staged secret survived cleanup");
    assert!(!staged.parent().unwrap().parent().unwrap().exists(), "private context survived cleanup");
}
