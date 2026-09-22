//! File-transfer primitives: safe path handling plus resumable, hash-verified read/write endpoints.
//! Network framing lives in the session crate; nothing here trusts peer-supplied paths.

use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
};

pub const CHUNK: usize = 64 * 1024;
/// Default ceiling for a single incoming file (1 TiB); hosts may set a lower cap.
pub const DEFAULT_MAX_FILE: u64 = 1 << 40;

#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error("unsafe or invalid file name")]
    BadName,
    #[error("file exceeds the allowed size")]
    TooLarge,
    #[error("chunk out of order")]
    OutOfOrder,
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
    "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

fn valid_component(c: &str) -> bool {
    if c.is_empty() || c.len() > 200 || c == "." || c == ".." {
        return false;
    }
    if c.ends_with('.') || c.ends_with(' ') {
        return false;
    }
    if c.chars()
        .any(|ch| ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\\' | '/'))
    {
        return false;
    }
    let stem = c.split('.').next().unwrap_or("").to_ascii_uppercase();
    !RESERVED.contains(&stem.as_str())
}

/// Converts a peer-supplied relative path (`/` separated, folders allowed) into a path that is
/// guaranteed to stay inside the destination directory.
pub fn sanitize_relative(name: &str) -> Result<PathBuf, FileError> {
    if name.is_empty() || name.len() > 1000 || name.starts_with('/') || name.contains('\\') {
        return Err(FileError::BadName);
    }
    let mut out = PathBuf::new();
    for part in name.split('/') {
        if !valid_component(part) {
            return Err(FileError::BadName);
        }
        out.push(part);
    }
    // Belt and braces: no component may be anything but a plain name.
    if out.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(FileError::BadName);
    }
    Ok(out)
}

/// Picks `name`, `name (1)`, ... so an incoming file never overwrites an existing one.
pub fn unique_path(dir: &Path, rel: &Path) -> PathBuf {
    let candidate = dir.join(rel);
    if !candidate.exists() && !part_path(&candidate).exists() {
        return candidate;
    }
    let stem = rel
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = rel
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let parent = rel.parent().map(Path::to_path_buf).unwrap_or_default();
    (1..10_000)
        .map(|i| dir.join(&parent).join(format!("{stem} ({i}){ext}")))
        .find(|p| !p.exists() && !part_path(p).exists())
        .unwrap_or(candidate)
}

pub fn part_path(target: &Path) -> PathBuf {
    let mut s = target.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

pub struct IncomingFile {
    target: PathBuf,
    part: PathBuf,
    file: File,
    pub expected: u64,
    pub received: u64,
}

impl IncomingFile {
    /// Opens (or resumes) the `.part` file. Returns the offset the sender must continue from.
    pub fn open(dir: &Path, rel: &str, expected: u64, max: u64, resume: bool) -> Result<Self, FileError> {
        if expected > max {
            return Err(FileError::TooLarge);
        }
        let rel = sanitize_relative(rel)?;
        let target = if resume {
            dir.join(&rel)
        } else {
            unique_path(dir, &rel)
        };
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let part = part_path(&target);
        let existing = if resume {
            fs::metadata(&part).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };
        let existing = if existing <= expected { existing } else { 0 };
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(existing == 0)
            .open(&part)?;
        Ok(Self {
            target,
            part,
            file,
            expected,
            received: existing,
        })
    }

    pub fn write_chunk(&mut self, offset: u64, data: &[u8]) -> Result<(), FileError> {
        if offset != self.received || self.received + data.len() as u64 > self.expected {
            return Err(FileError::OutOfOrder);
        }
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(data)?;
        self.received += data.len() as u64;
        Ok(())
    }

    /// Verifies length and SHA-256 over the whole part file, then atomically renames into place.
    pub fn finish(mut self, sha256: &[u8; 32]) -> Result<PathBuf, FileError> {
        self.file.flush()?;
        self.file.sync_all()?;
        if self.received != self.expected {
            return Err(FileError::OutOfOrder);
        }
        let digest = hash_file(&self.part)?;
        if &digest != sha256 {
            let _ = fs::remove_file(&self.part);
            return Err(FileError::BadName);
        }
        drop(self.file);
        fs::rename(&self.part, &self.target)?;
        Ok(self.target)
    }

    /// Cancelled transfers keep their `.part` so they can be resumed; `discard` removes it.
    pub fn discard(self) {
        drop(self.file);
        let _ = fs::remove_file(&self.part);
    }
}

pub fn hash_file(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().into())
}

pub struct OutgoingFile {
    file: File,
    pub size: u64,
    pub offset: u64,
}

impl OutgoingFile {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            file,
            size,
            offset: 0,
        })
    }

    pub fn seek_to(&mut self, offset: u64) -> std::io::Result<()> {
        self.file.seek(SeekFrom::Start(offset))?;
        self.offset = offset;
        Ok(())
    }

    /// Next chunk and its offset, or None at end of file.
    pub fn next_chunk(&mut self) -> std::io::Result<Option<(u64, Vec<u8>)>> {
        let mut buf = vec![0u8; CHUNK];
        let n = self.file.read(&mut buf)?;
        if n == 0 {
            return Ok(None);
        }
        buf.truncate(n);
        let at = self.offset;
        self.offset += n as u64;
        Ok(Some((at, buf)))
    }
}

/// Expands dropped paths (files and folders) into `(absolute path, relative wire name)` pairs.
pub fn expand_paths(paths: &[PathBuf]) -> std::io::Result<Vec<(PathBuf, String)>> {
    fn walk(
        root_name: &str,
        dir: &Path,
        out: &mut Vec<(PathBuf, String)>,
        depth: u32,
    ) -> std::io::Result<()> {
        if depth > 32 {
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = format!("{root_name}/{}", entry.file_name().to_string_lossy());
            let meta = entry.metadata()?;
            if meta.is_dir() {
                walk(&name, &entry.path(), out, depth + 1)?;
            } else if meta.is_file() {
                out.push((entry.path(), name));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    for p in paths {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let meta = fs::metadata(p)?;
        if meta.is_dir() {
            walk(&name, p, &mut out, 0)?;
        } else if meta.is_file() {
            out.push((p.clone(), name));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn rejects_traversal_and_hostile_names() {
        for bad in [
            "..",
            "../x",
            "a/../../x",
            "/abs",
            "C:\\x",
            "a\\b",
            "C:x",
            "con",
            "NUL.txt",
            "a/",
            "",
            "x:stream",
            "trailing.",
            "sp ",
            "a/./b",
            "wild*.txt",
            "q?.txt",
            "pipe|.txt",
            "ctl\u{7}.txt",
        ] {
            assert!(sanitize_relative(bad).is_err(), "should reject {bad:?}");
        }
        assert_eq!(
            sanitize_relative("dir/sub/file.txt").unwrap(),
            PathBuf::from("dir").join("sub").join("file.txt")
        );
        assert!(sanitize_relative("ملف.txt").is_ok());
        assert!(sanitize_relative(&"a".repeat(300)).is_err());
    }

    #[test]
    fn transfer_roundtrip_with_resume_and_verification() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        let data: Vec<u8> = (0..CHUNK * 3 + 123).map(|i| (i % 251) as u8).collect();
        let sp = src.path().join("big.bin");
        fs::write(&sp, &data).unwrap();
        let hash = hash_file(&sp).unwrap();

        // First attempt is interrupted after one chunk.
        let mut out = OutgoingFile::open(&sp).unwrap();
        let mut inc = IncomingFile::open(dst.path(), "big.bin", out.size, DEFAULT_MAX_FILE, true).unwrap();
        let (o, c) = out.next_chunk().unwrap().unwrap();
        inc.write_chunk(o, &c).unwrap();
        drop(inc);

        // Resume from what the receiver already has.
        let mut inc = IncomingFile::open(dst.path(), "big.bin", out.size, DEFAULT_MAX_FILE, true).unwrap();
        assert_eq!(inc.received, CHUNK as u64);
        let mut out = OutgoingFile::open(&sp).unwrap();
        out.seek_to(inc.received).unwrap();
        while let Some((o, c)) = out.next_chunk().unwrap() {
            inc.write_chunk(o, &c).unwrap();
        }
        let done = inc.finish(&hash).unwrap();
        assert_eq!(fs::read(done).unwrap(), data);
        assert!(!part_path(&dst.path().join("big.bin")).exists());
    }

    #[test]
    fn corrupted_transfer_is_rejected_and_cleaned() {
        let dst = tempdir().unwrap();
        let mut inc = IncomingFile::open(dst.path(), "x.bin", 4, DEFAULT_MAX_FILE, false).unwrap();
        inc.write_chunk(0, &[1, 2, 3, 4]).unwrap();
        assert!(inc.finish(&[0u8; 32]).is_err());
        assert!(!dst.path().join("x.bin").exists());
        assert!(!part_path(&dst.path().join("x.bin")).exists());
    }

    #[test]
    fn oversize_and_out_of_order_are_rejected() {
        let dst = tempdir().unwrap();
        assert!(matches!(
            IncomingFile::open(dst.path(), "a", 100, 10, false),
            Err(FileError::TooLarge)
        ));
        let mut inc = IncomingFile::open(dst.path(), "a", 10, 100, false).unwrap();
        assert!(inc.write_chunk(5, &[0]).is_err());
        assert!(inc.write_chunk(0, &[0; 11]).is_err());
    }

    #[test]
    fn existing_files_are_never_overwritten() {
        let dst = tempdir().unwrap();
        fs::write(dst.path().join("a.txt"), b"keep").unwrap();
        let p = unique_path(dst.path(), Path::new("a.txt"));
        assert_eq!(p.file_name().unwrap(), "a (1).txt");
    }

    #[test]
    fn folders_expand_with_relative_names() {
        let d = tempdir().unwrap();
        let root = d.path().join("proj");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.txt"), b"1").unwrap();
        fs::write(root.join("sub").join("b.txt"), b"2").unwrap();
        let mut names: Vec<_> = expand_paths(&[root])
            .unwrap()
            .into_iter()
            .map(|(_, n)| n)
            .collect();
        names.sort();
        assert_eq!(names, vec!["proj/a.txt", "proj/sub/b.txt"]);
    }
}
