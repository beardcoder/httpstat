//! TLS setup: rustls with the ring provider and the webpki root store.
//!
//! Configurations are built once and shared. Parsing a root store on every
//! request made `--count 100` measure our own start-up cost as part of the
//! handshake phase.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme};

use crate::error::{Error, Result};

/// How certificates should be verified.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Trust {
    /// Verify against the roots bundled with the binary.
    Bundled,
    /// Verify against a PEM bundle instead of the bundled roots, the way
    /// `curl --cacert` does. Needed for private PKI and TLS-inspecting proxies.
    File(PathBuf),
    /// Accept any certificate (`-k/--insecure`).
    None,
}

impl Trust {
    pub fn resolve(insecure: bool, ca_file: Option<&Path>) -> Trust {
        match (insecure, ca_file) {
            (true, _) => Trust::None,
            (false, Some(path)) => Trust::File(path.to_path_buf()),
            (false, None) => Trust::Bundled,
        }
    }
}

/// Built configurations, keyed by the trust setting that produced them.
type ConfigCache = Mutex<Vec<(Trust, Arc<ClientConfig>)>>;

/// The shared client configuration for a trust setting, built at most once.
pub fn config(trust: &Trust) -> Result<Arc<ClientConfig>> {
    static CACHE: OnceLock<ConfigCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(Vec::new()));
    // A poisoned lock here would mean a panic while building a config; there is
    // nothing to recover, so start the cache over rather than propagating it.
    let mut entries = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, config)) = entries.iter().find(|(key, _)| key == trust) {
        return Ok(Arc::clone(config));
    }
    let config = Arc::new(build(trust)?);
    entries.push((trust.clone(), Arc::clone(&config)));
    Ok(config)
}

fn build(trust: &Trust) -> Result<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .expect("the ring provider supports the default protocol versions");

    let mut config = match trust {
        Trust::None => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify(provider)))
            .with_no_client_auth(),
        Trust::Bundled => builder
            .with_root_certificates(bundled_roots())
            .with_no_client_auth(),
        Trust::File(path) => builder
            .with_root_certificates(roots_from_file(path)?)
            .with_no_client_auth(),
    };
    // The client speaks HTTP/1.1 only; advertising it avoids a server selecting
    // HTTP/2 and then talking a protocol we cannot parse.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

fn bundled_roots() -> rustls::RootCertStore {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    roots
}

/// Load a PEM bundle as the complete set of trusted roots.
pub fn roots_from_file(path: &Path) -> Result<rustls::RootCertStore> {
    let mut roots = rustls::RootCertStore::empty();
    let certificates = CertificateDer::pem_file_iter(path)
        .map_err(|e| Error::usage(format!("could not read CA bundle {}: {e}", path.display())))?;
    for certificate in certificates {
        let certificate = certificate.map_err(|e| {
            Error::usage(format!(
                "could not parse a certificate in {}: {e}",
                path.display()
            ))
        })?;
        roots.add(certificate).map_err(|e| {
            Error::usage(format!(
                "certificate in {} was rejected: {e}",
                path.display()
            ))
        })?;
    }
    if roots.is_empty() {
        return Err(Error::usage(format!(
            "no certificates found in CA bundle {}",
            path.display()
        )));
    }
    Ok(roots)
}

/// Certificate verifier used for `-k/--insecure`: accepts any certificate chain
/// while still checking that handshake signatures are well formed.
#[derive(Debug)]
struct NoVerify(Arc<CryptoProvider>);

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configs_are_built_once_and_shared_per_trust_setting() {
        let a = config(&Trust::Bundled).unwrap();
        let b = config(&Trust::Bundled).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(!Arc::ptr_eq(&a, &config(&Trust::None).unwrap()));
    }

    #[test]
    fn http_1_1_is_the_only_advertised_protocol() {
        assert_eq!(
            config(&Trust::Bundled).unwrap().alpn_protocols,
            vec![b"http/1.1".to_vec()]
        );
    }

    #[test]
    fn trust_is_resolved_from_the_flags_with_insecure_winning() {
        let path = Path::new("/tmp/ca.pem");
        assert_eq!(Trust::resolve(false, None), Trust::Bundled);
        assert_eq!(Trust::resolve(false, Some(path)), Trust::File(path.into()));
        assert_eq!(Trust::resolve(true, Some(path)), Trust::None);
    }

    #[test]
    fn a_missing_or_empty_ca_bundle_is_a_usage_error() {
        let missing = roots_from_file(Path::new("/nonexistent/ca.pem")).unwrap_err();
        assert_eq!(missing.exit_code(), crate::error::EXIT_USAGE);
        assert!(
            missing.to_string().contains("could not read CA bundle"),
            "{missing}"
        );

        let empty =
            std::env::temp_dir().join(format!("httpstat_empty_ca_{}.pem", std::process::id()));
        std::fs::write(&empty, b"not a certificate\n").unwrap();
        let error = roots_from_file(&empty).unwrap_err();
        assert!(
            error.to_string().contains("no certificates found"),
            "{error}"
        );
        let _ = std::fs::remove_file(&empty);
    }
}
