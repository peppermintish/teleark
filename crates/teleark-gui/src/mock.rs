//! Deterministic Phase 1 presentation data.
//!
//! The real adapters deliberately do not live in this crate. These records let
//! the reference screens be exercised while the Core ports are implemented.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferState {
    Downloading,
    Uploading,
    Waiting,
    Completed,
    Failed,
}

impl TransferState {
    pub const fn message_id(self) -> &'static str {
        match self {
            Self::Downloading => "transfer.state.downloading",
            Self::Uploading => "transfer.state.uploading",
            Self::Waiting => "transfer.state.waiting",
            Self::Completed => "transfer.state.completed",
            Self::Failed => "transfer.state.failed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct TransferRow {
    pub name: &'static str,
    pub source: &'static str,
    pub size: &'static str,
    pub transferred: &'static str,
    pub progress: f32,
    pub speed: &'static str,
    pub eta: &'static str,
    pub connections: &'static str,
    pub state: TransferState,
    pub destination: &'static str,
}

pub fn transfers(include_queued_upload: bool) -> Vec<TransferRow> {
    let mut rows = vec![
        TransferRow {
            name: "Product_Demo_4K.mkv",
            source: "Product Demos",
            size: "14.61 GB",
            transferred: "10.6 GB",
            progress: 72.3,
            speed: "12.1 MB/s",
            eta: "2m 18s",
            connections: "64 / 128",
            state: TransferState::Downloading,
            destination: "/Downloads/Videos",
        },
        TransferRow {
            name: "Cinema_4K_Collection.zip",
            source: "Cinema 4K",
            size: "52.48 GB",
            transferred: "23.9 GB",
            progress: 45.6,
            speed: "6.2 MB/s",
            eta: "12m 42s",
            connections: "48 / 96",
            state: TransferState::Downloading,
            destination: "/Downloads/Archives",
        },
        TransferRow {
            name: "AI_Course_Lesson_06.zip",
            source: "Learning Library",
            size: "3.28 GB",
            transferred: "620 MB",
            progress: 18.9,
            speed: "4.8 MB/s",
            eta: "8m 31s",
            connections: "32 / 64",
            state: TransferState::Downloading,
            destination: "/Downloads/Archives",
        },
        TransferRow {
            name: "Presentation_Final.mov",
            source: "My Storage",
            size: "9.74 GB",
            transferred: "6.2 GB",
            progress: 63.4,
            speed: "8.7 MB/s",
            eta: "6m 12s",
            connections: "16 / 32",
            state: TransferState::Uploading,
            destination: "Telegram / My Storage",
        },
        TransferRow {
            name: "Design_System.pdf",
            source: "Design Assets",
            size: "45.6 MB",
            transferred: "45.6 MB",
            progress: 100.0,
            speed: "—",
            eta: "—",
            connections: "—",
            state: TransferState::Completed,
            destination: "/Downloads/Documents",
        },
        TransferRow {
            name: "Assets_2026.rar",
            source: "Design Assets",
            size: "8.73 GB",
            transferred: "8.73 GB",
            progress: 100.0,
            speed: "—",
            eta: "—",
            connections: "—",
            state: TransferState::Completed,
            destination: "/Downloads/Archives",
        },
        TransferRow {
            name: "Linux_Ubuntu_24.04.iso",
            source: "Software",
            size: "5.28 GB",
            transferred: "0 B",
            progress: 0.0,
            speed: "—",
            eta: "—",
            connections: "—",
            state: TransferState::Waiting,
            destination: "/Downloads/Images",
        },
        TransferRow {
            name: "AE_Plugins_2025.zip",
            source: "Software",
            size: "3.91 GB",
            transferred: "0 B",
            progress: 0.0,
            speed: "—",
            eta: "—",
            connections: "—",
            state: TransferState::Waiting,
            destination: "/Downloads/Archives",
        },
        TransferRow {
            name: "PostgreSQL_16.3.dmg",
            source: "Software",
            size: "1.23 GB",
            transferred: "0 B",
            progress: 0.0,
            speed: "—",
            eta: "—",
            connections: "—",
            state: TransferState::Waiting,
            destination: "/Downloads/Images",
        },
        TransferRow {
            name: "Dataset_Project.tar.gz",
            source: "Learning Library",
            size: "19.82 GB",
            transferred: "0 B",
            progress: 0.0,
            speed: "—",
            eta: "—",
            connections: "—",
            state: TransferState::Waiting,
            destination: "/Downloads/Archives",
        },
        TransferRow {
            name: "movie_source_2160p.mkv",
            source: "Cinema 4K",
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
            TransferRow {
                name: "Movie_Archive.mkv",
                source: "My Storage",
                size: "73.6 GB",
                transferred: "0 B",
                progress: 0.0,
                speed: "—",
                eta: "—",
                connections: "—",
                state: TransferState::Waiting,
                destination: "Telegram / My Storage",
            },
        );
    }

    rows
}

pub struct ActivityLog {
    pub time: &'static str,
    pub subject: &'static str,
    pub message_id: &'static str,
    pub is_error: bool,
}

pub fn activity_logs() -> Vec<ActivityLog> {
    vec![
        ActivityLog {
            time: "12:36:42",
            subject: "Product_Demo_4K.mkv",
            message_id: "log.connected",
            is_error: false,
        },
        ActivityLog {
            time: "12:36:41",
            subject: "Cinema_4K_Collection.zip",
            message_id: "log.block_downloaded",
            is_error: false,
        },
        ActivityLog {
            time: "12:36:40",
            subject: "AI_Course_Lesson_06.zip",
            message_id: "log.file_list",
            is_error: false,
        },
        ActivityLog {
            time: "12:36:39",
            subject: "Design_System.pdf",
            message_id: "log.verified",
            is_error: false,
        },
        ActivityLog {
            time: "12:36:37",
            subject: "movie_source_2160p.mkv",
            message_id: "log.connection_timeout",
            is_error: true,
        },
        ActivityLog {
            time: "12:36:36",
            subject: "Assets_2026.rar",
            message_id: "log.saved",
            is_error: false,
        },
    ]
}

pub struct ConnectionRow {
    pub address: &'static str,
    pub progress: &'static str,
    pub speed: &'static str,
    pub client: &'static str,
    pub latency: &'static str,
}

pub fn connections() -> Vec<ConnectionRow> {
    vec![
        ConnectionRow {
            address: "91.108.10.21:443",
            progress: "84.3%",
            speed: "2.4 MB/s",
            client: "MTProto",
            latency: "28 ms",
        },
        ConnectionRow {
            address: "149.154.175.54:443",
            progress: "76.1%",
            speed: "2.1 MB/s",
            client: "MTProto",
            latency: "31 ms",
        },
        ConnectionRow {
            address: "95.161.64.12:443",
            progress: "63.8%",
            speed: "1.8 MB/s",
            client: "MTProto",
            latency: "35 ms",
        },
        ConnectionRow {
            address: "185.220.101.33:443",
            progress: "55.4%",
            speed: "1.6 MB/s",
            client: "MTProto",
            latency: "42 ms",
        },
        ConnectionRow {
            address: "149.154.167.91:443",
            progress: "49.2%",
            speed: "1.3 MB/s",
            client: "MTProto",
            latency: "37 ms",
        },
    ]
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
