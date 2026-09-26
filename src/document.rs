use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use anyhow::{Result, ensure};

pub const VIEW_BYTES: u64 = 4 * 1024 * 1024;

pub fn read(path: &Path, limit: u64) -> Result<(Vec<u8>, bool)> {
    let file = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path)?;
    ensure!(file.metadata()?.is_file(), "Not a regular file.");
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    let truncated = bytes.len() as u64 > limit;
    bytes.truncate(limit as usize);
    Ok((bytes, truncated))
}

pub fn source(path: &Path) -> Result<String> {
    let (bytes, truncated) = read(path, VIEW_BYTES)?;
    ensure!(!truncated, "Document exceeds the 4 MiB viewer limit");
    ensure!(!bytes.contains(&0), "Binary file; Markdown preview unavailable");
    Ok(String::from_utf8(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn viewer_rejects_special_and_oversized_files_and_accepts_empty_files() {
        let root = std::env::temp_dir().join(format!("peekback-document-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let fifo = root.join("pipe.md");
        let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(source(&fifo).unwrap_err().to_string().contains("regular file"));
        let file = root.join("file.md");
        fs::write(&file, "").unwrap();
        assert_eq!(source(&file).unwrap(), "");
        fs::File::options().write(true).open(&file).unwrap().set_len(VIEW_BYTES + 1).unwrap();
        assert!(source(&file).unwrap_err().to_string().contains("limit"));
        fs::remove_dir_all(root).unwrap();
    }
}
