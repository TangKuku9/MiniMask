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

/// SHA-256 hex digest of a string.
///
/// Previously used to store client token hashes; token hashing now goes through
/// [`sha256_hex_with_pepper`] (HMAC-SHA256 with a server-side pepper). Kept as a
/// public utility and for tests that compare peppered vs. plain digests.
#[allow(dead_code)]
pub fn sha256_hex(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    hex::encode(hasher.finalize())
}

/// SHA-256 hex digest of `token` mixed with a server-side `pepper` (P2-14).
///
/// The pepper is a per-deployment secret stored separately from `clients.json`
/// (in `data/token_pepper`). It prevents rainbow-table attacks against leaked
/// token hashes: an attacker who exfiltrates `clients.json` still cannot verify
/// candidate tokens without also obtaining the pepper.
///
/// We use HMAC-SHA256 to ensure proper domain separation between the token and
/// the pepper, rather than naive concatenation.
pub fn sha256_hex_with_pepper(token: &str, pepper: &str) -> String {
    use hmac::Mac;
    let mut hasher = hmac::Hmac::<Sha256>::new_from_slice(pepper.as_bytes())
        .expect("hmac key length always valid");
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize().into_bytes())
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
// Certificate generation (rcgen): self-signed CA + CA-signed server cert
// ---------------------------------------------------------------------------

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};

/// Generate a self-signed certificate + key pair in PEM form. Returns (cert_pem, key_pem).
///
/// Kept for backwards compatibility (e.g. the `gencert` CLI subcommand). The
/// server itself now uses [`gen_ca_and_server_cert`] for proper CA pinning.
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

/// Generate a self-signed CA certificate + a server certificate signed by that
/// CA. Returns `(ca_cert_pem, ca_key_pem, server_cert_pem, server_key_pem)`.
///
/// The CA certificate (`ca.pem`) is meant to be distributed to clients out of
/// band and pinned via [`build_tls_connector_with_ca`]. The server cert
/// (`cert.pem` / `key.pem`) is used by [`build_tls_acceptor`].
pub fn gen_ca_and_server_cert(
    san: &[String],
) -> Result<(String, String, String, String)> {
    // --- CA ---
    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    ca_params.distinguished_name = DistinguishedName::new();
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "MiniMask CA");
    let ca_key = KeyPair::generate().map_err(|e| anyhow!("generate CA key: {e}"))?;
    let ca_cert = ca_params
        .self_signed(&ca_key)
        .map_err(|e| anyhow!("self-sign CA: {e}"))?;

    // --- Server cert signed by CA ---
    let sans: Vec<String> = if san.is_empty() {
        vec!["localhost".to_string(), "127.0.0.1".to_string()]
    } else {
        san.to_vec()
    };
    let mut server_params = CertificateParams::new(sans)
        .map_err(|e| anyhow!("build server cert params: {e}"))?;
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    server_params.distinguished_name = DistinguishedName::new();
    server_params
        .distinguished_name
        .push(DnType::CommonName, "MiniMask Server");
    let server_key = KeyPair::generate().map_err(|e| anyhow!("generate server key: {e}"))?;
    let server_cert = server_params
        .signed_by(&server_key, &ca_cert, &ca_key)
        .map_err(|e| anyhow!("sign server cert: {e}"))?;

    Ok((
        ca_cert.pem(),
        ca_key.serialize_pem(),
        server_cert.pem(),
        server_key.serialize_pem(),
    ))
}

/// Ensure a CA + server cert/key exist at the given paths, generating them if
/// any of the four files is absent. The CA cert (`ca_path`) is meant to be
/// distributed to clients for CA pinning.
pub fn ensure_ca_and_server_cert(
    ca_path: &std::path::Path,
    ca_key_path: &std::path::Path,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
    san: &[String],
) -> Result<()> {
    if ca_path.exists() && ca_key_path.exists() && cert_path.exists() && key_path.exists() {
        return Ok(());
    }
    for p in [ca_path, ca_key_path, cert_path, key_path] {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).ok();
        }
    }
    tracing::info!("generating CA + server certificate (ca={}, cert={})", ca_path.display(), cert_path.display());
    let (ca_cert, ca_key, cert, key) = gen_ca_and_server_cert(san)
        .context("generate CA + server certificate")?;
    std::fs::write(ca_path, ca_cert).context("write ca.pem")?;
    std::fs::write(ca_key_path, ca_key).context("write ca_key.pem")?;
    std::fs::write(cert_path, cert).context("write cert.pem")?;
    std::fs::write(key_path, key).context("write key.pem")?;
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

/// Build a TLS connector that verifies the server certificate against a pinned
/// CA certificate. This is the default and recommended connector for clients.
///
/// The CA certificate is typically `data/ca.pem` distributed from the server
/// out of band. Clients must possess the matching CA to establish a tunnel,
/// preventing man-in-the-middle attacks even if an attacker intercepts the
/// 7443 port.
pub fn build_tls_connector_with_ca(
    ca_path: &std::path::Path,
) -> Result<tokio_rustls::TlsConnector> {
    let pem = std::fs::read(ca_path)
        .with_context(|| format!("read CA cert {}", ca_path.display()))?;
    let mut root_store = rustls::RootCertStore::empty();
    let mut reader = BufReader::new(&pem[..]);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow!("parse CA certs: {e}"))?;
    if certs.is_empty() {
        return Err(anyhow!(
            "no certificates found in CA file {}",
            ca_path.display()
        ));
    }
    for cert in certs {
        root_store
            .add(cert)
            .map_err(|e| anyhow!("add CA cert to root store: {e}"))?;
    }
    let cfg = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    Ok(tokio_rustls::TlsConnector::from(Arc::new(cfg)))
}

/// Build a TLS connector that trusts any server certificate. This completely
/// disables TLS verification and is **insecure**.
///
/// Kept only for local debugging (e.g. behind the hidden `--insecure-skip-verify`
/// client flag). Production deployments must use [`build_tls_connector_with_ca`].
pub fn build_dangerous_tls_connector() -> Result<tokio_rustls::TlsConnector> {
    tracing::warn!("building TLS connector with certificate verification DISABLED — use only for local debugging");
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
    fn pepper_changes_hash() {
        // P2-14: the same token hashed with different peppers must yield
        // different digests, and an empty pepper must differ from a non-empty
        // one. This verifies the pepper is actually mixed into the HMAC.
        let h1 = sha256_hex_with_pepper("mmk_token", "pepper-a");
        let h2 = sha256_hex_with_pepper("mmk_token", "pepper-b");
        assert_ne!(h1, h2, "different peppers must yield different hashes");
        // Verify against the plain SHA-256 too (no pepper).
        let plain = sha256_hex("mmk_token");
        assert_ne!(h1, plain, "peppered hash must differ from plain hash");
        // Deterministic for the same inputs.
        assert_eq!(h1, sha256_hex_with_pepper("mmk_token", "pepper-a"));
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

    #[test]
    fn ca_and_server_cert_generation() {
        let (ca_cert, ca_key, srv_cert, srv_key) =
            gen_ca_and_server_cert(&["localhost".into(), "127.0.0.1".into()]).unwrap();
        // All four PEM blobs must be present and well-formed.
        assert!(ca_cert.contains("BEGIN CERTIFICATE"), "CA cert PEM missing");
        assert!(ca_key.contains("BEGIN PRIVATE KEY"), "CA key PEM missing");
        assert!(srv_cert.contains("BEGIN CERTIFICATE"), "server cert PEM missing");
        assert!(srv_key.contains("BEGIN PRIVATE KEY"), "server key PEM missing");
        // The server cert and CA cert must be different blobs.
        assert_ne!(ca_cert, srv_cert, "CA cert and server cert are identical");

        // The CA cert must be loadable into a rustls root store (i.e. be a
        // valid PEM certificate), and the resulting connector must be usable.
        let tmp = std::env::temp_dir().join(format!(
            "minimask_test_ca_{}.pem",
            std::process::id()
        ));
        std::fs::write(&tmp, &ca_cert).unwrap();
        let connector = build_tls_connector_with_ca(&tmp);
        assert!(connector.is_ok(), "CA pinning connector failed to build");
        let _ = std::fs::remove_file(&tmp);
    }
}
