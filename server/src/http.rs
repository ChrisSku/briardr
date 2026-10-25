// Shared outbound HTTP clients (usage endpoint, Web Push, `briardr setup`'s download). rustls with the ring provider; the OS trust
// store verifies certificates, like Bun's fetch on the same host.
use std::time::Duration;

/// A client with the given overall request timeout. No redirects are followed, no proxy is used.
pub fn client(timeout: Duration) -> reqwest::Result<reqwest::Client> {
    builder(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

/// A client for downloads (`briardr setup` installing Herdr), which follows a few redirects: GitHub
/// sends release downloads on to its storage.
pub fn downloader(timeout: Duration) -> reqwest::Result<reqwest::Client> {
    builder(timeout)
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
}

fn builder(timeout: Duration) -> reqwest::ClientBuilder {
    // Installing twice is harmless; the first provider wins.
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder().timeout(timeout).no_proxy()
}
