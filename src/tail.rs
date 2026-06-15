//! Incremental file tailing, implemented in Rust to avoid cross-platform
//! `tail(1)` differences. A [`std::fs::File`] reader advances its position with
//! each read, so repeated [`pump_available`] calls relay only newly appended
//! bytes — exactly what the supervisor needs to stream the capture log to its
//! own stdout.

use std::io::{self, Read, Write};

/// Copy all currently-available bytes from `reader` to `writer`, flushing
/// `writer` afterwards. Returns the number of bytes copied (0 at EOF).
pub fn pump_available<R: Read, W: Write>(reader: &mut R, writer: &mut W) -> io::Result<u64> {
    let mut buf = [0u8; 8192];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
        total += n as u64;
    }
    writer.flush()?;
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{File, OpenOptions, remove_file};
    use std::io::Cursor;

    #[test]
    fn pumps_all_available_bytes_from_cursor() {
        let mut reader = Cursor::new(b"hello".to_vec());
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(pump_available(&mut reader, &mut out).unwrap(), 5);
        assert_eq!(out, b"hello");
    }

    #[test]
    fn returns_zero_at_eof() {
        let mut reader = Cursor::new(Vec::new());
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(pump_available(&mut reader, &mut out).unwrap(), 0);
        assert!(out.is_empty());
    }

    #[test]
    fn relays_only_newly_appended_bytes_across_calls() {
        let path = std::env::temp_dir().join(format!("nrun-tail-test-{}.log", std::process::id()));
        let _ = remove_file(&path);
        let mut appender = File::create(&path).unwrap();
        let mut reader = File::open(&path).unwrap();
        let mut out: Vec<u8> = Vec::new();

        appender.write_all(b"hello\n").unwrap();
        appender.flush().unwrap();
        assert_eq!(pump_available(&mut reader, &mut out).unwrap(), 6);
        assert_eq!(out, b"hello\n");

        // nothing new yet
        assert_eq!(pump_available(&mut reader, &mut out).unwrap(), 0);

        let mut appender2 = OpenOptions::new().append(true).open(&path).unwrap();
        appender2.write_all(b"world\n").unwrap();
        appender2.flush().unwrap();
        assert_eq!(pump_available(&mut reader, &mut out).unwrap(), 6);
        assert_eq!(out, b"hello\nworld\n");

        let _ = remove_file(&path);
    }
}
