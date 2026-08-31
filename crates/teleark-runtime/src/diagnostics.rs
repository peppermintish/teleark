use std::{path::Path, path::PathBuf, sync::OnceLock};

use teleark_core::{ApplicationError, ApplicationErrorKind};
use tracing::Level;
use tracing_appender::{
    non_blocking::{ErrorCounter, NonBlockingBuilder, WorkerGuard},
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{Layer as _, layer::SubscriberExt as _, util::SubscriberInitExt as _};

const DIAGNOSTIC_BUFFERED_LINES: usize = 4_096;
const RETAINED_DAILY_LOGS: usize = 15;

static DIAGNOSTICS: OnceLock<DiagnosticsState> = OnceLock::new();

struct DiagnosticsState {
    log_directory: PathBuf,
    dropped_events: ErrorCounter,
    _guard: WorkerGuard,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticsStatus {
    pub log_directory: PathBuf,
    pub dropped_event_count: usize,
}

/// Installs TeleArk's process-wide structured tracing subscriber.
///
/// Logs are newline-delimited JSON, rotate daily, retain a bounded history,
/// and are written by a dedicated bounded worker so network/storage workers do
/// not block on diagnostic I/O. Callers must only record explicitly safe fields.
pub fn initialize_diagnostics(log_directory: &Path) -> Result<DiagnosticsStatus, ApplicationError> {
    if let Some(status) = diagnostics_status() {
        return Ok(status);
    }
    if !log_directory.is_absolute() {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    std::fs::create_dir_all(log_directory)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("teleark")
        .filename_suffix("jsonl")
        .max_log_files(RETAINED_DAILY_LOGS)
        .build(log_directory)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
    let (writer, guard) = NonBlockingBuilder::default()
        .buffered_lines_limit(DIAGNOSTIC_BUFFERED_LINES)
        .lossy(true)
        .finish(file_appender);
    let dropped_events = writer.error_counter();
    let teleark_targets =
        tracing_subscriber::filter::filter_fn(|metadata| is_teleark_target(metadata.target()));
    let json_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_ansi(false)
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_target(true)
        .with_writer(writer)
        .with_filter(tracing_subscriber::filter::LevelFilter::from_level(
            Level::DEBUG,
        ))
        .with_filter(teleark_targets);
    tracing_subscriber::registry()
        .with(json_layer)
        .try_init()
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Conflict))?;
    DIAGNOSTICS
        .set(DiagnosticsState {
            log_directory: log_directory.to_owned(),
            dropped_events,
            _guard: guard,
        })
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Conflict))?;
    tracing::info!(
        event = "diagnostics.initialized",
        buffered_lines = DIAGNOSTIC_BUFFERED_LINES,
        retained_daily_logs = RETAINED_DAILY_LOGS,
        "structured diagnostics initialized"
    );
    diagnostics_status().ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))
}

pub fn diagnostics_status() -> Option<DiagnosticsStatus> {
    DIAGNOSTICS.get().map(|state| DiagnosticsStatus {
        log_directory: state.log_directory.clone(),
        dropped_event_count: state.dropped_events.dropped_lines(),
    })
}

fn is_teleark_target(target: &str) -> bool {
    target == "teleark" || target.starts_with("teleark_")
}

#[cfg(test)]
mod tests {
    use super::is_teleark_target;

    #[test]
    fn diagnostic_target_filter_excludes_unreviewed_dependencies() {
        assert!(is_teleark_target("teleark"));
        assert!(is_teleark_target("teleark_runtime::telegram"));
        assert!(is_teleark_target("teleark_gui::screens::transfers"));
        assert!(!is_teleark_target("grammers_client"));
        assert!(!is_teleark_target("hyper::client"));
        assert!(!is_teleark_target("teleark-unreviewed"));
    }
}
