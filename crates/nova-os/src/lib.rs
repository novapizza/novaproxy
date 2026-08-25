//! OS plumbing for NovaProxy: what to run, how to elevate it, and the privileged
//! helper that makes elevation a one-time cost.
//!
//! This is deliberately a separate crate from `nova-core`. [`helper`]'s server
//! side runs as **root**, and a root process should link the smallest possible
//! dependency tree — not the MITM engine, its TLS stack and its JS runtime.
//! `nova-core` re-exports [`oscmd`] and [`sysproxy`], so callers keep using
//! `nova_core::sysproxy::…` as before.

pub mod appicon;
pub mod helper;
pub mod oscmd;
pub mod sysproxy;
