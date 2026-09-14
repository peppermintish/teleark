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
        let appearance = if std::env::args().any(|arg| arg == "--preview-dark") {
            AppearancePreference::Dark
        } else {
            AppearancePreference::Light
        };
        self.preferences.appearance = appearance;
        theme::apply_appearance(appearance, window, cx);
        self.volume_space = Some(teleark_runtime::VolumeSpace {
            available_bytes: 248 * 1024 * 1024 * 1024,
            total_bytes: 1024 * 1024 * 1024 * 1024,
        });
        self.telegram_activity = TelegramActivity::Idle;
        self.vault_activity = VaultActivity::Idle;
        self.preference_persistence = PreferencePersistence::Idle;
        self.locale_persistence = LocalePersistence::Idle;
        self.configured_telegram_api_id = Some(12345);
        if matches!(
            state.as_str(),
            "login" | "login-refreshing" | "login-refreshed"
        ) {
            self.telegram_auth = TelegramAuthState::Unauthorized;
            self.page = Page::Account;
            if state != "login" {
                self.qr_login_error = Some(teleark_core::ApplicationErrorKind::Authorization);
                if state == "login-refreshing" {
                    self.telegram_activity = TelegramActivity::Working;
                } else {
                    self.telegram_auth = TelegramAuthState::QrCode {
                        deep_link: "TeleArk refreshed UI preview - not a login token".into(),
                        expires_at_unix_seconds: 1_900_000_000,
                    };
                }
            }
            return;
        }
        let account = TelegramAccount {
            id: 1,
            display_name: "Alex Chen".into(),
            username: None,
        };
        self.telegram_account = Some(account.clone());
        self.telegram_auth = TelegramAuthState::Authorized(account);
        if state.starts_with("session-loss-") {
            use session_loss::{Phase, SessionLoss};
            self.page = Page::Account;
            let phase = match state.as_str() {
                "session-loss-retiring" => Phase::Retiring,
                "session-loss-failed" => Phase::Failed,
                "session-loss-paused" => Phase::SignedOut,
                _ => Phase::Pausing,
            };
            self.session_loss = Some(SessionLoss::new(
                teleark_runtime::AuthorizationSnapshot {
                    generation: 1,
                    network_generation: 0,
                    account_id: Some(1),
                    phase: teleark_runtime::AuthorizationPhase::Revoked,
                },
                phase,
            ));
            if phase == Phase::SignedOut {
                self.telegram_account = None;
                self.telegram_auth = TelegramAuthState::QrCode {
                    deep_link: "TeleArk session recovery preview - not a login token".into(),
                    expires_at_unix_seconds: 1_900_000_000,
                };
            }
            return;
        }
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
        self.app_lock.locked = matches!(state.as_str(), "locked" | "locked-transfers");
        self.vault_status.locked =
            matches!(state.as_str(), "managed-key-loading" | "managed-key-error");
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
            if state.ends_with("locked") {
                self.vault_locked = true;
                self.vault_status.locked = true;
                self.vault_status.active_key_locked = true;
                // Reproduce the stale successful discovery notice from real startup.
                self.storage_notice = Some("storage-auto-found");
            }
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
            if state.ends_with("waiting") {
                self.storage_loading = true;
                self.storage_maintenance = Some(teleark_runtime::StorageMaintenance::new());
            }
        }
        if state == "storage-completed-locked" {
            self.vault_locked = true;
            self.vault_status.locked = true;
            self.vault_status.active_key_locked = true;
            self.storage_notice = Some("storage-repair-completed");
            self.storage_maintenance = Some(teleark_runtime::StorageMaintenance::new());
            self.storage_maintenance_preview = Some(completed_storage_maintenance_preview());
        }
        if state.starts_with("managed-key-") {
            self.page = Page::Settings;
            self.settings_section = SettingsSection::KeyVault;
            self.vault_advanced_expanded = true;
            self.vault_activity = match state.as_str() {
                "managed-key-loading" => VaultActivity::Working,
                "managed-key-error" => {
                    VaultActivity::Failed(teleark_core::ApplicationErrorKind::PermissionDenied)
                }
                _ => VaultActivity::Idle,
            };
            if state == "managed-key-loading" {
                self.vault_key_progress = Some(teleark_runtime::VaultKeyProgress::new());
            }
        }
        if state == "new-key" {
            self.vault_new_epoch_confirmation = true;
            self.pending_vault_action = Some(VaultAction::Upload);
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
            let synced = matches!(
                state.as_str(),
                "channel-sync-synced" | "channel-sync-synced-locked" | "channel-sync-private"
            );
            self.app_lock.locked = state == "channel-sync-synced-locked";
            let failed = state == "channel-sync-failed";
            let phase = if waiting {
                ChannelSyncPhase::RateLimited
            } else if synced {
                ChannelSyncPhase::Idle
            } else if failed {
                ChannelSyncPhase::Failed
            } else {
                ChannelSyncPhase::Receiving
            };
            if synced {
                self.telegram_files.truncate(7);
                self.vault_locked = true;
                self.vault_status.locked = true;
                self.vault_status.active_key_locked = true;
            }
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
                last_completed_at: Some(now - Duration::from_secs(if synced { 12 } else { 300 })),
                retry_at: waiting.then_some(now + Duration::from_secs(30)),
                failure: failed.then_some(teleark_core::ApplicationErrorKind::Network),
                queued: if synced { 0 } else { 3 },
                failed_channels: usize::from(failed),
                committed_pages: 4,
                data_revision: 4,
                active: Vec::new(),
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
                managed_watch: (!synced).then_some(teleark_runtime::ManagedChannelWatch {
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
                managed_review_pending: !waiting && !synced && !failed,
                managed_scan: None,
            });
            if state == "channel-sync-private" {
                use teleark_runtime::{ManagedChannelChange, ManagedChannelChangeKind};
                let changes: Vec<_> = [
                    (ManagedChannelChangeKind::Edited, 41),
                    (ManagedChannelChangeKind::Deleted, 42),
                    (ManagedChannelChangeKind::Gap, 0),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, (kind, message_id))| ManagedChannelChange {
                    sequence: index as u64 + 1,
                    message_id,
                    kind,
                    observed_at_unix_ms: self
                        .sync_time_anchor
                        .unix_millis(now - Duration::from_secs(9 - index as u64 * 3)),
                })
                .collect();
                if let Some(snapshot) = &mut self.channel_sync_snapshot {
                    snapshot.managed_watch = Some(teleark_runtime::ManagedChannelWatch {
                        catalog_ready: true,
                        change_count: 3,
                        acknowledged_count: 0,
                        last_changed_at_unix_ms: changes.last().map(|c| c.observed_at_unix_ms),
                        changes,
                    });
                }
            }
            if state == "channel-sync-vault-retry" {
                self.page = Page::Storage;
                self.storage_view = StorageView::Files;
                self.vault_locked = false;
                self.vault_activity = VaultActivity::Succeeded;
                self.managed_scan_loading = true;
                self.managed_scan_cancellation = Some(TelegramScanCancellation::new());
                self.channel_sync_details = true;
                if let Some(snapshot) = &mut self.channel_sync_snapshot {
                    snapshot.managed_scan = Some(teleark_runtime::ManagedScanStatus {
                        chat_id: 9000,
                        phase: ChannelSyncPhase::Waiting,
                        phase_started: now,
                        last_activity: now,
                        completed: 1,
                        total: Some(3),
                        cached: 1,
                        rejected: 0,
                        failure: Some(teleark_core::ApplicationErrorKind::Network),
                        retry_after: Some(Duration::from_secs(60)),
                    });
                }
            }
        }
        // Synthetic rows represent an accepted local projection, just as a
        // completed cache read does, so navigation can retain the fixture.
        self.channel_loaded_scope = self
            .telegram_account
            .as_ref()
            .zip(self.selected_chat_id)
            .map(|(account, chat)| (account.id, chat));
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
        for row in &mut rows {
            if !matches!(
                row.state,
                crate::mock::TransferState::Uploading | crate::mock::TransferState::Downloading
            ) {
                row.eta = self.tr("transfer-value-unavailable");
            }
        }
        for index in 0..48 {
            let mut row = fixture[index % fixture.len()].clone();
            row.name = format!("{:02} {}", index + 1, row.name).into();
            rows.push(row);
        }
        self.preview_transfer_rows = rows;
        if state == "upload-pipeline" {
            self.preview_upload_pipeline();
            return;
        }
        if state == "upload-history" {
            self.preview_upload_history();
        }
        if state == "batch-groups" {
            self.preview_batch_groups();
        }
        if state == "batch-large" {
            let group_index = self
                .preview_transfer_rows
                .iter()
                .position(|row| row.vault_batch_id == Some(17) && !row.batch_child)
                .expect("preview batch");
            let name = self.tr_with(
                "transfer-batch-upload-name",
                MessageArgs::new().with("count", "12"),
            );
            let group = &mut self.preview_transfer_rows[group_index];
            group.name = name;
            group.batch_summary.as_mut().expect("preview summary").total = 12;
            let template = self.preview_transfer_rows[group_index + 6].clone();
            let extra = (0..6).map(|index| {
                let mut row = template.clone();
                row.vault_transfer_id = Some(1000 + index);
                row.name = format!("Additional recording {}.wav", index + 1).into();
                row
            });
            self.preview_transfer_rows
                .splice(group_index + 7..group_index + 7, extra);
        }
        match state.as_str() {
            "quit-confirm" | "quit-pausing" | "quit-failed" => {
                self.app_lock.locked = true;
                self.upload_in_flight = true;
                self.transition = Some(super::lifecycle::Transition {
                    action: super::lifecycle::TransitionAction::Quit,
                    phase: match state.as_str() {
                        "quit-pausing" => super::lifecycle::TransitionPhase::Pausing,
                        "quit-failed" => super::lifecycle::TransitionPhase::Failed,
                        _ => super::lifecycle::TransitionPhase::Confirm,
                    },
                    started: std::time::Instant::now(),
                });
            }
            "recovery-guidance" => self.preview_recovery_failure(),
            "native-cleanup" => self.preview_native_cleanup(false),
            "native-cleanup-failed" => self.preview_native_cleanup(true),
            "recovery-guide" => {
                self.page = Page::Storage;
                self.show_storage_guide = true;
            }
            "speed-limits" => self.preview_speed_limits(window, cx),
            "returning" => self.page = Page::Account,
            "upload-progress" | "session-active" | "locked-transfers" => {
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
                        "transfer-upload-activity-container-bytes",
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
            "upload-folder" => {
                self.page = Page::Storage;
                self.show_upload = true;
                self.vault_activity = VaultActivity::Failed(
                    teleark_core::ApplicationErrorKind::UploadFolderUnsupported,
                );
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
            "channel-selected" => {
                self.page = Page::Channel;
                self.channel_file_table.update(cx, |table, cx| {
                    table.set_selected_row(1, cx);
                });
                self.selected_channel_message_ids.insert(4999);
            }
            "locked" => self.page = Page::Storage,
            "unlock" => {
                self.page = Page::Storage;
                self.show_upload = true;
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

    pub(crate) fn preview_batch_groups(&mut self) {
        // Short adjacent groups expose both ends alongside ordinary tasks.
        self.preview_transfer_rows.retain(|row| {
            !row.batch_child
                || row.runtime_task_id.is_some_and(|id| id < 202)
                || row.vault_transfer_id.is_some_and(|id| id < 302)
        });
        let upload_name = self.tr_with(
            "transfer-batch-upload-name",
            MessageArgs::new().with("count", "2"),
        );
        let download_name = self.tr_with(
            "transfer-batch-name",
            MessageArgs::new()
                .with("count", "2")
                .with("source", "Kyoto · September"),
        );
        let unavailable = self.tr("transfer-value-unavailable");
        for row in &mut self.preview_transfer_rows {
            if let Some(summary) = row.batch_summary.as_mut() {
                summary.total = 2;
                summary.completed = 2;
                summary.file_names.truncate(2);
                row.name = if row.direction == crate::mock::TransferDirection::Upload {
                    upload_name.clone()
                } else {
                    download_name.clone()
                };
                row.size = format_bytes(self.localizer.locale(), 3 * 123 * 1024 * 1024).into();
                row.transferred = row.size.clone();
                row.progress = 100.0;
                row.state = crate::mock::TransferState::Completed;
                row.eta = unavailable.clone();
            }
        }
        self.preview_transfer_rows
            .insert(0, crate::mock::transfers(false)[0].clone());
        self.expanded_transfer_batches.insert(42);
        self.expanded_transfer_batches
            .insert(0x6000_0000_0000_0000 | 17);
        self.page = Page::Transfers;
    }
}

/// A completed repair timeline for isolated native previews and geometry regressions.
pub(crate) fn completed_storage_maintenance_preview() -> teleark_runtime::StorageMaintenanceSnapshot
{
    use teleark_runtime::{StorageMaintenance, StorageMaintenancePhase as Phase};
    let mut snapshot = StorageMaintenance::new().snapshot();
    snapshot.phase = Phase::Completed;
    snapshot.finished = true;
    snapshot.timeline = [
        Phase::Checking,
        Phase::FindingRecord,
        Phase::Pinning,
        Phase::Updating,
        Phase::Muting,
        Phase::Archiving,
        Phase::Verifying,
        Phase::Completed,
    ]
    .into_iter()
    .enumerate()
    .map(|(index, phase)| (phase, index as u64 * 1000))
    .collect();
    snapshot
}
