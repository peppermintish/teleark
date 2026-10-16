//! Best-effort playback properties, never payload decoding or free-form tags.
//! Runs in retained background owners; reads and seeks share limits.
use std::{
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    time::{Duration, Instant},
};
use teleark_core::TransferError;
use teleark_crypto::MediaProperties;

const MAX_READ: u64 = 16 * 1024 * 1024;
const MAX_OPERATIONS: u32 = 8192;
const MAX_TIME: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PlaybackMetadata {
    pub duration_ms: Option<u64>,
    pub properties: MediaProperties,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProbeState {
    Reading,
    Ready,
    Unavailable,
    Limited,
}

pub(crate) fn inspect(
    path: &Path,
    mut phase: impl FnMut(ProbeState),
    cancelled: impl Fn() -> bool,
) -> Result<PlaybackMetadata, TransferError> {
    if cancelled() {
        return Err(TransferError::Cancelled);
    }
    // Acknowledgment precedes even opening the source.
    phase(ProbeState::Reading);
    let result = match std::fs::File::open(path) {
        // Third-party parser diagnostics may contain unselected free-form tags.
        // Only our sanitized phase/outcome events leave this extraction scope.
        Ok(file) => {
            tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), || {
                inspect_reader(file, &cancelled)
            })
        }
        Err(_) => Ok((PlaybackMetadata::default(), false)),
    }?;
    phase(if result.1 {
        ProbeState::Limited
    } else if result.0.duration_ms.is_some() || !result.0.properties.is_empty() {
        ProbeState::Ready
    } else {
        ProbeState::Unavailable
    });
    Ok(result.0)
}

fn inspect_reader(
    reader: impl Read + Seek,
    cancelled: impl Fn() -> bool,
) -> Result<(PlaybackMetadata, bool), TransferError> {
    use lofty::{config::ParseOptions, file::AudioFile, probe::Probe};
    use nom_exif::{MediaParser, MediaSource, TrackInfoTag};
    let mut reader = LimitedReader {
        inner: reader,
        cancelled,
        remaining: MAX_READ,
        operations: MAX_OPERATIONS,
        started: Instant::now(),
        limited: false,
    };
    let mut output = PlaybackMetadata::default();
    if let Ok(source) = MediaSource::seekable(&mut reader)
        && let Ok(info) = MediaParser::new().parse_track(source)
    {
        output.duration_ms = info
            .get(TrackInfoTag::DurationMs)
            .and_then(|value| value.as_u64())
            .filter(|value| *value <= i64::MAX as u64);
        output.properties.width = info
            .get(TrackInfoTag::Width)
            .and_then(|value| value.as_u32())
            .filter(|value| *value != 0);
        output.properties.height = info
            .get(TrackInfoTag::Height)
            .and_then(|value| value.as_u32())
            .filter(|value| *value != 0);
    }
    if reader.seek(SeekFrom::Start(0)).is_ok() {
        let options = ParseOptions::new()
            .read_tags(false)
            .read_cover_art(false)
            .max_junk_bytes(4096);
        if let Ok(probe) = Probe::new(&mut reader).options(options).guess_file_type()
            && let Ok(file) = probe.read()
        {
            let properties = file.properties();
            // Lofty uses zero for missing duration; do not turn that into a measurement.
            output.duration_ms = output.duration_ms.or_else(|| {
                u64::try_from(properties.duration().as_millis())
                    .ok()
                    .filter(|value| *value > 0 && *value <= i64::MAX as u64)
            });
            output.properties.sample_rate_hz = properties.sample_rate().filter(|v| *v != 0);
            output.properties.channels = properties.channels().filter(|v| *v != 0).map(u32::from);
            output.properties.bit_depth = properties.bit_depth().filter(|v| *v != 0).map(u32::from);
        }
    }
    if (reader.cancelled)() {
        return Err(TransferError::Cancelled);
    }
    Ok((output, reader.limited))
}

struct LimitedReader<R, C> {
    inner: R,
    cancelled: C,
    remaining: u64,
    operations: u32,
    started: Instant,
    limited: bool,
}

impl<R, C: Fn() -> bool> LimitedReader<R, C> {
    fn check(&mut self) -> io::Result<()> {
        if (self.cancelled)() {
            return Err(io::Error::other("media probe cancelled"));
        }
        if self.operations == 0 || self.started.elapsed() >= MAX_TIME {
            self.limited = true;
            return Err(io::Error::other("media probe limit"));
        }
        self.operations -= 1;
        Ok(())
    }
}

impl<R: Read, C: Fn() -> bool> Read for LimitedReader<R, C> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            self.limited = true;
            return Err(io::Error::other("media probe byte limit"));
        }
        let count = buffer.len().min(self.remaining as usize);
        let count = self.inner.read(&mut buffer[..count])?;
        self.remaining -= count as u64;
        self.check()?;
        Ok(count)
    }
}

impl<R: Seek, C: Fn() -> bool> Seek for LimitedReader<R, C> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.check()?;
        let position = self.inner.seek(position)?;
        self.check()?;
        Ok(position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, io::Cursor};

    #[test]
    fn content_detected_video_and_audio_properties() {
        for bytes in [
            include_bytes!("vault/fixtures/media/video.mp4").as_slice(),
            include_bytes!("vault/fixtures/media/video.webm").as_slice(),
        ] {
            let (metadata, limited) = inspect_reader(Cursor::new(bytes), || false).expect("probe");
            assert!(!limited);
            assert_eq!(metadata.duration_ms, Some(2000));
            assert_eq!(metadata.properties.width, Some(160));
            assert_eq!(metadata.properties.height, Some(90));
        }
        for bytes in [
            include_bytes!("vault/fixtures/media/audio.flac").as_slice(),
            include_bytes!("vault/fixtures/media/audio.mp3").as_slice(),
        ] {
            let (metadata, limited) = inspect_reader(Cursor::new(bytes), || false).expect("probe");
            assert!(!limited);
            assert!(
                metadata
                    .duration_ms
                    .is_some_and(|value| (2000..=2100).contains(&value))
            );
            assert_eq!(metadata.properties.sample_rate_hz, Some(48_000));
            assert_eq!(metadata.properties.channels, Some(2));
            assert_eq!(metadata.properties.width, None);
        }
    }

    #[test]
    fn acknowledgment_precedes_open_and_cancellation_and_failure_are_safe() {
        let temp = tempfile::tempdir().expect("directory");
        let source = temp.path().join("misleading-name.mp4");
        let mut states = Vec::new();
        let metadata = inspect(
            &source,
            |state| {
                states.push(state);
                if state == ProbeState::Reading {
                    std::fs::write(&source, include_bytes!("vault/fixtures/media/audio.flac"))
                        .expect("source");
                }
            },
            || false,
        )
        .expect("properties");
        assert_eq!(states, [ProbeState::Reading, ProbeState::Ready]);
        assert_eq!(metadata.properties.sample_rate_hz, Some(48_000));
        assert_eq!(metadata.properties.bit_depth, Some(16));
        std::fs::write(&source, b"not a media file").expect("source");
        states.clear();
        assert_eq!(
            inspect(&source, |s| states.push(s), || false).expect("optional metadata"),
            PlaybackMetadata::default()
        );
        assert_eq!(states, [ProbeState::Reading, ProbeState::Unavailable]);
        let cancelled = Cell::new(false);
        assert_eq!(
            inspect(&source, |_| cancelled.set(true), || cancelled.get()),
            Err(TransferError::Cancelled)
        );
    }

    #[test]
    fn byte_operation_time_and_cancel_limits_do_not_reset_on_seek() {
        let cancelled = Cell::new(false);
        let mut reader = LimitedReader {
            inner: Cursor::new(vec![0; 64]),
            cancelled: || cancelled.get(),
            remaining: 4,
            operations: 20,
            started: Instant::now(),
            limited: false,
        };
        assert_eq!(reader.read(&mut [0; 8]).expect("bounded read"), 4);
        reader.seek(SeekFrom::Start(0)).expect("rewind");
        assert!(reader.read(&mut [0; 1]).is_err());
        assert!(reader.limited);
        reader.remaining = 4;
        reader.limited = false;
        reader.operations = 0;
        assert!(reader.seek(SeekFrom::Start(0)).is_err());
        reader.operations = 20;
        reader.started = Instant::now() - MAX_TIME;
        assert!(reader.read(&mut [0; 1]).is_err());
        reader.started = Instant::now();
        reader.limited = false;
        cancelled.set(true);
        assert!(reader.read(&mut [0; 1]).is_err());
        assert!(!reader.limited, "cancellation is a distinct terminal state");
    }

    #[test]
    fn sparse_wav_skips_payload_and_hostile_metadata_is_optional() {
        let dir = tempfile::tempdir().expect("directory");
        let source = dir.path().join("audio.bin");
        let size = 48_000u32 * 2 * 2 * 1800;
        let mut header = b"RIFF".to_vec();
        header.extend_from_slice(&(size + 36).to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&16u32.to_le_bytes());
        header.extend_from_slice(&1u16.to_le_bytes());
        header.extend_from_slice(&2u16.to_le_bytes());
        header.extend_from_slice(&48_000u32.to_le_bytes());
        header.extend_from_slice(&192_000u32.to_le_bytes());
        header.extend_from_slice(&4u16.to_le_bytes());
        header.extend_from_slice(&16u16.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&size.to_le_bytes());
        std::fs::write(&source, &header).expect("header");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .expect("file")
            .set_len(u64::from(size) + 44)
            .expect("sparse payload");
        let metadata = inspect(&source, |_| {}, || false).expect("bounded seek");
        assert_eq!(metadata.duration_ms, Some(1_800_000));
        assert_eq!(metadata.properties.channels, Some(2));
        for bytes in [
            b"\0\0\0\x01moov\x7f\xff\xff\xff\xff\xff\xff\xff".as_slice(),
            b"\x1a\x45\xdf\xa3\xff\xff".as_slice(),
        ] {
            assert!(inspect_reader(Cursor::new(bytes), || false).is_ok());
        }
    }

    #[test]
    fn blocked_reader_does_not_stop_another_probe_and_cancel_survives_release() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        };
        struct Blocked {
            bytes: Cursor<Vec<u8>>,
            entered: Option<mpsc::Sender<()>>,
            release: mpsc::Receiver<()>,
        }
        impl Read for Blocked {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                if let Some(entered) = self.entered.take() {
                    entered.send(()).expect("entered");
                    self.release
                        .recv_timeout(Duration::from_secs(10))
                        .expect("release");
                }
                self.bytes.read(output)
            }
        }
        impl Seek for Blocked {
            fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
                self.bytes.seek(position)
            }
        }
        let (entered, waiting) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let worker = std::thread::spawn(move || {
            inspect_reader(
                Blocked {
                    bytes: Cursor::new(include_bytes!("vault/fixtures/media/video.mp4").to_vec()),
                    entered: Some(entered),
                    release: blocked,
                },
                || worker_cancelled.load(Ordering::Acquire),
            )
        });
        waiting
            .recv_timeout(Duration::from_secs(10))
            .expect("blocked read");
        let (other, _) = inspect_reader(
            Cursor::new(include_bytes!("vault/fixtures/media/audio.flac")),
            || false,
        )
        .expect("independent probe");
        assert_eq!(other.duration_ms, Some(2000));
        cancelled.store(true, Ordering::Release);
        release.send(()).expect("release");
        assert_eq!(
            worker.join().expect("worker"),
            Err(TransferError::Cancelled)
        );
    }
}
