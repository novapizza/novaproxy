//! The UI does not keep body previews for flows it is not showing — holding
//! every one of them in the webview is what exhausted its memory during a long
//! capture. These cover the other half of that: the backend serving a body back
//! on demand, and putting one back on a flow that is about to be replayed.

use std::sync::Arc;

use nova_proto::{BodyPreview, Flow};
use novaproxy_lib::commands::{hydrate_request_body, read_body_from};
use novaproxy_lib::state::AppState;

fn state(tag: &str) -> Arc<AppState> {
    let dir = std::env::temp_dir().join(format!("novaproxy-body-refetch-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    Arc::new(AppState::new(dir))
}

fn flow(id: &str) -> Flow {
    nova_core::flow::new_flow(
        id.into(),
        0,
        "POST".into(),
        "https".into(),
        "api.example".into(),
        "/submit".into(),
        "https://api.example/submit".into(),
        "127.0.0.1:1".into(),
        "HTTP/1.1".into(),
        Vec::new(),
    )
}

fn body(text: &str) -> BodyPreview {
    BodyPreview {
        size: text.len() as u64,
        truncated: false,
        media_type: Some("application/json".into()),
        decoded_from: None,
        text: Some(text.into()),
        base64: None,
        spilled: false,
    }
}

/// What the UI sends back for a flow whose preview it dropped: the metadata, but
/// neither the text nor the base64.
fn stripped(of: &BodyPreview) -> BodyPreview {
    BodyPreview {
        text: None,
        base64: None,
        ..of.clone()
    }
}

#[test]
fn an_unspilled_body_comes_back_from_the_retained_flow() {
    let st = state("inline");
    let mut f = flow("f1");
    f.request_body = Some(body(r#"{"hello":"world"}"#));
    f.response_body = Some(body(r#"{"ok":true}"#));
    st.flows.insert(f);

    let req = read_body_from(&st, "f1", "request", None, None).expect("request body served");
    assert_eq!(req.text.as_deref(), Some(r#"{"hello":"world"}"#));
    // It never went to disk, so the Inspector must not offer to load "the full
    // body" from a store that does not have it.
    assert!(!req.spilled);

    let res = read_body_from(&st, "f1", "response", None, None).expect("response body served");
    assert_eq!(res.text.as_deref(), Some(r#"{"ok":true}"#));
}

#[test]
fn a_flow_with_no_body_says_so_rather_than_inventing_one() {
    let st = state("nobody");
    st.flows.insert(flow("f1"));
    let err = read_body_from(&st, "f1", "request", None, None).expect_err("no body to serve");
    assert!(err.contains("no request body"), "{err}");
}

#[test]
fn an_evicted_flow_is_reported_as_gone() {
    let st = state("gone");
    let err = read_body_from(&st, "missing", "response", None, None).expect_err("nothing to serve");
    assert!(err.contains("no longer retained"), "{err}");
}

#[test]
fn an_unknown_side_is_rejected() {
    let st = state("side");
    st.flows.insert(flow("f1"));
    assert!(read_body_from(&st, "f1", "trailers", None, None).is_err());
}

#[test]
fn replaying_a_flow_the_ui_stripped_still_sends_its_body() {
    let st = state("hydrate");
    let full = body(r#"{"replay":"me"}"#);
    let mut retained = flow("f1");
    retained.request_body = Some(full.clone());
    st.flows.insert(retained);

    // The UI holds the metadata only, and sends that.
    let mut sent = flow("f1");
    sent.request_body = Some(stripped(&full));
    hydrate_request_body(&st, &mut sent);

    assert_eq!(
        sent.request_body.as_ref().and_then(|b| b.text.as_deref()),
        Some(r#"{"replay":"me"}"#),
    );
}

#[test]
fn hydrating_leaves_a_body_the_ui_already_has_alone() {
    let st = state("keep");
    let mut retained = flow("f1");
    retained.request_body = Some(body("from the store"));
    st.flows.insert(retained);

    let mut sent = flow("f1");
    sent.request_body = Some(body("edited by hand"));
    hydrate_request_body(&st, &mut sent);

    assert_eq!(
        sent.request_body.as_ref().and_then(|b| b.text.as_deref()),
        Some("edited by hand"),
    );
}

#[test]
fn hydrating_a_flow_that_captured_no_body_is_a_noop() {
    let st = state("noop");
    st.flows.insert(flow("f1"));

    let mut sent = flow("f1");
    hydrate_request_body(&st, &mut sent);
    assert!(sent.request_body.is_none());

    // An empty body is not a missing one either: nothing to fetch, nothing to do.
    let mut empty = flow("f1");
    empty.request_body = Some(BodyPreview {
        size: 0,
        truncated: false,
        media_type: None,
        decoded_from: None,
        text: None,
        base64: None,
        spilled: false,
    });
    hydrate_request_body(&st, &mut empty);
    assert_eq!(empty.request_body.as_ref().map(|b| b.size), Some(0));
}
