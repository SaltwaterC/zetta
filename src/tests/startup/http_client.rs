use super::*;

#[test]
fn the_client_is_built_on_first_use_not_construction() {
    let client = LazyHttpClient::new("Zetta/test");
    assert!(!client.is_built());

    // Built by `ReqwestClient::user_agent`, it sends the agent as a default
    // header and reports no proxy, as the client built at launch did.
    assert!(client.proxy().is_none());
    assert!(client.is_built());
}
