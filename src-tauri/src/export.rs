//! Session and HAR export, written to the file one flow at a time.
//!
//! Exports used to be built in the webview: the engine handed over every
//! retained flow with its bodies read back from disk in one `Vec<Flow>`, and the
//! page turned that into one JSON string. With 10,000 flows and half a megabyte
//! per body side that is gigabytes held twice over. Here each flow is fetched,
//! filled from the body store, optionally redacted, serialized and dropped
//! before the next one; memory stays at one flow's worth whatever the size of
//! the capture.

use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nova_proto::{BodyPreview, Flow, Header};
use serde_json::{json, Value};

use crate::state::AppState;

/// What to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// NovaProxy's own `.nova` session document.
    Nova,
    /// HTTP Archive 1.2.
    Har,
}

impl Format {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "nova" | "session" => Ok(Self::Nova),
            "har" => Ok(Self::Har),
            other => Err(format!("unknown export format {other:?}; expected \"nova\" or \"har\"")),
        }
    }
}

/// Write `ids` (in that order) to `path`. A flow in `extra` is taken from
/// there — an imported session lives only in the UI — and otherwise from the
/// engine's store, with spilled bodies read back. Ids found in neither are
/// skipped. Returns how many flows were written.
///
/// The file is written next to `path` and renamed into place at the end, so a
/// failed export never leaves half a document where the old one was.
pub fn export(
    state: &AppState,
    path: &Path,
    format: Format,
    ids: &[String],
    extra: Vec<Flow>,
    redact: bool,
) -> Result<u32, String> {
    let mut extra: HashMap<String, Flow> = extra.into_iter().map(|f| (f.id.clone(), f)).collect();
    let tmp = temp_path(path);
    let result = (|| -> std::io::Result<u32> {
        let mut out = BufWriter::with_capacity(256 * 1024, std::fs::File::create(&tmp)?);
        match format {
            Format::Nova => out.write_all(br#"{"format":"novaproxy-session","version":1,"flows":["#)?,
            Format::Har => {
                let creator = json!({"name": "NovaProxy", "version": env!("CARGO_PKG_VERSION")});
                write!(out, r#"{{"log":{{"version":"1.2","creator":{creator},"entries":["#)?;
            }
        }
        let mut written = 0u32;
        for id in ids {
            let flow = match extra.remove(id) {
                Some(f) => f,
                None => match state.flows.get(id) {
                    Some(mut f) => {
                        crate::commands::fill_spilled(state, &mut f, crate::commands::EXPORT_BODY);
                        f
                    }
                    None => continue,
                },
            };
            let flow = if redact { nova_core::redact::flow(&flow) } else { flow };
            if written > 0 {
                out.write_all(b",")?;
            }
            match format {
                Format::Nova => serde_json::to_writer(&mut out, &flow)?,
                Format::Har => serde_json::to_writer(&mut out, &har_entry(&flow))?,
            }
            written += 1;
        }
        out.write_all(match format {
            Format::Nova => b"]}",
            Format::Har => b"]}}",
        })?;
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        Ok(written)
    })();
    match result {
        Ok(n) => {
            std::fs::rename(&tmp, path).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            Ok(n)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(format!("cannot write {}: {e}", path.display()))
        }
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".partial");
    path.with_file_name(name)
}

/// The Tauri command behind Save Session and Export as HAR.
#[tauri::command]
pub async fn export_flows(
    state: tauri::State<'_, Arc<AppState>>,
    path: String,
    format: String,
    ids: Vec<String>,
    extra: Vec<Flow>,
    redact: bool,
) -> Result<u32, String> {
    let format = Format::parse(&format)?;
    let app: Arc<AppState> = (*state).clone();
    tauri::async_runtime::spawn_blocking(move || export(&app, Path::new(&path), format, &ids, extra, redact))
        .await
        .map_err(|e| e.to_string())?
}

/* ----------------------------------- HAR ----------------------------------- */

/// One HAR 1.2 entry.
///
/// Real measurements where there are some, HAR's -1 ("not available") where
/// there are none — never a fabricated zero. `connect` includes `ssl`, as the
/// spec says, and `send` + `wait` split time to first byte the way the Timing
/// panel does (`nova_core::timing::server_wait_ms`), so the phases add up to
/// `time` instead of counting setup twice.
pub fn har_entry(f: &Flow) -> Value {
    let t = &f.timings;
    let dur = f.duration_ms.unwrap_or(0.0);
    let dns = t.dns_ms.unwrap_or(-1.0);
    let ssl = t.tls_ms.unwrap_or(-1.0);
    let connect = t.connect_ms.map(|c| c + ssl.max(0.0)).unwrap_or(-1.0);
    let setup = dns.max(0.0) + connect.max(0.0);
    let send = t.ttfb_ms.and(t.request_ms).map(|r| (r - setup).max(0.0)).unwrap_or(0.0);
    let wait = match t.ttfb_ms {
        Some(ttfb) => (ttfb - setup - send).max(0.0),
        None => dur,
    };
    let receive = t.download_ms.unwrap_or(0.0);

    let mut request = json!({
        "method": f.method,
        "url": f.url,
        "httpVersion": f.http_version,
        "cookies": [],
        "headers": har_headers(&f.request_headers),
        "queryString": query_string(&f.url),
        "headersSize": -1,
        "bodySize": f.request_size,
    });
    if let Some(text) = f.request_body.as_ref().and_then(|b| b.text.as_ref()) {
        request["postData"] = json!({
            "mimeType": f.request_body.as_ref().and_then(|b| b.media_type.clone()).unwrap_or_default(),
            "text": text,
        });
    }
    let mut entry = json!({
        "startedDateTime": iso_time(f.started_at),
        "time": dur,
        "request": request,
        "response": {
            "status": f.status.unwrap_or(0),
            "statusText": "",
            "httpVersion": f.http_version,
            "cookies": [],
            "headers": har_headers(&f.response_headers),
            "content": content(f),
            "redirectURL": header(&f.response_headers, "location").unwrap_or(""),
            "headersSize": -1,
            "bodySize": f.response_size,
        },
        "cache": {},
        "timings": {"blocked": -1, "dns": dns, "connect": connect, "ssl": ssl, "send": send, "wait": wait, "receive": receive},
    });
    if let Some(e) = &f.error {
        entry["comment"] = json!(e);
    }
    entry
}

fn content(f: &Flow) -> Value {
    let body: Option<&BodyPreview> = f.response_body.as_ref();
    let mut c = json!({
        "size": body.map(|b| b.size).unwrap_or(f.response_size),
        "mimeType": f.content_type.clone().unwrap_or_default(),
        "text": "",
    });
    if let Some(b) = body {
        if let Some(text) = &b.text {
            c["text"] = json!(text);
        } else if let Some(b64) = &b.base64 {
            c["text"] = json!(b64);
            c["encoding"] = json!("base64");
        }
        if b.truncated {
            c["comment"] = json!("body truncated by NovaProxy; size is the full length");
        }
    }
    c
}

fn har_headers(hs: &[Header]) -> Value {
    Value::Array(hs.iter().map(|h| json!({"name": h.name, "value": h.value})).collect())
}

fn header<'a>(hs: &'a [Header], name: &str) -> Option<&'a str> {
    hs.iter().find(|h| h.name.eq_ignore_ascii_case(name)).map(|h| h.value.as_str())
}

/// The URL's query as HAR pairs, decoded like `URLSearchParams`; a URL that
/// does not parse has none.
fn query_string(url: &str) -> Value {
    let Some((_, rest)) = url.split_once("://") else { return json!([]) };
    let Some((_, qs)) = rest.split_once('?') else { return json!([]) };
    let qs = qs.split('#').next().unwrap_or(qs);
    Value::Array(
        qs.split('&')
            .filter(|p| !p.is_empty())
            .map(|p| {
                let (k, v) = p.split_once('=').unwrap_or((p, ""));
                json!({"name": nova_core::redact::decode_component(k), "value": nova_core::redact::decode_component(v)})
            })
            .collect(),
    )
}

fn iso_time(ms: f64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nova_proto::Timings;

    fn flow(id: &str) -> Flow {
        let mut f = nova_core::flow::new_flow(
            id.into(),
            0,
            "GET".into(),
            "https".into(),
            "example.com".into(),
            "/search?q=hello&n=2".into(),
            "https://example.com/search?q=hello&n=2".into(),
            "127.0.0.1:1".into(),
            "HTTP/1.1".into(),
            vec![Header { name: "accept".into(), value: "*/*".into() }],
        );
        f.status = Some(200);
        f.duration_ms = Some(123.0);
        f.response_size = 42;
        f.content_type = Some("application/json".into());
        f
    }

    fn text_body(t: &str) -> BodyPreview {
        BodyPreview {
            size: t.len() as u64,
            truncated: false,
            media_type: Some("application/json".into()),
            decoded_from: None,
            text: Some(t.into()),
            base64: None,
            spilled: false,
        }
    }

    #[test]
    fn query_strings_are_decoded_pairs_and_bad_urls_have_none() {
        let e = har_entry(&flow("f"));
        assert_eq!(e["request"]["queryString"], json!([{"name": "q", "value": "hello"}, {"name": "n", "value": "2"}]));
        let mut bad = flow("b");
        bad.url = "::::not a url::::".into();
        assert_eq!(har_entry(&bad)["request"]["queryString"], json!([]));
        let mut enc = flow("e");
        enc.url = "https://x.test/?a%20b=c+d".into();
        assert_eq!(har_entry(&enc)["request"]["queryString"], json!([{"name": "a b", "value": "c d"}]));
    }

    #[test]
    fn post_data_only_for_text_request_bodies() {
        assert!(har_entry(&flow("a"))["request"].get("postData").is_none());
        let mut b = flow("b");
        b.request_body = Some(text_body("{}"));
        assert_eq!(har_entry(&b)["request"]["postData"], json!({"mimeType": "application/json", "text": "{}"}));
    }

    #[test]
    fn measured_timings_map_to_har_phases_that_add_up() {
        let mut f = flow("t");
        f.duration_ms = Some(200.0);
        f.timings = Timings {
            dns_ms: Some(10.0),
            connect_ms: Some(20.0),
            tls_ms: Some(30.0),
            ttfb_ms: Some(120.0),
            download_ms: Some(80.0),
            ..Timings::default()
        };
        let t = &har_entry(&f)["timings"];
        assert_eq!(t["dns"], 10.0);
        assert_eq!(t["connect"], 50.0, "connect includes the handshake");
        assert_eq!(t["ssl"], 30.0);
        assert_eq!(t["wait"], 60.0);
        assert_eq!(t["receive"], 80.0);
        assert_eq!(t["blocked"], -1);

        // An upload that outlasted setup is `send`, not server time.
        f.timings.request_ms = Some(90.0);
        let t = &har_entry(&f)["timings"];
        assert_eq!((t["send"].as_f64().unwrap(), t["wait"].as_f64().unwrap()), (30.0, 30.0));
    }

    #[test]
    fn unmeasured_phases_are_minus_one_and_wait_falls_back_to_duration() {
        let mut f = flow("u");
        f.timings = Timings { connection_reused: true, ttfb_ms: Some(70.0), ..Timings::default() };
        let t = &har_entry(&f)["timings"];
        assert_eq!((t["dns"].as_f64(), t["connect"].as_f64(), t["ssl"].as_f64()), (Some(-1.0), Some(-1.0), Some(-1.0)));
        assert_eq!(t["wait"], 70.0);
        assert_eq!(har_entry(&flow("d"))["timings"]["wait"], 123.0);
    }

    #[test]
    fn binary_responses_are_base64_encoded() {
        let mut f = flow("bin");
        f.response_body =
            Some(BodyPreview { base64: Some("AAEC".into()), text: None, truncated: true, ..text_body("") });
        let c = &har_entry(&f)["response"]["content"];
        assert_eq!((c["text"].as_str(), c["encoding"].as_str()), (Some("AAEC"), Some("base64")));
        assert!(c["comment"].as_str().unwrap().contains("truncated"));
    }

    #[test]
    fn exports_stream_in_order_take_extra_flows_first_and_redact_on_request() {
        let dir = std::env::temp_dir().join(format!("novaproxy-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::new(dir.clone());
        let mut live = flow("f1");
        live.request_headers.push(Header { name: "Authorization".into(), value: "Bearer s3cret".into() });
        state.flows.insert(live);
        state.flows.insert(flow("f2"));
        let mut imported = flow("f2");
        imported.url = "https://imported.test/".into();

        let path = dir.join("out.nova");
        let ids = ["f2", "missing", "f1"].map(String::from);
        let n = export(&state, &path, Format::Nova, &ids, vec![imported], false).unwrap();
        assert_eq!(n, 2);
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["format"], "novaproxy-session");
        assert_eq!(doc["flows"][0]["url"], "https://imported.test/", "the UI's copy wins over a clashing engine id");
        assert_eq!(doc["flows"][1]["id"], "f1");
        assert!(!path.with_file_name("out.nova.partial").exists());

        let har = dir.join("out.har");
        export(&state, &har, Format::Har, &["f1".to_string()], Vec::new(), true).unwrap();
        let text = std::fs::read_to_string(&har).unwrap();
        let doc: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["log"]["entries"].as_array().map(Vec::len), Some(1));
        assert!(!text.contains("s3cret") && text.contains("Bearer [redacted]"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
