// Shared outbound HTTP client (usage endpoint, Web Push). rustls with the ring provider; the OS trust
// store verifies certificates, like Bun's fetch on the same host.
use std::time::Duration;

/// A client with the given overall request timeout. No redirects are followed, no proxy is used.
pub fn client(timeout: Duration) -> reqwest::Result<reqwest::Client> {
    // Installing twice is harmless; the first provider wins.
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
}
