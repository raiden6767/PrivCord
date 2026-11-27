//! Certificates that get and renew themselves (ACME, for example Let's Encrypt).
//!
//! The name is proved with TLS-ALPN-01 (RFC 8737) on the HTTPS port: the authority connects to port 443 of the domain asking for
//! the protocol `acme-tls/1`, and the TLS resolver answers with a throwaway certificate that carries the proof. So nothing but
//! the HTTPS listener has to be reachable (no port 80, no DNS access). The issued certificate and its key are written to
//! `data_dir/tls`, and the server starts using them at once. It is looked at again twice a day and renewed 30 days before it
//! runs out; a failed attempt is retried after a growing pause (5 minutes, up to 6 hours).

use crate::{config::TlsConfig, record::Codec, tls::Tls};
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier, NewAccount,
    NewOrder, OrderStatus, RetryPolicy,
};
use rustls::pki_types::{pem::PemObject, CertificateDer};
use std::{path::Path, sync::Arc, time::Duration};

const LETS_ENCRYPT: &str = "https://acme-v02.api.letsencrypt.org/directory";
/// A certificate with less than this left is renewed.
const RENEW_BEFORE_DAYS: i64 = 30;
const CHECK_EVERY: Duration = Duration::from_secs(12 * 3600);

/// Whole days until the first certificate in `cert_file` expires (negative: it has), or `None` when there is none.
pub fn days_left(cert_file: &Path) -> Option<i64> {
    let der = CertificateDer::pem_file_iter(cert_file)
        .ok()?
        .next()?
        .ok()?;
    let (_, cert) = x509_parser::parse_x509_certificate(der.as_ref()).ok()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let end = cert.validity().not_after.timestamp();
    Some((end - i64::try_from(now).ok()?).div_euclid(86_400))
}

/// Runs until `stop`: keeps the certificate valid.
pub async fn run(
    tls: Arc<Tls>,
    cfg: TlsConfig,
    data_dir: std::path::PathBuf,
    codec: Arc<Codec>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let mut failures = 0u32;
    loop {
        let wait = match ensure(&tls, &cfg, &data_dir, &codec).await {
            Ok(next) => {
                failures = 0;
                next
            }
            Err(e) => {
                failures += 1;
                tls.clear_challenges();
                eprintln!("privcord-server: certificate (ACME): {e}");
                Duration::from_secs((300 * 3u64.pow(failures.min(4))).min(6 * 3600))
            }
        };
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            _ = stop.wait_for(|s| *s) => return,
        }
    }
}

/// Gets a certificate if there is none or it is about to run out. Returns how long until it is worth looking again.
pub async fn ensure(
    tls: &Tls,
    cfg: &TlsConfig,
    data_dir: &Path,
    codec: &Codec,
) -> Result<Duration, String> {
    let (cert_file, _) = tls.acme_files();
    if let Some(days) = days_left(&cert_file) {
        if days > RENEW_BEFORE_DAYS {
            let until_renewal =
                Duration::from_secs(u64::try_from(days - RENEW_BEFORE_DAYS).unwrap_or(0) * 86_400);
            return Ok(until_renewal.min(CHECK_EVERY));
        }
    }
    issue(tls, cfg, data_dir, codec).await?;
    eprintln!("privcord-server: certificate for {} issued", cfg.domain);
    Ok(CHECK_EVERY)
}

async fn account(cfg: &TlsConfig, data_dir: &Path, codec: &Codec) -> Result<Account, String> {
    let builder = || {
        if cfg.acme_ca_root.trim().is_empty() {
            Account::builder()
        } else {
            Account::builder_with_root(cfg.acme_ca_root.trim())
        }
        .map_err(|e| format!("could not set up the ACME client: {e}"))
    };
    let file = data_dir.join("tls").join("acme-account.json");
    if let Ok(text) = std::fs::read_to_string(&file) {
        let json = match codec.decode("acme-account", text.trim()) {
            crate::record::Decoded::Payload(p) => Some(p),
            crate::record::Decoded::NeedsKey => {
                return Err(format!(
                    "{} is encrypted but no storage key is set",
                    file.display()
                ))
            }
            crate::record::Decoded::Corrupt => None,
        };
        if let Some(creds) = json.and_then(|j| serde_json::from_str::<AccountCredentials>(&j).ok())
        {
            return builder()?
                .from_credentials(creds)
                .await
                .map_err(|e| format!("could not use the saved ACME account: {e}"));
        }
    }
    let directory = if cfg.acme_directory.trim().is_empty() {
        LETS_ENCRYPT.to_owned()
    } else {
        cfg.acme_directory.trim().to_owned()
    };
    let contact = format!("mailto:{}", cfg.acme_email.trim());
    let contacts: Vec<&str> = if cfg.acme_email.trim().is_empty() {
        Vec::new()
    } else {
        vec![contact.as_str()]
    };
    let (account, creds) = builder()?
        .create(
            &NewAccount {
                contact: &contacts,
                terms_of_service_agreed: cfg.acme_accept_terms,
                only_return_existing: false,
            },
            directory,
            None,
        )
        .await
        .map_err(|e| format!("could not make an ACME account: {e}"))?;
    let json = serde_json::to_string(&creds).map_err(|e| e.to_string())?;
    crate::fsutil::write_private_atomic(&file, codec.encode("acme-account", &json).as_bytes())?;
    Ok(account)
}

async fn issue(tls: &Tls, cfg: &TlsConfig, data_dir: &Path, codec: &Codec) -> Result<(), String> {
    let account = account(cfg, data_dir, codec).await?;
    let domain = cfg.domain.trim().to_ascii_lowercase();
    let ids = [Identifier::Dns(domain.clone())];
    let mut order = account
        .new_order(&NewOrder::new(&ids))
        .await
        .map_err(|e| format!("could not start the order: {e}"))?;
    let mut authorizations = order.authorizations();
    while let Some(result) = authorizations.next().await {
        let mut authz = result.map_err(|e| format!("authorization: {e}"))?;
        match authz.status {
            AuthorizationStatus::Pending => {}
            AuthorizationStatus::Valid => continue,
            other => {
                return Err(format!(
                    "the authority will not authorize {domain}: {other:?}"
                ))
            }
        }
        let mut challenge = authz
            .challenge(ChallengeType::TlsAlpn01)
            .ok_or("the authority offers no TLS-ALPN-01 challenge for this name")?;
        let proof = challenge.key_authorization().digest();
        tls.set_challenge(
            &domain,
            crate::tls::challenge_cert(&domain, proof.as_ref())?,
        );
        challenge
            .set_ready()
            .await
            .map_err(|e| format!("could not tell the authority the challenge is ready: {e}"))?;
    }
    let status = order
        .poll_ready(&RetryPolicy::default())
        .await
        .map_err(|e| format!("the authority could not check {domain}: {e} (is port 443 of {domain} this server's HTTPS port?)"))?;
    if status != OrderStatus::Ready {
        return Err(format!("the order did not become ready: {status:?}"));
    }
    let key_pem = order
        .finalize()
        .await
        .map_err(|e| format!("could not finish the order: {e}"))?;
    let chain_pem = order
        .poll_certificate(&RetryPolicy::default())
        .await
        .map_err(|e| format!("could not fetch the certificate: {e}"))?;
    tls.clear_challenges();
    let (cert_file, key_file) = tls.acme_files();
    // The key first (private from its first byte), then the certificate: the pair is only installed once both are there.
    crate::fsutil::write_private_atomic(&key_file, key_pem.as_bytes())?;
    crate::fsutil::write_private_atomic(&cert_file, chain_pem.as_bytes())?;
    tls.reload_if_changed()?;
    Ok(())
}
