use gascan_core::runtime::{
    ResourceKind, ResourceOwnership, SandboxLabel, classify_resource_ownership,
};
use gascan_core::sandbox::SandboxId;

fn owned_container_id() -> SandboxId {
    SandboxId::test("owned")
}

#[test]
fn a_container_must_be_named_by_its_sandbox_id() {
    let id = owned_container_id();
    assert_eq!(
        classify_resource_ownership(
            ResourceKind::Container,
            id.as_str(),
            Some("gascan"),
            SandboxLabel::Parsed(&id),
        ),
        ResourceOwnership::GasCanOwned,
    );
    assert_eq!(
        classify_resource_ownership(
            ResourceKind::Container,
            "some-other-name",
            Some("gascan"),
            SandboxLabel::Parsed(&id),
        ),
        ResourceOwnership::Mismatched,
        "a container whose name and sandbox-id label disagree is not ours to delete",
    );
}

#[test]
fn a_volume_or_network_need_not_be_named_by_its_sandbox_id() {
    let id = owned_container_id();
    for kind in [ResourceKind::Volume, ResourceKind::Network] {
        assert_eq!(
            classify_resource_ownership(
                kind,
                "workspace-data",
                Some("gascan"),
                SandboxLabel::Parsed(&id)
            ),
            ResourceOwnership::GasCanOwned,
            "kind {kind:?}",
        );
    }
}

#[test]
fn an_unlabelled_resource_is_foreign_and_a_foreign_manager_is_foreign() {
    for kind in [
        ResourceKind::Container,
        ResourceKind::Volume,
        ResourceKind::Network,
    ] {
        assert_eq!(
            classify_resource_ownership(kind, "anything", None, SandboxLabel::Absent),
            ResourceOwnership::Foreign,
            "kind {kind:?}",
        );
        assert_eq!(
            classify_resource_ownership(kind, "anything", Some("other-tool"), SandboxLabel::Absent),
            ResourceOwnership::Foreign,
            "kind {kind:?}",
        );
    }
}

#[test]
fn a_half_labelled_or_unparseable_resource_is_mismatched() {
    let id = owned_container_id();
    for kind in [
        ResourceKind::Container,
        ResourceKind::Volume,
        ResourceKind::Network,
    ] {
        assert_eq!(
            classify_resource_ownership(
                kind,
                id.as_str(),
                Some("gascan"),
                SandboxLabel::Unparseable
            ),
            ResourceOwnership::Mismatched,
            "kind {kind:?}",
        );
        assert_eq!(
            classify_resource_ownership(kind, id.as_str(), Some("gascan"), SandboxLabel::Absent),
            ResourceOwnership::Mismatched,
            "kind {kind:?}",
        );
        assert_eq!(
            classify_resource_ownership(kind, id.as_str(), None, SandboxLabel::Parsed(&id)),
            ResourceOwnership::Mismatched,
            "kind {kind:?}",
        );
    }
}
