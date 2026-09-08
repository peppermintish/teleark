//! Synthetic, isolated fixtures. Called only after --preview-ui disables every runtime owner.
use super::*;

impl TeleArkApp {
    pub(super) fn refresh_preview_library(&mut self, cx: &mut Context<Self>) {
        let local = self.library_view == LibraryView::Local;
        let names = [
            "京都の春 — 旅行写真.jpg",
            "Project notes.pdf",
            "Archive.zip",
            "Field recording.flac",
            "Documentary.mp4",
        ];
        let text = self.search_input.read(cx).value().to_lowercase();
        let kind = library_kind_for_selection(self.library_kind_selection);
        let rows = names
            .iter()
            .enumerate()
            .filter_map(|(index, name)| {
                let file_kind = teleark_runtime::classify_file(std::path::Path::new(name));
                if kind.is_some_and(|kind| kind != file_kind)
                    || !name.to_lowercase().contains(&text)
                {
                    return None;
                }
                Some(crate::library_state::LibraryRowView {
                    id: if local {
                        crate::library_state::LibraryRowId::Local(
                            teleark_runtime::LocalLibraryKey::NativeDownload(index as u64 + 1),
                        )
                    } else {
                        crate::library_state::LibraryRowId::Catalog(
                            teleark_core::LogicalFileId::new(index as u64 + 1),
                        )
                    },
                    name: (*name).into(),
                    size_bytes: 1024 * 1024 * 123,
                    kind: file_kind,
                    source_name: Some("Design Library".into()),
                    local_source_path: local.then(|| {
                        std::path::PathBuf::from("/tmp/teleark-preview/Downloads").join(name)
                    }),
                    source_chat_id: Some(101),
                    source_account_id: Some(1),
                    source_message_id: Some(index as i64 + 1),
                    modified_at_unix_ms: Some(1_788_624_000_000),
                    remote_state: if local {
                        teleark_core::RemoteState::LocalOnly
                    } else {
                        teleark_core::RemoteState::Uploaded
                    },
                    encryption_state: teleark_core::EncryptionState::Unencrypted,
                    verification_state: teleark_core::VerificationState::Unverified,
                    package_id: None,
                    part_count: 0,
                })
            })
            .collect::<Vec<_>>();
        self.library_content = LibraryContent::from_snapshot(LibrarySnapshot {
            total_matching: rows.len() as u64,
            rows,
            next_cursor: None,
            statistics: LibraryStatistics::default(),
        });
        self.selected_file = 0;
        cx.notify();
    }

    pub(super) fn initialize_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visual_preview {
            return;
        }
        let state = std::env::args()
            .find_map(|arg| arg.strip_prefix("--preview-state=").map(str::to_owned))
            .unwrap_or_default();
        if std::env::args().any(|arg| arg == "--preview-dark") {
            self.preferences.appearance = AppearancePreference::Dark;
            theme::apply_appearance(AppearancePreference::Dark, window, cx);
        }
        self.preferences.lock_vault_when_hidden = false;
        self.volume_space = Some(teleark_runtime::VolumeSpace {
            available_bytes: 248 * 1024 * 1024 * 1024,
            total_bytes: 1024 * 1024 * 1024 * 1024,
        });
        self.telegram_activity = TelegramActivity::Idle;
        self.vault_activity = VaultActivity::Idle;
        self.preference_persistence = PreferencePersistence::Idle;
        self.locale_persistence = LocalePersistence::Idle;
        self.configured_telegram_api_id = Some(12345);
        if state == "login" {
            self.telegram_auth = TelegramAuthState::Unauthorized;
            self.page = Page::Account;
            return;
        }
        let account = TelegramAccount {
            id: 1,
            display_name: "Alex Chen".into(),
            username: None,
        };
        self.telegram_account = Some(account.clone());
        self.telegram_auth = TelegramAuthState::Authorized(account);
        self.telegram_chats = (1..=200)
            .map(|index| TelegramChatSummary {
                sync_pts: None,
                id: 1000 + index,
                name: format!(
                    "{} {index:03}",
                    [
                        "Design Library · Product research and reference archives",
                        "旅行摄影 · 世界各地的风景、人文与城市影像资料库",
                        "映像資料 · 旅の記録とデザイン参考コレクション",
                        "Archives",
                    ][index as usize % 4]
                ),
                username: None,
                kind: TelegramChatKind::Channel,
            })
            .collect();
        let storage = TelegramChatSummary {
            sync_pts: None,
            id: 9000,
            name: self.tr("storage-remote-title").to_string(),
            username: None,
            kind: TelegramChatKind::Channel,
        };
        self.storage_status = teleark_runtime::StorageChannelStatus::Ready(storage);
        self.storage_notice = Some("storage-auto-found");
        self.selected_chat_id = Some(9000);
        self.vault_status.configured = true;
        self.vault_status.locked = state == "locked" || state == "unlock";
        self.vault_status.active_key_locked = self.vault_status.locked;
        self.vault_locked = self.vault_status.locked;
        let names = [
            "Coastal Journey.mov",
            "Project Aurora — 设计稿.zip",
            "京都の春 • 写真集.zip",
            "Annual Report.pdf",
            "Field Recordings.wav",
            "Workshop Notes.md",
        ];
        self.managed_vault_files = std::sync::Arc::new(
            names
                .iter()
                .enumerate()
                .map(|(index, name)| ManagedVaultFile {
                    vault_id: None,
                    health: teleark_runtime::VaultFileHealth::Unchecked,
                    part_message_ids: Vec::new(),
                    package_numeric_id: index as u64 + 1,
                    package_id: format!("{:032x}", index + 1),
                    logical_name: (*name).into(),
                    relative_path: None,
                    mime_type: None,
                    media_kind: FileKind::Document,
                    size_bytes: 1024 * 1024 * (index as u64 + 1) * 123,
                    encoded_size_bytes: 1024 * 1024 * (index as u64 + 1) * 123 + 512,
                    part_count: 3,
                    created_at_unix_ms: 1_788_624_000_000,
                    manifest_message_id: 100 + index as i64,
                    related_remote_names: Vec::new(),
                })
                .collect(),
        );
        if state.starts_with("storage-resilience") {
            if let Some(channel) = self.storage_status.channel().cloned() {
                self.storage_status = teleark_runtime::StorageChannelStatus::Degraded {
                    channel,
                    health: teleark_runtime::StorageChannelHealth::IdentityMissing,
                };
            }
            self.storage_notice = None;
            let statuses = [
                teleark_runtime::VaultFileHealth::MissingParts,
                teleark_runtime::VaultFileHealth::Present,
                teleark_runtime::VaultFileHealth::KeyUnavailable,
                teleark_runtime::VaultFileHealth::MissingManifest,
                teleark_runtime::VaultFileHealth::Unchecked,
            ];
            for (index, file) in std::sync::Arc::make_mut(&mut self.managed_vault_files)
                .iter_mut()
                .enumerate()
            {
                file.health = statuses[index % statuses.len()];
                file.vault_id = Some([index as u8 + 1; 16]);
            }
            self.selected_telegram_message_id = Some(100);
            self.show_channel_detail = true;
            if state.ends_with("repair") {
                self.storage_confirmation = Some(super::storage::StorageAction::Repair);
            }
            if state.ends_with("archive") {
                self.storage_confirmation = Some(super::storage::StorageAction::Archive);
            }
            if state.ends_with("waiting") {
                self.storage_loading = true;
                self.storage_maintenance = Some(teleark_runtime::StorageMaintenance::new());
            }
        }
        if state == "new-key" {
            self.vault_new_epoch_confirmation = true;
            self.unlock_intent = Some(UnlockIntent::Upload);
            self.vault_locked = true;
            self.vault_status.locked = true;
            self.vault_status.active_key_locked = true;
        }

        self.telegram_files = (0..5000)
            .map(|index| TelegramFileSummary {
                message_id: 5000 - index,
                sent_at_unix_ms: 1_788_624_000_000,
                modified_at_unix_ms: 1_788_624_000_000,
                file_name: format!("{index:04} {}", names[index as usize % names.len()]),
                caption: (0..24).map(|line| format!("Field note {line:02} · 京都の春 — 摄影素材与项目记录。Original source content remains unchanged.\n")).collect(),
                mime_type: None,
                size_bytes: 1024 * 1024 * 123,
            })
            .collect();
        self.telegram_files_exhausted = true;
        if state == "dialogs-failed" || state == "dialogs-waiting" {
            self.page = Page::Storage;
            self.storage_status = teleark_runtime::StorageChannelStatus::Missing;
            self.telegram_chats.clear();
            self.dialogs
                .transition(super::dialogs::Phase::Reading, None);
            self.dialogs.transition(
                if state == "dialogs-waiting" {
                    super::dialogs::Phase::Waiting
                } else {
                    super::dialogs::Phase::Failed
                },
                Some(teleark_core::ApplicationErrorKind::Server),
            );
        }
        if state.starts_with("channel-sync") {
            use teleark_runtime::{ChannelSyncEvent, ChannelSyncPhase, ChannelSyncSnapshot};
            let now = std::time::Instant::now();
            let waiting = state == "channel-sync-wait";
            let phase = if waiting {
                ChannelSyncPhase::RateLimited
            } else {
                ChannelSyncPhase::Receiving
            };
            self.page = Page::Channel;
            self.storage_view = StorageView::RawFiles;
            self.selected_chat_id = Some(1001);
            self.last_channel_id = Some(1001);
            self.telegram_files_exhausted = false;
            self.channel_sync_details = waiting;
            self.channel_sync_snapshot = Some(ChannelSyncSnapshot {
                account_id: 1,
                phase,
                chat_id: Some(1001),
                phase_started: now - Duration::from_secs(12),
                last_activity: now - Duration::from_secs(8),
                retry_at: waiting.then_some(now + Duration::from_secs(30)),
                failure: None,
                queued: 3,
                failed_channels: 0,
                committed_pages: 4,
                data_revision: 4,
                events: [
                    ChannelSyncPhase::Queued,
                    ChannelSyncPhase::ReadingLocal,
                    ChannelSyncPhase::Receiving,
                    ChannelSyncPhase::Persisting,
                    phase,
                ]
                .into_iter()
                .map(|phase| ChannelSyncEvent {
                    phase,
                    chat_id: Some(1001),
                    at: now - Duration::from_secs(12),
                    failure: None,
                })
                .collect(),
                dropped_events: 0,
                overflow_signals: 0,
                managed_chat_id: Some(9000),
                managed_watch: Some(teleark_runtime::ManagedChannelWatch {
                    catalog_ready: true,
                    change_count: 2,
                    acknowledged_count: 0,
                    last_changed_at_unix_ms: Some(1_788_912_000_000),
                    changes: vec![teleark_runtime::ManagedChannelChange {
                        sequence: 2,
                        message_id: 42,
                        kind: teleark_runtime::ManagedChannelChangeKind::Deleted,
                        observed_at_unix_ms: 1_788_912_000_000,
                    }],
                }),
                managed_review_pending: !waiting,
                managed_scan: None,
            });
        }
        self.refresh_channel_file_table(cx);
        self.refresh_preview_library(cx);
        let fixture = crate::mock::transfers(false);
        let mut rows = Vec::new();
        for (batch_id, upload) in [(42_u64, false), (17_u64, true)] {
            let mut group = fixture[0].clone();
            group.runtime_batch_id = (!upload).then_some(batch_id);
            group.vault_batch_id = upload.then_some(batch_id);
            group.name = if upload {
                self.tr_with(
                    "transfer-batch-upload-name",
                    MessageArgs::new().with("count", "6"),
                )
            } else {
                self.tr_with(
                    "transfer-batch-name",
                    MessageArgs::new()
                        .with("count", "6")
                        .with("source", "Kyoto · September"),
                )
            };
            group.source = if upload {
                "TeleArk".into()
            } else {
                "Kyoto · September".into()
            };
            group.direction = if upload {
                crate::mock::TransferDirection::Upload
            } else {
                crate::mock::TransferDirection::Download
            };
            group.state = if upload {
                crate::mock::TransferState::Uploading
            } else {
                crate::mock::TransferState::Completed
            };
            group.progress = if upload { 35.0 } else { 100.0 };
            group.size = format_bytes(self.locale(), 21 * 123 * 1024 * 1024).into();
            group.transferred = if upload {
                format_bytes(self.locale(), 775 * 1024 * 1024).into()
            } else {
                group.size.clone()
            };
            group.batch_summary = Some(crate::mock::BatchSummary {
                file_names: names.iter().take(3).map(|name| (*name).into()).collect(),
                total: 6,
                completed: if upload { 2 } else { 6 },
                failed: 0,
                queued_at_unix_ms: if upload {
                    1_788_710_400_000
                } else {
                    1_788_624_000_000
                },
            });
            rows.push(group.clone());
            for (index, name) in names.iter().enumerate() {
                let mut row = group.clone();
                row.batch_summary = None;
                row.batch_child = true;
                row.name = (*name).into();
                let size_bytes = (index as u64 + 1) * 123 * 1024 * 1024;
                row.size = format_bytes(self.locale(), size_bytes).into();
                row.destination = format!("/Preview/Downloads/{name}").into();
                if !upload {
                    let destination = std::path::PathBuf::from(row.destination.as_ref());
                    self.local_downloads.insert(
                        destination.clone(),
                        super::local_files::LocalDownloadObservation {
                            file: teleark_runtime::DownloadedFileRecord {
                                cursor: teleark_runtime::DownloadedFilesCursor {
                                    kind: 0,
                                    id: 200 + index as u64,
                                },
                                account_id: 1,
                                chat_id: 9000,
                                message_id: Some(5000 - index as i64),
                                package_id: Some(format!("{:032x}", index + 1)),
                                destination,
                                size_bytes,
                                completed_at_unix_ms: 1_788_624_000_000,
                            },
                            presence: match index {
                                1 => teleark_runtime::LocalFilePresence::Missing,
                                2 => teleark_runtime::LocalFilePresence::SizeChanged,
                                _ => teleark_runtime::LocalFilePresence::Present,
                            },
                        },
                    );
                }
                row.runtime_task_id = (!upload).then_some(200 + index as u64);
                row.vault_transfer_id = upload.then_some(300 + index as u64);
                row.state = if !upload || index < 2 {
                    crate::mock::TransferState::Completed
                } else if index == 2 {
                    crate::mock::TransferState::Uploading
                } else {
                    crate::mock::TransferState::Waiting
                };
                row.progress = if row.state == crate::mock::TransferState::Completed {
                    100.0
                } else {
                    0.0
                };
                row.transferred = if row.state == crate::mock::TransferState::Completed {
                    row.size.clone()
                } else {
                    format_bytes(self.locale(), 0).into()
                };
                rows.push(row);
            }
        }
        for index in 0..48 {
            let mut row = fixture[index % fixture.len()].clone();
            row.name = format!("{:02} {}", index + 1, row.name).into();
            rows.push(row);
        }
        self.preview_transfer_rows = rows;
        match state.as_str() {
            "returning" => self.page = Page::Account,
            "upload-progress" | "session-active" => {
                self.page = Page::Transfers;
                self.nav_selection = "nav-uploads";
                self.upload_in_flight = true;
                self.vault_activity = VaultActivity::Working;
                let mut row = fixture[0].clone();
                row.name = "京都 — Archive.zip".into();
                row.source = "TeleArk".into();
                row.direction = crate::mock::TransferDirection::Upload;
                row.state = crate::mock::TransferState::Uploading;
                row.progress = 26.0 / 60.0 * 100.0;
                row.activity = Some(self.tr("transfer-upload-sending-bytes"));
                row.activity_detail = Some(
                    self.tr_with(
                        "transfer-upload-activity-bytes",
                        MessageArgs::new()
                            .with(
                                "phase",
                                self.tr("transfer-upload-sending-bytes").to_string(),
                            )
                            .with("done", format_bytes(self.locale(), 26 * 1024 * 1024))
                            .with("total", format_bytes(self.locale(), 60 * 1024 * 1024))
                            .with(
                                "elapsed",
                                teleark_i18n::format::format_duration_millis(self.locale(), 28_000),
                            ),
                    ),
                );
                row.size = format_bytes(self.locale(), 60 * 1024 * 1024).into();
                row.transferred = format_bytes(self.locale(), 26 * 1024 * 1024).into();
                row.speed = self.tr("transfer-value-unavailable");
                if state == "session-active" {
                    self.vault_activity = VaultActivity::Idle;
                    row.vault_transfer_id = Some(700);
                    row.vault_batch_id = None;
                    row.runtime_task_id = None;
                    row.runtime_batch_id = None;
                    row.caption = Some("synthetic encrypted package".into());
                    self.show_transfer_detail = true;
                }
                self.preview_transfer_rows = vec![row];
            }
            "upload-preflight" => {
                self.page = Page::Transfers;
                self.preview_transfer_rows.clear();
                self.upload_in_flight = true;
                self.vault_activity = VaultActivity::Working;
            }
            "setup" => {
                self.page = Page::Storage;
                self.storage_status = teleark_runtime::StorageChannelStatus::Missing;
                self.storage_notice = None;
                self.storage_loading = true;
            }
            "raw" => {
                self.page = Page::Storage;
                self.storage_view = StorageView::RawFiles;
            }
            "locked" => self.page = Page::Storage,
            "unlock" => {
                self.page = Page::Storage;
                self.unlock_intent = Some(UnlockIntent::Upload);
            }
            "proxy-ready" | "proxy-failed" | "proxy-testing" => {
                use teleark_runtime::{NetworkPhase, NetworkSnapshot, ProxyFailure};
                self.page = Page::Settings;
                self.settings_section = SettingsSection::Network;
                self.proxy.enabled = true;
                let now = std::time::Instant::now();
                let phase = match state.as_str() {
                    "proxy-failed" => NetworkPhase::Blocked(ProxyFailure::Unreachable),
                    "proxy-testing" => NetworkPhase::Testing,
                    _ => NetworkPhase::ProxyReady,
                };
                self.proxy.snapshot = Some(NetworkSnapshot {
                    revision: 1,
                    generation: 1,
                    proxy_enabled: true,
                    phase,
                    phase_started: now,
                    last_activity: now,
                    events: Default::default(),
                    omitted_events: 0,
                });
                if state == "proxy-testing" {
                    self.proxy.action = proxy::ProxyAction::Testing;
                }
            }
            "about" => {
                self.page = Page::Settings;
                self.settings_section = SettingsSection::About;
            }
            "appearance" => {
                self.page = Page::Settings;
                self.settings_section = SettingsSection::Appearance;
            }
            _ => {}
        }
    }
}
