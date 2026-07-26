use gascan_core::runtime::same_immutable_image;

const DIGEST_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[test]
fn tagged_and_canonical_references_share_repository_and_digest_identity() {
    let tagged = format!("ghcr.io/example/workspace:release@sha256:{DIGEST_A}");
    let canonical = format!("ghcr.io/example/workspace@sha256:{DIGEST_A}");

    assert!(same_immutable_image(&tagged, &canonical));
    assert!(same_immutable_image(&canonical, &tagged));
}

#[test]
fn identity_rejects_different_repository_digest_and_invalid_references() {
    let approved = format!("ghcr.io/example/workspace:release@sha256:{DIGEST_A}");
    for different in [
        format!("ghcr.io/other/workspace@sha256:{DIGEST_A}"),
        format!("ghcr.io/example/workspace@sha256:{DIGEST_B}"),
        "ghcr.io/example/workspace:latest".to_owned(),
        format!("ghcr.io/example/workspace@sha256:{}", "z".repeat(64)),
        format!("ghcr.io/example/workspace:@sha256:{DIGEST_A}"),
    ] {
        assert!(
            !same_immutable_image(&approved, &different),
            "accepted different or malformed identity: {different}"
        );
    }
}
