//! One-file backup and restore of everything the server keeps in `data_dir` (the queue, the Nostr events, the admin list, the
//! TURN secret, the TLS account), so moving to another server is: `backup`, copy the file, `restore`.
//!
//! The file is a plain stream of `name, size, bytes, sha256` records and an end marker with the file count, so a cut-off or
//! changed file is refused whole. With a passphrase the stream is encrypted in 1 MiB AES-256-GCM chunks under an Argon2id key.
//! The storage key (`storage.encrypt`) lives outside `data_dir` and is never put in a backup: keep it separately.

// Every slice below is cut to a length just checked (the bytes read, the chunk size), so none can run past its end.
#![allow(clippy::indexing_slicing)]

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 6] = b"PCBK1\n";
const CHUNK: usize = 1 << 20;
/// The marker the backup command leaves in `data_dir`; the health check reads it. Not part of a backup.
pub const MARKER: &str = "last-backup";
const CONFIG_NAME: &str = "@config";

/// Writes plain bytes, or encrypted chunks (`u32 length` then ciphertext, the nonce is the chunk counter).
struct Sink<W: Write> {
    out: W,
    cipher: Option<Aes256Gcm>,
    buf: Vec<u8>,
    n: u64,
}

fn nonce(n: u64) -> Nonce<aes_gcm::aes::cipher::consts::U12> {
    let mut b = [0u8; 12];
    b[4..].copy_from_slice(&n.to_be_bytes());
    Nonce::from(b)
}

impl<W: Write> Sink<W> {
    fn seal(&mut self) -> std::io::Result<()> {
        if let Some(c) = &self.cipher {
            let ct = c
                .encrypt(&nonce(self.n), self.buf.as_slice())
                .map_err(|_| std::io::Error::other("encryption failed"))?;
            self.n += 1;
            self.out.write_all(&(ct.len() as u32).to_be_bytes())?;
            self.out.write_all(&ct)?;
            self.buf.clear();
        }
        Ok(())
    }
    fn finish(mut self) -> std::io::Result<W> {
        if self.cipher.is_some() {
            self.seal()?;
        }
        self.out.flush()?;
        Ok(self.out)
    }
}

impl<W: Write> Write for Sink<W> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.cipher.is_none() {
            return self.out.write(data);
        }
        self.buf.extend_from_slice(data);
        while self.buf.len() >= CHUNK {
            let rest = self.buf.split_off(CHUNK);
            self.seal()?;
            self.buf = rest;
        }
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The reading side of [`Sink`].
struct Source<R: Read> {
    inp: R,
    cipher: Option<Aes256Gcm>,
    buf: Vec<u8>,
    at: usize,
    n: u64,
}

impl<R: Read> Read for Source<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let Some(c) = &self.cipher else {
            return self.inp.read(out);
        };
        if self.at == self.buf.len() {
            let mut len = [0u8; 4];
            match self.inp.read(&mut len[..1])? {
                0 => return Ok(0),
                _ => self.inp.read_exact(&mut len[1..])?,
            }
            let len = u32::from_be_bytes(len) as usize;
            if len > CHUNK + 16 {
                return Err(std::io::Error::other("not a PrivCord backup"));
            }
            let mut ct = vec![0u8; len];
            self.inp.read_exact(&mut ct)?;
            self.buf = c.decrypt(&nonce(self.n), ct.as_slice()).map_err(|_| {
                std::io::Error::other("wrong passphrase, or the backup was changed")
            })?;
            self.n += 1;
            self.at = 0;
        }
        let k = out.len().min(self.buf.len() - self.at);
        out[..k].copy_from_slice(&self.buf[self.at..self.at + k]);
        self.at += k;
        Ok(k)
    }
}

fn key(pass: &str, salt: &[u8; 16]) -> Result<Aes256Gcm, String> {
    let mut k = [0u8; 32];
    argon2::Argon2::default()
        .hash_password_into(pass.as_bytes(), salt, &mut k)
        .map_err(|e| format!("passphrase: {e}"))?;
    Aes256Gcm::new_from_slice(&k).map_err(|e| e.to_string())
}

fn io(e: std::io::Error) -> String {
    e.to_string()
}

/// Every file under `dir`, as names with `/` separators (sorted), leaving out the backup marker and things that are not files.
fn list(dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    fn walk(base: &Path, at: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
        for e in std::fs::read_dir(at).map_err(io)? {
            let e = e.map_err(io)?;
            let ft = e.file_type().map_err(io)?;
            if ft.is_dir() {
                walk(base, &e.path(), out)?;
            } else if ft.is_file() {
                let rel = e
                    .path()
                    .strip_prefix(base)
                    .map_err(|e| e.to_string())?
                    .to_path_buf();
                let name = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                if name != MARKER {
                    out.push((name, e.path()));
                }
            }
        }
        Ok(())
    }
    let mut v = Vec::new();
    walk(dir, dir, &mut v)?;
    v.sort();
    Ok(v)
}

/// The event database cannot be copied while the server holds it open (the copy could be torn): refuse, with what to do.
fn check_not_in_use(dir: &Path) -> Result<(), String> {
    let db = dir.join("nostr-events.redb");
    if db.exists() && redb::Database::open(&db).is_err() {
        return Err("the event database is in use: stop the server first (`systemctl stop privcord-server`), then run this again".into());
    }
    Ok(())
}

#[derive(Debug)]
pub struct Summary {
    pub files: u64,
    pub bytes: u64,
}

fn put_file<W: Write>(w: &mut W, name: &str, mut f: impl Read, size: u64) -> Result<(), String> {
    let nb = name.as_bytes();
    w.write_all(&[1]).map_err(io)?;
    w.write_all(&(nb.len() as u16).to_be_bytes()).map_err(io)?;
    w.write_all(nb).map_err(io)?;
    w.write_all(&size.to_be_bytes()).map_err(io)?;
    let (mut h, mut left, mut buf) = (Sha256::new(), size, vec![0u8; 64 * 1024]);
    while left > 0 {
        let want = buf.len().min(left as usize);
        let got = f.read(&mut buf[..want]).map_err(io)?;
        if got == 0 {
            return Err(format!("{name} got shorter while it was being read"));
        }
        h.update(&buf[..got]);
        w.write_all(&buf[..got]).map_err(io)?;
        left -= got as u64;
    }
    w.write_all(&h.finalize()).map_err(io)
}

/// Writes the backup of `data_dir` (and the settings file, when given) to `out`, which must not exist.
pub fn create(
    data_dir: &Path,
    config: Option<&Path>,
    out: &Path,
    pass: Option<&str>,
) -> Result<Summary, String> {
    if !data_dir.is_dir() {
        return Err(format!(
            "{} is not a folder: nothing to back up",
            data_dir.display()
        ));
    }
    check_not_in_use(data_dir)?;
    let files = list(data_dir)?;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let f = o.open(out).map_err(|e| {
        format!(
            "cannot create {} (it must not exist yet): {e}",
            out.display()
        )
    })?;
    let res = (|| {
        let mut f = std::io::BufWriter::new(f);
        f.write_all(MAGIC).map_err(io)?;
        let cipher = match pass {
            Some(p) => {
                let mut salt = [0u8; 16];
                crate::fsutil::random(&mut salt);
                f.write_all(&[1]).map_err(io)?;
                f.write_all(&salt).map_err(io)?;
                Some(key(p, &salt)?)
            }
            None => {
                f.write_all(&[0]).map_err(io)?;
                None
            }
        };
        let mut w = Sink {
            out: f,
            cipher,
            buf: Vec::new(),
            n: 0,
        };
        let (mut count, mut bytes) = (0u64, 0u64);
        for (name, path) in &files {
            let file = std::fs::File::open(path).map_err(|e| format!("{name}: {e}"))?;
            let size = file.metadata().map_err(io)?.len();
            put_file(&mut w, name, file, size)?;
            count += 1;
            bytes += size;
        }
        if let Some(c) = config.filter(|c| c.is_file()) {
            let file = std::fs::File::open(c).map_err(io)?;
            let size = file.metadata().map_err(io)?.len();
            put_file(&mut w, CONFIG_NAME, file, size)?;
            count += 1;
            bytes += size;
        }
        w.write_all(&[0]).map_err(io)?;
        w.write_all(&count.to_be_bytes()).map_err(io)?;
        let f = w.finish().map_err(io)?;
        f.into_inner()
            .map_err(|e| e.to_string())?
            .sync_all()
            .map_err(io)?;
        Ok(Summary {
            files: count,
            bytes,
        })
    })();
    match res {
        Ok(s) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            crate::fsutil::write_private_atomic(
                &data_dir.join(MARKER),
                now.to_string().as_bytes(),
            )?;
            Ok(s)
        }
        Err(e) => {
            let _ = std::fs::remove_file(out);
            Err(e)
        }
    }
}

fn read_n(r: &mut impl Read, n: usize) -> Result<Vec<u8>, String> {
    let mut v = vec![0u8; n];
    r.read_exact(&mut v)
        .map_err(|e| format!("the backup is cut short or damaged: {e}"))?;
    Ok(v)
}

/// A name that stays inside the folder it is unpacked into.
fn safe(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('\\')
        && !name.contains('\0')
        && name
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != "..")
}

/// Unpacks `archive` into `data_dir`, which must be empty or missing unless `replace` is set (then what is there is moved
/// aside to `<data_dir>.before-restore`, never deleted). Nothing is touched until the whole backup has been checked.
/// Returns the summary and, if the backup held the settings file, where it was written (next to the data, not over yours).
pub fn restore(
    archive: &Path,
    data_dir: &Path,
    pass: Option<&str>,
    replace: bool,
) -> Result<(Summary, Option<PathBuf>), String> {
    let mut f = std::io::BufReader::new(
        std::fs::File::open(archive).map_err(|e| format!("{}: {e}", archive.display()))?,
    );
    if read_n(&mut f, 6)? != MAGIC {
        return Err("this is not a PrivCord backup".into());
    }
    let cipher = match read_n(&mut f, 1)?[0] {
        0 => None,
        1 => {
            let p = pass
                .ok_or("this backup is encrypted: give the passphrase (--passphrase-env NAME)")?;
            let salt: [u8; 16] = read_n(&mut f, 16)?.try_into().map_err(|_| "bad header")?;
            Some(key(p, &salt)?)
        }
        _ => return Err("this backup was made by a newer version".into()),
    };
    let mut r = Source {
        inp: f,
        cipher,
        buf: Vec::new(),
        at: 0,
        n: 0,
    };

    let stage = PathBuf::from(format!("{}.restoring", data_dir.display()));
    if stage.exists() {
        std::fs::remove_dir_all(&stage).map_err(io)?;
    }
    crate::fsutil::create_private_dir(&stage)?;
    let res = (|| {
        let (mut count, mut bytes, mut config) = (0u64, 0u64, None);
        loop {
            match read_n(&mut r, 1)?[0] {
                0 => {
                    let n = u64::from_be_bytes(read_n(&mut r, 8)?.try_into().map_err(|_| "bad")?);
                    if n != count {
                        return Err("the backup does not add up (files missing)".to_string());
                    }
                    break;
                }
                1 => {}
                _ => return Err("the backup is damaged".into()),
            }
            let nl = u16::from_be_bytes(read_n(&mut r, 2)?.try_into().map_err(|_| "bad")?) as usize;
            let name = String::from_utf8(read_n(&mut r, nl)?).map_err(|_| "bad file name")?;
            if name != CONFIG_NAME && !safe(&name) {
                return Err(format!("the backup holds an unsafe file name: {name:?}"));
            }
            let size = u64::from_be_bytes(read_n(&mut r, 8)?.try_into().map_err(|_| "bad")?);
            let dest = if name == CONFIG_NAME {
                stage.join("restored-config.toml")
            } else {
                stage.join(&name)
            };
            if let Some(p) = dest.parent() {
                crate::fsutil::create_private_dir(p)?;
            }
            let mut o = std::fs::OpenOptions::new();
            o.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
            let mut out =
                std::io::BufWriter::new(o.open(&dest).map_err(|e| format!("{name}: {e}"))?);
            let (mut h, mut left, mut buf) = (Sha256::new(), size, vec![0u8; 64 * 1024]);
            while left > 0 {
                let want = buf.len().min(left as usize);
                let got = r.read(&mut buf[..want]).map_err(io)?;
                if got == 0 {
                    return Err("the backup is cut short".into());
                }
                h.update(&buf[..got]);
                out.write_all(&buf[..got]).map_err(io)?;
                left -= got as u64;
            }
            out.into_inner()
                .map_err(|e| e.to_string())?
                .sync_all()
                .map_err(io)?;
            if read_n(&mut r, 32)? != h.finalize().as_slice() {
                return Err(format!(
                    "{name} does not match its checksum: the backup is damaged"
                ));
            }
            if name == CONFIG_NAME {
                config = Some(PathBuf::from("restored-config.toml"));
            }
            count += 1;
            bytes += size;
        }
        Ok((
            Summary {
                files: count,
                bytes,
            },
            config,
        ))
    })();
    let (summary, config) = match res {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(e);
        }
    };

    if data_dir.exists() {
        let busy = std::fs::read_dir(data_dir).map_err(io)?.next().is_some();
        if busy {
            if !replace {
                let _ = std::fs::remove_dir_all(&stage);
                return Err(format!(
                    "{} already holds data; to replace it (it is moved aside, not deleted) add --replace",
                    data_dir.display()
                ));
            }
            check_not_in_use(data_dir).inspect_err(|_| {
                let _ = std::fs::remove_dir_all(&stage);
            })?;
            let aside = PathBuf::from(format!("{}.before-restore", data_dir.display()));
            if aside.exists() {
                let _ = std::fs::remove_dir_all(&stage);
                return Err(format!(
                    "{} exists already: move it away first",
                    aside.display()
                ));
            }
            std::fs::rename(data_dir, &aside).map_err(io)?;
        } else {
            std::fs::remove_dir(data_dir).map_err(io)?;
        }
    }
    std::fs::rename(&stage, data_dir).map_err(io)?;
    crate::fsutil::sync_dir(data_dir.parent());
    Ok((summary, config.map(|c| data_dir.join(c))))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn fill(d: &Path) {
        std::fs::create_dir_all(d.join("tls")).unwrap();
        std::fs::write(d.join("queue.jsonl"), b"one\ntwo\n").unwrap();
        std::fs::write(d.join("turn-secret"), b"s3cret").unwrap();
        std::fs::write(d.join("tls/acme-account.json"), vec![7u8; 3 * CHUNK + 5]).unwrap();
    }

    #[test]
    fn a_backup_restores_to_the_same_files_with_and_without_a_passphrase() {
        for pass in [None, Some("correct horse")] {
            let t = tempfile::tempdir().unwrap();
            let (src, out, dst) = (
                t.path().join("a"),
                t.path().join("b.pcbk"),
                t.path().join("c"),
            );
            fill(&src);
            let cfg = t.path().join("c.toml");
            std::fs::write(&cfg, "x=1").unwrap();
            let s = create(&src, Some(&cfg), &out, pass).unwrap();
            assert_eq!(s.files, 4);
            assert!(src.join(MARKER).exists());
            assert!(create(&src, None, &out, pass).is_err(), "never overwrites");
            let (_, conf) = restore(&out, &dst, pass, false).unwrap();
            assert_eq!(
                std::fs::read(dst.join("queue.jsonl")).unwrap(),
                b"one\ntwo\n"
            );
            assert_eq!(
                std::fs::read(dst.join("tls/acme-account.json"))
                    .unwrap()
                    .len(),
                3 * CHUNK + 5
            );
            assert!(!dst.join(MARKER).exists());
            assert_eq!(std::fs::read(conf.unwrap()).unwrap(), b"x=1");
        }
    }

    #[test]
    fn a_wrong_passphrase_a_changed_byte_or_a_cut_file_is_refused_and_nothing_is_written() {
        let t = tempfile::tempdir().unwrap();
        let (src, out) = (t.path().join("a"), t.path().join("b.pcbk"));
        fill(&src);
        create(&src, None, &out, Some("right")).unwrap();
        let dst = t.path().join("c");
        assert!(restore(&out, &dst, Some("wrong"), false).is_err());
        assert!(restore(&out, &dst, None, false).is_err());
        let good = std::fs::read(&out).unwrap();
        let mut bad = good.clone();
        let mid = bad.len() / 2;
        bad[mid] ^= 1;
        std::fs::write(&out, &bad).unwrap();
        assert!(restore(&out, &dst, Some("right"), false).is_err());
        std::fs::write(&out, &good[..good.len() - 40]).unwrap();
        assert!(restore(&out, &dst, Some("right"), false).is_err());
        assert!(!dst.exists());
        assert!(!t.path().join("c.restoring").exists());

        // plain: a changed byte inside a file is caught by its checksum
        let (out2, dst2) = (t.path().join("p.pcbk"), t.path().join("d"));
        create(&src, None, &out2, None).unwrap();
        let mut b = std::fs::read(&out2).unwrap();
        let i = b.windows(3).position(|w| w == b"two").unwrap();
        b[i] = b'X';
        std::fs::write(&out2, &b).unwrap();
        assert!(restore(&out2, &dst2, None, false).is_err());
        assert!(!dst2.exists());
    }

    #[test]
    fn restore_will_not_mix_into_existing_data_and_moves_it_aside_only_when_told() {
        let t = tempfile::tempdir().unwrap();
        let (src, out, dst) = (
            t.path().join("a"),
            t.path().join("b.pcbk"),
            t.path().join("c"),
        );
        fill(&src);
        create(&src, None, &out, None).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(dst.join("old"), b"old").unwrap();
        assert!(restore(&out, &dst, None, false).is_err());
        assert_eq!(std::fs::read(dst.join("old")).unwrap(), b"old");
        restore(&out, &dst, None, true).unwrap();
        assert!(dst.join("queue.jsonl").exists() && !dst.join("old").exists());
        assert_eq!(
            std::fs::read(t.path().join("c.before-restore/old")).unwrap(),
            b"old"
        );
    }

    #[test]
    fn names_that_climb_out_of_the_folder_are_unsafe() {
        for bad in ["", "../x", "a/../x", "/x", "a//b", "a\\b", "."] {
            assert!(!safe(bad), "{bad:?}");
        }
        assert!(safe("tls/acme-account.json"));
    }
}
