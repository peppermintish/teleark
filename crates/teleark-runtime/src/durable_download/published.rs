//! Retained read owner for output published before an interrupted final checkpoint.
use std::{
    fs::{File, Metadata},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};
use teleark_core::TransferError;

pub(super) struct PublishedOutput {
    path: PathBuf,
    file: File,
    identity: Metadata,
}

impl PublishedOutput {
    pub(super) fn open(path: &Path) -> Result<Option<Self>, TransferError> {
        let named = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(TransferError::PermissionDenied),
        };
        if !named.is_file() {
            return Err(TransferError::PermissionDenied);
        }
        let file = File::open(path).map_err(|_| TransferError::PermissionDenied)?;
        let identity = file
            .metadata()
            .map_err(|_| TransferError::PermissionDenied)?;
        if !same_file(&named, &identity) {
            return Err(TransferError::PermissionDenied);
        }
        let output = Self {
            path: path.to_owned(),
            file,
            identity,
        };
        output.validate_name()?;
        Ok(Some(output))
    }

    fn validate_name(&self) -> Result<(), TransferError> {
        let named =
            std::fs::symlink_metadata(&self.path).map_err(|_| TransferError::PermissionDenied)?;
        if !same_file(&self.identity, &named) {
            return Err(TransferError::PermissionDenied);
        }
        Ok(())
    }

    pub(super) fn verify(
        &mut self,
        size: u64,
        digest: [u8; 32],
        mut check: impl FnMut() -> Result<(), TransferError>,
    ) -> Result<(), TransferError> {
        self.validate_name()?;
        if self
            .file
            .metadata()
            .map_err(|_| TransferError::PermissionDenied)?
            .len()
            != size
        {
            return Err(TransferError::HashMismatch);
        }
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|_| TransferError::PermissionDenied)?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = [0; 1024 * 1024];
        loop {
            check()?;
            let count = self
                .file
                .read(&mut buffer)
                .map_err(|_| TransferError::PermissionDenied)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        self.validate_name()?;
        if hasher.finalize().as_bytes() != &digest {
            return Err(TransferError::HashMismatch);
        }
        Ok(())
    }

    pub(super) fn read_range(
        &mut self,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, TransferError> {
        self.validate_name()?;
        let length = usize::try_from(length).map_err(|_| TransferError::HashMismatch)?;
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|_| TransferError::PermissionDenied)?;
        let mut bytes = vec![0; length];
        self.file
            .read_exact(&mut bytes)
            .map_err(|_| TransferError::HashMismatch)?;
        self.validate_name()?;
        Ok(bytes)
    }
}

fn same_file(original: &Metadata, named: &Metadata) -> bool {
    if !original.is_file() || !named.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        original.dev() == named.dev() && original.ino() == named.ino()
    }
    #[cfg(not(unix))]
    {
        // File bytes are always read from the retained handle. Platforms without
        // stable std file IDs additionally compare the available timestamps.
        original.len() == named.len()
            && original.created().ok() == named.created().ok()
            && original.modified().ok() == named.modified().ok()
    }
}
