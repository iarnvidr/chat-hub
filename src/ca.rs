//! Persistent, per-install interception CA. Trust is never installed automatically.
use rcgen::{CertificateParams, DistinguishedName, DnType, IsCa, KeyPair, PKCS_ECDSA_P256_SHA256};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub enum CaError {
    Io(std::io::Error),
    Certificate(rcgen::Error),
    Invalid(String),
}
impl std::fmt::Display for CaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for CaError {}
impl From<std::io::Error> for CaError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<rcgen::Error> for CaError {
    fn from(e: rcgen::Error) -> Self {
        Self::Certificate(e)
    }
}

pub struct Ca {
    pub cert_pem: String,
    cert: rcgen::Certificate,
    key: KeyPair,
    dir: PathBuf,
}
impl Ca {
    pub fn load_or_create(dir: impl AsRef<Path>) -> Result<Self, CaError> {
        let dir = dir.as_ref();
        fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        let cp = dir.join("ca.pem");
        let kp = dir.join("ca-key.pem");
        if cp.exists() || kp.exists() {
            if !cp.exists() || !kp.exists() {
                return Err(CaError::Invalid(
                    "incomplete CA pair; refusing implicit replacement".into(),
                ));
            }
            let cert_pem = fs::read_to_string(cp)?;
            let key_pem = fs::read_to_string(kp)?;
            let key = KeyPair::from_pem(&key_pem)?;
            let params = CertificateParams::from_ca_cert_pem(&cert_pem)?;
            let cert = params.clone().self_signed(&key)?;
            return Ok(Self {
                cert_pem,
                cert,
                key,
                dir: dir.to_path_buf(),
            });
        }
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
        let mut p = CertificateParams::new(Vec::<String>::new())?;
        p.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "Chat Hub local capture CA");
        p.distinguished_name = dn;
        p.key_usages = vec![
            rcgen::KeyUsagePurpose::KeyCertSign,
            rcgen::KeyUsagePurpose::CrlSign,
        ];
        p.use_authority_key_identifier_extension = true;
        let cert = p.clone().self_signed(&key)?;
        let pem = cert.pem();
        // Stage each file under a unique create_new name, then rename into place.
        // If either operation fails, leave detectable partial state; never overwrite it
        // implicitly on the next invocation.
        write_atomic(&kp, key.serialize_pem().as_bytes())?;
        write_atomic(&cp, pem.as_bytes())?;
        Ok(Self {
            cert_pem: pem,
            cert,
            key,
            dir: dir.to_path_buf(),
        })
    }
    /// DER-encoded public certificate for platform trust APIs.
    pub fn cert_der(&self) -> Result<Vec<u8>, CaError> {
        Ok(self.cert.der().as_ref().to_vec())
    }
    pub fn key_path(&self) -> PathBuf {
        self.dir.join("ca-key.pem")
    }
    pub fn leaf_identity(&self, host: &str) -> Result<(String, String), CaError> {
        if !allowed_capture_host(host) {
            return Err(CaError::Invalid("host is not interceptable".into()));
        }
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
        let mut params = CertificateParams::new(vec![host.to_owned()])?;
        params.not_before = time::OffsetDateTime::now_utc() - time::Duration::minutes(5);
        params.not_after = params.not_before + time::Duration::days(397);
        params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![
            rcgen::KeyUsagePurpose::DigitalSignature,
            rcgen::KeyUsagePurpose::KeyEncipherment,
        ];
        params.use_authority_key_identifier_extension = true;
        // RFC 5280 method-1 SKI: SHA-1 over the subjectPublicKey BIT STRING.
        let ski = ring::digest::digest(
            &ring::digest::SHA1_FOR_LEGACY_USE_ONLY,
            &key.public_key_der(),
        );
        let mut ski_der = vec![0x04, ski.as_ref().len() as u8];
        ski_der.extend_from_slice(ski.as_ref());
        params
            .custom_extensions
            .push(rcgen::CustomExtension::from_oid_content(
                &[2, 5, 29, 14],
                ski_der,
            ));
        let cert = params.signed_by(&key, &self.cert, &self.key)?.pem();
        Ok((cert, key.serialize_pem()))
    }
    pub fn leaf_pem(&self, host: &str) -> Result<String, CaError> {
        self.leaf_identity(host).map(|(cert, _)| cert)
    }
}

/// Capture the provider's exact web origin only; all other hosts tunnel.
pub fn allowed_capture_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("chatgpt.com")
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), CaError> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| CaError::Invalid("missing CA directory".into()))?;
    let name = path
        .file_name()
        .ok_or_else(|| CaError::Invalid("invalid CA filename".into()))?
        .to_string_lossy();
    let temp = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| -> Result<(), CaError> {
        let mut file = opts.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scopes_hosts() {
        assert!(allowed_capture_host("chatgpt.com"));
        assert!(!allowed_capture_host("api.chatgpt.com"));
        for h in [
            "accounts.chatgpt.com",
            "auth.chatgpt.com",
            "login.chatgpt.com",
            "signin.chatgpt.com",
            "nested.auth.chatgpt.com",
            "accounts.openai.com",
            "auth.openai.com",
            "evilchatgpt.com",
        ] {
            assert!(!allowed_capture_host(h));
        }
    }
    #[test]
    fn persists_ca_and_signs_after_reload() {
        let dir = std::env::temp_dir().join(format!("chat-hub-ca-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let ca = Ca::load_or_create(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(ca.key_path()).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(dir.join("ca.pem"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let original = ca.cert_pem.clone();
        let root_der = ca.cert_der().unwrap();
        let (_, root) = x509_parser::parse_x509_certificate(&root_der).unwrap();
        let root_bc = root.basic_constraints().unwrap().unwrap();
        assert!(root_bc.value.ca);
        assert!(root.key_usage().unwrap().unwrap().value.key_cert_sign());
        assert!(root.key_usage().unwrap().unwrap().value.crl_sign());
        assert!(root.extensions().iter().any(|e| matches!(
            e.parsed_extension(),
            x509_parser::extensions::ParsedExtension::SubjectKeyIdentifier(_)
        )));
        let (leaf, key) = ca.leaf_identity("chatgpt.com").unwrap();
        let leaf_der = rustls_pemfile::certs(&mut leaf.as_bytes())
            .next()
            .unwrap()
            .unwrap();
        let (_, parsed_leaf) = x509_parser::parse_x509_certificate(&leaf_der).unwrap();
        let leaf_aki = parsed_leaf
            .extensions()
            .iter()
            .find_map(|e| match e.parsed_extension() {
                x509_parser::extensions::ParsedExtension::AuthorityKeyIdentifier(aki) => {
                    aki.key_identifier.as_ref().map(|identifier| identifier.0)
                }
                _ => None,
            })
            .expect("leaf AKI must contain the issuer key identifier");
        let root_ski = root
            .extensions()
            .iter()
            .find_map(|e| match e.parsed_extension() {
                x509_parser::extensions::ParsedExtension::SubjectKeyIdentifier(ski) => Some(ski.0),
                _ => None,
            })
            .expect("root must contain an SKI");
        assert_eq!(leaf_aki, root_ski);
        assert!(parsed_leaf.extensions().iter().any(|e| matches!(
            e.parsed_extension(),
            x509_parser::extensions::ParsedExtension::SubjectKeyIdentifier(_)
        )));
        let leaf_bc = parsed_leaf.basic_constraints().unwrap().unwrap();
        assert!(!leaf_bc.value.ca);
        let eku = parsed_leaf.extended_key_usage().unwrap().unwrap();
        assert!(eku.value.server_auth);
        let san = parsed_leaf.subject_alternative_name().unwrap().unwrap();
        assert!(san.value.general_names.iter().any(|n| matches!(
            n,
            x509_parser::extensions::GeneralName::DNSName("chatgpt.com")
        )));
        assert!(
            parsed_leaf.validity().not_after.timestamp()
                - parsed_leaf.validity().not_before.timestamp()
                <= 397 * 86400
        );
        assert!(leaf.contains("BEGIN CERTIFICATE"));
        let certs = rustls_pemfile::certs(&mut leaf.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let private_key = rustls_pemfile::private_key(&mut key.as_bytes())
            .unwrap()
            .unwrap();
        rustls::ServerConfig::builder_with_provider(
            rustls::crypto::ring::default_provider().into(),
        )
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, private_key)
        .unwrap();
        drop(ca);
        let reloaded = Ca::load_or_create(&dir).unwrap();
        assert_eq!(reloaded.cert_pem, original);
        // A leaf signed after reload proves the persisted certificate and private key match.
        assert!(reloaded
            .leaf_pem("chatgpt.com")
            .unwrap()
            .contains("BEGIN CERTIFICATE"));
        assert!(Ca::load_or_create(&dir).unwrap().key_path().exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
