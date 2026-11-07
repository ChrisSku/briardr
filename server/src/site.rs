// Show a file of a stream's folder as a web page inside Briardr: an HTML file with the CSS, scripts,
// images and other pages it links to by relative path.
//
// The page runs in a sandboxed frame without its own origin, so it can't act as Briardr, and a frame
// like that doesn't send the device cookie with its requests. A link is therefore a random token in
// the path (`/site/<token>/<path>`), minted for one stream and device and valid for twelve hours. It
// stops working when the device is revoked, the stream goes away or the Files plugin is turned off.
// Every file is resolved inside the stream's folder, as Files does.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use crate::app::{now_ms, response, App, Resp};
use crate::docs::resolve_inside;

const TTL_MS: u64 = 12 * 3600 * 1000;
const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
/// Scripts may run, but the page can reach only its own folder: no other sites, no forms, no frames.
const CSP: &str = "default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; \
img-src 'self' data: blob:; font-src 'self' data:; media-src 'self' data: blob:; connect-src 'self'; \
form-action 'none'; base-uri 'self'; sandbox allow-scripts allow-popups allow-popups-to-escape-sandbox";
/// Opens links to other sites in a new tab: the frame itself can't show them.
const LINKS: &str = "<script>document.addEventListener('click',function(e){var a=e.target.closest&&e.target.closest('a[href]');if(!a)return;var u;try{u=new URL(a.href)}catch(x){return}if(/^https?:$/.test(u.protocol)&&u.origin!==location.origin){e.preventDefault();window.open(u.href,'_blank','noopener')}},true)</script>";

struct Link {
    stream: String,
    device: String,
    expires: u64,
}

#[derive(Default)]
pub struct SiteLinks(Mutex<HashMap<String, Link>>);

impl SiteLinks {
    /// A link for this stream and device: the one it already has, or a new one.
    pub fn mint(&self, stream: &str, device: &str) -> String {
        let now = now_ms();
        let mut links = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        links.retain(|_, l| l.expires > now);
        // Reused only while more than half its time is left, so a page open for hours isn't cut off.
        if let Some((token, _)) = links
            .iter()
            .find(|(_, l)| l.stream == stream && l.device == device && l.expires > now + TTL_MS / 2)
        {
            return token.clone();
        }
        let token = hex::encode(rand::random::<[u8; 24]>());
        links.insert(
            token.clone(),
            Link {
                stream: stream.to_string(),
                device: device.to_string(),
                expires: now + TTL_MS,
            },
        );
        token
    }

    fn lookup(&self, token: &str) -> Option<(String, String)> {
        let links = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let l = links.get(token).filter(|l| l.expires > now_ms())?;
        Some((l.stream.clone(), l.device.clone()))
    }
}

fn not_found() -> Resp {
    response(
        404,
        "Not found",
        &[("content-type", "text/plain; charset=utf-8".to_string())],
    )
}

/// Serves `rest`, the part of the path after `/site/`: `<token>/<path in the stream's folder>`.
pub async fn serve(app: &App, rest: &str) -> Resp {
    let Some((token, path)) = rest.split_once('/') else {
        return not_found();
    };
    let Some((stream, device)) = app.site_links.lookup(token) else {
        return not_found();
    };
    if !app.d.auth.list().iter().any(|d| d.id == device) || !app.plugins.get().is_enabled("files") {
        return not_found();
    }
    let Some(path) = crate::app::decode_uri_component(path) else {
        return not_found();
    };
    let Some(cwd) = app
        .stream_with_folder(&stream)
        .ok()
        .and_then(|s| s.cwd.map(PathBuf::from))
    else {
        return not_found();
    };
    let found = tokio::task::spawn_blocking(move || {
        let mut abs = resolve_inside(&cwd, &path).ok()?;
        if abs.is_dir() {
            abs =
                resolve_inside(&cwd, &format!("{}/index.html", path.trim_end_matches('/'))).ok()?;
        }
        let meta = std::fs::metadata(&abs).ok()?;
        (meta.is_file() && meta.len() <= MAX_FILE_BYTES).then(|| (std::fs::read(&abs).ok(), abs))
    })
    .await
    .ok()
    .flatten();
    let Some((Some(mut body), abs)) = found else {
        return not_found();
    };
    let mime = crate::static_files::mime_for(&abs.to_string_lossy());
    if mime.starts_with("text/html") {
        body = with_link_script(body);
    }
    response(
        200,
        body,
        &[
            ("content-type", mime.to_string()),
            ("cache-control", "no-store".to_string()),
            ("content-security-policy", CSP.to_string()),
            // The frame has no origin of its own, so its requests count as cross-origin.
            ("cross-origin-resource-policy", "cross-origin".to_string()),
        ],
    )
}

/// The page with the link script before `</head>`, or at the end when it has no head.
fn with_link_script(body: Vec<u8>) -> Vec<u8> {
    let at = body
        .windows(7)
        .position(|w| w.eq_ignore_ascii_case(b"</head>"))
        .unwrap_or(body.len());
    let mut out = Vec::with_capacity(body.len() + LINKS.len());
    out.extend_from_slice(&body[..at]);
    out.extend_from_slice(LINKS.as_bytes());
    out.extend_from_slice(&body[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_link_script_goes_before_the_head_ends() {
        let out = String::from_utf8(with_link_script(
            b"<!doctype html><HEAD></HEAD><body>x".to_vec(),
        ))
        .unwrap();
        assert!(out.starts_with("<!doctype html><HEAD><script>"));
        assert!(out.ends_with("</script></HEAD><body>x"));
        let out = String::from_utf8(with_link_script(b"<p>x".to_vec())).unwrap();
        assert!(out.starts_with("<p>x<script>"));
    }

    #[test]
    fn links_are_reused_then_expire_per_device() {
        let links = SiteLinks::default();
        let a = links.mint("s1", "d1");
        assert_eq!(links.mint("s1", "d1"), a);
        assert_ne!(links.mint("s1", "d2"), a);
        assert_eq!(links.lookup(&a), Some(("s1".into(), "d1".into())));
        assert_eq!(links.lookup("nope"), None);
    }
}
