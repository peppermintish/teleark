//! Deterministic Phase 1 presentation data.
//!
//! The real adapters deliberately do not live in this crate. These records let
//! the reference screens be exercised while the Core ports are implemented.

use gpui_kit::SharedString;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferState {
    Downloading,
    Uploading,
    Waiting,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferDirection {
    Upload,
    Download,
}

impl TransferState {
    pub const fn message_id(self) -> &'static str {
        match self {
            Self::Downloading => "transfer.state.downloading",
            Self::Uploading => "transfer.state.uploading",
            Self::Waiting => "transfer.state.waiting",
            Self::Paused => "transfer.state.paused",
            Self::Completed => "transfer.state.completed",
            Self::Failed => "transfer.state.failed",
            Self::Cancelled => "transfer.state.cancelled",
        }
    }
}

#[derive(Clone, Debug)]
pub struct BatchSummary {
    pub file_names: Vec<SharedString>,
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub queued_at_unix_ms: i64,
}

#[derive(Clone, Debug)]
pub struct TransferRow {
    pub activity: Option<SharedString>,
    pub activity_detail: Option<SharedString>,
    pub runtime_task_id: Option<u64>,
    pub vault_transfer_id: Option<u64>,
    pub vault_batch_id: Option<u64>,
    pub runtime_batch_id: Option<u64>,
    pub batch_child: bool,
    pub batch_summary: Option<BatchSummary>,
    pub message_id: Option<i64>,
    pub message_sent_at_unix_ms: Option<i64>,
    pub caption: Option<SharedString>,
    pub mime_type: Option<SharedString>,
    pub name: SharedString,
    pub source: SharedString,
    pub direction: TransferDirection,
    pub size: SharedString,
    pub transferred: SharedString,
    pub progress: f32,
    pub speed: SharedString,
    pub eta: SharedString,
    pub connections: SharedString,
    pub state: TransferState,
    pub destination: SharedString,
}

macro_rules! transfer_row {
    (
        name: $name:expr,
        source: $source:expr,
        direction: $direction:expr,
        size: $size:expr,
        transferred: $transferred:expr,
        progress: $progress:expr,
        speed: $speed:expr,
        eta: $eta:expr,
        connections: $connections:expr,
        state: $state:expr,
        destination: $destination:expr $(,)?
    ) => {
        TransferRow {
            activity: None,
            activity_detail: None,
            runtime_task_id: None,
            vault_transfer_id: None,
            vault_batch_id: None,
            runtime_batch_id: None,
            batch_child: false,
            batch_summary: None,
            message_id: None,
            message_sent_at_unix_ms: None,
            caption: None,
            mime_type: None,
            name: $name.into(),
            source: $source.into(),
            direction: $direction,
            size: $size.into(),
            transferred: $transferred.into(),
            progress: $progress,
            speed: $speed.into(),
            eta: $eta.into(),
            connections: $connections.into(),
            state: $state,
            destination: $destination.into(),
        }
    };
}

pub fn transfers(include_queued_upload: bool) -> Vec<TransferRow> {
    let mut rows = vec![
        transfer_row! {
            name: "Product_Demo_4K.mkv",
            source: "Product Demos",
            direction: TransferDirection::Download,
            size: "14.61 GB",
            transferred: "10.6 GB",
            progress: 72.3,
            speed: "12.1 MB/s",
            eta: "2m 18s",
            connections: "64 / 128",
            state: TransferState::Downloading,
            destination: "/Downloads/Videos",
        },
        transfer_row! {
            name: "Cinema_4K_Collection.zip",
            source: "Cinema 4K",
            direction: TransferDirection::Download,
            size: "52.48 GB",
            transferred: "23.9 GB",
            progress: 45.6,
            speed: "6.2 MB/s",
            eta: "12m 42s",
            connections: "48 / 96",
            state: TransferState::Downloading,
            destination: "/Downloads/Archives",
        },
        transfer_row! {
            name: "AI_Course_Lesson_06.zip",
            source: "Learning Library",
            direction: TransferDirection::Download,
            size: "3.28 GB",
            transferred: "620 MB",
            progress: 18.9,
            speed: "4.8 MB/s",
            eta: "8m 31s",
            connections: "32 / 64",
            state: TransferState::Downloading,
            destination: "/Downloads/Archives",
        },
        transfer_row! {
            name: "Presentation_Final.mov",
            source: "TeleArk",
            direction: TransferDirection::Upload,
            size: "9.74 GB",
            transferred: "6.2 GB",
            progress: 63.4,
            speed: "8.7 MB/s",
            eta: "6m 12s",
            connections: "16 / 32",
            state: TransferState::Uploading,
            destination: "Telegram / TeleArk",
        },
        transfer_row! {
            name: "Design_System.pdf",
            source: "Design Assets",
            direction: TransferDirection::Download,
            size: "45.6 MB",
            transferred: "45.6 MB",
            progress: 100.0,
            speed: "—",
            eta: "—",
            connections: "—",
            state: TransferState::Completed,
            destination: "/Downloads/Documents",
        },
        transfer_row! {
            name: "Assets_2026.rar",
            source: "Design Assets",
            direction: TransferDirection::Download,
            size: "8.73 GB",
            transferred: "8.73 GB",
            progress: 100.0,
            speed: "—",
            eta: "—",
            connections: "—",
            state: TransferState::Completed,
            destination: "/Downloads/Archives",
        },
        transfer_row! {
            name: "Linux_Ubuntu_24.04.iso",
            source: "Software",
            direction: TransferDirection::Download,
            size: "5.28 GB",
            transferred: "0 B",
            progress: 0.0,
            speed: "—",
            eta: "—",
            connections: "—",
            state: TransferState::Waiting,
            destination: "/Downloads/Images",
        },
        transfer_row! {
            name: "AE_Plugins_2025.zip",
            source: "Software",
            direction: TransferDirection::Download,
            size: "3.91 GB",
            transferred: "0 B",
            progress: 0.0,
            speed: "—",
            eta: "—",
            connections: "—",
            state: TransferState::Waiting,
            destination: "/Downloads/Archives",
        },
        transfer_row! {
            name: "PostgreSQL_16.3.dmg",
            source: "Software",
            direction: TransferDirection::Download,
            size: "1.23 GB",
            transferred: "0 B",
            progress: 0.0,
            speed: "—",
            eta: "—",
            connections: "—",
            state: TransferState::Waiting,
            destination: "/Downloads/Images",
        },
        transfer_row! {
            name: "Dataset_Project.tar.gz",
            source: "Learning Library",
            direction: TransferDirection::Download,
            size: "19.82 GB",
            transferred: "0 B",
            progress: 0.0,
            speed: "—",
            eta: "—",
            connections: "—",
            state: TransferState::Waiting,
            destination: "/Downloads/Archives",
        },
        transfer_row! {
            name: "movie_source_2160p.mkv",
            source: "Cinema 4K",
            direction: TransferDirection::Download,
            size: "22.14 GB",
            transferred: "22.1 GB",
            progress: 100.0,
            speed: "—",
            eta: "—",
            connections: "—",
            state: TransferState::Failed,
            destination: "/Downloads/Videos",
        },
    ];

    if include_queued_upload {
        rows.insert(
            0,
            transfer_row! {
                name: "Movie_Archive.mkv",
                source: "TeleArk",
                direction: TransferDirection::Upload,
                size: "73.6 GB",
                transferred: "0 B",
                progress: 0.0,
                speed: "—",
                eta: "—",
                connections: "—",
                state: TransferState::Waiting,
                destination: "Telegram / TeleArk",
            },
        );
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_overlay_action_adds_a_deterministic_queue_fixture() {
        let baseline = transfers(false);
        let queued = transfers(true);

        assert_eq!(queued.len(), baseline.len() + 1);
        assert_eq!(queued[0].name, "Movie_Archive.mkv");
        assert_eq!(queued[0].state, TransferState::Waiting);
        assert_eq!(queued[1].name, baseline[0].name);
    }
}
