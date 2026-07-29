//! Body storage: keep a small preview in RAM, spill the rest to disk.
//!
//! Captured bodies used to live entirely in memory, which put a hard ceiling on
//! how long a session could run against a chatty app — every retained byte was
//! also re-serialized onto the UI channel with each flow snapshot. This module
//! splits the two concerns:
//!
//! * **Inline preview** — at most [`BodyStore::inline_cap`] bytes, held on the
//!   [`Flow`](nova_proto::Flow) and streamed to the UI. Bounds both RAM and IPC.
//! * **Spill file** — everything beyond the preview, written to
//!   `<data-dir>/bodies/<flow-id>-<side>.bin` as it streams past, so the full
//!   body stays available (Inspector "load full body", HAR export, MCP tools)
//!   without occupying memory.
//!
//! Disk use is bounded three ways: a per-body cap, a total byte budget with
//! FIFO eviction of the oldest spill files, and deletion of a flow's files when
//! the flow itself is evicted from the retention window.
//!
//! Spill files hold the bytes **exactly as they appeared on the wire** — still
//! `Content-Encoding`-compressed. Decoding happens on read, so a 200 MB gzip
//! stream costs its compressed size on disk, not its inflated size.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{bail, Context, Result};

use crate::flow::Side;

/// Bytes of each body retained inline (in RAM, and sent to the UI).
pub const DEFAULT_INLINE_CAP: usize = 512 * 1024;
/// Largest single body written to disk. Beyond this the body is genuinely
/// truncated — an inspector is not an archive.
pub const DEFAULT_PER_BODY_CAP: u64 = 64 * 1024 * 1024;
/// Total spill budget across all bodies; oldest files are evicted first.
pub const DEFAULT_DISK_BUDGET: u64 = 1024 * 1024 * 1024;

/// Suffix used in a spill file name for each half of the exchange.
fn side_tag(side: Side) -> &'static str {
    match side {
        Side::Request => "req",
        Side::Response => "res",
    }
}

#[derive(Default)]
struct DiskState {
    /// Finished spill files in write order, with their sizes. Front = oldest.
    files: VecDeque<(PathBuf, u64)>,
    used: u64,
}

/// Bounded on-disk store for captured bodies.
pub struct BodyStore {
    /// `None` disables spilling entirely (used by tests and the example proxy):
    /// bodies are then capped at the inline preview and nothing touches disk.
    dir: Option<PathBuf>,
    inline_cap: usize,
    per_body_cap: u64,
    disk_budget: u64,
    state: Mutex<DiskState>,
}

impl BodyStore {
    /// Create a store rooted at `dir`, discarding any spill files left behind by
    /// a previous session (nothing references them once the app exits).
    ///
    /// Falls back to a memory-only store if the directory can't be created, so a
    /// read-only data dir degrades to "previews only" rather than failing to
    /// capture.
    pub fn new(dir: PathBuf, inline_cap: usize, per_body_cap: u64, disk_budget: u64) -> Self {
        let dir = match std::fs::create_dir_all(&dir) {
            Ok(()) => {
                clear_dir(&dir);
                Some(dir)
            }
            Err(e) => {
                tracing::warn!("body spill disabled, cannot use {}: {e}", dir.display());
                None
            }
        };
        Self {
            dir,
            inline_cap,
            per_body_cap,
            disk_budget,
            state: Mutex::new(DiskState::default()),
        }
    }

    /// A store that never spills: bodies are capped at `inline_cap` bytes.
    pub fn memory_only(inline_cap: usize) -> Self {
        Self {
            dir: None,
            inline_cap,
            per_body_cap: 0,
            disk_budget: 0,
            state: Mutex::new(DiskState::default()),
        }
    }

    /// Bytes retained inline per body.
    pub fn inline_cap(&self) -> usize {
        self.inline_cap
    }

    /// Whether this store can spill at all.
    pub fn spills(&self) -> bool {
        self.dir.is_some() && self.per_body_cap > 0 && self.disk_budget > 0
    }

    /// Total bytes currently held in spill files.
    pub fn used_bytes(&self) -> u64 {
        self.state.lock().unwrap().used
    }

    /// Largest body this store will write to disk.
    pub fn per_body_cap(&self) -> u64 {
        self.per_body_cap
    }

    /// Path a body would spill to, or `None` when spilling is disabled.
    pub fn path_for(&self, flow_id: &str, side: Side) -> Option<PathBuf> {
        // Flow ids are engine-generated (`f<seq>`), but sanitize anyway so a
        // future id scheme can never escape the bodies directory.
        let safe: String = flow_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        self.dir
            .as_ref()
            .map(|d| d.join(format!("{safe}-{}.bin", side_tag(side))))
    }

    /// Record a finished spill file and evict oldest files until the total is
    /// within budget.
    pub fn register(&self, path: PathBuf, len: u64) {
        let mut state = self.state.lock().unwrap();
        state.used = state.used.saturating_add(len);
        state.files.push_back((path, len));
        while state.used > self.disk_budget {
            let Some((old, size)) = state.files.pop_front() else { break };
            let _ = std::fs::remove_file(&old);
            state.used = state.used.saturating_sub(size);
        }
    }

    /// Delete both spill files belonging to `flow_id` (the flow was evicted).
    pub fn remove_flow(&self, flow_id: &str) {
        let paths: Vec<PathBuf> = [Side::Request, Side::Response]
            .into_iter()
            .filter_map(|s| self.path_for(flow_id, s))
            .collect();
        if paths.is_empty() {
            return;
        }
        let mut state = self.state.lock().unwrap();
        let mut freed = 0u64;
        state.files.retain(|(p, size)| {
            if paths.contains(p) {
                freed += *size;
                false
            } else {
                true
            }
        });
        state.used = state.used.saturating_sub(freed);
        for p in paths {
            let _ = std::fs::remove_file(p);
        }
    }

    /// Read a spilled body back, still content-encoded, capped at `max_bytes`.
    /// Returns `(bytes, total_len, truncated)`.
    pub fn read(&self, flow_id: &str, side: Side, max_bytes: u64) -> Result<(Vec<u8>, u64, bool)> {
        let Some(path) = self.path_for(flow_id, side) else {
            bail!("body spilling is disabled");
        };
        let meta = std::fs::metadata(&path)
            .with_context(|| format!("no stored body for flow {flow_id}"))?;
        let total = meta.len();
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        if bytes.len() as u64 > max_bytes {
            let keep = max_bytes as usize;
            Ok((bytes[..keep].to_vec(), total, true))
        } else {
            Ok((bytes, total, false))
        }
    }
}

/// Remove every file directly inside `dir`, ignoring failures.
fn clear_dir(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Outcome of capturing one body.
pub struct Captured {
    /// The inline preview bytes (still content-encoded), at most `inline_cap`.
    pub inline: Vec<u8>,
    /// True wire size of the whole body.
    pub total: u64,
    /// The inline preview is shorter than the body.
    pub truncated: bool,
    /// The full body (up to the per-body cap) is available on disk.
    pub spilled: bool,
}

/// Accumulates a body as it streams past: the first `inline_cap` bytes stay in
/// memory for the preview, and the whole thing is mirrored to a spill file once
/// it outgrows that preview. Forwarding is unaffected — this only bounds what we
/// *retain*.
pub struct BodyCapture<'a> {
    store: &'a BodyStore,
    path: Option<PathBuf>,
    file: Option<std::io::BufWriter<std::fs::File>>,
    written: u64,
    inline: Vec<u8>,
    total: u64,
    truncated: bool,
}

impl<'a> BodyCapture<'a> {
    pub fn new(store: &'a BodyStore, flow_id: &str, side: Side) -> Self {
        Self {
            path: store.path_for(flow_id, side),
            store,
            file: None,
            written: 0,
            inline: Vec::new(),
            total: 0,
            truncated: false,
        }
    }

    /// Take in one streamed chunk.
    pub fn push(&mut self, chunk: &[u8]) {
        self.total += chunk.len() as u64;

        let cap = self.store.inline_cap();
        if self.inline.len() < cap {
            let room = cap - self.inline.len();
            let take = room.min(chunk.len());
            self.inline.extend_from_slice(&chunk[..take]);
        }
        if self.total > self.inline.len() as u64 {
            self.truncated = true;
        }

        if !self.store.spills() {
            return;
        }
        // Lazily open the spill file the moment the body outgrows the preview,
        // then mirror everything — including the bytes already held inline, so
        // the file is the complete body and not just its tail.
        if self.file.is_none() && self.total > cap as u64 {
            let Some(path) = self.path.clone() else { return };
            match std::fs::File::create(&path) {
                Ok(f) => {
                    let mut w = std::io::BufWriter::new(f);
                    // `inline` holds exactly the first `cap` bytes at this point.
                    if w.write_all(&self.inline).is_err() {
                        return;
                    }
                    self.written = self.inline.len() as u64;
                    self.file = Some(w);
                    // The current chunk's already-inlined prefix is on disk; the
                    // remainder is written by the tail-write below.
                    let inlined_from_this_chunk =
                        chunk.len() - (self.total - self.inline.len() as u64) as usize;
                    self.write(&chunk[inlined_from_this_chunk..]);
                    return;
                }
                Err(e) => {
                    tracing::warn!("cannot spill body to {}: {e}", path.display());
                    self.path = None;
                    return;
                }
            }
        }
        if self.file.is_some() {
            self.write(chunk);
        }
    }

    /// Append to the spill file, stopping at the per-body cap.
    fn write(&mut self, chunk: &[u8]) {
        let Some(file) = self.file.as_mut() else { return };
        let cap = self.store.per_body_cap();
        if self.written >= cap {
            return;
        }
        let room = (cap - self.written) as usize;
        let slice = &chunk[..room.min(chunk.len())];
        if let Err(e) = file.write_all(slice) {
            tracing::warn!("body spill write failed: {e}");
            self.file = None;
            self.path = None;
            return;
        }
        self.written += slice.len() as u64;
    }

    /// Finish the body: flush the spill file, register it against the disk
    /// budget, and report what was retained.
    pub fn finish(mut self) -> Captured {
        let mut spilled = false;
        if let Some(mut file) = self.file.take() {
            if file.flush().is_ok() {
                if let Some(path) = self.path.take() {
                    self.store.register(path, self.written);
                    spilled = true;
                }
            }
        }
        Captured {
            inline: self.inline,
            total: self.total,
            truncated: self.truncated,
            spilled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("novaproxy-bodystore-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn store(name: &str, inline: usize, per_body: u64, budget: u64) -> BodyStore {
        BodyStore::new(tmpdir(name), inline, per_body, budget)
    }

    #[test]
    fn small_body_stays_inline_and_never_touches_disk() {
        let s = store("small", 1024, DEFAULT_PER_BODY_CAP, DEFAULT_DISK_BUDGET);
        let mut c = BodyCapture::new(&s, "f1", Side::Response);
        c.push(b"hello ");
        c.push(b"world");
        let out = c.finish();
        assert_eq!(out.inline, b"hello world");
        assert_eq!(out.total, 11);
        assert!(!out.truncated);
        assert!(!out.spilled);
        assert_eq!(s.used_bytes(), 0);
        assert!(!s.path_for("f1", Side::Response).unwrap().exists());
    }

    #[test]
    fn large_body_spills_the_complete_stream_including_the_inline_prefix() {
        let s = store("large", 4, DEFAULT_PER_BODY_CAP, DEFAULT_DISK_BUDGET);
        let mut c = BodyCapture::new(&s, "f2", Side::Response);
        c.push(b"abcdefgh"); // one chunk straddling the inline cap
        c.push(b"ijkl");
        let out = c.finish();

        assert_eq!(out.inline, b"abcd", "preview is capped");
        assert_eq!(out.total, 12);
        assert!(out.truncated);
        assert!(out.spilled);

        // The file must be the WHOLE body — the classic bug here is writing only
        // the post-cap tail, which silently corrupts every large body.
        let path = s.path_for("f2", Side::Response).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"abcdefghijkl");
        assert_eq!(s.used_bytes(), 12);
    }

    #[test]
    fn spill_stops_at_the_per_body_cap() {
        let s = store("perbody", 2, 6, DEFAULT_DISK_BUDGET);
        let mut c = BodyCapture::new(&s, "f3", Side::Request);
        c.push(&[b'x'; 100]);
        let out = c.finish();
        assert_eq!(out.total, 100, "true wire size is still counted in full");
        assert!(out.truncated);
        let path = s.path_for("f3", Side::Request).unwrap();
        assert_eq!(std::fs::read(&path).unwrap().len(), 6, "capped at per_body_cap");
        assert_eq!(s.used_bytes(), 6);
    }

    #[test]
    fn exact_inline_fit_is_not_truncated_and_does_not_spill() {
        let s = store("exact", 5, DEFAULT_PER_BODY_CAP, DEFAULT_DISK_BUDGET);
        let mut c = BodyCapture::new(&s, "f4", Side::Request);
        c.push(b"12345");
        let out = c.finish();
        assert_eq!(out.inline, b"12345");
        assert!(!out.truncated, "a body that exactly fills the preview is complete");
        assert!(!out.spilled);
    }

    #[test]
    fn empty_body_produces_nothing() {
        let s = store("empty", 16, DEFAULT_PER_BODY_CAP, DEFAULT_DISK_BUDGET);
        let out = BodyCapture::new(&s, "f5", Side::Response).finish();
        assert!(out.inline.is_empty());
        assert_eq!(out.total, 0);
        assert!(!out.truncated);
        assert!(!out.spilled);
    }

    #[test]
    fn memory_only_store_caps_bodies_and_writes_no_files() {
        let s = BodyStore::memory_only(4);
        let mut c = BodyCapture::new(&s, "f6", Side::Response);
        c.push(b"abcdefgh");
        let out = c.finish();
        assert_eq!(out.inline, b"abcd");
        assert_eq!(out.total, 8);
        assert!(out.truncated);
        assert!(!out.spilled);
        assert!(s.path_for("f6", Side::Response).is_none());
    }

    #[test]
    fn disk_budget_evicts_the_oldest_files_first() {
        // Budget of 20 bytes; three 12-byte bodies must not exceed it.
        let s = store("budget", 2, DEFAULT_PER_BODY_CAP, 20);
        for id in ["a", "b", "c"] {
            let mut c = BodyCapture::new(&s, id, Side::Response);
            c.push(&[b'y'; 12]);
            c.finish();
        }
        assert!(s.used_bytes() <= 20, "used {} exceeds budget", s.used_bytes());
        assert!(
            !s.path_for("a", Side::Response).unwrap().exists(),
            "oldest spill file is evicted"
        );
        assert!(s.path_for("c", Side::Response).unwrap().exists(), "newest survives");
    }

    #[test]
    fn read_returns_the_spilled_bytes_and_reports_truncation() {
        let s = store("read", 2, DEFAULT_PER_BODY_CAP, DEFAULT_DISK_BUDGET);
        let mut c = BodyCapture::new(&s, "f7", Side::Response);
        c.push(b"0123456789");
        c.finish();

        let (bytes, total, truncated) = s.read("f7", Side::Response, 1024).unwrap();
        assert_eq!(bytes, b"0123456789");
        assert_eq!(total, 10);
        assert!(!truncated);

        let (bytes, total, truncated) = s.read("f7", Side::Response, 4).unwrap();
        assert_eq!(bytes, b"0123", "capped at max_bytes");
        assert_eq!(total, 10, "total is the full stored size");
        assert!(truncated);
    }

    #[test]
    fn read_of_an_unspilled_body_is_an_error_not_an_empty_body() {
        let s = store("missing", 1024, DEFAULT_PER_BODY_CAP, DEFAULT_DISK_BUDGET);
        assert!(s.read("nope", Side::Response, 1024).is_err());
    }

    #[test]
    fn remove_flow_deletes_both_sides_and_reclaims_the_budget() {
        let s = store("remove", 2, DEFAULT_PER_BODY_CAP, DEFAULT_DISK_BUDGET);
        for side in [Side::Request, Side::Response] {
            let mut c = BodyCapture::new(&s, "f8", side);
            c.push(&[b'z'; 10]);
            c.finish();
        }
        assert_eq!(s.used_bytes(), 20);
        s.remove_flow("f8");
        assert_eq!(s.used_bytes(), 0);
        assert!(!s.path_for("f8", Side::Request).unwrap().exists());
        assert!(!s.path_for("f8", Side::Response).unwrap().exists());
    }

    #[test]
    fn new_store_clears_files_left_by_a_previous_session() {
        let dir = tmpdir("stale");
        std::fs::create_dir_all(&dir).unwrap();
        let stale = dir.join("f0-res.bin");
        std::fs::write(&stale, b"leftover").unwrap();
        let _s = BodyStore::new(dir, 1024, DEFAULT_PER_BODY_CAP, DEFAULT_DISK_BUDGET);
        assert!(!stale.exists(), "stale spill files are unreferenced and must go");
    }

    #[test]
    fn path_for_sanitizes_the_flow_id() {
        let s = store("path", 16, DEFAULT_PER_BODY_CAP, DEFAULT_DISK_BUDGET);
        let p = s.path_for("../../etc/passwd", Side::Response).unwrap();
        assert_eq!(p.parent(), s.dir.as_deref(), "must stay inside the bodies dir");
        assert!(!p.to_string_lossy().contains(".."));
    }

    #[test]
    fn request_and_response_spill_to_distinct_files() {
        let s = store("sides", 1, DEFAULT_PER_BODY_CAP, DEFAULT_DISK_BUDGET);
        assert_ne!(
            s.path_for("f9", Side::Request),
            s.path_for("f9", Side::Response)
        );
    }
}
