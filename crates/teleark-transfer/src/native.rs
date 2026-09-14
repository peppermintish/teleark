use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read as _, Seek as _, SeekFrom, Write as _},
    path::{Path, PathBuf},
};

use teleark_core::TransferError;

use crate::{
    ContentDigest, DestinationId, DigestPort, FileSystemPort, SourceId, SourceIdentity, SourcePort,
};

/// Production BLAKE3 implementation for transfer integrity evidence.
#[derive(Clone, Copy, Debug, Default)]
pub struct Blake3Digest;

impl DigestPort for Blake3Digest {
    fn digest(&self, bytes: &[u8]) -> ContentDigest {
        ContentDigest(*blake3::hash(bytes).as_bytes())
    }
}

/// Thread-confined native filesystem adapter used by the cooperative engine.
///
/// Paths are registered under opaque identifiers before work is enqueued, so
/// transfer scheduling and checkpoints never carry user-controlled path text.
#[derive(Debug, Default)]
pub struct NativeFileSystem {
    sources: BTreeMap<SourceId, PathBuf>,
    destinations: BTreeMap<DestinationId, PathBuf>,
    partials: BTreeMap<DestinationId, File>,
}

impl NativeFileSystem {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Hash a retained partial with a cancellation checkpoint before each read.
    pub fn digest_partial_cancellable(
        &mut self,
        destination_id: DestinationId,
        check: impl FnMut() -> Result<(), TransferError>,
    ) -> Result<ContentDigest, TransferError> {
        let file = self
            .partials
            .get_mut(&destination_id)
            .ok_or(TransferError::PermissionDenied)?;
        file.seek(SeekFrom::Start(0))
            .map_err(|error| map_destination_io(&error))?;
        hash_reader_cancellable(file, false, check)
    }

    pub fn register_source(
        &mut self,
        id: SourceId,
        path: impl Into<PathBuf>,
    ) -> Result<(), TransferError> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(TransferError::SourceMissing);
        }
        self.sources.insert(id, path);
        Ok(())
    }

    pub fn register_destination(
        &mut self,
        id: DestinationId,
        path: impl Into<PathBuf>,
    ) -> Result<(), TransferError> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(TransferError::PermissionDenied);
        }
        self.partials.remove(&id);
        self.destinations.insert(id, path);
        Ok(())
    }

    fn source_path(&self, id: SourceId) -> Result<&Path, TransferError> {
        self.sources
            .get(&id)
            .map(PathBuf::as_path)
            .ok_or(TransferError::SourceMissing)
    }

    fn destination_path(&self, id: DestinationId) -> Result<&Path, TransferError> {
        self.destinations
            .get(&id)
            .map(PathBuf::as_path)
            .ok_or(TransferError::PermissionDenied)
    }

    fn partial_path(&self, id: DestinationId) -> Result<PathBuf, TransferError> {
        let destination = self.destination_path(id)?;
        let mut name = destination
            .file_name()
            .ok_or(TransferError::PermissionDenied)?
            .to_os_string();
        name.push(".partial");
        Ok(destination.with_file_name(name))
    }
}

impl SourcePort for NativeFileSystem {
    fn source_identity(&self, source_id: SourceId) -> Result<SourceIdentity, TransferError> {
        let metadata = std::fs::metadata(self.source_path(source_id)?)
            .map_err(|error| map_source_io(&error))?;
        if !metadata.is_file() {
            return Err(TransferError::SourceMissing);
        }
        Ok(metadata_identity(&metadata))
    }

    fn read_source_range(
        &mut self,
        source_id: SourceId,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, TransferError> {
        let capacity = usize::try_from(length).map_err(|_| TransferError::SourceChanged)?;
        let mut file =
            File::open(self.source_path(source_id)?).map_err(|error| map_source_io(&error))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| map_source_io(&error))?;
        let mut bytes = Vec::with_capacity(capacity);
        file.take(length)
            .read_to_end(&mut bytes)
            .map_err(|error| map_source_io(&error))?;
        if bytes.len() != capacity {
            return Err(TransferError::SourceChanged);
        }
        Ok(bytes)
    }

    fn digest_source(&mut self, source_id: SourceId) -> Result<ContentDigest, TransferError> {
        let file =
            File::open(self.source_path(source_id)?).map_err(|error| map_source_io(&error))?;
        hash_reader(file, true)
    }
}

impl FileSystemPort for NativeFileSystem {
    fn prepare_partial(
        &mut self,
        destination_id: DestinationId,
        total_bytes: u64,
    ) -> Result<(), TransferError> {
        let destination = self.destination_path(destination_id)?;
        match std::fs::symlink_metadata(destination) {
            Ok(_) => return Err(TransferError::PermissionDenied),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(map_destination_io(&error)),
        }
        if let Some(parent) = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| map_destination_io(&error))?;
        }
        let partial = self.partial_path(destination_id)?;
        let existing_metadata = match std::fs::symlink_metadata(&partial) {
            Ok(metadata) if metadata.is_file() => Some(metadata),
            Ok(_) => return Err(TransferError::PermissionDenied),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(map_destination_io(&error)),
        };
        let file = OpenOptions::new()
            .create_new(existing_metadata.is_none())
            .truncate(false)
            .read(true)
            .write(true)
            .open(&partial)
            .map_err(|error| map_destination_io(&error))?;
        let opened = file
            .metadata()
            .map_err(|error| map_destination_io(&error))?;
        if !opened.is_file() {
            return Err(TransferError::PermissionDenied);
        }
        #[cfg(unix)]
        if existing_metadata.as_ref().is_some_and(|previous| {
            metadata_identity(previous).filesystem_id != metadata_identity(&opened).filesystem_id
        }) {
            return Err(TransferError::PermissionDenied);
        }
        let existing = opened.len();
        if existing != 0 && existing != total_bytes {
            return Err(TransferError::HashMismatch);
        }
        file.set_len(total_bytes)
            .map_err(|error| map_destination_io(&error))?;
        self.partials.insert(destination_id, file);
        Ok(())
    }

    fn write_partial(
        &mut self,
        destination_id: DestinationId,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), TransferError> {
        let file = self
            .partials
            .get_mut(&destination_id)
            .ok_or(TransferError::PermissionDenied)?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| map_destination_io(&error))?;
        file.write_all(bytes)
            .map_err(|error| map_destination_io(&error))
    }

    fn read_partial(
        &mut self,
        destination_id: DestinationId,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, TransferError> {
        let capacity = usize::try_from(length).map_err(|_| TransferError::HashMismatch)?;
        let file = self
            .partials
            .get_mut(&destination_id)
            .ok_or(TransferError::PermissionDenied)?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| map_destination_io(&error))?;
        let mut bytes = Vec::with_capacity(capacity);
        file.take(length)
            .read_to_end(&mut bytes)
            .map_err(|error| map_destination_io(&error))?;
        if bytes.len() != capacity {
            return Err(TransferError::HashMismatch);
        }
        Ok(bytes)
    }

    fn digest_partial(
        &mut self,
        destination_id: DestinationId,
    ) -> Result<ContentDigest, TransferError> {
        let file = self
            .partials
            .get_mut(&destination_id)
            .ok_or(TransferError::PermissionDenied)?;
        file.seek(SeekFrom::Start(0))
            .map_err(|error| map_destination_io(&error))?;
        hash_reader(file, false)
    }

    fn flush_partial(&mut self, destination_id: DestinationId) -> Result<(), TransferError> {
        self.partials
            .get(&destination_id)
            .ok_or(TransferError::PermissionDenied)?
            .sync_all()
            .map_err(|error| map_destination_io(&error))
    }

    fn atomic_finalize(&mut self, destination_id: DestinationId) -> Result<(), TransferError> {
        let destination = self.destination_path(destination_id)?.to_owned();
        let partial = self.partial_path(destination_id)?;
        let handle = self
            .partials
            .get(&destination_id)
            .ok_or(TransferError::PermissionDenied)?;
        let named =
            std::fs::symlink_metadata(&partial).map_err(|error| map_destination_io(&error))?;
        if !named.is_file() {
            return Err(TransferError::PermissionDenied);
        }
        #[cfg(unix)]
        if metadata_identity(&named).filesystem_id
            != metadata_identity(
                &handle
                    .metadata()
                    .map_err(|error| map_destination_io(&error))?,
            )
            .filesystem_id
        {
            return Err(TransferError::PermissionDenied);
        }
        handle
            .sync_all()
            .map_err(|error| map_destination_io(&error))?;
        // Sibling paths share a filesystem. Publication must fail atomically if
        // another owner creates the destination, including a dangling symlink.
        std::fs::hard_link(&partial, &destination).map_err(|error| map_destination_io(&error))?;
        if let Some(parent) = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| map_destination_io(&error))?;
        }
        // Keep the recoverable name until the published directory entry is
        // durable. Cleanup failure cannot invalidate the verified output.
        self.partials.remove(&destination_id);
        let _ = std::fs::remove_file(partial);
        Ok(())
    }

    fn final_exists(&self, destination_id: DestinationId) -> bool {
        self.destination_path(destination_id)
            .is_ok_and(Path::exists)
    }
}

fn hash_reader(reader: impl std::io::Read, source: bool) -> Result<ContentDigest, TransferError> {
    hash_reader_cancellable(reader, source, || Ok(()))
}

fn hash_reader_cancellable(
    mut reader: impl std::io::Read,
    source: bool,
    mut check: impl FnMut() -> Result<(), TransferError>,
) -> Result<ContentDigest, TransferError> {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        check()?;
        let count = reader.read(&mut buffer).map_err(|error| {
            if source {
                map_source_io(&error)
            } else {
                map_destination_io(&error)
            }
        })?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(ContentDigest(*hasher.finalize().as_bytes()))
}

#[cfg(unix)]
fn metadata_identity(metadata: &std::fs::Metadata) -> SourceIdentity {
    use std::os::unix::fs::MetadataExt as _;

    let filesystem_id = (u128::from(metadata.dev()) << 64) | u128::from(metadata.ino());
    let modified_at_units = timestamp_units(metadata.mtime(), metadata.mtime_nsec());
    let revision = timestamp_units(metadata.ctime(), metadata.ctime_nsec());
    SourceIdentity {
        filesystem_id,
        size_bytes: metadata.len(),
        modified_at_units,
        revision,
    }
}

#[cfg(unix)]
fn timestamp_units(seconds: i64, nanoseconds: i64) -> u64 {
    let seconds = u64::try_from(seconds).unwrap_or_default();
    let nanoseconds = u64::try_from(nanoseconds)
        .unwrap_or_default()
        .min(999_999_999);
    seconds
        .saturating_mul(1_000_000_000)
        .saturating_add(nanoseconds)
}

#[cfg(not(unix))]
fn metadata_identity(metadata: &std::fs::Metadata) -> SourceIdentity {
    let modified_at_units = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or_default();
    SourceIdentity {
        filesystem_id: 0,
        size_bytes: metadata.len(),
        modified_at_units,
        revision: modified_at_units,
    }
}

fn map_source_io(error: &std::io::Error) -> TransferError {
    match error.kind() {
        std::io::ErrorKind::NotFound => TransferError::SourceMissing,
        std::io::ErrorKind::PermissionDenied => TransferError::PermissionDenied,
        _ => TransferError::SourceChanged,
    }
}

fn map_destination_io(error: &std::io::Error) -> TransferError {
    match error.kind() {
        std::io::ErrorKind::StorageFull => TransferError::DiskFull,
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::AlreadyExists => {
            TransferError::PermissionDenied
        }
        _ => TransferError::PermissionDenied,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_file_hash_stops_between_reads() {
        let mut checks = 0;
        let result =
            super::hash_reader_cancellable(std::io::repeat(1).take(4 * 1024 * 1024), false, || {
                checks += 1;
                if checks == 2 {
                    Err(TransferError::Cancelled)
                } else {
                    Ok(())
                }
            });
        assert_eq!(result, Err(TransferError::Cancelled));
        assert_eq!(checks, 2);
    }

    #[test]
    fn source_ranges_and_identity_use_real_files() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("source.bin");
        std::fs::write(&source, b"abcdefgh")?;
        let mut files = NativeFileSystem::new();
        files.register_source(SourceId(7), &source)?;

        let identity = files.source_identity(SourceId(7))?;
        assert_eq!(identity.size_bytes, 8);
        assert_eq!(files.read_source_range(SourceId(7), 2, 4)?, b"cdef");

        std::fs::write(&source, b"changed")?;
        assert_ne!(files.source_identity(SourceId(7))?, identity);
        Ok(())
    }

    #[test]
    fn partial_output_is_flushed_then_atomically_published()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let destination = directory.path().join("restored.bin");
        let mut files = NativeFileSystem::new();
        files.register_destination(DestinationId(11), &destination)?;

        files.prepare_partial(DestinationId(11), 8)?;
        files.write_partial(DestinationId(11), 4, b"efgh")?;
        files.write_partial(DestinationId(11), 0, b"abcd")?;
        assert_eq!(files.read_partial(DestinationId(11), 0, 8)?, b"abcdefgh");
        files.flush_partial(DestinationId(11))?;
        files.atomic_finalize(DestinationId(11))?;

        assert_eq!(std::fs::read(&destination)?, b"abcdefgh");
        assert!(files.final_exists(DestinationId(11)));
        assert!(!directory.path().join("restored.bin.partial").exists());
        Ok(())
    }

    #[test]
    fn destination_collision_never_overwrites_a_completed_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let destination = directory.path().join("existing.bin");
        std::fs::write(&destination, b"keep")?;
        let mut files = NativeFileSystem::new();
        files.register_destination(DestinationId(3), &destination)?;

        assert_eq!(
            files.prepare_partial(DestinationId(3), 4),
            Err(TransferError::PermissionDenied)
        );
        assert_eq!(std::fs::read(destination)?, b"keep");
        Ok(())
    }

    #[test]
    fn destination_created_after_preparation_preserves_both_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let destination = directory.path().join("late.bin");
        let mut files = NativeFileSystem::new();
        files.register_destination(DestinationId(3), &destination)?;
        files.prepare_partial(DestinationId(3), 4)?;
        files.write_partial(DestinationId(3), 0, b"ours")?;
        files.flush_partial(DestinationId(3))?;
        std::fs::write(&destination, b"keep")?;
        assert_eq!(
            files.atomic_finalize(DestinationId(3)),
            Err(TransferError::PermissionDenied)
        );
        assert_eq!(std::fs::read(&destination)?, b"keep");
        assert_eq!(files.read_partial(DestinationId(3), 0, 4)?, b"ours");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn retained_partial_handle_does_not_write_a_replacement_path()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let destination = directory.path().join("output.bin");
        let partial = directory.path().join("output.bin.partial");
        let retained = directory.path().join("retained.bin");
        let mut files = NativeFileSystem::new();
        files.register_destination(DestinationId(3), &destination)?;
        files.prepare_partial(DestinationId(3), 4)?;
        std::fs::rename(&partial, &retained)?;
        std::fs::write(&partial, b"keep")?;
        files.write_partial(DestinationId(3), 0, b"ours")?;
        files.flush_partial(DestinationId(3))?;
        assert_eq!(files.read_partial(DestinationId(3), 0, 4)?, b"ours");
        assert_eq!(
            files.digest_partial(DestinationId(3))?,
            Blake3Digest.digest(b"ours")
        );
        assert_eq!(std::fs::read(&partial)?, b"keep");
        assert_eq!(std::fs::read(&retained)?, b"ours");
        assert_eq!(
            files.atomic_finalize(DestinationId(3)),
            Err(TransferError::PermissionDenied)
        );
        assert!(!destination.exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn partial_links_and_directories_preserve_unrelated_targets()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let destination = directory.path().join("output.bin");
        let partial = directory.path().join("output.bin.partial");
        let target = directory.path().join("unrelated.bin");
        std::fs::write(&target, b"")?;
        let mut files = NativeFileSystem::new();
        files.register_destination(DestinationId(3), &destination)?;
        for target_exists in [true, false] {
            if !target_exists {
                std::fs::remove_file(&target)?;
            }
            std::os::unix::fs::symlink(&target, &partial)?;
            assert_eq!(
                files.prepare_partial(DestinationId(3), 32),
                Err(TransferError::PermissionDenied)
            );
            assert_eq!(std::fs::read_link(&partial)?, target);
            if target_exists {
                assert!(std::fs::read(&target)?.is_empty());
            } else {
                assert!(!target.exists());
            }
            std::fs::remove_file(&partial)?;
        }
        std::fs::create_dir(&partial)?;
        assert_eq!(
            files.prepare_partial(DestinationId(3), 32),
            Err(TransferError::PermissionDenied)
        );
        assert!(partial.is_dir());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn dangling_destination_link_is_not_replaced() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let destination = directory.path().join("link.bin");
        let target = directory.path().join("absent.bin");
        let mut files = NativeFileSystem::new();
        files.register_destination(DestinationId(3), &destination)?;
        files.prepare_partial(DestinationId(3), 4)?;
        files.write_partial(DestinationId(3), 0, b"ours")?;
        std::os::unix::fs::symlink(&target, &destination)?;
        assert_eq!(
            files.atomic_finalize(DestinationId(3)),
            Err(TransferError::PermissionDenied)
        );
        assert_eq!(std::fs::read_link(&destination)?, target);
        assert_eq!(files.read_partial(DestinationId(3), 0, 4)?, b"ours");
        Ok(())
    }
}
