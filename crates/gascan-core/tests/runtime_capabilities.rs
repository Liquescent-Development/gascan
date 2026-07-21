use gascan_core::runtime::{NetworkIsolation, RuntimeCapabilities, RuntimeVersion};

#[test]
fn capabilities_round_trip_without_backend_fields() {
    let value = RuntimeCapabilities {
        version: RuntimeVersion::new(1, 0, 0),
        bind_mounts: true,
        named_volumes: true,
        tty: true,
        signals: true,
        loopback_publish: true,
        resource_limits: true,
        offline: NetworkIsolation::Proven,
    };
    let json = serde_json::to_string(&value).unwrap();
    assert!(!json.contains("apple"));
    assert_eq!(
        serde_json::from_str::<RuntimeCapabilities>(&json).unwrap(),
        value
    );
}
