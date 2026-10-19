// HTTP(S) listeners for the main server and the preview ports (Bun.serve's role in the server).
// HTTP/1.1 with WebSocket upgrades; TLS through rustls with a certificate that can be swapped in place,
// so renewed or upgraded certificates apply to new connections without restarting the listener.
use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, Response};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;

use crate::tls::TlsMaterial;

const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

pub type HandlerFuture = Pin<Box<dyn Future<Output = Response<Body>> + Send>>;
/// The request handler: the request and the peer's IP address as Bun's `requestIP` reports it.
pub type Handler = Arc<dyn Fn(Request<Body>, Option<String>) -> HandlerFuture + Send + Sync>;

/// The server certificate, replaceable while listeners run.
pub struct CertResolver {
    key: RwLock<Arc<CertifiedKey>>,
}

impl fmt::Debug for CertResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CertResolver")
    }
}

impl CertResolver {
    pub fn new(m: &TlsMaterial) -> Result<Arc<Self>, String> {
        Ok(Arc::new(CertResolver {
            key: RwLock::new(certified_key(m)?),
        }))
    }

    /// Use `m` for every new connection; open connections keep the old certificate until they close.
    pub fn replace(&self, m: &TlsMaterial) -> Result<(), String> {
        let key = certified_key(m)?;
        *self.key.write().unwrap_or_else(|e| e.into_inner()) = key;
        Ok(())
    }

    pub fn acceptor(self: &Arc<Self>) -> TlsAcceptor {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut cfg = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(self.clone());
        cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
        TlsAcceptor::from(Arc::new(cfg))
    }
}

impl ResolvesServerCert for CertResolver {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.key.read().unwrap_or_else(|e| e.into_inner()).clone())
    }
}

fn certified_key(m: &TlsMaterial) -> Result<Arc<CertifiedKey>, String> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&m.cert_pem)
        .collect::<Result<_, _>>()
        .map_err(|_| "unreadable certificate".to_string())?;
    if certs.is_empty() {
        return Err("no certificate".into());
    }
    let key =
        PrivateKeyDer::from_pem_slice(&m.key_pem).map_err(|_| "unreadable key".to_string())?;
    let signer = rustls::crypto::ring::sign::any_supported_type(&key)
        .map_err(|_| "unsupported key".to_string())?;
    Ok(Arc::new(CertifiedKey::new(certs, signer)))
}

/// Serves `listener` until `shutdown` turns true; then stops accepting and drops every open
/// connection (Bun's `server.stop(true)`).
pub async fn serve(
    listener: TcpListener,
    tls: Option<TlsAcceptor>,
    handler: Handler,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut conns = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, peer)) = accepted else { continue };
                let _ = stream.set_nodelay(true);
                let handler = handler.clone();
                let tls = tls.clone();
                conns.spawn(async move {
                    match tls {
                        Some(acceptor) => {
                            let Ok(Ok(stream)) = tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await else {
                                return;
                            };
                            serve_conn(TokioIo::new(stream), peer, handler).await;
                        }
                        None => serve_conn(TokioIo::new(stream), peer, handler).await,
                    }
                });
            }
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
        }
    }
    conns.shutdown().await;
}

async fn serve_conn<I>(io: TokioIo<I>, peer: SocketAddr, handler: Handler)
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let ip = peer.ip().to_string();
    let service = hyper::service::service_fn(move |req: Request<Incoming>| {
        let fut = handler(req.map(Body::new), Some(ip.clone()));
        async move { Ok::<_, std::convert::Infallible>(fut.await) }
    });
    let _ = hyper::server::conn::http1::Builder::new()
        .serve_connection(io, service)
        .with_upgrades()
        .await;
}
