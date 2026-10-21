//! Receiver-side storage: chunks are written in place into a pre-sized `.owpart` file, renamed
//! to its final name when complete.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Extension of incomplete downloads.
const PART_EXT: &str = "owpart";

#[cfg(unix)]
fn write_at(f: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(f, buf, offset)
}

#[cfg(unix)]
fn read_at(f: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(f, buf, offset)
}

#[cfg(windows)]
fn write_at(f: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = f.seek_write(buf, offset)?;
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}

#[cfg(windows)]
fn read_at(f: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = f.seek_read(buf, offset)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buf = &mut buf[n..];
        offset += n as u64;
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn write_at(f: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = f;
    f.seek(SeekFrom::Start(offset))?;
    f.write_all(buf)
}

#[cfg(not(any(unix, windows)))]
fn read_at(f: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = f;
    f.seek(SeekFrom::Start(offset))?;
    f.read_exact(buf)
}

/// Reads `len` bytes at `offset` of an existing file (sender side, or a finished download).
///
/// # Errors
///
/// I/O errors.
pub(crate) fn read_range(path: &Path, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    let f = File::open(path)?;
    let mut buf = vec![0u8; len];
    read_at(&f, &mut buf, offset)?;
    Ok(buf)
}

/// File being downloaded.
#[derive(Debug)]
pub struct PartFile {
    file: File,
    part: PathBuf,
    target: PathBuf,
}

impl PartFile {
    /// Path of the incomplete file for `target` (`<target>.owpart`).
    #[must_use]
    pub fn part_path(target: &Path) -> PathBuf {
        let mut name = target
            .file_name()
            .map(std::ffi::OsStr::to_os_string)
            .unwrap_or_default();
        name.push(".");
        name.push(PART_EXT);
        target.with_file_name(name)
    }

    /// Opens (or creates) the incomplete file of `target`, sized to `size` bytes.
    ///
    /// # Errors
    ///
    /// I/O errors.
    pub fn open(target: &Path, size: u64) -> io::Result<Self> {
        let part = Self::part_path(target);
        if let Some(dir) = part.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&part)?;
        if file.metadata()?.len() != size {
            file.set_len(size)?;
        }
        Ok(Self {
            file,
            part,
            target: target.to_path_buf(),
        })
    }

    /// Final path.
    #[must_use]
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// Path of the incomplete file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.part
    }

    /// Writes plaintext at `offset`.
    ///
    /// # Errors
    ///
    /// I/O errors.
    pub fn write(&self, offset: u64, data: &[u8]) -> io::Result<()> {
        write_at(&self.file, data, offset)
    }

    /// Reads plaintext at `offset`.
    ///
    /// # Errors
    ///
    /// I/O errors.
    pub fn read(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        read_at(&self.file, &mut buf, offset)?;
        Ok(buf)
    }

    /// Flushes written data to disk (call before persisting the bitmap).
    ///
    /// # Errors
    ///
    /// I/O errors.
    pub fn sync(&self) -> io::Result<()> {
        self.file.sync_data()
    }

    /// Renames the complete file to its final name (a free name next to it if taken). Returns the
    /// final path.
    ///
    /// # Errors
    ///
    /// I/O errors.
    pub fn finish(self) -> io::Result<PathBuf> {
        self.file.sync_all()?;
        drop(self.file);
        let target = if self.target.exists() {
            unique_path(&self.target)
        } else {
            self.target
        };
        std::fs::rename(&self.part, &target)?;
        Ok(target)
    }
}

/// Bidirectional formatting characters: they can disguise an extension (`gpj.exe` shown as
/// `exe.jpg`).
fn is_bidi(c: char) -> bool {
    matches!(c, '\u{200e}' | '\u{200f}' | '\u{061c}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Makes a received file name safe: no directories, no control or reserved characters, no
/// Windows reserved device names, at most 180 bytes. Never empty.
#[must_use]
pub fn safe_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let mut out: String = base
        .chars()
        .map(|c| {
            if c.is_control() || is_bidi(c) || "<>:\"|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    out = out.trim().trim_end_matches(['.', ' ']).to_owned();
    while out.starts_with('.') {
        out.remove(0);
    }
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        out.insert(0, '_');
    }
    if out.len() > 180 {
        let ext = Path::new(&out)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let keep_ext = ext.len() <= 16 && !ext.is_empty();
        let budget = if keep_ext { 180 - ext.len() - 1 } else { 180 };
        let mut cut = budget;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        let mut short = out[..cut].to_owned();
        if keep_ext {
            short.push('.');
            short.push_str(ext);
        }
        out = short;
    }
    if out.is_empty() {
        "file".to_owned()
    } else {
        out
    }
}

/// `path` if free, else `name (1).ext`, `name (2).ext`, ...
#[must_use]
pub fn unique_path(path: &Path) -> PathBuf {
    let free = |p: &Path| !p.exists() && !PartFile::part_path(p).exists();
    if free(path) {
        return path.to_path_buf();
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = path.extension().and_then(|e| e.to_str());
    (1..)
        .map(|n| {
            path.with_file_name(match ext {
                Some(e) => format!("{stem} ({n}).{e}"),
                None => format!("{stem} ({n})"),
            })
        })
        .find(|p| free(p))
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_file_name("C:\\Windows\\a.exe"), "a.exe");
        assert_eq!(safe_file_name("con.txt"), "_con.txt");
        assert_eq!(safe_file_name("COM1"), "_COM1");
        assert_eq!(safe_file_name("a<b>:c?.png"), "a_b__c_.png");
        assert_eq!(safe_file_name(".hidden"), "hidden");
        assert_eq!(safe_file_name("..."), "file");
        assert_eq!(safe_file_name("x\u{202e}gpj.exe"), "x_gpj.exe");
        let long = format!("{}.mkv", "é".repeat(200));
        let s = safe_file_name(&long);
        assert!(s.len() <= 180 && s.ends_with(".mkv"));
    }

    #[test]
    fn part_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("x.bin");
        let p = PartFile::open(&target, 10).unwrap();
        p.write(5, b"world").unwrap();
        p.write(0, b"hello").unwrap();
        assert_eq!(p.read(0, 10).unwrap(), b"helloworld");
        std::fs::write(&target, b"taken").unwrap();
        let fin = p.finish().unwrap();
        assert_eq!(fin, dir.path().join("x (1).bin"));
        assert_eq!(std::fs::read(fin).unwrap(), b"helloworld");
    }
}
