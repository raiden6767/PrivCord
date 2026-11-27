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
    std::fs::rename(&tmp, path).map_err(|e| format!("could not save {}: {e}", path.display()))?;
    sync_dir(path.parent());
    Ok(())
}

/// Makes a rename or a new file in `dir` survive a power cut: the folder's own entry is synced too. (Best effort: not every
/// system allows opening a folder; Windows keeps it on its own.)
pub fn sync_dir(dir: Option<&Path>) {
    #[cfg(unix)]
    if let Some(d) = dir
        .filter(|d| !d.as_os_str().is_empty())
        .or(Some(Path::new(".")))
    {
        if let Ok(f) = std::fs::File::open(d) {
            let _ = f.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Cuts a log back to its last complete line. A crash in the middle of a write leaves half a line; the next line appended
/// would be glued to it and lost with it. Returns how many bytes were cut.
pub fn trim_torn_tail(path: &Path) -> std::io::Result<u64> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let len = f.metadata()?.len();
    if len == 0 {
        return Ok(0);
    }
    f.seek(SeekFrom::Start(len - 1))?;
    let mut last = [0u8; 1];
    f.read_exact(&mut last)?;
    if last[0] == b'\n' {
        return Ok(0);
    }
    // Walk back in blocks to the last newline.
    let mut end = len;
    let mut keep = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    while end > 0 {
        let start = end.saturating_sub(buf.len() as u64);
        let n = usize::try_from(end - start).unwrap_or(buf.len());
        f.seek(SeekFrom::Start(start))?;
        f.read_exact(buf.get_mut(..n).unwrap_or(&mut []))?;
        if let Some(i) = buf
            .get(..n)
            .and_then(|b| b.iter().rposition(|c| *c == b'\n'))
        {
            keep = start + i as u64 + 1;
            break;
        }
        end = start;
    }
    f.set_len(keep)?;
    f.sync_all()?;
    Ok(len - keep)
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
