//! Google Drive v3 transport for the Wasl sync engine.
//!
//! A hand-written REST client on `reqwest` + `serde` (no Drive client library, no external sync
//! tool), ported from the Android app's `drive` package:
//!
//! - [`DriveClient`]: metadata, listing, the Changes feed, revisions, resumable downloads and
//!   uploads, with the app's retry policy (one token refresh, exponential backoff, `Retry-After`).
//! - [`TokenProvider`] and [`SessionStore`]: injected; the OAuth flow and the SQLite session store
//!   live in other crates.
//! - [`DriveNodeCache`]: the account-wide metadata cache (one crawl, then Changes deltas).
//! - [`BandwidthLimiter`]: the token bucket the transfer loops share.

mod client;
mod download;
mod error;
mod limiter;
mod models;
mod node_cache;
mod session;
mod time;
mod upload;

pub use client::{DriveClient, DriveClientConfig, DriveEndpoints, ListPage, TokenProvider};
pub use download::{DownloadProgress, md5_file};
pub use error::{DriveError, Result};
pub use limiter::BandwidthLimiter;
pub use models::*;
pub use node_cache::{
    AccountState, DriveNode, DriveNodeCache, MemoryNodeStore, NodeStore, RemoteEntry, Snapshot,
    SubtreeListing,
};
pub use session::{MemorySessionStore, SessionStore};
pub use time::{parse_rfc3339, to_rfc3339};
pub use upload::{SessionRef, UploadProgress, UploadRequest, session_store_key};
