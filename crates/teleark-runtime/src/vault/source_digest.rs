//! Bounded source admission hashing before any persistent nonce reservation.
use std::{
    io::Read,
    path::Path,
    time::{Duration, Instant},
};
use teleark_core::TransferError;
use teleark_transfer::ContentDigest;

pub(super) struct SourceDigests {
    pub whole: ContentDigest,
    pub parts: Vec<ContentDigest>,
}
pub(super) fn inspect(
    source: &Path,
    part_sizes: &[u64],
    mut progress: impl FnMut(u64, u64) -> Result<(), TransferError>,
    mut cancelled: impl FnMut() -> bool,
) -> Result<SourceDigests, TransferError> {
    let total = part_sizes
        .iter()
        .try_fold(0u64, |sum, size| sum.checked_add(*size))
        .ok_or(TransferError::SourceChanged)?;
    if cancelled() {
        return Err(TransferError::Cancelled);
    }
    progress(0, total)?;
    if cancelled() {
        return Err(TransferError::Cancelled);
    }
    let mut file = std::fs::File::open(source).map_err(map_io)?;
    hash_reader(&mut file, part_sizes, total, progress, cancelled)
}
fn hash_reader(
    reader: &mut impl Read,
    part_sizes: &[u64],
    total: u64,
    mut progress: impl FnMut(u64, u64) -> Result<(), TransferError>,
    mut cancelled: impl FnMut() -> bool,
) -> Result<SourceDigests, TransferError> {
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut whole = blake3::Hasher::new();
    let mut parts = Vec::with_capacity(part_sizes.len());
    let mut completed = 0u64;
    let mut last_progress = Instant::now();
    for &size in part_sizes {
        if size == 0 {
            return Err(TransferError::SourceChanged);
        }
        let mut remaining = size;
        let mut part = blake3::Hasher::new();
        while remaining != 0 {
            if cancelled() {
                return Err(TransferError::Cancelled);
            }
            let count = remaining.min(buffer.len() as u64) as usize;
            reader.read_exact(&mut buffer[..count]).map_err(map_io)?;
            if cancelled() {
                return Err(TransferError::Cancelled);
            }
            part.update(&buffer[..count]);
            whole.update(&buffer[..count]);
            remaining -= count as u64;
            completed += count as u64;
            if last_progress.elapsed() >= Duration::from_millis(100) {
                progress(completed, total)?;
                last_progress = Instant::now();
            }
        }
        parts.push(ContentDigest(*part.finalize().as_bytes()));
        progress(completed, total)?;
    }
    if cancelled() {
        return Err(TransferError::Cancelled);
    }
    let mut extra = [0u8; 1];
    if reader.read(&mut extra).map_err(map_io)? != 0 || completed != total {
        return Err(TransferError::SourceChanged);
    }
    if cancelled() {
        return Err(TransferError::Cancelled);
    }
    Ok(SourceDigests {
        whole: ContentDigest(*whole.finalize().as_bytes()),
        parts,
    })
}
fn map_io(error: std::io::Error) -> TransferError {
    match error.kind() {
        std::io::ErrorKind::NotFound => TransferError::SourceMissing,
        std::io::ErrorKind::PermissionDenied => TransferError::PermissionDenied,
        _ => TransferError::SourceChanged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn metadata_changes_during_hashing_preserve_full_and_part_content_checks() {
        use std::os::unix::fs::PermissionsExt as _;
        use teleark_transfer::{NativeFileSystem, SourceId, SourcePort as _};
        let dir = tempfile::tempdir().expect("directory");
        let source = dir.path().join("metadata.bin");
        std::fs::write(&source, b"abcdef").expect("source");
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o644))
            .expect("initial permissions");
        let mut files = NativeFileSystem::new();
        files
            .register_source(SourceId(1), &source)
            .expect("source handle");
        let before = files.source_identity(SourceId(1)).expect("before");
        let digests = inspect(
            &source,
            &[2, 4],
            |completed, _| {
                if completed == 2 {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600))
                        .expect("metadata-only change between reads");
                }
                Ok(())
            },
            || false,
        )
        .expect("hash completes");
        let after = files.source_identity(SourceId(1)).expect("after");
        assert_ne!(before.revision, after.revision);
        assert!(crate::vault_recovery::upload_source_metadata_matches(
            before, after
        ));
        assert_eq!(digests.whole.0, *blake3::hash(b"abcdef").as_bytes());
        assert_eq!(digests.parts[0].0, *blake3::hash(b"ab").as_bytes());
        assert_eq!(digests.parts[1].0, *blake3::hash(b"cdef").as_bytes());
    }

    #[test]
    fn cancellation_is_checked_each_read_independently_of_progress_throttling() {
        use std::cell::Cell;
        struct CancellingReader<'a> {
            cancelled: &'a Cell<bool>,
            bytes: usize,
        }
        impl Read for CancellingReader<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                buffer.fill(42);
                self.bytes += buffer.len();
                self.cancelled.set(true);
                Ok(buffer.len())
            }
        }
        let cancelled = Cell::new(false);
        let progress_calls = Cell::new(0);
        let mut reader = CancellingReader {
            cancelled: &cancelled,
            bytes: 0,
        };
        let result = hash_reader(
            &mut reader,
            &[3 * 1024 * 1024],
            3 * 1024 * 1024,
            |_, _| {
                progress_calls.set(progress_calls.get() + 1);
                Ok(())
            },
            || cancelled.get(),
        );
        assert!(matches!(result, Err(TransferError::Cancelled)));
        assert_eq!(reader.bytes, 1024 * 1024);
        assert_eq!(
            progress_calls.get(),
            0,
            "stop cannot wait for a progress update"
        );
        let result = hash_reader(&mut reader, &[1], 1, |_, _| Ok(()), || true);
        assert!(matches!(result, Err(TransferError::Cancelled)));
        assert_eq!(
            reader.bytes,
            1024 * 1024,
            "pre-cancelled work must not read"
        );
    }

    #[test]
    fn source_admission_hashes_exact_layout_and_detects_length_changes() {
        let mut cursor = std::io::Cursor::new(b"abcdef");
        let result = hash_reader(&mut cursor, &[2, 4], 6, |_, _| Ok(()), || false).expect("hash");
        assert_eq!(result.whole.0, *blake3::hash(b"abcdef").as_bytes());
        assert_eq!(result.parts[0].0, *blake3::hash(b"ab").as_bytes());
        assert_eq!(result.parts[1].0, *blake3::hash(b"cdef").as_bytes());
        for bytes in [b"abcde".as_slice(), b"abcdefg".as_slice()] {
            assert!(
                hash_reader(
                    &mut std::io::Cursor::new(bytes),
                    &[2, 4],
                    6,
                    |_, _| Ok(()),
                    || false
                )
                .is_err()
            );
        }
    }
    #[test]
    fn admission_acknowledgment_precedes_open_and_can_cancel() {
        let missing = std::env::temp_dir().join("teleark-missing-source-admission-fixture");
        let result = inspect(
            &missing,
            &[4],
            |bytes, total| {
                assert_eq!((bytes, total), (0, 4));
                Err(TransferError::Cancelled)
            },
            || false,
        );
        assert!(matches!(result, Err(TransferError::Cancelled)));
    }
}
