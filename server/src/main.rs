use anyhow::{bail, Result};
use remotex_server::{config::Config, logging, serve, tls};
use std::{net::TcpListener, path::PathBuf, sync::Arc};

#[cfg(windows)]
mod service;

fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

async fn run_console(cfg: Config) -> Result<()> {
    let listener = TcpListener::bind((cfg.host.as_str(), cfg.port))?;
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    serve(Arc::new(cfg), listener, shutdown).await
}

fn gen_cert(args: &[String]) -> Result<()> {
    let mut names = Vec::new();
    let mut out = PathBuf::from("certs");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ip" | "--dns" => names.push(it.next().cloned().unwrap_or_default()),
            "--out" => out = PathBuf::from(it.next().cloned().unwrap_or_default()),
            other => bail!("unknown argument {other}"),
        }
    }
    let fp = tls::generate_self_signed(&names, &out)?;
    println!("Certificate written to {}", out.display());
    println!("SHA-256 fingerprint (put this in branding.json as serverCertSha256):");
    println!("{fp}");
    Ok(())
}

fn main() -> Result<()> {
    install_crypto_provider();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("gen-cert") => gen_cert(&args[1..]),
        #[cfg(windows)]
        Some("service") => service::run(),
        Some("run") | None => {
            let cfg = Config::from_env()?;
            let _guards = logging::init(&cfg.log_dir, true);
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(run_console(cfg))
        }
        Some(other) => bail!("unknown command `{other}` (expected: run | service | gen-cert)"),
    }
}
