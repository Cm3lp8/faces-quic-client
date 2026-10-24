//! Server authentication for the existing QUIC transport, independent of the host.
//! BoringSSL verifies the chain, validity and handshake signature. WebPKI verifies
//! the DNS/IP SAN before HTTP/3 or any queued application request is admitted.
use rustls_pki_types::{CertificateDer, ServerName};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct ServerTlsConfig {
    identity: ServerName<'static>,
    trust_pem: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerTlsError {
    InvalidIdentity,
    InvalidTrustFile,
    CertificateMissing,
    IdentityMismatch,
    Configuration,
}
impl std::fmt::Display for ServerTlsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "server TLS authentication failed: {self:?}")
    }
}
impl std::error::Error for ServerTlsError {}

impl ServerTlsConfig {
    pub fn system_roots(identity: &str) -> Result<Self, ServerTlsError> {
        let identity = ServerName::try_from(identity.to_owned())
            .map_err(|_| ServerTlsError::InvalidIdentity)?;
        Ok(Self {
            identity,
            trust_pem: None,
        })
    }

    /// The public trust anchor must come from trusted configuration, never from
    /// an unverified first connection. A missing/malformed file is an error.
    pub fn with_trust_pem(identity: &str, file: impl AsRef<Path>) -> Result<Self, ServerTlsError> {
        let mut config = Self::system_roots(identity)?;
        config.trust_pem = Some(file.as_ref().to_path_buf());
        config.quic_config()?;
        Ok(config)
    }

    pub(crate) fn quic_config(&self) -> Result<quiche::Config, ServerTlsError> {
        let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION)
            .map_err(|_| ServerTlsError::Configuration)?;
        config.verify_peer(true);
        if let Some(file) = &self.trust_pem {
            let file = file
                .to_str()
                .filter(|s| !s.contains('\0'))
                .ok_or(ServerTlsError::InvalidTrustFile)?;
            config
                .load_verify_locations_from_file(file)
                .map_err(|_| ServerTlsError::InvalidTrustFile)?;
        }
        Ok(config)
    }

    pub(crate) fn sni(&self) -> Option<&str> {
        match &self.identity {
            ServerName::DnsName(name) => Some(name.as_ref()),
            // quiche 0.23 uses X509_VERIFY_PARAM_set1_host, which is a DNS check.
            // An IP is checked against the IP SAN below; it is not sent as SNI.
            _ => None,
        }
    }

    pub(crate) fn verify_identity(&self, peer_der: Option<&[u8]>) -> Result<(), ServerTlsError> {
        let der = CertificateDer::from(peer_der.ok_or(ServerTlsError::CertificateMissing)?);
        let certificate =
            webpki::EndEntityCert::try_from(&der).map_err(|_| ServerTlsError::IdentityMismatch)?;
        certificate
            .verify_is_valid_for_subject_name(&self.identity)
            .map_err(|_| ServerTlsError::IdentityMismatch)
    }
}

#[cfg(test)]
mod tests;
