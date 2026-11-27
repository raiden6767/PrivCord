//! Files that hold secrets or private data: created readable by this program's user only, from the first byte (not made
//! private afterwards), and replaced atomically.

use std::io::Write;
use std::path::Path;

/// Creates `dir` (and parents) so only this user can enter it. An existing directory is left as it is.
pub fn create_private_dir(dir: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))
    }
}

/// Opens (creating if needed) a file for appending, mode 0600.
pub fn open_private_append(path: &Path) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

/// Writes `data` to `path` so that it is never visible half-written or with wide permissions: a private temporary file in
/// the same folder, synced to disk, then renamed over the target.
pub fn write_private_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    let mut o = std::fs::OpenOptions::new();
    o.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o
        .open(&tmp)
        .map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    f.write_all(data)
        .map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    f.sync_all()
        .map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| format!("could not save {}: {e}", path.display()))
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn private_files_and_folders_are_private_from_the_start() {
        let d = tempfile::tempdir().unwrap();
        let sub = d.path().join("data/inner");
        create_private_dir(&sub).unwrap();
        assert_eq!(
            std::fs::metadata(&sub).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let f = sub.join("secret");
        write_private_atomic(&f, b"one").unwrap();
        write_private_atomic(&f, b"two").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "two");
        assert_eq!(
            std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let log = sub.join("log");
        drop(open_private_append(&log).unwrap());
        assert_eq!(
            std::fs::metadata(&log).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
