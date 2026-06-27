//! Utility helpers: cryptography, password hashing, JWT, token & id generation,
//! self-signed certificate generation.

use anyhow::{anyhow, Context, Result};
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const COOKIE_NAME: &str = "mmk_token";

/// Combined tokio I/O trait so we can type-erase a `TcpStream` or `TlsStream`
/// behind a single `Box<dyn AsyncStream + Send + Unpin>`.
pub trait AsyncStream: tokio::io::AsyncRead + tokio::io::AsyncWrite {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite> AsyncStream for T {}

/// Install the rustls `ring` crypto provider as the process default. Safe to
/// call multiple times.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

// ---------------------------------------------------------------------------
// Password hashing (argon2id)
// ---------------------------------------------------------------------------

pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon = Argon2::default();
    let hash = argon
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow!("argon2 hash: {e}"))?
        .to_string();
    Ok(hash)
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

// ---------------------------------------------------------------------------
// Random ids / tokens
// ---------------------------------------------------------------------------

fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

pub fn gen_client_id() -> String {
    format!("cli_{}", URL_SAFE_NO_PAD.encode(&random_bytes(9)))
}

pub fn gen_mapping_id() -> String {
    format!("map_{}", URL_SAFE_NO_PAD.encode(&random_bytes(9)))
}

pub fn gen_token() -> String {
    format!("mmk_{}", URL_SAFE_NO_PAD.encode(&random_bytes(32)))
}

pub fn gen_jwt_secret() -> String {
    URL_SAFE_NO_PAD.encode(&random_bytes(48))
}

/// SHA-256 hex digest of a string (used to store client tokens without plaintext).
pub fn sha256_hex(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    hex::encode(hasher.finalize())
}

// ---------------------------------------------------------------------------
// JWT
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: i64,
}

pub fn make_jwt(username: &str, secret: &str, ttl_hours: u64) -> Result<String> {
    let claims = Claims {
        sub: username.to_string(),
        exp: (Utc::now() + Duration::hours(ttl_hours as i64)).timestamp(),
    };
    encode(&Header::default(), &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .map_err(|e| anyhow!("jwt encode: {e}"))
}

pub fn verify_jwt(token: &str, secret: &str) -> Result<Claims> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|e| anyhow!("jwt decode: {e}"))?;
    Ok(data.claims)
}

// ---------------------------------------------------------------------------
// Self-signed certificate generation (rcgen)
// ---------------------------------------------------------------------------

/// Generate a self-signed certificate + key pair in PEM form. Returns (cert_pem, key_pem).
pub fn gen_self_signed_cert(san: &[String]) -> Result<(String, String)> {
    let subject_alt_names: Vec<String> = if san.is_empty() {
        vec!["localhost".to_string(), "127.0.0.1".to_string()]
    } else {
        san.to_vec()
    };
    let certified = rcgen::generate_simple_self_signed(subject_alt_names)
        .map_err(|e| anyhow!("rcgen generate: {e}"))?;
    let cert_pem = certified.cert.pem();
    let key_pem = certified.key_pair.serialize_pem();
    Ok((cert_pem, key_pem))
}

/// Ensure a self-signed cert/key exist at the given paths, generating them if absent.
pub fn ensure_self_signed_cert(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
    san: &[String],
) -> Result<()> {
    if cert_path.exists() && key_path.exists() {
        return Ok(());
    }
    if let Some(parent) = cert_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    if let Some(parent) = key_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    tracing::info!("generating self-signed certificate at {}", cert_path.display());
    let (cert_pem, key_pem) = gen_self_signed_cert(san)
        .context("generate self-signed certificate")?;
    std::fs::write(cert_path, cert_pem).context("write cert.pem")?;
    std::fs::write(key_path, key_pem).context("write key.pem")?;
    Ok(())
}

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use std::io::BufReader;
use std::sync::Arc;

/// A certificate verifier that accepts any server certificate. Used by the
/// companion client to connect to the auto-generated self-signed server cert.
/// In production you should replace this with a pinned CA / TOFU verifier.
#[derive(Debug)]
struct NoVerifier;

impl ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
            .to_vec()
    }
}

/// Build a `TlsAcceptor` for the server from PEM cert + key files.
pub fn build_tls_acceptor(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> Result<tokio_rustls::TlsAcceptor> {
    let certs = load_certs(cert_path)?;
    let key = load_key(key_path)?;
    let cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| anyhow!("build server TLS config: {e}"))?;
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(cfg)))
}

/// Build a TLS connector that trusts any server certificate (for the
/// companion client connecting to a self-signed server).
pub fn build_dangerous_tls_connector() -> Result<tokio_rustls::TlsConnector> {
    let cfg = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerifier) as Arc<dyn ServerCertVerifier>)
        .with_no_client_auth();
    Ok(tokio_rustls::TlsConnector::from(Arc::new(cfg)))
}

fn load_certs(path: &std::path::Path) -> Result<Vec<CertificateDer<'static>>> {
    let pem = std::fs::read(path).with_context(|| format!("read cert {}", path.display()))?;
    let mut rd = BufReader::new(&pem[..]);
    rustls_pemfile::certs(&mut rd)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow!("parse certs: {e}"))
}

fn load_key(path: &std::path::Path) -> Result<PrivateKeyDer<'static>> {
    let pem = std::fs::read(path).with_context(|| format!("read key {}", path.display()))?;
    let mut rd = BufReader::new(&pem[..]);
    rustls_pemfile::private_key(&mut rd)
        .map_err(|e| anyhow!("parse key: {e}"))?
        .ok_or_else(|| anyhow!("no private key in {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let h = hash_password("hunter2").unwrap();
        assert!(verify_password("hunter2", &h));
        assert!(!verify_password("wrong", &h));
    }

    #[test]
    fn token_is_unique() {
        assert_ne!(gen_token(), gen_token());
        assert!(gen_token().starts_with("mmk_"));
    }

    #[test]
    fn jwt_roundtrip() {
        let secret = "secret";
        let t = make_jwt("admin", secret, 1).unwrap();
        let c = verify_jwt(&t, secret).unwrap();
        assert_eq!(c.sub, "admin");
        assert!(verify_jwt("bad", secret).is_err());
    }

    #[test]
    fn cert_generation() {
        let (c, k) = gen_self_signed_cert(&["localhost".into()]).unwrap();
        assert!(c.contains("BEGIN CERTIFICATE"));
        assert!(k.contains("BEGIN PRIVATE KEY"));
    }
}
