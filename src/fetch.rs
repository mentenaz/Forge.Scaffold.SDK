//! The only door to the network.
//!
//! The crate never opens a connection itself. The host (the Forge panel)
//! implements [`Fetch`] with whatever HTTP client it already has, and may run
//! the calls on a background thread. That keeps this crate free of an async
//! runtime and testable without a network.

use crate::error::Result;

/// Answer to a [`Fetch::get`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchResult {
    /// The server said "304 Not Modified" for the `etag` we sent.
    NotModified,
    /// A full response body.
    Body { bytes: Vec<u8>, etag: Option<String> },
}

/// Minimal HTTP GET.
///
/// Implementations should follow redirects, send `If-None-Match: <etag>` when
/// `etag` is given, and map HTTP errors (status >= 400) and network failures to
/// [`crate::ScaffoldError::Fetch`]. Only `https` URLs are ever passed in.
pub trait Fetch {
    fn get(&self, url: &str, etag: Option<&str>) -> Result<FetchResult>;
}
