//! Fault injection: the bodies and pages a Fault rule answers with.
//!
//! "Dropping" a connection without a response is done with a body that fails
//! on its very first poll. hyper (1.x, HTTP/1.1, which is all the decrypted
//! side speaks) only buffers the response head until the body yields; a body
//! that errors before yielding anything makes hyper tear the connection down
//! with the head still unsent, so the client reads end-of-stream and nothing
//! else: the "empty reply" an app reports as a network error (status 0).
//! `tests/fault_injection.rs` pins this behaviour down.

use std::time::Duration;

use bytes::Bytes;
use futures::{stream, StreamExt};
use http_body_util::BodyStream;
use hudsucker::Body;
use nova_proto::{Fault, FaultMode, FAULT_MAX_DELAY_MS};

/// Pause after the bytes a CutBody passes, before failing. hyper flushes its
/// write buffer only when the body is pending; failing right away would drop
/// the head and the bytes too, turning a cut body into a dropped connection.
const CUT_FLUSH_PAUSE: Duration = Duration::from_millis(200);

/// What the WAF page looks like: a plain HTML 403, not JSON.
pub const WAF_403_PAGE: &str = "<html>\r\n<head><title>403 Forbidden</title></head>\r\n<body>\r\n<center><h1>403 Forbidden</h1></center>\r\n</body>\r\n</html>\r\n";

fn reset(msg: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::ConnectionReset, msg)
}

/// A body that fails immediately: the connection closes with no response.
pub fn abort_body() -> Body {
    Body::from_stream(stream::once(async {
        Err::<Bytes, std::io::Error>(reset("NovaProxy fault: connection dropped"))
    }))
}

/// Pass the first `n` bytes of `body`, then close the connection mid-body.
pub fn cut_body(body: Body, n: u64) -> Body {
    let mut left = n;
    let data = BodyStream::new(body)
        .filter_map(|frame| async move {
            match frame {
                Ok(f) => f.into_data().ok().map(Ok),
                Err(e) => Some(Err(e)),
            }
        })
        .map(move |chunk: Result<Bytes, hudsucker::Error>| {
            chunk.map(|c| {
                let take = left.min(c.len() as u64) as usize;
                left -= take as u64;
                c.slice(..take)
            })
        })
        .take_while(|c| futures::future::ready(c.as_ref().is_ok_and(|b| !b.is_empty())));
    let fail = stream::once(async {
        tokio::time::sleep(CUT_FLUSH_PAUSE).await;
        Err::<Bytes, hudsucker::Error>(reset("NovaProxy fault: body cut").into())
    });
    // `n == 0` still sends the head, then fails: an empty body cut short.
    Body::from_stream(data.chain(fail))
}

/// The rule's hold, capped.
pub fn delay(fault: &Fault) -> Duration {
    Duration::from_millis(fault.delay_ms.unwrap_or(0).min(FAULT_MAX_DELAY_MS) as u64)
}

/// Whether the mode forwards the request and acts on its response.
pub fn acts_on_response(mode: FaultMode) -> bool {
    matches!(mode, FaultMode::PassCut | FaultMode::CutBody | FaultMode::Delay)
}

/// Short label for logs and flow errors.
pub fn label(mode: FaultMode) -> &'static str {
    match mode {
        FaultMode::DropBefore => "drop-before",
        FaultMode::HoldClose => "hold-close",
        FaultMode::HoldStatus => "hold-status",
        FaultMode::FakeResponse => "fake-response",
        FaultMode::Waf403 => "waf-403",
        FaultMode::ProxyAuth407 => "proxy-auth-407",
        FaultMode::PassCut => "pass-cut",
        FaultMode::CutBody => "cut-body",
        FaultMode::Delay => "delay",
    }
}

/// The answer a request-phase mode that responds sends: `(status, content
/// type, body)`. `None` for modes that drop or forward.
pub fn canned_answer(fault: &Fault) -> Option<(u16, String, String)> {
    let custom = |default_status: u16, default_ct: &str| {
        (
            fault.status.filter(|s| (100..=999).contains(s)).unwrap_or(default_status),
            fault.content_type.clone().filter(|c| !c.trim().is_empty()).unwrap_or_else(|| default_ct.to_string()),
            fault.body.clone().unwrap_or_default(),
        )
    };
    match fault.mode {
        FaultMode::HoldStatus => Some(custom(504, "text/html")),
        FaultMode::FakeResponse => Some(custom(200, "application/json")),
        FaultMode::Waf403 => Some((403, "text/html".into(), WAF_403_PAGE.into())),
        FaultMode::ProxyAuth407 => Some((407, "text/plain; charset=utf-8".into(), String::new())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fault(mode: FaultMode) -> Fault {
        Fault { mode, ..Default::default() }
    }

    #[test]
    fn delay_is_capped() {
        let f = Fault { delay_ms: Some(u32::MAX), ..fault(FaultMode::HoldClose) };
        assert_eq!(delay(&f), Duration::from_millis(FAULT_MAX_DELAY_MS as u64));
        assert_eq!(delay(&fault(FaultMode::HoldClose)), Duration::ZERO);
    }

    #[test]
    fn canned_answers_default_per_mode() {
        assert_eq!(canned_answer(&fault(FaultMode::HoldStatus)).unwrap().0, 504);
        assert_eq!(canned_answer(&fault(FaultMode::FakeResponse)).unwrap().0, 200);
        let waf = canned_answer(&fault(FaultMode::Waf403)).unwrap();
        assert_eq!((waf.0, waf.1.as_str()), (403, "text/html"));
        assert!(waf.2.contains("403 Forbidden"));
        assert_eq!(canned_answer(&fault(FaultMode::ProxyAuth407)).unwrap().0, 407);
        assert!(canned_answer(&fault(FaultMode::DropBefore)).is_none());
        assert!(canned_answer(&fault(FaultMode::PassCut)).is_none());
    }

    #[test]
    fn custom_status_body_and_type_are_used_and_bad_status_falls_back() {
        let f = Fault {
            status: Some(502),
            body: Some("{\"error\":\"x\"}".into()),
            content_type: Some("application/problem+json".into()),
            ..fault(FaultMode::FakeResponse)
        };
        assert_eq!(canned_answer(&f).unwrap(), (502, "application/problem+json".into(), "{\"error\":\"x\"}".into()));
        let bad = Fault { status: Some(42), ..fault(FaultMode::HoldStatus) };
        assert_eq!(canned_answer(&bad).unwrap().0, 504);
    }

    #[test]
    fn only_forwarding_modes_act_on_the_response() {
        assert!(acts_on_response(FaultMode::PassCut));
        assert!(acts_on_response(FaultMode::CutBody));
        assert!(acts_on_response(FaultMode::Delay));
        assert!(!acts_on_response(FaultMode::DropBefore));
        assert!(!acts_on_response(FaultMode::ProxyAuth407));
    }
}
