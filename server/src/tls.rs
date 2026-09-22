//! Self-signed certificate generator for IP-only testing. The client pins the printed fingerprint,
//! so certificate verification stays enabled; with a real domain use a CA-issued certificate instead.

use anyhow::{bail, Result};
use rcgen::{date_time_ymd, CertificateParams, DistinguishedName, DnType, KeyPair, SanType};
use sha2::{Digest, Sha256};
use std::{net::IpAddr, path::Path};

pub fn generate_self_signed(names: &[String], out_dir: &Path) -> Result<String> {
    if names.is_empty() {
        bail!("at least one --ip or --dns name is required");
    }
    let mut params = CertificateParams::default();
    params.subject_alt_names = names
        .iter()
        .map(|n| match n.parse::<IpAddr>() {
            Ok(ip) => Ok(SanType::IpAddress(ip)),
            Err(_) => n.clone().try_into().map(SanType::DnsName),
        })
        .collect::<Result<_, _>>()?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, names[0].clone());
    params.distinguished_name = dn;
    params.not_before = date_time_ymd(2024, 1, 1);
    params.not_after = date_time_ymd(2044, 1, 1);

    let key = KeyPair::generate()?;
    let cert = params.self_signed(&key)?;
    std::fs::create_dir_all(out_dir)?;
    std::fs::write(out_dir.join("server.crt"), cert.pem())?;
    std::fs::write(out_dir.join("server.key"), key.serialize_pem())?;
    Ok(fingerprint(cert.der()))
}

pub fn fingerprint(der: &[u8]) -> String {
    hex::encode(Sha256::digest(der))
}
