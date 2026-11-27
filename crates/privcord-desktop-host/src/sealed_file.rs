//! Small settings files kept on disk only as ciphertext.
//!
//! The desktop shell saves its network settings (relay addresses, a TURN password) so a start in the tray has them. That
//! file is AES-256-GCM encrypted under a random key held in the operating system's credential store, never written to
//! disk beside it. A file that is not ours (an older plain-text one) is not opened.

use privcord_crypto::{
    ramseal::{open_with_key, seal_with_key},
    rng::{Entropy, OsEntropy},
};

const MAGIC: &[u8; 4] = b"PCS1";
const LABEL: &str = "desktop-settings-file/v1";

/// A new random key to keep in the credential store.
pub fn new_key() -> Option<[u8; 32]> {
    let mut key = privcord_crypto::kdf::blank::<32>();
    OsEntropy.fill(&mut key).ok()?;
    Some(key)
}

/// What to write to disk for `plain`.
pub fn seal(key: &[u8; 32], plain: &[u8]) -> Option<Vec<u8>> {
    let sealed = seal_with_key(key, LABEL, plain).ok()?;
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&sealed);
    Some(out)
}

/// The plain bytes of a file made by [`seal`]; `None` for anything else or a wrong key.
pub fn open(key: &[u8; 32], file: &[u8]) -> Option<Vec<u8>> {
    let body = file.strip_prefix(MAGIC.as_slice())?;
    open_with_key(key, LABEL, body).map(|p| p.to_vec())
}

/// Whether a file on disk is one of ours (and so not plain text).
pub fn is_sealed(file: &[u8]) -> bool {
    file.starts_with(MAGIC)
}

/// The settings the page keeps (and may ask for by name): nothing else is ever read or written, so a name cannot point
/// outside the folder.
const BLOBS: [&str; 3] = ["settings", "network", "servers"];

fn blob_path(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    BLOBS
        .contains(&name)
        .then(|| dir.join(format!("{name}.sealed")))
}

/// Saves `text` for `name` in `dir`, encrypted under `key`.
pub fn write_blob(
    dir: &std::path::Path,
    key: &[u8; 32],
    name: &str,
    text: &str,
) -> Result<(), String> {
    let path = blob_path(dir, name).ok_or("unknown settings name")?;
    let sealed = seal(key, text.as_bytes()).ok_or("could not encrypt")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(path, sealed).map_err(|e| e.to_string())
}

/// What was saved for `name`, or `None` if nothing was or it cannot be opened with `key`.
pub fn read_blob(dir: &std::path::Path, key: &[u8; 32], name: &str) -> Option<String> {
    let bytes = std::fs::read(blob_path(dir, name)?).ok()?;
    String::from_utf8(open(key, &bytes)?).ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn named_settings_are_saved_encrypted_and_only_known_names_work() {
        let dir = tempfile::tempdir().unwrap();
        let key = new_key().unwrap();
        let text = r#"{"displayName":"Asha Menon","turn":"hunter2"}"#;
        write_blob(dir.path(), &key, "settings", text).unwrap();
        let on_disk = std::fs::read(dir.path().join("settings.sealed")).unwrap();
        assert!(!on_disk.windows(5).any(|w| w == b"Asha "));
        assert!(!on_disk.windows(7).any(|w| w == b"hunter2"));
        assert_eq!(read_blob(dir.path(), &key, "settings").unwrap(), text);
        assert!(read_blob(dir.path(), &new_key().unwrap(), "settings").is_none());
        assert!(
            read_blob(dir.path(), &key, "network").is_none(),
            "nothing saved yet"
        );
        assert!(write_blob(dir.path(), &key, "../escape", "x").is_err());
        assert!(read_blob(dir.path(), &key, "../settings").is_none());
    }

    #[test]
    fn the_file_holds_no_plain_text_and_opens_only_with_its_key() {
        let key = new_key().unwrap();
        let plain = br#"{"turn":{"credential":"hunter2-turn-password"}}"#;
        let file = seal(&key, plain).unwrap();
        assert!(is_sealed(&file));
        assert!(!file.windows(7).any(|w| w == b"hunter2"));
        assert_eq!(open(&key, &file).unwrap(), plain);
        assert!(open(&new_key().unwrap(), &file).is_none(), "another key");
        assert!(
            open(&key, plain).is_none(),
            "an old plain-text file is not ours"
        );
        assert!(!is_sealed(plain));
    }
}
