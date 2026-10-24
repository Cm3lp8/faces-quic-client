use super::*;
use std::{
    fs,
    process::{Command, Stdio},
};

struct Certificate {
    root: PathBuf,
    cert: PathBuf,
    key: PathBuf,
}
impl Certificate {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("fragments-tls-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let config = root.join("openssl.cnf");
        fs::write(&config, "[req]\nprompt=no\ndistinguished_name=dn\nx509_extensions=ext\n[dn]\nCN=auth.test.invalid\n[ext]\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:auth.test.invalid,IP:127.0.0.1\n").unwrap();
        let cert = root.join("cert.pem");
        let key = root.join("test-only-key.pem");
        assert!(
            Command::new("openssl")
                .args([
                    "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1"
                ])
                .arg("-config")
                .arg(config)
                .arg("-keyout")
                .arg(&key)
                .arg("-out")
                .arg(&cert)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
        Self { root, cert, key }
    }
    fn expire(&mut self) {
        let expired = self.root.join("expired.pem");
        let request = self.root.join("request.pem");
        fs::write(self.root.join("index"), "").unwrap();
        fs::write(self.root.join("serial"), "01\n").unwrap();
        let config = self.root.join("ca.cnf");
        let mut source = fs::read_to_string(self.root.join("openssl.cnf")).unwrap();
        source += &format!(
            "[ca]\ndefault_ca=CA_default\n[CA_default]\ndatabase={}/index\nserial={}/serial\nnew_certs_dir={}\ncertificate={}\nprivate_key={}\ndefault_md=sha256\npolicy=policy\n[policy]\ncommonName=supplied\n",
            self.root.display(),
            self.root.display(),
            self.root.display(),
            self.cert.display(),
            self.key.display()
        );
        fs::write(&config, source).unwrap();
        assert!(
            Command::new("openssl")
                .args(["req", "-new", "-key"])
                .arg(&self.key)
                .arg("-config")
                .arg(self.root.join("openssl.cnf"))
                .arg("-out")
                .arg(&request)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
        let output = Command::new("openssl")
            .args(["ca", "-selfsign", "-batch", "-config"])
            .arg(config)
            .args([
                "-startdate",
                "20200101000000Z",
                "-enddate",
                "20200102000000Z",
                "-extensions",
                "ext",
                "-in",
            ])
            .arg(request)
            .arg("-out")
            .arg(&expired)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        self.cert = expired;
    }
}
impl Drop for Certificate {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

// Real QUIC/TLS handshakes, no socket and no application data. The production
// identity gate is exercised only after the TLS stack accepted the chain.
fn authenticated(tls: &ServerTlsConfig, server: &Certificate) -> bool {
    let mut client_config = tls.quic_config().unwrap();
    client_config
        .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
        .unwrap();
    let mut server_config = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    server_config
        .load_cert_chain_from_pem_file(server.cert.to_str().unwrap())
        .unwrap();
    server_config
        .load_priv_key_from_pem_file(server.key.to_str().unwrap())
        .unwrap();
    server_config
        .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
        .unwrap();
    let client_addr = "127.0.0.1:1234".parse().unwrap();
    let server_addr = "127.0.0.1:4321".parse().unwrap();
    let mut client = quiche::connect(
        tls.sni(),
        &quiche::ConnectionId::from_ref(&[1; 16]),
        client_addr,
        server_addr,
        &mut client_config,
    )
    .unwrap();
    let mut server = quiche::accept(
        &quiche::ConnectionId::from_ref(&[2; 16]),
        None,
        server_addr,
        client_addr,
        &mut server_config,
    )
    .unwrap();
    let mut bytes = [0; 65535];
    for _ in 0..32 {
        let mut progress = false;
        while let Ok((n, info)) = client.send(&mut bytes) {
            progress = true;
            if server
                .recv(
                    &mut bytes[..n],
                    quiche::RecvInfo {
                        from: info.from,
                        to: info.to,
                    },
                )
                .is_err()
            {
                return false;
            }
        }
        while let Ok((n, info)) = server.send(&mut bytes) {
            progress = true;
            if client
                .recv(
                    &mut bytes[..n],
                    quiche::RecvInfo {
                        from: info.from,
                        to: info.to,
                    },
                )
                .is_err()
            {
                return false;
            }
        }
        if client.is_established() {
            return tls.verify_identity(client.peer_cert()).is_ok();
        }
        if !progress {
            return false;
        }
    }
    false
}

#[test]
fn trusted_self_signed_certificate_accepts_only_its_dns_and_ip_sans() {
    let cert = Certificate::new();
    for name in ["auth.test.invalid", "127.0.0.1"] {
        assert!(
            authenticated(
                &ServerTlsConfig::with_trust_pem(name, &cert.cert).unwrap(),
                &cert
            ),
            "{name}"
        );
    }
    for name in ["other.test.invalid", "127.0.0.2"] {
        assert!(
            !authenticated(
                &ServerTlsConfig::with_trust_pem(name, &cert.cert).unwrap(),
                &cert
            ),
            "{name}"
        );
    }
}

#[test]
fn another_certificate_with_the_same_name_and_an_expired_trusted_certificate_are_rejected() {
    let trust = Certificate::new();
    let other = Certificate::new();
    let tls = ServerTlsConfig::with_trust_pem("127.0.0.1", &trust.cert).unwrap();
    assert!(!authenticated(&tls, &other));
    let mut expired = Certificate::new();
    expired.expire();
    assert!(!authenticated(
        &ServerTlsConfig::with_trust_pem("127.0.0.1", &expired.cert).unwrap(),
        &expired
    ));
}

#[test]
fn missing_or_invalid_trust_and_missing_peer_certificate_never_fall_back_to_insecure_mode() {
    assert!(ServerTlsConfig::system_roots("").is_err());
    assert!(ServerTlsConfig::system_roots("https://auth.test.invalid/path").is_err());
    let cert = Certificate::new();
    assert!(
        ServerTlsConfig::with_trust_pem("auth.test.invalid", cert.root.join("missing.pem"))
            .is_err()
    );
    fs::write(cert.root.join("invalid.pem"), "not a certificate").unwrap();
    assert!(
        ServerTlsConfig::with_trust_pem("auth.test.invalid", cert.root.join("invalid.pem"))
            .is_err()
    );
    let tls = ServerTlsConfig::system_roots("auth.test.invalid").unwrap();
    assert_eq!(
        tls.verify_identity(None),
        Err(ServerTlsError::CertificateMissing)
    );
    assert!(!authenticated(&tls, &cert));
}
