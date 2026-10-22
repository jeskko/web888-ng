// SPDX-License-Identifier: GPL-3.0-or-later
//! TLS certificate: configured files, or a self-signed one generated on
//! first start and kept in the state directory (so browsers' exceptions
//! survive restarts).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

#[derive(Clone, Debug)]
pub enum Tls {
    /// Plain HTTP only.
    Off,
    /// Self-signed, stored as `cert.pem`/`key.pem` under the directory.
    SelfSigned(PathBuf),
    Files {
        cert: PathBuf,
        key: PathBuf,
    },
}

type AnyResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn hostname() -> Option<String> {
    let h = std::fs::read_to_string("/proc/sys/kernel/hostname").ok()?;
    let h = h.trim();
    (!h.is_empty()).then(|| h.to_string())
}

/// A new self-signed certificate and key (PEM).
pub fn self_signed() -> AnyResult<(String, String)> {
    let mut names = vec!["localhost".to_string(), "127.0.0.1".into(), "::1".into()];
    if let Some(h) = hostname() {
        names.push(format!("{h}.local"));
        names.push(h);
    }
    let mut params = rcgen::CertificateParams::new(names)?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "web888d (self-signed)");
    let key = rcgen::KeyPair::generate()?;
    let cert = params.self_signed(&key)?;
    Ok((cert.pem(), key.serialize_pem()))
}

/// Read the stored self-signed pair, or make and store one. If the
/// directory isn't writable, the new pair lives only in memory (and
/// browsers ask again after each restart).
fn load_or_create(dir: &Path) -> AnyResult<(String, String)> {
    let (cp, kp) = (dir.join("cert.pem"), dir.join("key.pem"));
    if let (Ok(c), Ok(k)) = (std::fs::read_to_string(&cp), std::fs::read_to_string(&kp)) {
        return Ok((c, k));
    }
    let (c, k) = self_signed()?;
    let stored = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&cp, &c))
        .and_then(|()| {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&kp)?
                .write_all(k.as_bytes())
        });
    match stored {
        Ok(()) => eprintln!("web: new self-signed certificate in {}", dir.display()),
        Err(e) => eprintln!(
            "web: self-signed certificate not stored ({}: {e}); browsers will ask again after a restart",
            dir.display()
        ),
    }
    Ok((c, k))
}

pub fn acceptor(tls: &Tls) -> AnyResult<Option<TlsAcceptor>> {
    let (cert, key) = match tls {
        Tls::Off => return Ok(None),
        Tls::SelfSigned(dir) => load_or_create(dir)?,
        Tls::Files { cert, key } => (
            std::fs::read_to_string(cert).map_err(|e| format!("{}: {e}", cert.display()))?,
            std::fs::read_to_string(key).map_err(|e| format!("{}: {e}", key.display()))?,
        ),
    };
    let certs = CertificateDer::pem_slice_iter(cert.as_bytes()).collect::<Result<Vec<_>, _>>()?;
    let key = PrivateKeyDer::from_pem_slice(key.as_bytes())?;
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let mut cfg = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    // WebSockets need HTTP/1.1 (hyper has no RFC 8441 extended CONNECT here)
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Some(TlsAcceptor::from(Arc::new(cfg))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_signed_pair_is_reused() {
        let dir = std::env::temp_dir().join(format!("web888-tls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = load_or_create(&dir).unwrap();
        let b = load_or_create(&dir).unwrap();
        assert_eq!(a, b);
        assert!(acceptor(&Tls::SelfSigned(dir.clone())).unwrap().is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
