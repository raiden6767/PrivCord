//! Refuses an update that is older than the running build, even when its version number says otherwise.
//!
//! The update's signature covers the installer and a "trusted comment" that carries the time it was signed; both are
//! checked by the updater against our public key. The version number in `latest.json` is not signed, so someone who can
//! write the release could pair an old signed installer with a higher version. The signing time is what they cannot fake.

use base64::Engine;

/// The signed "trusted comment" of the signature the manifest carries.
fn trusted_comment(signature: &str) -> Option<String> {
    let text = match base64::engine::general_purpose::STANDARD.decode(signature.trim()) {
        Ok(bytes) => String::from_utf8(bytes).ok()?,
        Err(_) => signature.to_owned(),
    };
    text.lines()
        .find_map(|l| l.strip_prefix("trusted comment:"))
        .map(str::to_owned)
}

/// The version the build was signed as (the release workflow writes it into the signature).
pub fn signed_version(signature: &str) -> Option<String> {
    let comment = trusted_comment(signature)?;
    let rest = comment.split("version:").nth(1)?;
    let v: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
    (!v.is_empty()).then_some(v)
}

/// When the update's signature was made (Unix seconds), read from the signature the manifest carries. `None` when the
/// text is not a signature in the expected shape.
pub fn signed_at(signature: &str) -> Option<i64> {
    let comment = trusted_comment(signature)?;
    let rest = comment.split("timestamp:").nth(1)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// True when the update is not what the manifest says: its signature names another version than the manifest's
/// `version`, or says it was signed before `built_at` (the running build's own time). A signature this cannot read is
/// not refused here (the updater has already verified it).
pub fn is_rollback(signature: &str, manifest_version: &str, built_at: i64) -> bool {
    signed_version(signature).is_some_and(|v| v != manifest_version)
        || (built_at > 0 && signed_at(signature).is_some_and(|t| t < built_at))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(ts: i64, version: &str) -> String {
        let file = format!(
            "untrusted comment: signature from tauri secret key\nAAAA\ntrusted comment: timestamp:{ts}\tfile:PrivCord_{version}_x64-setup.exe\tversion:{version}\nBBBB\n"
        );
        base64::engine::general_purpose::STANDARD.encode(file)
    }

    #[test]
    fn reads_the_signing_time_and_version() {
        let s = sig(1_700_000_123, "0.1.31");
        assert_eq!(signed_at(&s), Some(1_700_000_123));
        assert_eq!(signed_version(&s).as_deref(), Some("0.1.31"));
        assert_eq!(signed_at("not a signature"), None);
    }

    #[test]
    fn an_old_installer_under_a_new_version_number_is_refused() {
        // Signed as 0.1.20, announced as 0.1.99.
        assert!(is_rollback(
            &sig(1_700_000_900, "0.1.20"),
            "0.1.99",
            1_700_000_500
        ));
        // Signed before this build was made.
        assert!(is_rollback(
            &sig(1_700_000_000, "0.1.99"),
            "0.1.99",
            1_700_000_500
        ));
        // The honest case.
        assert!(!is_rollback(
            &sig(1_700_000_900, "0.1.99"),
            "0.1.99",
            1_700_000_500
        ));
        // Unreadable: left to the updater, which has verified it.
        assert!(!is_rollback("junk", "0.1.99", 1_700_000_500));
        assert!(
            !is_rollback(&sig(1, "0.1.99"), "0.1.99", 0),
            "no build time known"
        );
    }
}
