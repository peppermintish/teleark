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
}

impl NativeFileSystem {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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
        if destination.exists() {
            return Err(TransferError::PermissionDenied);
        }
        if let Some(parent) = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| map_destination_io(&error))?;
        }
        let partial = self.partial_path(destination_id)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(partial)
            .map_err(|error| map_destination_io(&error))?;
        let existing = file
            .metadata()
            .map_err(|error| map_destination_io(&error))?
            .len();
        if existing != 0 && existing != total_bytes {
            return Err(TransferError::HashMismatch);
        }
        file.set_len(total_bytes)
            .map_err(|error| map_destination_io(&error))
    }

    fn write_partial(
        &mut self,
        destination_id: DestinationId,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), TransferError> {
        let mut file = OpenOptions::new()
            .write(true)
            .open(self.partial_path(destination_id)?)
            .map_err(|error| map_destination_io(&error))?;
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
        let mut file = File::open(self.partial_path(destination_id)?)
            .map_err(|error| map_destination_io(&error))?;
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
        let file = File::open(self.partial_path(destination_id)?)
            .map_err(|error| map_destination_io(&error))?;
        hash_reader(file, false)
    }

    fn flush_partial(&mut self, destination_id: DestinationId) -> Result<(), TransferError> {
        OpenOptions::new()
            .write(true)
            .open(self.partial_path(destination_id)?)
            .and_then(|file| file.sync_all())
            .map_err(|error| map_destination_io(&error))
    }

    fn atomic_finalize(&mut self, destination_id: DestinationId) -> Result<(), TransferError> {
        let destination = self.destination_path(destination_id)?.to_owned();
        if destination.exists() {
            return Err(TransferError::PermissionDenied);
        }
        std::fs::rename(self.partial_path(destination_id)?, &destination)
            .map_err(|error| map_destination_io(&error))?;
        if let Some(parent) = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| map_destination_io(&error))?;
        }
        Ok(())
    }

    fn final_exists(&self, destination_id: DestinationId) -> bool {
        self.destination_path(destination_id)
            .is_ok_and(Path::exists)
    }
}

fn hash_reader(mut reader: File, source: bool) -> Result<ContentDigest, TransferError> {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
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
}
