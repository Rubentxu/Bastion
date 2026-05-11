//! Automatic TLS Certificate Management
//!
//! Generates and manages CA and server/client certificates for mTLS.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DnType, ExtendedKeyUsagePurpose, Ia5String,
    IsCa, KeyPair, SanType,
};
use time::{Duration, OffsetDateTime};
use tokio::sync::OnceCell;
use tonic::transport::{
    Certificate as TonicCertificate, ClientTlsConfig, Identity, ServerTlsConfig,
};

static AUTO_TLS: OnceCell<AutoTls> = OnceCell::const_new();

/// Automatic TLS manager for the bastion gateway
#[derive(Clone)]
pub struct AutoTls {
    base_dir: PathBuf,
    ca_cert_pem: String,
    #[allow(dead_code)]
    ca_key_pem: String,
    /// Parsed CA certificate for signing (wrapped in Arc for cloneability)
    #[allow(dead_code)]
    ca_cert: Arc<Certificate>,
}

impl std::fmt::Debug for AutoTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutoTls")
            .field("base_dir", &self.base_dir)
            .field("ca_cert_pem", &"[redacted]")
            .field("ca_key_pem", &"[redacted]")
            .finish()
    }
}

/// Obtains the global AutoTls instance
pub fn get_auto_tls() -> &'static AutoTls {
    AUTO_TLS.get().expect("AutoTls not initialized")
}

/// Initialize AutoTls from the given base directory.
/// Generates CA + gateway cert on first run, loads on subsequent runs.
pub async fn init_or_load(base_dir: PathBuf) -> Result<&'static AutoTls> {
    AUTO_TLS
        .get_or_try_init(|| async { AutoTls::init_or_load(&base_dir).await })
        .await?;
    Ok(AUTO_TLS.get().expect("AutoTls not initialized"))
}

impl AutoTls {
    /// Initialize or load TLS certificates from the given base directory.
    /// Creates the directory structure if it doesn't exist.
    pub async fn init_or_load(base_dir: &Path) -> Result<Self> {
        let tls_dir = base_dir.join("tls");
        std::fs::create_dir_all(&tls_dir).context("Failed to create TLS directory")?;

        let ca_cert_path = tls_dir.join("ca-cert.pem");
        let ca_key_path = tls_dir.join("ca-key.pem");
        let gateway_cert_path = tls_dir.join("gateway-cert.pem");
        let gateway_key_path = tls_dir.join("gateway-key.pem");

        let (ca_cert_pem, ca_key_pem, ca_cert) = if ca_cert_path.exists() && ca_key_path.exists() {
            let cert_pem =
                std::fs::read_to_string(&ca_cert_path).context("Failed to read CA cert")?;
            let key_pem = std::fs::read_to_string(&ca_key_path).context("Failed to read CA key")?;
            let ca_cert = load_ca_certificate(&cert_pem, &key_pem)?;
            (cert_pem, key_pem, Arc::new(ca_cert))
        } else {
            // Generate new CA
            let (cert_pem, key_pem, ca_cert) = generate_ca()?;
            std::fs::write(&ca_cert_path, &cert_pem).context("Failed to write CA cert")?;
            std::fs::write(&ca_key_path, &key_pem).context("Failed to write CA key")?;
            set_file_permissions(&ca_cert_path, 0o644)?;
            set_file_permissions(&ca_key_path, 0o600)?;
            (cert_pem, key_pem, ca_cert)
        };

        // Generate/load gateway certificate
        if !gateway_cert_path.exists() || !gateway_key_path.exists() {
            let (cert, key) = generate_gateway_cert(&ca_cert, &ca_key_pem)?;
            std::fs::write(&gateway_cert_path, &cert).context("Failed to write gateway cert")?;
            std::fs::write(&gateway_key_path, &key).context("Failed to write gateway key")?;
            set_file_permissions(&gateway_cert_path, 0o644)?;
            set_file_permissions(&gateway_key_path, 0o600)?;
        }

        Ok(Self {
            base_dir: tls_dir,
            ca_cert_pem,
            ca_key_pem,
            ca_cert,
        })
    }

    /// Issue a client certificate for a worker with the given sandbox_id.
    /// Returns (cert_pem, key_pem).
    #[allow(dead_code)]
    pub fn issue_worker_cert(&self, sandbox_id: &str) -> Result<(String, String)> {
        generate_worker_cert(sandbox_id, &self.ca_cert, &self.ca_key_pem)
    }

    /// Get the path to the worker CA certificate
    #[allow(dead_code)]
    pub fn worker_ca_cert_path(&self) -> PathBuf {
        self.base_dir.join("ca-cert.pem")
    }

    /// Get a tonic ServerTlsConfig for the gateway
    pub fn server_config(&self) -> Result<ServerTlsConfig> {
        let cert_path = self.base_dir.join("gateway-cert.pem");
        let key_path = self.base_dir.join("gateway-key.pem");

        let cert = std::fs::read_to_string(&cert_path).context("Failed to read gateway cert")?;
        let key = std::fs::read_to_string(&key_path).context("Failed to read gateway key")?;

        let identity = Identity::from_pem(cert, key);

        // Load CA cert for client certificate verification (mTLS)
        let ca_cert = TonicCertificate::from_pem(self.ca_cert_pem.as_bytes());

        Ok(ServerTlsConfig::new()
            .identity(identity)
            .client_ca_root(ca_cert))
    }

    /// Get a tonic ClientTlsConfig for workers connecting to the gateway
    #[allow(dead_code)]
    pub fn client_config(&self) -> Result<ClientTlsConfig> {
        let ca_cert = TonicCertificate::from_pem(self.ca_cert_pem.as_bytes());

        Ok(ClientTlsConfig::new()
            .ca_certificate(ca_cert)
            .domain_name("bastion-gateway"))
    }

    /// Get the base TLS directory
    #[allow(dead_code)]
    pub fn base_dir(&self) -> &PathBuf {
        &self.base_dir
    }
}

/// Load a CA certificate from PEM strings
fn load_ca_certificate(ca_cert_pem: &str, ca_key_pem: &str) -> Result<Certificate> {
    let ca_key_pair = KeyPair::from_pem(ca_key_pem).context("Failed to parse CA key")?;
    let ca_params = CertificateParams::from_ca_cert_pem(ca_cert_pem)
        .context("Failed to parse CA certificate")?;
    // Create a Certificate from the params - this is just for storing/serializing
    // Note: we use self_signed since we're not actually using this cert for signing,
    // just for keeping the certificate object around
    let ca_cert = ca_params
        .self_signed(&ca_key_pair)
        .context("Failed to create CA certificate object")?;
    Ok(ca_cert)
}

/// Generate a self-signed CA certificate
fn generate_ca() -> Result<(String, String, Arc<Certificate>)> {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params
        .distinguished_name
        .push(DnType::CommonName, "Bastion CA");
    params
        .distinguished_name
        .push(DnType::OrganizationName, "Bastion");

    let not_before = OffsetDateTime::now_utc();
    let not_after = not_before + Duration::days(365 * 10); // 10 years
    params.not_before = not_before;
    params.not_after = not_after;

    let key_pair = KeyPair::generate().context("Failed to generate CA key pair")?;
    let cert = params
        .self_signed(&key_pair)
        .context("Failed to self-sign CA certificate")?;

    Ok((cert.pem(), key_pair.serialize_pem(), Arc::new(cert)))
}

/// Generate a gateway server certificate signed by the CA
fn generate_gateway_cert(ca_cert: &Certificate, ca_key_pem: &str) -> Result<(String, String)> {
    let ca_key_pair = KeyPair::from_pem(ca_key_pem).context("Failed to parse CA key")?;

    let mut params = CertificateParams::default();
    params
        .distinguished_name
        .push(DnType::CommonName, "bastion-gateway");
    params
        .distinguished_name
        .push(DnType::OrganizationName, "Bastion");

    let not_before = OffsetDateTime::now_utc();
    let not_after = not_before + Duration::days(365); // 1 year
    params.not_before = not_before;
    params.not_after = not_after;

    // Set Subject Alternative Names for the gateway
    params.subject_alt_names = vec![
        SanType::DnsName(Ia5String::try_from("localhost").context("Invalid DNS name")?),
        SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
    ];

    // Server auth extended key usage
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];

    let key_pair = KeyPair::generate().context("Failed to generate gateway key pair")?;

    // Serialize and sign with CA
    let cert = params
        .signed_by(&key_pair, ca_cert, &ca_key_pair)
        .context("Failed to sign gateway certificate with CA")?;

    Ok((cert.pem(), key_pair.serialize_pem()))
}

/// Generate a worker client certificate signed by the CA
#[allow(dead_code)]
fn generate_worker_cert(
    sandbox_id: &str,
    ca_cert: &Certificate,
    ca_key_pem: &str,
) -> Result<(String, String)> {
    let ca_key_pair = KeyPair::from_pem(ca_key_pem).context("Failed to parse CA key")?;

    let mut params = CertificateParams::default();
    params
        .distinguished_name
        .push(DnType::CommonName, sandbox_id);

    let not_before = OffsetDateTime::now_utc();
    let not_after = not_before + Duration::hours(24); // 24 hours
    params.not_before = not_before;
    params.not_after = not_after;

    // Client auth extended key usage
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];

    let key_pair = KeyPair::generate().context("Failed to generate worker key pair")?;

    // Serialize and sign with CA
    let cert = params
        .signed_by(&key_pair, ca_cert, &ca_key_pair)
        .context("Failed to sign worker certificate with CA")?;

    Ok((cert.pem(), key_pair.serialize_pem()))
}

fn set_file_permissions(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .context("Failed to set file permissions")?;
    }
    Ok(())
}
