use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use gascan_image_tools::{RedirectRules, walk_redirects_with};
use reqwest::Url;

#[test]
fn unapproved_intermediate_redirect_is_rejected_before_contact() {
    let contacts = Arc::new(AtomicUsize::new(0));
    let observed = contacts.clone();
    let rules = RedirectRules::for_test_http_origins(["approved.test".to_owned()], 3);
    let result = walk_redirects_with("http://approved.test/artifact", rules, move |url| {
        observed.fetch_add(1, Ordering::SeqCst);
        if url.host_str() == Some("approved.test") {
            Ok(Some(Url::parse("http://unapproved.test/intermediate")?))
        } else {
            Ok(None)
        }
    });

    assert!(result.is_err());
    assert_eq!(contacts.load(Ordering::SeqCst), 1);
}
