use gpui_kit::component::{
    Disableable as _, Icon, IconName,
    button::ButtonVariants as _,
    checkbox::Checkbox,
    tab::{Tab, TabBar},
};
use gpui_kit::{
    AnyElement, AppContext as _, Context, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use teleark_core::ApplicationErrorKind;
use teleark_i18n::{
    MessageArgs,
    format::{
        format_bytes, format_decimal, format_duration_millis, format_integer, format_percent,
        format_speed, format_unix_millis,
    },
};
use teleark_runtime::{
    ChannelDownloadCleanup, ChannelDownloadCleanupPhase, ChannelDownloadEventKind,
    ChannelDownloadSnapshot, ChannelDownloadState, ChannelDownloadVerification, ControllerDecision,
    ControllerDecisionOutcome, ControllerDecisionReason, ControllerPhase, DownloadPartState,
    TransferBottleneck, TransferControlParameters, TransferTelemetrySnapshot, TunableParameter,
    VaultTransferDirection, VaultTransferSnapshot, VaultTransferState, VaultUploadActivity,
    VaultUploadPhase,
};

use crate::{
    app::TeleArkApp,
    components::{self, Tone},
    layout::LayoutPolicy,
    mock::{BatchSummary, TransferDirection, TransferRow, TransferState},
    theme,
};

mod batch_style;
mod batch_window;
mod projection;
mod upload_activity;
#[cfg(test)]
use crate::mock::transfers;
use batch_style::BatchRowPosition;
pub(crate) use projection::TransferProjectionCache;
use projection::{TransferItem, vault_action_supported, vault_transfer_state};

// The view owns one command batch. Dropping the owner stops at the next task
// boundary, without interrupting an atomic runtime operation already in progress.
pub(crate) struct TransferActionJob {
    _task: gpui_kit::Task<()>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for TransferActionJob {
    fn drop(&mut self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

/// Aggregate only the published receipt window; snapshot/historical speeds may be stale.
fn batch_receipt_rate<'a>(
    rates: impl Iterator<Item = &'a teleark_runtime::TransferRate>,
    remaining: u64,
) -> (Option<u64>, Option<u64>) {
    let mut speed = None::<u64>;
    let mut logical = 0_u64;
    for rate in rates {
        if let Some(bytes) = rate.bytes_per_second {
            speed = Some(speed.unwrap_or_default().saturating_add(bytes));
        }
        logical = logical.saturating_add(rate.logical_bytes_per_second.unwrap_or_default());
    }
    let eta = (logical > 0 && remaining > 0).then(|| {
        (u128::from(remaining) * 1000 / u128::from(logical)).min(u128::from(u64::MAX)) as u64
    });
    (speed, eta)
}

impl TeleArkApp {
    pub(crate) fn preview_upload_history(&mut self) {
        let telemetry = TransferTelemetrySnapshot {
            phase: ControllerPhase::Ramp,
            parameters: TransferControlParameters::conservative_upload(),
            goodput_bytes_per_second: 0,
            encryption_bytes_per_second: 0,
            disk_bytes_per_second: 0,
            round_trip_time_p95_millis: 0,
            estimated_bdp_bytes: 0,
            inflight_bytes: 0,
            target_inflight_bytes: 0,
            cpu_utilization_basis_points: 0,
            encrypted_queue_length: 0,
            network_waiting_for_encryption_millis: 0,
            encryption_waiting_for_network_millis: 0,
            bottleneck: TransferBottleneck::Unknown,
            parts: Default::default(),
            queues: Default::default(),
            memory: Default::default(),
            memory_budget_bytes: 0,
            lanes: vec![],
            decisions: vec![],
        };
        let account_id = self
            .telegram_account
            .as_ref()
            .map_or(7, |account| account.id);
        let rows = [
            VaultTransferState::Completed,
            VaultTransferState::Interrupted,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, state)| {
            let complete = state == VaultTransferState::Completed;
            std::sync::Arc::new(VaultTransferSnapshot {
                recovery_state: None,
                restored: true,
                upload_activity: None,
                id: 801 + index as u64,
                account_id,
                chat_id: 90,
                batch_id: Some(80),
                queued_at_unix_ms: 1_788_950_000_000,
                direction: VaultTransferDirection::Upload,
                file_name: if complete {
                    "Project archive.zip".into()
                } else {
                    "Field recording.wav".into()
                },
                package_id: complete.then(|| "0102030405060708".into()),
                size_bytes: 128 * 1024 * 1024,
                transferred_bytes: if complete {
                    128 * 1024 * 1024
                } else {
                    60 * 1024 * 1024
                },
                completed_parts: if complete { 3 } else { 1 },
                part_count: 3,
                started_at_unix_ms: 1_788_950_000_000,
                duration_ms: complete.then_some(8_000),
                average_bytes_per_second: complete.then_some(16 * 1024 * 1024),
                destination: None,
                session_log_path: None,
                telemetry: telemetry.clone(),
                state,
            })
        })
        .collect::<Vec<_>>();
        self.vault_locked = false;
        self.vault_transfer_view = teleark_runtime::TransferSnapshotView {
            revision: self.vault_transfer_view.revision + 1,
            items: rows.clone().into(),
            omitted_items: 12,
            ..Default::default()
        };
        let members = rows.iter().map(std::sync::Arc::as_ref).collect::<Vec<_>>();
        let mut presented = vec![
            self.transfer_row_from_vault_batch(80, &members)
                .expect("synthetic batch"),
        ];
        for row in &rows {
            let mut child = self.transfer_row_from_vault_snapshot(row);
            child.batch_child = true;
            presented.push(child);
        }
        self.focused_transfer_key = Some(transfer_selection_key(&presented[2], 2));
        self.show_transfer_detail = true;
        self.preview_transfer_rows = presented;
        self.expanded_transfer_batches.insert(vault_batch_key(80));
        self.page = crate::app::Page::Transfers;
        self.nav_selection = "nav-uploads";
    }

    pub(crate) fn preview_upload_pipeline(&mut self) {
        self.preview_upload_history();
        let mut snapshot = (*self.vault_transfer_view.items[1]).clone();
        snapshot.state = VaultTransferState::Running;
        snapshot.restored = false;
        // Synthetic maximum-container geometry for grouped-map layout review.
        snapshot.size_bytes = 5 * 1024 * 1024 * 1024;
        snapshot.transferred_bytes = 2_039_984_825;
        snapshot.completed_parts = 1;
        snapshot.part_count = 3;
        snapshot.telemetry.parameters = TransferControlParameters {
            transfer_connection_count: 2,
            inflight_rpcs_per_connection: 15,
            active_file_count: 3,
            inflight_parts_per_file: 10,
            encryption_worker_count: 1,
            encrypted_part_queue_depth: 2,
        };
        let mut activity = VaultUploadActivity::new(VaultUploadPhase::Uploading);
        activity.started = std::time::Instant::now() - std::time::Duration::from_secs(8);
        activity.last_activity = std::time::Instant::now();
        activity.queued = 2;
        activity.active = 10;
        activity.bytes = 20 * 512 * 1024;
        activity.total = 2_040_109_465;
        activity.uploaded_bytes = snapshot.transferred_bytes + 10 * 1024 * 1024;
        activity.parts = (0..3892)
            .map(|index| teleark_runtime::VaultUploadPart {
                state: if index < 20 {
                    teleark_runtime::VaultUploadPartState::Acknowledged
                } else if index < 30 {
                    teleark_runtime::VaultUploadPartState::Uploading
                } else {
                    teleark_runtime::VaultUploadPartState::Queued
                },
                attempt: 1,
            })
            .collect();
        activity.events = (0..40)
            .map(|index| teleark_runtime::VaultUploadEvent {
                at: activity.started + std::time::Duration::from_millis(index * 200),
                phase: VaultUploadPhase::Uploading,
                part: Some(index as u32 / 2),
                acknowledged: index % 2 == 1,
                attempt: 1,
                wait_millis: 0,
            })
            .collect();
        activity.samples = (1..=8)
            .map(|index| teleark_runtime::VaultUploadSample {
                elapsed_millis: index * 1000,
                interval_millis: 1000,
                bytes_per_second: (index % 3 + 1) * 512 * 1024,
            })
            .collect();
        snapshot.upload_activity = Some(activity);
        let row = self.transfer_row_from_vault_snapshot(&snapshot);
        self.focused_transfer_key = Some(transfer_selection_key(&row, 0));
        self.preview_transfer_rows = vec![row];
        self.vault_transfer_view.items = vec![std::sync::Arc::new(snapshot)].into();
        self.vault_transfer_view.omitted_items = 0;
    }

    pub(crate) fn preview_recovery_failure(&mut self) {
        self.preview_upload_history();
        let mut snapshot = (*self.vault_transfer_view.items[1]).clone();
        snapshot.batch_id = None;
        snapshot.state =
            VaultTransferState::Failed(teleark_core::ApplicationErrorKind::SourceChanged);
        snapshot.recovery_state = Some(teleark_runtime::VaultRecoveryState::Blocked);
        let row = self.transfer_row_from_vault_snapshot(&snapshot);
        self.focused_transfer_key = Some(transfer_selection_key(&row, 0));
        self.preview_transfer_rows = vec![row];
        self.vault_transfer_view.items = vec![std::sync::Arc::new(snapshot)].into();
        self.vault_transfer_view.omitted_items = 0;
    }

    pub(crate) fn preview_native_cleanup(&mut self, failed: bool) {
        self.preview_upload_history();
        let template = &self.vault_transfer_view.items[0];
        let now = current_unix_millis().unwrap_or(0);
        let snapshot = ChannelDownloadSnapshot {
            cleanup: Some(ChannelDownloadCleanup {
                phase: if failed {
                    ChannelDownloadCleanupPhase::Failed(ApplicationErrorKind::PermissionDenied)
                } else {
                    ChannelDownloadCleanupPhase::WaitingForWriter
                },
                requested_at_unix_ms: now - 4200,
                phase_since_unix_ms: now - 4200,
                last_activity_at_unix_ms: now - 1200,
                retry_requested: false,
            }),
            account_id: Some(template.account_id),
            id: 901,
            batch_id: None,
            chat_id: 9000,
            message_id: 101,
            message_sent_at_unix_ms: None,
            file_name: "Research archive.zip".into(),
            caption: None,
            mime_type: None,
            size_bytes: 256 * 1024 * 1024,
            transferred_bytes: 0,
            destination: "/Preview/Downloads/Research archive.zip".into(),
            state: ChannelDownloadState::Cancelled,
            verification: ChannelDownloadVerification::NotReached,
            queued_at_unix_ms: now - 30000,
            started_at_unix_ms: Some(now - 29000),
            finished_at_unix_ms: Some(now - 4200),
            queue_wait_ms: Some(1000),
            duration_ms: Some(24800),
            acknowledged_bytes: 0,
            current_bytes_per_second: None,
            average_bytes_per_second: None,
            eta_ms: None,
            attempts: 1,
            failure: None,
            events: vec![],
            event_history_omitted: 0,
            part_events: Default::default(),
            session_log_path: None,
            telemetry: template.telemetry.clone(),
        };
        let row = self.transfer_row_from_snapshot(&snapshot, false);
        self.focused_transfer_key = Some(snapshot.id);
        self.preview_transfer_rows = vec![row];
        self.native_transfer_view.items = vec![std::sync::Arc::new(snapshot)].into();
        self.native_transfer_view.omitted_items = 0;
        self.vault_transfer_view.items = vec![].into();
        self.vault_transfer_view.omitted_items = 0;
        self.nav_selection = "nav-downloads";
    }

    fn transfer_items(&self) -> std::sync::Arc<Vec<TransferItem>> {
        if self.visual_preview {
            return std::sync::Arc::new(
                self.preview_transfer_rows
                    .iter()
                    .cloned()
                    .map(|row| TransferItem::Preview(std::sync::Arc::new(row)))
                    .collect(),
            );
        }
        let account = self.telegram_account.as_ref().map(|account| account.id);
        if let Some(items) = self.transfer_projection_cache.borrow().get(self, account) {
            return items;
        }
        let snapshots: Vec<_> = self
            .native_transfer_view
            .items
            .iter()
            .filter(|row| row.account_id.is_none() || row.account_id == account)
            .cloned()
            .collect();
        let mut native_batches =
            std::collections::BTreeMap::<u64, Vec<std::sync::Arc<ChannelDownloadSnapshot>>>::new();
        for row in &snapshots {
            if let Some(batch) = row.batch_id {
                native_batches.entry(batch).or_default().push(row.clone());
            }
        }
        let mut native = Vec::new();
        for row in snapshots.iter().rev() {
            if let Some(batch) = row.batch_id {
                if let Some(mut members) = native_batches.remove(&batch) {
                    members.sort_by_key(|row| row.id);
                    if members.len() == 1 {
                        native.push(TransferItem::Native(members[0].clone(), false));
                        continue;
                    }
                    native.push(TransferItem::NativeBatch(batch, members.clone().into()));
                    native.extend(
                        members
                            .into_iter()
                            .rev()
                            .map(|row| TransferItem::Native(row, true)),
                    );
                }
            } else {
                native.push(TransferItem::Native(row.clone(), false));
            }
        }
        let snapshots: Vec<_> = self
            .vault_transfer_view
            .items
            .iter()
            .filter(|row| Some(row.account_id) == account)
            .cloned()
            .collect();
        let mut batches =
            std::collections::BTreeMap::<u64, Vec<std::sync::Arc<VaultTransferSnapshot>>>::new();
        for row in &snapshots {
            if let Some(batch) = row.batch_id {
                batches.entry(batch).or_default().push(row.clone());
            }
        }
        let mut items = Vec::new();
        for row in snapshots.iter().rev() {
            if let Some(batch) = row.batch_id {
                if let Some(members) = batches.remove(&batch) {
                    if members.len() == 1 {
                        items.push(TransferItem::Vault(members[0].clone(), false));
                        continue;
                    }
                    items.push(TransferItem::VaultBatch(
                        batch,
                        row.clone(),
                        members.clone().into(),
                    ));
                    items.extend(
                        members
                            .into_iter()
                            .map(|row| TransferItem::Vault(row, true)),
                    );
                }
            } else {
                items.push(TransferItem::Vault(row.clone(), false));
            }
        }
        items.extend(native);
        let items = std::sync::Arc::new(items);
        self.transfer_projection_cache
            .borrow_mut()
            .set(self, account, items.clone());
        items
    }

    #[cfg(test)]
    pub(crate) fn transfer_rows(&self) -> Vec<TransferRow> {
        self.transfer_items()
            .iter()
            .map(|item| item.row(self))
            .collect()
    }

    fn transfer_row_from_vault_snapshot(&self, snapshot: &VaultTransferSnapshot) -> TransferRow {
        let snapshot = self
            .vault_transfer_view
            .updates
            .get(&snapshot.id)
            .map_or(snapshot, std::sync::Arc::as_ref);
        let state = vault_transfer_state(snapshot);
        let direction = match snapshot.direction {
            VaultTransferDirection::Upload => TransferDirection::Upload,
            VaultTransferDirection::Download => TransferDirection::Download,
        };
        let destination = match snapshot.direction {
            VaultTransferDirection::Upload => self.tr("transfer-vault-storage-channel"),
            VaultTransferDirection::Download => snapshot
                .destination
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned().into())
                .unwrap_or_else(|| self.tr("transfer-value-unavailable")),
        };
        let activity = snapshot.upload_activity.as_ref().filter(|_| {
            matches!(
                snapshot.state,
                VaultTransferState::Queued | VaultTransferState::Running
            )
        });
        let transferred = vault_display_bytes(snapshot);
        let unverified_saved = snapshot.restored
            && snapshot.direction == VaultTransferDirection::Download
            && state != TransferState::Completed;
        let unavailable = unverified_saved && snapshot.file_name.is_empty();
        TransferRow {
            activity: if unavailable {
                Some(self.tr("transfer-recovery-unavailable"))
            } else if unverified_saved {
                Some(self.tr(state.message_id()))
            } else {
                match snapshot.state {
                    VaultTransferState::Interrupted => Some(self.tr("transfer-upload-interrupted")),
                    VaultTransferState::Pausing => Some(self.tr("transfer-upload-pausing")),
                    VaultTransferState::Cancelling => Some(self.tr("transfer-upload-cancelling")),
                    _ => activity.map(|activity| self.tr(upload_phase_message_id(activity.phase))),
                }
            },
            activity_detail: if unavailable {
                Some(self.tr("transfer-recovery-unavailable-detail"))
            } else if unverified_saved {
                Some(self.tr("transfer-recovery-verification-pending"))
            } else {
                self.receipt_activity_detail(
                    self.vault_transfer_view.rates.get(&snapshot.id),
                    activity.map(|activity| self.upload_activity_detail(activity)),
                )
            },
            runtime_task_id: None,
            vault_transfer_id: Some(snapshot.id),
            vault_batch_id: snapshot.batch_id,
            runtime_batch_id: None,
            batch_child: false,
            batch_summary: None,
            message_id: None,
            message_sent_at_unix_ms: None,
            caption: snapshot.package_id.clone().map(Into::into),
            mime_type: Some(self.tr("transfer-vault-encrypted-type")),
            name: if unavailable {
                self.tr("transfer-recovery-saved-download")
            } else {
                snapshot.file_name.clone().into()
            },
            source: self.tr("storage-channel-title"),
            direction,
            size: if unavailable {
                self.tr("transfer-value-unavailable")
            } else {
                format_bytes(self.locale(), snapshot.size_bytes).into()
            },
            transferred: if unverified_saved {
                self.tr("transfer-value-unavailable")
            } else {
                format_bytes(self.locale(), transferred).into()
            },
            progress: transfer_progress(
                transferred,
                snapshot.size_bytes,
                state == TransferState::Completed,
            ),
            speed: self
                .vault_transfer_view
                .rates
                .get(&snapshot.id)
                .and_then(|rate| rate.bytes_per_second)
                .or_else(|| {
                    (state == TransferState::Completed)
                        .then_some(snapshot.average_bytes_per_second)
                        .flatten()
                })
                .map(|speed| format_speed(self.locale(), speed).into())
                .unwrap_or_else(|| {
                    self.tr(
                        if state == TransferState::Uploading || state == TransferState::Downloading
                        {
                            "transfer-rate-sampling"
                        } else {
                            "transfer-value-unavailable"
                        },
                    )
                }),
            eta: self
                .vault_transfer_view
                .rates
                .get(&snapshot.id)
                .and_then(|rate| rate.eta_millis)
                .map(|eta| format_duration_millis(self.locale(), eta).into())
                .unwrap_or_else(|| self.tr("transfer-value-unavailable")),
            connections: if snapshot.restored {
                self.tr("transfer-value-unavailable")
            } else {
                self.tr_with(
                    "transfer-controller-connections-value",
                    MessageArgs::new()
                        .with(
                            "connections",
                            format_integer(
                                self.locale(),
                                u64::from(snapshot.telemetry.parameters.transfer_connection_count),
                            ),
                        )
                        .with(
                            "rpcs",
                            format_integer(
                                self.locale(),
                                u64::from(
                                    snapshot.telemetry.parameters.inflight_rpcs_per_connection,
                                ),
                            ),
                        ),
                )
            },
            state,
            destination,
        }
    }

    fn receipt_activity_detail(
        &self,
        rate: Option<&teleark_runtime::TransferRate>,
        phase: Option<SharedString>,
    ) -> Option<SharedString> {
        let Some(rate) = rate.filter(|rate| rate.awaiting_acknowledgement) else {
            return phase;
        };
        let waiting = rate.last_acknowledgement.map_or_else(
            || self.tr("transfer-rate-awaiting-first"),
            |at| {
                self.tr_with(
                    "transfer-rate-awaiting",
                    MessageArgs::new().with(
                        "elapsed",
                        format_duration_millis(
                            self.locale(),
                            at.elapsed().as_millis().min(u64::MAX as u128) as u64,
                        ),
                    ),
                )
            },
        );
        Some(phase.map_or_else(
            || waiting.clone(),
            |phase| format!("{phase} · {waiting}").into(),
        ))
    }

    fn upload_activity_detail(&self, activity: &VaultUploadActivity) -> SharedString {
        let args = MessageArgs::new()
            .with(
                "phase",
                self.tr(upload_phase_message_id(activity.phase)).to_string(),
            )
            .with(
                "elapsed",
                format_duration_millis(
                    self.locale(),
                    u64::try_from(activity.since.elapsed().as_millis()).unwrap_or(u64::MAX),
                ),
            );
        let detail = if activity.total > 0 {
            self.tr_with(
                if activity.phase == VaultUploadPhase::Uploading {
                    "transfer-upload-activity-container-bytes"
                } else {
                    "transfer-upload-activity-bytes"
                },
                args.with("done", format_bytes(self.locale(), activity.bytes))
                    .with("total", format_bytes(self.locale(), activity.total)),
            )
        } else {
            self.tr_with("transfer-upload-activity-elapsed", args)
        };
        if activity.persistence_since.is_some() {
            self.tr_with(
                "transfer-persistence-parallel",
                MessageArgs::new().with("activity", detail.to_string()),
            )
        } else {
            detail
        }
    }

    fn transfer_row_from_vault_batch(
        &self,
        batch_id: u64,
        items: &[&VaultTransferSnapshot],
    ) -> Option<TransferRow> {
        let latest: Vec<_> = items
            .iter()
            .map(|snapshot| {
                self.vault_transfer_view
                    .updates
                    .get(&snapshot.id)
                    .map_or(*snapshot, std::sync::Arc::as_ref)
            })
            .collect();
        let items = latest.as_slice();
        let mut row = self.transfer_row_from_vault_snapshot(items.first()?);
        row.vault_transfer_id = None;
        row.vault_batch_id = Some(batch_id);
        row.batch_summary = Some(BatchSummary {
            file_names: items
                .iter()
                .take(3)
                .map(|item| item.file_name.clone().into())
                .collect(),
            total: items.len(),
            completed: items
                .iter()
                .filter(|item| item.state == VaultTransferState::Completed)
                .count(),
            failed: items
                .iter()
                .filter(|item| {
                    matches!(
                        item.state,
                        VaultTransferState::Failed(_) | VaultTransferState::Interrupted
                    )
                })
                .count(),
            queued_at_unix_ms: items
                .iter()
                .map(|item| item.queued_at_unix_ms)
                .min()
                .unwrap_or_default(),
        });
        let total = items
            .iter()
            .fold(0_u64, |sum, item| sum.saturating_add(item.size_bytes));
        let transferred = items.iter().fold(0_u64, |sum, item| {
            sum.saturating_add(vault_display_bytes(item))
        });
        let active = items
            .iter()
            .find(|item| item.state == VaultTransferState::Running)
            .or_else(|| {
                items.iter().find(|item| {
                    item.state == VaultTransferState::Queued && item.upload_activity.is_some()
                })
            });
        let active_row = active.map(|item| self.transfer_row_from_vault_snapshot(item));
        row.activity = active_row.as_ref().and_then(|row| row.activity.clone());
        row.activity_detail = active_row
            .as_ref()
            .and_then(|row| row.activity_detail.clone());
        let (speed, eta) = batch_receipt_rate(
            items
                .iter()
                .filter_map(|item| self.vault_transfer_view.rates.get(&item.id)),
            total.saturating_sub(transferred),
        );
        row.speed = speed
            .map(|speed| format_speed(self.locale(), speed).into())
            .unwrap_or_else(|| {
                self.tr(if active.is_some() {
                    "transfer-rate-sampling"
                } else {
                    "transfer-value-unavailable"
                })
            });
        row.eta = eta
            .map(|eta| format_duration_millis(self.locale(), eta).into())
            .unwrap_or_else(|| self.tr("transfer-value-unavailable"));
        row.state = aggregate_transfer_states(
            &items
                .iter()
                .map(|item| vault_transfer_state(item))
                .collect::<Vec<_>>(),
        );
        row.name = self.tr_with(
            "transfer-batch-upload-name",
            MessageArgs::new().with("count", format_integer(self.locale(), items.len() as u64)),
        );
        row.size = format_bytes(self.locale(), total).into();
        row.transferred = format_bytes(self.locale(), transferred).into();
        row.progress = transfer_progress(transferred, total, row.state == TransferState::Completed);
        Some(row)
    }

    fn native_cleanup_detail(&self, cleanup: ChannelDownloadCleanup) -> SharedString {
        let elapsed = current_unix_millis()
            .unwrap_or(cleanup.last_activity_at_unix_ms)
            .saturating_sub(cleanup.phase_since_unix_ms)
            .max(0) as u64;
        let reason = match cleanup.phase {
            ChannelDownloadCleanupPhase::Failed(ApplicationErrorKind::InvalidRequest) => {
                self.tr("native-cleanup-unsupported")
            }
            ChannelDownloadCleanupPhase::Failed(kind) => self.tr_with(
                "native-cleanup-error-guidance",
                MessageArgs::new().with(
                    "error",
                    self.tr(native_download_error_message_id(kind)).to_string(),
                ),
            ),
            _ => self.tr(if cleanup.retry_requested {
                "native-cleanup-retry-waiting"
            } else {
                "native-cleanup-explanation"
            }),
        };
        self.tr_with(
            "native-cleanup-detail",
            MessageArgs::new()
                .with("reason", reason.to_string())
                .with("elapsed", format_duration_millis(self.locale(), elapsed))
                .with(
                    "last",
                    format_unix_millis(self.locale(), cleanup.last_activity_at_unix_ms),
                ),
        )
    }

    fn transfer_row_from_snapshot(
        &self,
        snapshot: &ChannelDownloadSnapshot,
        batch_child: bool,
    ) -> TransferRow {
        let snapshot = self
            .native_transfer_view
            .updates
            .get(&snapshot.id)
            .map_or(snapshot, std::sync::Arc::as_ref);
        let state = transfer_state(native_display_state(snapshot));
        let speed = self
            .native_transfer_view
            .rates
            .get(&snapshot.id)
            .and_then(|rate| rate.bytes_per_second)
            .or_else(|| {
                (state == TransferState::Completed)
                    .then_some(snapshot.average_bytes_per_second)
                    .flatten()
            })
            .map(|speed| format_speed(self.locale(), speed))
            .unwrap_or_else(|| {
                self.tr(if state == TransferState::Downloading {
                    "transfer-rate-sampling"
                } else {
                    "transfer-value-unavailable"
                })
                .to_string()
            });
        let progress = transfer_progress(
            snapshot.transferred_bytes,
            snapshot.size_bytes,
            state == TransferState::Completed,
        );
        let source = self.telegram_source_name(snapshot.chat_id);
        TransferRow {
            activity: snapshot
                .cleanup
                .map(|cleanup| self.tr(native_cleanup_message_id(cleanup.phase))),
            activity_detail: self.receipt_activity_detail(
                self.native_transfer_view.rates.get(&snapshot.id),
                snapshot
                    .cleanup
                    .map(|cleanup| self.native_cleanup_detail(cleanup)),
            ),
            runtime_task_id: Some(snapshot.id),
            vault_transfer_id: None,
            vault_batch_id: None,
            runtime_batch_id: snapshot.batch_id,
            batch_child,
            batch_summary: None,
            message_id: Some(snapshot.message_id),
            message_sent_at_unix_ms: snapshot.message_sent_at_unix_ms,
            caption: snapshot.caption.clone().map(Into::into),
            mime_type: snapshot.mime_type.clone().map(Into::into),
            name: snapshot.file_name.clone().into(),
            source: source.into(),
            direction: TransferDirection::Download,
            size: format_bytes(self.locale(), snapshot.size_bytes).into(),
            transferred: format_bytes(self.locale(), snapshot.transferred_bytes).into(),
            progress,
            speed: speed.into(),
            eta: self
                .native_transfer_view
                .rates
                .get(&snapshot.id)
                .and_then(|rate| rate.eta_millis)
                .map(|eta| format_duration_millis(self.locale(), eta))
                .unwrap_or_else(|| self.tr("transfer-value-unavailable").to_string())
                .into(),
            connections: self.tr_with(
                "transfer-controller-connections-value",
                MessageArgs::new()
                    .with(
                        "connections",
                        format_integer(
                            self.locale(),
                            u64::from(snapshot.telemetry.parameters.transfer_connection_count),
                        ),
                    )
                    .with(
                        "rpcs",
                        format_integer(
                            self.locale(),
                            u64::from(snapshot.telemetry.parameters.inflight_rpcs_per_connection),
                        ),
                    ),
            ),
            state,
            destination: snapshot.destination.to_string_lossy().into_owned().into(),
        }
    }

    fn transfer_row_from_batch(
        &self,
        batch_id: u64,
        items: &[&ChannelDownloadSnapshot],
    ) -> TransferRow {
        let latest: Vec<_> = items
            .iter()
            .map(|snapshot| {
                self.native_transfer_view
                    .updates
                    .get(&snapshot.id)
                    .map_or(*snapshot, std::sync::Arc::as_ref)
            })
            .collect();
        let items = latest.as_slice();
        let total_bytes = items
            .iter()
            .fold(0_u64, |total, item| total.saturating_add(item.size_bytes));
        let transferred_bytes = items.iter().fold(0_u64, |total, item| {
            total.saturating_add(item.transferred_bytes)
        });
        let (current_speed, eta_ms) = batch_receipt_rate(
            items
                .iter()
                .filter_map(|item| self.native_transfer_view.rates.get(&item.id)),
            total_bytes.saturating_sub(transferred_bytes),
        );
        let state = aggregate_batch_state(items);
        let source = items
            .first()
            .map(|item| self.telegram_source_name(item.chat_id))
            .unwrap_or_default();
        let destination = items
            .first()
            .and_then(|item| item.destination.parent())
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        TransferRow {
            activity: items
                .iter()
                .find_map(|item| item.cleanup)
                .map(|cleanup| self.tr(native_cleanup_message_id(cleanup.phase))),
            activity_detail: items
                .iter()
                .find_map(|item| item.cleanup)
                .map(|cleanup| self.native_cleanup_detail(cleanup)),
            runtime_task_id: None,
            vault_transfer_id: None,
            vault_batch_id: None,
            runtime_batch_id: Some(batch_id),
            batch_child: false,
            batch_summary: Some(BatchSummary {
                file_names: items
                    .iter()
                    .take(3)
                    .map(|item| item.file_name.clone().into())
                    .collect(),
                total: items.len(),
                completed: items
                    .iter()
                    .filter(|item| item.state == ChannelDownloadState::Completed)
                    .count(),
                failed: items
                    .iter()
                    .filter(|item| {
                        matches!(native_display_state(item), ChannelDownloadState::Failed(_))
                    })
                    .count(),
                queued_at_unix_ms: items
                    .iter()
                    .map(|item| item.queued_at_unix_ms)
                    .min()
                    .unwrap_or_default(),
            }),
            message_id: None,
            message_sent_at_unix_ms: None,
            caption: None,
            mime_type: None,
            name: self.tr_with(
                "transfer-batch-name",
                MessageArgs::new()
                    .with("count", format_integer(self.locale(), items.len() as u64))
                    .with("source", source.clone()),
            ),
            source: source.into(),
            direction: TransferDirection::Download,
            size: format_bytes(self.locale(), total_bytes).into(),
            transferred: format_bytes(self.locale(), transferred_bytes).into(),
            progress: transfer_progress(
                transferred_bytes,
                total_bytes,
                state == TransferState::Completed,
            ),
            speed: current_speed
                .map(|speed| format_speed(self.locale(), speed).into())
                .unwrap_or_else(|| {
                    self.tr(if state == TransferState::Downloading {
                        "transfer-rate-sampling"
                    } else {
                        "transfer-value-unavailable"
                    })
                }),
            eta: eta_ms
                .map(|eta| format_duration_millis(self.locale(), eta).into())
                .unwrap_or_else(|| self.tr("transfer-value-unavailable")),
            connections: self.tr("transfer-value-unavailable"),
            state,
            destination: destination.into(),
        }
    }

    fn telegram_source_name(&self, chat_id: i64) -> String {
        self.telegram_chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .map(|chat| chat.name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| {
                self.tr_with(
                    "library-source-telegram-chat",
                    MessageArgs::new().with("chat_id", chat_id.to_string()),
                )
                .to_string()
            })
    }

    fn runtime_transfer_snapshot(
        &self,
        id: u64,
    ) -> Option<std::sync::Arc<ChannelDownloadSnapshot>> {
        self.native_transfer_view
            .updates
            .get(&id)
            .cloned()
            .or_else(|| {
                self.native_transfer_view
                    .items
                    .iter()
                    .find(|snapshot| snapshot.id == id)
                    .cloned()
            })
    }

    pub(crate) fn vault_transfer_snapshot(
        &self,
        id: u64,
    ) -> Option<std::sync::Arc<VaultTransferSnapshot>> {
        self.vault_transfer_view
            .updates
            .get(&id)
            .cloned()
            .or_else(|| {
                self.vault_transfer_view
                    .items
                    .iter()
                    .find(|snapshot| snapshot.id == id)
                    .cloned()
            })
    }

    pub(crate) fn render_transfers(
        &self,
        _window: &mut Window,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let padding = layout.content_padding();
        let query = self.search_input.read(cx).value().to_lowercase();
        let all_transfer_rows = self.transfer_items();
        let presentation = self.transfer_projection_cache.borrow_mut().presentation(
            self,
            all_transfer_rows,
            &query,
        );
        let [uploading, downloading, waiting, completed, failed] = presentation.stats;
        let account = self
            .telegram_account
            .as_ref()
            .map_or(0, |account| account.id);
        let total_speed = self
            .native_transfer_view
            .account_rates
            .get(&account)
            .into_iter()
            .chain(self.vault_transfer_view.account_rates.get(&account))
            .fold(0_u64, |total, rate| {
                total
                    .saturating_add(rate.download_bytes_per_second)
                    .saturating_add(rate.upload_bytes_per_second)
            });
        let upload_activity = presentation
            .activity_row
            .as_ref()
            .and_then(|row| row.activity_detail(self));
        let transfer_rows = presentation.rows.clone();
        let selected = self
            .focused_transfer_key
            .and_then(|key| presentation.key_indices.get(&key))
            .and_then(|index| transfer_rows.get(*index))
            .map(|row| row.row(self));
        let visible_transfer_keys = presentation.keys.clone();
        let selection_count = presentation.selected_count;
        let all_visible_selected =
            !visible_transfer_keys.is_empty() && selection_count == visible_transfer_keys.len();
        let summary = div()
            .flex_none()
            .px(px(padding))
            .py_3()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .text_size(px(24.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tr("transfer-title")),
            )
            .child(self.speed_limits_button("transfer-speed-limits", cx))
            .child(div().flex_1())
            .child(
                div()
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(self.tr("transfer-summary-total-speed")),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::blue())
                    .child(if total_speed == 0 && uploading > 0 {
                        self.tr("transfer-value-unavailable").to_string()
                    } else {
                        format_speed(self.locale(), total_speed)
                    }),
            )
            .child(
                components::button(
                    "transfer-manage",
                    self.tr("transfer-manage"),
                    Some(IconName::Settings2),
                    false,
                )
                .ghost()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.transfer_controls_expanded = !this.transfer_controls_expanded;
                    cx.notify();
                })),
            )
            .child(
                components::button(
                    "transfer-upload",
                    self.tr("storage-channel-upload-action"),
                    Some(IconName::Plus),
                    true,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.page = crate::app::Page::Storage;
                    if this.storage_channel_id().is_some() {
                        this.open_vault_action(crate::app::VaultAction::Upload, cx);
                    } else {
                        this.select_storage(crate::app::StorageView::Files, cx);
                    }
                })),
            );
        let facets = [
            ("nav-transfers-all", "nav-all-transfers"),
            ("nav-uploads", "transfer-uploads"),
            ("nav-downloads", "transfer-downloads"),
            ("nav-waiting", "transfer-summary-waiting"),
            ("nav-completed", "transfer-summary-completed"),
            ("nav-failed", "transfer-summary-failed"),
        ];
        let filters = div().flex_none().px(px(padding)).pb_3().child(
            TabBar::new("transfer-facets")
                .segmented()
                .selected_index(
                    facets
                        .iter()
                        .position(|(id, _)| *id == self.nav_selection)
                        .unwrap_or(0),
                )
                .children(
                    facets
                        .iter()
                        .map(|(_, label)| Tab::new().label(self.tr(label))),
                )
                .on_click(cx.listener(move |this, index: &usize, _, cx| {
                    if let Some((id, _)) = facets.get(*index) {
                        this.nav_selection = id;
                        this.focused_transfer_key = None;
                        this.selected_transfer_keys.clear();
                        this.pending_transfer_bulk_delete.clear();
                        this.show_transfer_detail = false;
                        this.transfer_scroll.scroll_to(gpui_kit::ListOffset {
                            item_ix: 0,
                            offset_in_item: px(0.0),
                        });
                        cx.notify();
                    }
                })),
        );
        let toolbar = div()
            .flex_none()
            .min_h(px(50.0))
            .px(px(padding))
            .py_2()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(div().text_xs().text_color(theme::text_secondary()).child(
                if selection_count == 0 {
                    self.tr("transfer-scope-visible")
                } else {
                    self.tr_with(
                        "transfer-footer-selected",
                        MessageArgs::new().with(
                            "count",
                            format_integer(self.locale(), selection_count as u64),
                        ),
                    )
                },
            ))
            .children(
                [
                    TransferAction::Resume,
                    TransferAction::Pause,
                    TransferAction::Retry,
                    TransferAction::Cancel,
                    TransferAction::Delete,
                ]
                .into_iter()
                .map(|action| {
                    let ids = presentation.actions[&(action as usize)].clone();
                    components::button(
                        ("transfer-bulk", action as usize),
                        self.tr(action.label()),
                        Some(action.icon()),
                        false,
                    )
                    .disabled(ids.is_empty() || self.transfer_action_job.is_some())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if action == TransferAction::Delete {
                            this.pending_transfer_bulk_delete = ids.as_ref().clone();
                        } else {
                            this.apply_transfer_action(action, &ids, cx);
                        }
                        cx.notify();
                    }))
                }),
            )
            .when(selection_count > 0, |bar| {
                bar.child(
                    components::button(
                        "transfer-clear-selection",
                        self.tr("telegram-files-clear-selection"),
                        None,
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.selected_transfer_keys.clear();
                        this.pending_transfer_bulk_delete.clear();
                        cx.notify();
                    })),
                )
            })
            .when(self.transfer_action_job.is_some(), |bar| {
                bar.child(
                    div()
                        .text_xs()
                        .text_color(theme::blue())
                        .child(self.tr("transfer-actions-applying")),
                )
            })
            .when_some(self.transfer_action_error, |bar, error| {
                bar.child(
                    div()
                        .w_full()
                        .text_xs()
                        .text_color(theme::red())
                        .child(self.tr(native_download_error_message_id(error))),
                )
            })
            .when(
                !self.pending_transfer_bulk_delete.is_empty()
                    || self.pending_transfer_delete.is_some(),
                |bar| {
                    let ids = if let Some(id) = self.pending_transfer_delete {
                        vec![id]
                    } else {
                        self.pending_transfer_bulk_delete.clone()
                    };
                    bar.child(self.render_transfer_delete_confirmation(ids, cx))
                },
            );

        let header = components::list_row()
            .px_3()
            .flex()
            .items_center()
            .bg(theme::sidebar())
            .border_y_1()
            .border_color(theme::border())
            .child(
                div().w(px(28.0)).flex_none().child(
                    Checkbox::new("transfers-select-all")
                        .accessibility_label(self.tr("telegram-files-select-all"))
                        .checked(all_visible_selected)
                        .on_click(cx.listener({
                            let visible_transfer_keys = visible_transfer_keys.clone();
                            move |this, checked: &bool, _, cx| {
                                toggle_visible_selection(
                                    &mut this.selected_transfer_keys,
                                    &visible_transfer_keys,
                                    !*checked,
                                );
                                cx.notify();
                            }
                        })),
                ),
            )
            .child(transfer_header(self.tr("table-name"), None))
            .child(transfer_header(
                self.tr("transfer-bytes-heading"),
                Some(140.0),
            ))
            .child(transfer_header(self.tr("transfer-eta-heading"), Some(72.0)))
            .child(transfer_header(self.tr("table-progress"), Some(188.0)))
            .child(transfer_header(self.tr("transfer-actions"), Some(116.0)));

        let has_rows = !transfer_rows.is_empty();
        let row_count = transfer_rows.len();
        let rows = transfer_rows;
        let keys = presentation.keys.clone();
        if !presentation.scroll_applied.replace(true) {
            let mut previous = self.transfer_list_keys.borrow_mut();
            if *previous != *keys {
                let offset = self.transfer_scroll.logical_scroll_top();
                let anchor = previous.get(offset.item_ix).copied();
                self.transfer_scroll.reset(row_count);
                if let Some(index) =
                    anchor.and_then(|anchor| keys.iter().position(|key| *key == anchor))
                {
                    self.transfer_scroll.scroll_to(gpui_kit::ListOffset {
                        item_ix: index,
                        offset_in_item: offset.offset_in_item,
                    });
                }
                *previous = keys.as_ref().clone();
            }
        }
        let virtual_rows = gpui_kit::list(
            self.transfer_scroll.clone(),
            cx.processor(move |this, index: usize, _, cx| {
                rows.get(index)
                    .cloned()
                    .map(|item| {
                        let row = item.row(this);
                        this.render_transfer_row(index, row, BatchRowPosition::at(&rows, index), cx)
                    })
                    .unwrap_or_else(|| div().into_any_element())
            }),
        )
        .w_full()
        .flex_1()
        .min_h_0();

        let omitted_tasks = self
            .native_transfer_view
            .omitted_items
            .saturating_add(self.vault_transfer_view.omitted_items);
        let table_footer = components::list_footer("transfer-list-footer")
            .child(self.tr_with(
                "transfer-footer-selected",
                MessageArgs::new().with(
                    "count",
                    format_integer(self.locale(), selection_count as u64),
                ),
            ))
            .child(self.tr_with(
                if omitted_tasks > 0 {
                    "transfer-footer-total-retained"
                } else {
                    "transfer-footer-total-live"
                },
                MessageArgs::new().with(
                    "count",
                    format_integer(
                        self.locale(),
                        (uploading + downloading + waiting + completed + failed) as u64,
                    ),
                ),
            ))
            .when(omitted_tasks > 0, |footer| {
                footer.child(self.tr_with(
                    "transfer-history-omitted",
                    MessageArgs::new().with("count", format_integer(self.locale(), omitted_tasks)),
                ))
            })
            .child(self.tr_with(
                "transfer-footer-downloading-live",
                MessageArgs::new().with("count", format_integer(self.locale(), downloading as u64)),
            ))
            .child(self.tr_with(
                "transfer-footer-waiting-live",
                MessageArgs::new().with("count", format_integer(self.locale(), waiting as u64)),
            ))
            .when(!layout.is_compact(), |footer| footer.child(div().flex_1()));

        let table =
            div()
                .flex_1()
                .min_h_0()
                .mx(px(padding))
                .rounded(theme::RADIUS_MEDIUM)
                .border_1()
                .border_color(theme::border())
                .bg(theme::surface())
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(header)
                .when(has_rows, |table| table.child(virtual_rows))
                .when(!has_rows, |table| {
                    table.child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap_3()
                            .child(
                                Icon::new(crate::assets::Symbol::Transfer)
                                    .size(px(34.0))
                                    .text_color(theme::blue()),
                            )
                            .child(div().text_sm().text_color(theme::text_secondary()).child(
                                self.tr(if self.upload_in_flight {
                                    "transfer-waiting"
                                } else {
                                    "transfer-empty"
                                }),
                            )),
                    )
                })
                .child(table_footer);

        let main = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(summary)
            .child(filters)
            .when(self.upload_in_flight || upload_activity.is_some(), |main| {
                main.child(
                    div()
                        .id("upload-preflight-status")
                        .debug_selector(|| "upload-preflight-status".into())
                        .flex_none()
                        .px(px(padding))
                        .py_2()
                        .text_sm()
                        .text_color(theme::blue())
                        .child(
                            upload_activity.unwrap_or_else(|| self.tr("transfer-upload-preparing")),
                        ),
                )
            })
            .when(
                self.transfer_controls_expanded
                    || selection_count > 0
                    || self.transfer_action_error.is_some()
                    || self.pending_transfer_delete.is_some()
                    || !self.pending_transfer_bulk_delete.is_empty(),
                |main| main.child(toolbar),
            )
            .child(table)
            .pb(px(padding));

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .relative()
            .bg(theme::canvas())
            .child(main)
            .when_some(
                selected.filter(|_| self.show_transfer_detail),
                |page, selected| {
                    if layout.docks_transfer_inspector() {
                        page.child(self.render_transfer_detail(selected, layout, cx))
                    } else {
                        page.child(
                            div()
                                .absolute()
                                .right_0()
                                .top_0()
                                .bottom_0()
                                .shadow_lg()
                                .child(self.render_transfer_detail(selected, layout, cx)),
                        )
                    }
                },
            )
            .into_any_element()
    }

    fn redownload_completed(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.transfer_action_job.is_some() {
            return;
        }
        let Some(transfers) = self.transfers.clone() else {
            return;
        };
        self.transfer_action_error = None;
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let work = cx.background_spawn(async move { transfers.redownload_completed(id) });
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                this.transfer_action_job = None;
                match result {
                    Ok(id) => {
                        this.focused_transfer_key = Some(id);
                        this.monitor_channel_download(id, cx);
                    }
                    Err(error) => this.transfer_action_error = Some(error.kind()),
                }
                cx.notify();
            });
        });
        self.transfer_action_job = Some(TransferActionJob {
            _task: task,
            cancelled,
        });
    }

    fn apply_transfer_action(
        &mut self,
        action: TransferAction,
        ids: &[u64],
        cx: &mut Context<Self>,
    ) {
        if self.transfer_action_job.is_some() || ids.is_empty() {
            return;
        }
        let Some(transfers) = self.transfers.clone() else {
            return;
        };
        self.transfer_action_error = None;
        let ids = ids.to_vec();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancellation = cancelled.clone();
        let work = cx.background_spawn(async move {
            execute_transfer_actions(action, &ids, &cancellation, |id| match action {
                TransferAction::Pause => transfers.pause(id),
                TransferAction::Resume => transfers.resume(id),
                TransferAction::Retry => transfers.retry(id),
                TransferAction::Cancel => transfers.cancel(id),
                TransferAction::Delete => transfers.delete(id),
            })
        });
        let task = cx.spawn(async move |this, cx| {
            let (deleted, failure) = work.await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                if !deleted.is_empty() {
                    this.show_transfer_detail = false;
                }
                for id in deleted {
                    this.selected_transfer_keys.remove(&id);
                }
                this.transfer_action_error = failure;
                this.transfer_action_job = None;
                cx.notify();
            });
        });
        self.transfer_action_job = Some(TransferActionJob {
            _task: task,
            cancelled,
        });
        cx.notify();
    }

    fn render_transfer_delete_confirmation(
        &self,
        ids: Vec<u64>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .w_full()
            .p_3()
            .rounded(theme::RADIUS_SMALL)
            .bg(theme::amber_soft())
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(div().flex_1().text_sm().child(self.tr_with(
                "transfer-delete-confirmation",
                MessageArgs::new().with("count", format_integer(self.locale(), ids.len() as u64)),
            )))
            .child(
                components::button(
                    "transfer-confirm-delete",
                    self.tr("action-confirm-delete-task"),
                    None,
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.apply_transfer_action(TransferAction::Delete, &ids, cx);
                    this.pending_transfer_bulk_delete.clear();
                    this.pending_transfer_delete = None;
                    cx.notify();
                })),
            )
            .child(
                components::button(
                    "transfer-dismiss-delete",
                    self.tr("action-cancel"),
                    None,
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.pending_transfer_bulk_delete.clear();
                    this.pending_transfer_delete = None;
                    cx.notify();
                })),
            )
            .into_any_element()
    }

    fn stop_saved_vault_batch(&mut self, batch: u64, cx: &mut Context<Self>) {
        if self.transfer_action_job.is_some() || self.visual_preview {
            return;
        }
        let (Some(vault), Some(account)) = (
            self.vault.clone(),
            self.telegram_account.as_ref().map(|account| account.id),
        ) else {
            return;
        };
        let generation = self.telegram_login_generation;
        self.transfer_action_error = None;
        let work =
            cx.background_spawn(
                async move { vault.submit_stop_upload_batch(account, batch)?.wait() },
            );
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                this.transfer_action_job = None;
                if this.telegram_login_generation == generation
                    && this.telegram_account.as_ref().map(|account| account.id) == Some(account)
                {
                    this.transfer_action_error = result.err().map(|error| error.kind());
                }
                cx.notify();
            });
        });
        self.transfer_action_job = Some(TransferActionJob {
            _task: task,
            cancelled: Default::default(),
        });
        cx.notify();
    }

    fn render_transfer_actions(
        &self,
        transfer: &TransferRow,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selection_key = transfer_selection_key(transfer, index);
        let mut actions = div()
            .w(px(116.0))
            .flex_none()
            .flex()
            .justify_end()
            .items_center()
            .gap_1();
        if let Some(id) = transfer.runtime_task_id {
            for action in [
                TransferAction::Pause,
                TransferAction::Resume,
                TransferAction::Retry,
                TransferAction::Cancel,
                TransferAction::Delete,
            ] {
                if self
                    .runtime_transfer_snapshot(id)
                    .is_some_and(|snapshot| action.supports_snapshot(&snapshot))
                {
                    actions = actions.child(
                        components::list_icon_button(
                            (action.element_id(), id),
                            action.icon(),
                            self.tr(action.label()),
                        )
                        .ghost()
                        .h(theme::LIST_CONTROL_SIZE)
                        .w(theme::LIST_CONTROL_SIZE)
                        .disabled(self.transfer_action_job.is_some() || self.visual_preview)
                        .ghost()
                        .h(theme::LIST_CONTROL_SIZE)
                        .w(theme::LIST_CONTROL_SIZE)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if action == TransferAction::Delete {
                                this.pending_transfer_delete = Some(id);
                                this.pending_transfer_bulk_delete.clear();
                            } else {
                                this.apply_transfer_action(action, &[id], cx);
                            }
                            cx.notify();
                        })),
                    );
                }
            }
        }
        if let Some(id) = transfer.vault_transfer_id
            && let Some(snapshot) = self.vault_transfer_snapshot(id)
        {
            for action in [
                TransferAction::Pause,
                TransferAction::Resume,
                TransferAction::Retry,
                TransferAction::Cancel,
                TransferAction::Delete,
            ] {
                if !vault_action_supported(action, &snapshot) {
                    continue;
                }
                let control = matches!(
                    action,
                    TransferAction::Pause | TransferAction::Cancel | TransferAction::Delete
                );
                actions = actions.child(
                    components::list_icon_button(
                        (action.element_id(), id),
                        action.icon(),
                        self.tr(action.label()),
                    )
                    .ghost()
                    .size(theme::LIST_CONTROL_SIZE)
                    .disabled(
                        self.visual_preview
                            || self.vault_transfer_jobs.contains_key(&(
                                snapshot.account_id,
                                id,
                                control,
                            )),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.apply_vault_transfer_action(id, action, cx);
                    })),
                );
            }
        }
        if transfer.vault_transfer_id.is_none()
            && let Some(batch_id) = transfer.vault_batch_id
            && matches!(
                transfer.state,
                TransferState::Uploading | TransferState::Waiting
            )
        {
            actions = actions.child(
                components::list_icon_button(
                    ("vault-batch-stop", batch_id),
                    IconName::CircleX,
                    self.tr("upload-stop-after-current"),
                )
                .ghost()
                .size(theme::LIST_CONTROL_SIZE)
                .disabled(self.visual_preview || self.transfer_action_job.is_some())
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.stop_saved_vault_batch(batch_id, cx);
                })),
            );
        }
        if self.transfers.is_none()
            && transfer.runtime_task_id.is_none()
            && transfer.vault_transfer_id.is_none()
        {
            for action in [
                TransferAction::Pause,
                TransferAction::Resume,
                TransferAction::Retry,
                TransferAction::Cancel,
                TransferAction::Delete,
            ] {
                if action.supports_view(transfer.state) {
                    actions = actions.child(
                        components::list_icon_button(
                            (action.element_id(), index),
                            action.icon(),
                            self.tr(action.label()),
                        )
                        .ghost()
                        .h(theme::LIST_CONTROL_SIZE)
                        .w(theme::LIST_CONTROL_SIZE)
                        .disabled(true),
                    );
                }
            }
        }
        if transfer.state == TransferState::Completed
            && transfer.direction == TransferDirection::Download
            && (transfer.runtime_task_id.is_some() || transfer.vault_transfer_id.is_some())
        {
            let destination = std::path::PathBuf::from(transfer.destination.as_ref());
            let reveal = destination.clone();
            let available = self.local_presence_for_path(&destination)
                == Some(teleark_runtime::LocalFilePresence::Present);
            if available {
                actions = actions.child(
                    components::list_icon_button(
                        ("transfer-reveal", index),
                        IconName::FolderOpen,
                        self.tr("action-show-in-folder"),
                    )
                    .ghost()
                    .h(theme::LIST_CONTROL_SIZE)
                    .w(theme::LIST_CONTROL_SIZE)
                    .disabled(self.visual_preview)
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        cx.reveal_path(&reveal);
                    }),
                );
            }
            if !available && let Some(id) = transfer.runtime_task_id {
                actions = actions.child(
                    components::list_icon_button(
                        ("transfer-redownload", id),
                        IconName::Redo2,
                        self.tr("transfer-download-again"),
                    )
                    .ghost()
                    .size(theme::LIST_CONTROL_SIZE)
                    .disabled(self.transfer_action_job.is_some() || self.visual_preview)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.redownload_completed(id, cx);
                    })),
                );
            }
        }
        {
            actions = actions.child(
                components::list_icon_button(
                    ("transfer-details", index),
                    IconName::Info,
                    self.tr("transfer-show-details"),
                )
                .ghost()
                .h(theme::LIST_CONTROL_SIZE)
                .w(theme::LIST_CONTROL_SIZE)
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.selected_file = index;
                    this.focused_transfer_key = Some(selection_key);
                    this.show_transfer_detail = true;
                    this.transfer_detail_scroll
                        .set_offset(gpui_kit::point(px(0.0), px(0.0)));
                    this.batch_detail_scroll
                        .scroll_to_item(0, gpui_kit::ScrollStrategy::Top);
                    cx.notify();
                })),
            );
        }
        actions.into_any_element()
    }

    fn render_transfer_row(
        &self,
        index: usize,
        transfer: TransferRow,
        position: BatchRowPosition,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focused = self.focused_transfer_key == Some(transfer_selection_key(&transfer, index));
        let selection_key = transfer_selection_key(&transfer, index);
        let selected = self.selected_transfer_keys.contains(&selection_key);
        let tone = transfer_tone(transfer.state);
        let actions = self.render_transfer_actions(&transfer, index, cx);
        let bytes_label: SharedString =
            format!("{} / {}", transfer.transferred, transfer.size).into();
        let bytes_tooltip = bytes_label.clone();
        let eta_tooltip = transfer.eta.clone();
        let batch_group_id = transfer
            .runtime_batch_id
            .filter(|_| transfer.runtime_task_id.is_none() && !transfer.batch_child)
            .or_else(|| {
                transfer
                    .vault_batch_id
                    .filter(|_| transfer.vault_transfer_id.is_none() && !transfer.batch_child)
                    .map(vault_batch_key)
            });
        let batch_expanded =
            batch_group_id.is_some_and(|id| self.expanded_transfer_batches.contains(&id));
        let batch_count = transfer
            .batch_summary
            .as_ref()
            .map_or(0, |batch| batch.total);
        let is_batch = transfer.batch_summary.is_some();
        let target = batch_window::TransferRowTarget {
            index,
            key: selection_key,
            batch: batch_group_id.map(|id| (id, batch_count)),
        };

        let local_presence = (transfer.state == TransferState::Completed
            && transfer.direction == TransferDirection::Download
            && (transfer.runtime_task_id.is_some() || transfer.vault_transfer_id.is_some()))
        .then(|| self.local_presence_for_path(std::path::Path::new(transfer.destination.as_ref())));
        let row = div()
            .id(("transfer-row", selection_key))
            .debug_selector(move || format!("transfer-row-{selection_key}"))
            .w_full()
            .h(if is_batch {
                theme::BATCH_ROW_HEIGHT
            } else {
                theme::ROW_HEIGHT
            })
            .relative()
            .px_3()
            .when(transfer.batch_child, |row| {
                row.pl(theme::BATCH_MEMBER_INDENT)
            })
            .flex()
            .items_center()
            .text_size(theme::LIST_TEXT_SIZE)
            .cursor_pointer()
            .focusable()
            .tab_index(0)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.activate_transfer_row(target, window, cx);
            }))
            .on_key_down(
                cx.listener(move |this, event: &gpui_kit::KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.activate_transfer_row(target, window, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .child(
                div()
                    .debug_selector(move || format!("transfer-checkbox-{selection_key}"))
                    .w(px(28.0))
                    .flex_none()
                    .child(
                        Checkbox::new(("transfer-select", selection_key))
                            .accessibility_label(self.tr("telegram-file-select-action"))
                            .checked(selected)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                cx.stop_propagation();
                                if *checked {
                                    this.selected_transfer_keys.insert(selection_key);
                                } else {
                                    this.selected_transfer_keys.remove(&selection_key);
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(transfer.batch_child, |name| name.pl_4())
                    .font_weight(FontWeight::MEDIUM)
                    .when_some(batch_group_id, |name, batch_id| {
                        name.child(
                            components::list_icon_button(
                                ("batch-expand", batch_id),
                                if batch_expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                },
                                self.tr(if batch_count > batch_window::INLINE_BATCH_LIMIT {
                                    "transfer-batch-open-window"
                                } else if batch_expanded {
                                    "transfer-collapse-batch"
                                } else {
                                    "transfer-expand-batch"
                                }),
                            )
                            .ghost()
                            .size(theme::LIST_CONTROL_SIZE)
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.activate_transfer_row(target, window, cx);
                                },
                            )),
                        )
                    })
                    .child(
                        Icon::new(if transfer.direction == TransferDirection::Upload {
                            IconName::ArrowUp
                        } else {
                            IconName::ArrowDown
                        })
                        .size(theme::LIST_ICON_SIZE)
                        .text_color(
                            if transfer.direction == TransferDirection::Upload {
                                Tone::Purple.foreground()
                            } else {
                                theme::blue()
                            },
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pr_4()
                            .child(
                                div()
                                    .debug_selector(move || {
                                        format!("transfer-title-{selection_key}")
                                    })
                                    .text_size(theme::LIST_TEXT_SIZE)
                                    .when(is_batch, |title| title.font_weight(FontWeight::SEMIBOLD))
                                    .line_height(theme::LIST_LINE_HEIGHT)
                                    .truncate()
                                    .child(transfer.name.clone()),
                            )
                            .when_some(transfer.batch_summary.as_ref(), |name, batch| {
                                name.child(
                                    div()
                                        .debug_selector(move || {
                                            format!("batch-progress-{selection_key}")
                                        })
                                        .text_size(theme::LIST_SECONDARY_TEXT_SIZE)
                                        .line_height(theme::LIST_LINE_HEIGHT)
                                        .text_color(theme::text_secondary())
                                        .truncate()
                                        .child(format!(
                                            "{} · {}",
                                            self.batch_progress_label(batch),
                                            transfer.size
                                        )),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .w(px(140.0))
                    .flex_none()
                    .pr_3()
                    .text_color(theme::text_secondary())
                    .text_size(theme::LIST_TEXT_SIZE)
                    .truncate()
                    .id(("transfer-bytes", selection_key))
                    .debug_selector(move || format!("transfer-bytes-{selection_key}"))
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(bytes_tooltip.clone())
                            .build(window, cx)
                    })
                    .child(bytes_label),
            )
            .child(
                div()
                    .w(px(72.0))
                    .flex_none()
                    .pr_3()
                    .text_color(theme::text_muted())
                    .text_size(theme::LIST_TEXT_SIZE)
                    .truncate()
                    .id(("transfer-eta", selection_key))
                    .debug_selector(move || format!("transfer-eta-{selection_key}"))
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(eta_tooltip.clone())
                            .build(window, cx)
                    })
                    .child(transfer.eta.clone()),
            )
            .child(
                div()
                    .w(px(188.0))
                    .flex_none()
                    .pr_5()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(theme::LIST_TEXT_SIZE)
                            .line_height(px(12.0))
                            .text_color(tone.foreground())
                            .child(
                                div().min_w_0().truncate().child(
                                    transfer
                                        .activity
                                        .clone()
                                        .unwrap_or_else(|| self.tr(transfer.state.message_id())),
                                ),
                            )
                            .child(if transfer.activity.is_some() && transfer.progress == 0.0 {
                                self.tr("transfer-value-unavailable").to_string()
                            } else {
                                format_percent(
                                    self.locale(),
                                    f64::from(transfer.progress) / 100.0,
                                    0,
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(components::progress(
                                transfer.progress,
                                tone,
                                self.tr("table-progress"),
                            )))
                            .child(
                                div()
                                    .max_w(px(100.0))
                                    .text_size(theme::LIST_TEXT_SIZE)
                                    .line_height(px(12.0))
                                    .text_color(theme::text_muted())
                                    .truncate()
                                    .when(
                                        local_presence.flatten().is_some_and(|presence| {
                                            presence != teleark_runtime::LocalFilePresence::Present
                                        }),
                                        |label| label.text_color(theme::amber()),
                                    )
                                    .child(if let Some(presence) = local_presence {
                                        self.local_presence_label(presence)
                                    } else if matches!(
                                        transfer.state,
                                        TransferState::Uploading | TransferState::Downloading
                                    ) {
                                        transfer.speed
                                    } else if matches!(
                                        transfer.state,
                                        TransferState::Waiting | TransferState::Paused
                                    ) {
                                        transfer.eta
                                    } else {
                                        "".into()
                                    }),
                            ),
                    ),
            )
            .child(actions);
        position.decorate(row, selection_key, focused || selected, cx)
    }

    fn render_transfer_telemetry(
        &self,
        telemetry: TransferTelemetrySnapshot,
        transfer_is_active: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let replay_mode = self.transfer_inspector_replay || !transfer_is_active;
        let decision_count = telemetry.decisions.len();
        let omitted_decisions = telemetry
            .decisions
            .first()
            .map_or(0, |decision| decision.sequence.saturating_sub(1));
        let replay_cursor = self
            .transfer_replay_cursor
            .min(decision_count.saturating_sub(1));
        let decisions: Vec<_> = if replay_mode {
            telemetry
                .decisions
                .get(replay_cursor)
                .copied()
                .into_iter()
                .collect()
        } else {
            telemetry
                .decisions
                .iter()
                .rev()
                .take(8)
                .rev()
                .copied()
                .collect()
        };
        let decision_rows = decisions
            .into_iter()
            .map(|decision| self.render_controller_decision(decision));
        let live_button = components::button(
            "transfer-telemetry-live",
            self.tr("transfer-mode-live"),
            Some(IconName::ChartPie),
            !replay_mode,
        )
        .disabled(!transfer_is_active)
        .on_click(cx.listener(|this, _, _, cx| {
            this.transfer_inspector_replay = false;
            cx.notify();
        }));
        let replay_button = components::button(
            "transfer-telemetry-replay",
            self.tr("transfer-mode-replay"),
            Some(IconName::Redo),
            replay_mode,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.transfer_inspector_replay = true;
            this.transfer_replay_cursor = decision_count.saturating_sub(1);
            cx.notify();
        }));
        let previous_button = components::button(
            "transfer-replay-previous",
            self.tr("transfer-replay-previous"),
            Some(IconName::ChevronLeft),
            false,
        )
        .disabled(!replay_mode || replay_cursor == 0 || decision_count == 0)
        .on_click(cx.listener(|this, _, _, cx| {
            this.transfer_replay_cursor = this.transfer_replay_cursor.saturating_sub(1);
            cx.notify();
        }));
        let next_button = components::button(
            "transfer-replay-next",
            self.tr("transfer-replay-next"),
            Some(IconName::ChevronRight),
            false,
        )
        .disabled(!replay_mode || replay_cursor.saturating_add(1) >= decision_count)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.transfer_replay_cursor = this
                .transfer_replay_cursor
                .saturating_add(1)
                .min(decision_count.saturating_sub(1));
            cx.notify();
        }));
        let lane_rows: Vec<AnyElement> = telemetry
            .lanes
            .iter()
            .map(|lane| {
                let status = if lane.paused_until_millis.is_some() {
                    self.tr("transfer-lane-paused")
                } else {
                    self.tr("transfer-lane-active")
                };
                components::list_summary(
                    SharedString::from(format!(
                        "transfer-lane-{}-{}",
                        lane.data_center_id, lane.lane_id
                    )),
                    self.tr_with(
                        "transfer-lane-value",
                        MessageArgs::new()
                            .with(
                                "dc",
                                format_integer(self.locale(), lane.data_center_id.max(0) as u64),
                            )
                            .with(
                                "lane",
                                format_integer(self.locale(), u64::from(lane.lane_id)),
                            )
                            .with(
                                "inflight",
                                format_integer(self.locale(), u64::from(lane.inflight_rpc_count)),
                            )
                            .with(
                                "speed",
                                format_speed(self.locale(), lane.throughput_bytes_per_second),
                            )
                            .with(
                                "rtt",
                                format_duration_millis(
                                    self.locale(),
                                    lane.round_trip_time_p95_millis,
                                ),
                            )
                            .with("status", status.to_string()),
                    ),
                )
                .into_any_element()
            })
            .collect();

        components::card()
            .mt_2()
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.tr("transfer-controller-title")),
                    )
                    .child(div().flex().gap_2().child(live_button).child(replay_button)),
            )
            .child(
                div().text_xs().text_color(theme::text_secondary()).child(
                    self.tr_with(
                        "transfer-controller-queue-waits-value",
                        MessageArgs::new()
                            .with(
                                "network",
                                format_duration_millis(
                                    self.locale(),
                                    telemetry.network_waiting_for_encryption_millis,
                                ),
                            )
                            .with(
                                "encryption",
                                format_duration_millis(
                                    self.locale(),
                                    telemetry.encryption_waiting_for_network_millis,
                                ),
                            )
                            .with(
                                "parts",
                                format_decimal(
                                    self.locale(),
                                    telemetry.parts.completed_parts_per_second_milli as f64
                                        / 1_000.0,
                                    2,
                                ),
                            ),
                    ),
                ),
            )
            .child(
                div().text_xs().text_color(theme::text_secondary()).child(
                    self.tr_with(
                        "transfer-controller-buffers-value",
                        MessageArgs::new()
                            .with(
                                "plaintext",
                                format_bytes(
                                    self.locale(),
                                    telemetry.memory.plaintext_buffer_bytes,
                                ),
                            )
                            .with(
                                "encrypted",
                                format_bytes(
                                    self.locale(),
                                    telemetry.memory.encrypted_buffer_bytes,
                                ),
                            )
                            .with(
                                "network",
                                format_bytes(
                                    self.locale(),
                                    telemetry.memory.network_inflight_bytes,
                                ),
                            )
                            .with(
                                "writer",
                                format_bytes(self.locale(), telemetry.memory.writer_queue_bytes),
                            ),
                    ),
                ),
            )
            .child(
                div()
                    .pt_2()
                    .border_t_1()
                    .border_color(theme::border())
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tr("transfer-controller-decisions")),
            )
            .when(decision_count == 0, |card| {
                card.child(
                    div()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(self.tr("transfer-controller-no-decisions")),
                )
            })
            .when(omitted_decisions > 0, |card| {
                card.child(
                    div().text_xs().text_color(theme::text_muted()).child(
                        self.tr_with(
                            "transfer-controller-history-omitted",
                            MessageArgs::new()
                                .with("count", format_integer(self.locale(), omitted_decisions)),
                        ),
                    ),
                )
            })
            .children(decision_rows)
            .when(replay_mode && decision_count != 0, |card| {
                card.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(previous_button)
                        .child(
                            div().text_xs().text_color(theme::text_muted()).child(
                                self.tr_with(
                                    "transfer-replay-position",
                                    MessageArgs::new()
                                        .with(
                                            "current",
                                            format_integer(
                                                self.locale(),
                                                replay_cursor.saturating_add(1) as u64,
                                            ),
                                        )
                                        .with(
                                            "total",
                                            format_integer(self.locale(), decision_count as u64),
                                        ),
                                ),
                            ),
                        )
                        .child(next_button),
                )
            })
            .child(
                div()
                    .pt_2()
                    .border_t_1()
                    .border_color(theme::border())
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tr("transfer-controller-lanes")),
            )
            .when(lane_rows.is_empty(), |card| {
                card.child(
                    div()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(self.tr("transfer-controller-lanes-unavailable")),
                )
            })
            .children(lane_rows)
            .into_any_element()
    }

    fn render_controller_decision(&self, decision: ControllerDecision) -> AnyElement {
        let parameter = decision.parameter.map_or_else(
            || self.tr("transfer-controller-no-parameter"),
            |parameter| self.tr(parameter_message_id(parameter)),
        );
        let before_value = decision
            .parameter
            .map(|parameter| control_parameter_value(decision.before, parameter));
        let after_value = decision
            .parameter
            .map(|parameter| control_parameter_value(decision.after, parameter));
        let change = match (before_value, after_value) {
            (Some(before), Some(after)) => format!("{parameter}: {before} → {after}"),
            _ => parameter.to_string(),
        };
        let outcome = decision_outcome_message_id(decision.outcome);
        let tone = match decision.outcome {
            ControllerDecisionOutcome::Keep
            | ControllerDecisionOutcome::OverrideSoftLimit
            | ControllerDecisionOutcome::ResumeLane => Tone::Green,
            ControllerDecisionOutcome::Rollback | ControllerDecisionOutcome::PauseLane => Tone::Red,
            ControllerDecisionOutcome::Confirm
            | ControllerDecisionOutcome::Recover
            | ControllerDecisionOutcome::RespectSoftLimit => Tone::Amber,
            _ => Tone::Blue,
        };
        div()
            .p_2()
            .rounded(theme::RADIUS_SMALL)
            .bg(theme::canvas())
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!(
                        "{} · {}",
                        self.tr(controller_phase_message_id(decision.phase)),
                        change
                    )))
                    .child(components::badge(self.tr(outcome), tone)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_secondary())
                    .child(self.tr(decision_reason_message_id(decision.reason))),
            )
            .child(
                div().text_xs().text_color(theme::text_muted()).child(
                    self.tr_with(
                        "transfer-decision-throughput-value",
                        MessageArgs::new()
                            .with(
                                "before",
                                format_speed(
                                    self.locale(),
                                    decision.baseline_goodput_bytes_per_second,
                                ),
                            )
                            .with(
                                "after",
                                format_speed(
                                    self.locale(),
                                    decision.observed_goodput_bytes_per_second,
                                ),
                            )
                            .with(
                                "change",
                                format_percent(
                                    self.locale(),
                                    f64::from(decision.goodput_change_basis_points) / 10_000.0,
                                    1,
                                ),
                            )
                            .with(
                                "elapsed",
                                format_duration_millis(self.locale(), decision.observed_at_millis),
                            ),
                    ),
                ),
            )
            .into_any_element()
    }

    fn batch_progress_label(&self, batch: &BatchSummary) -> SharedString {
        self.tr_with(
            "transfer-batch-progress",
            MessageArgs::new()
                .with(
                    "completed",
                    format_integer(self.locale(), batch.completed as u64),
                )
                .with("total", format_integer(self.locale(), batch.total as u64))
                .with("failed", format_integer(self.locale(), batch.failed as u64)),
        )
    }

    fn render_batch_detail(
        &self,
        transfer: &TransferRow,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let members = if self.visual_preview {
            self.preview_transfer_rows
                .iter()
                .filter(|row| {
                    row.batch_child
                        && row.runtime_batch_id == transfer.runtime_batch_id
                        && row.vault_batch_id == transfer.vault_batch_id
                })
                .cloned()
                .map(|row| TransferItem::Preview(std::sync::Arc::new(row)))
                .collect::<Vec<_>>()
        } else if let Some(batch) = transfer.runtime_batch_id {
            self.native_transfer_view
                .items
                .iter()
                .filter(|row| row.batch_id == Some(batch))
                .cloned()
                .map(|row| TransferItem::Native(row, true))
                .collect::<Vec<_>>()
        } else {
            self.vault_transfer_view
                .items
                .iter()
                .filter(|row| row.batch_id == transfer.vault_batch_id)
                .cloned()
                .map(|row| TransferItem::Vault(row, true))
                .collect::<Vec<_>>()
        };
        let batch_key = transfer
            .runtime_batch_id
            .or_else(|| transfer.vault_batch_id.map(vault_batch_key));
        let member_count = members.len();
        let members = std::sync::Arc::new(members);
        let files = gpui_kit::uniform_list(
            "batch-member-list",
            member_count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|index| members.get(index))
                    .map(|item| {
                        let row = item.row(this);
                        let selection_key = transfer_selection_key(&row, 0);
                        let presence = (row.direction == TransferDirection::Download
                            && row.state == TransferState::Completed)
                            .then(|| {
                                this.local_presence_for_path(std::path::Path::new(
                                    row.destination.as_ref(),
                                ))
                            });
                        gpui_kit::base::Button::new(("batch-member", selection_key))
                            .accessibility_label(row.name.clone())
                            .w_full()
                            .h(theme::ROW_HEIGHT)
                            .text_size(theme::LIST_TEXT_SIZE)
                            .line_height(theme::LIST_LINE_HEIGHT)
                            .px_4()
                            .flex()
                            .items_center()
                            .gap_2()
                            .border_b_1()
                            .border_color(theme::border())
                            .hover(|row| row.bg(theme::blue_pale()))
                            .child(
                                div()
                                    .w_full()
                                    .truncate()
                                    .text_left()
                                    .text_size(theme::LIST_TEXT_SIZE)
                                    .child(row.name.clone()),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(theme::LIST_SECONDARY_TEXT_SIZE)
                                            .text_color(theme::text_secondary())
                                            .child(row.size.clone()),
                                    )
                                    .child(components::badge(
                                        presence
                                            .map(|presence| this.local_presence_label(presence))
                                            .unwrap_or_else(|| this.tr(row.state.message_id())),
                                        match presence {
                                            Some(Some(
                                                teleark_runtime::LocalFilePresence::Present,
                                            )) => Tone::Green,
                                            Some(Some(_)) => Tone::Amber,
                                            Some(None) => Tone::Neutral,
                                            None => transfer_tone(row.state),
                                        },
                                    )),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(batch_key) = batch_key {
                                    if member_count > batch_window::INLINE_BATCH_LIMIT {
                                        this.open_transfer_batch_window(batch_key, window, cx);
                                    } else {
                                        this.expanded_transfer_batches.insert(batch_key);
                                    }
                                }
                                this.show_transfer_detail = true;
                                this.focused_transfer_key = Some(selection_key);
                                this.transfer_detail_scroll
                                    .set_offset(gpui_kit::point(px(0.0), px(0.0)));
                                cx.notify();
                            }))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.batch_detail_scroll)
        .w_full()
        .flex_1()
        .min_h_0();
        components::inspector_panel("batch-inspector", layout.transfer_inspector_width())
            .child(
                div()
                    .p_4()
                    .flex_none()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(transfer.name.clone()),
                            )
                            .child(
                                components::icon_button(
                                    "batch-close-details",
                                    IconName::Close,
                                    self.tr("transfer-close-details"),
                                )
                                .ghost()
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.show_transfer_detail = false;
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                    .child(
                        div()
                            .mt_2()
                            .text_xs()
                            .text_color(theme::text_secondary())
                            .child(format!("{} · {}", transfer.source, transfer.size)),
                    )
                    .when_some(transfer.batch_summary.as_ref(), |body, batch| {
                        body.child(
                            div()
                                .mt_3()
                                .text_sm()
                                .child(self.batch_progress_label(batch)),
                        )
                        .child(
                            div()
                                .mt_2()
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child(format_unix_millis(self.locale(), batch.queued_at_unix_ms)),
                        )
                    }),
            )
            .child(
                div()
                    .px_4()
                    .h(px(36.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .text_xs()
                    .child(self.tr("detail-tab-file-list")),
            )
            .child(files)
            .into_any_element()
    }

    fn render_transfer_detail(
        &self,
        transfer: TransferRow,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if transfer.batch_summary.is_some() {
            return self.render_batch_detail(&transfer, layout, cx);
        }
        let tone = transfer_tone(transfer.state);
        let active = matches!(
            transfer.state,
            TransferState::Downloading | TransferState::Uploading | TransferState::Paused
        );
        let waiting = transfer.state == TransferState::Waiting;
        let completed = transfer.state == TransferState::Completed;
        let failed = transfer.state == TransferState::Failed;
        let runtime_backed = transfer.runtime_task_id.is_some();
        let vault_backed = transfer.vault_transfer_id.is_some();
        let upload = transfer.direction == TransferDirection::Upload;
        let preview_upload = self.visual_preview && upload && !runtime_backed && !vault_backed;
        let runtime_snapshot = transfer
            .runtime_task_id
            .and_then(|id| self.runtime_transfer_snapshot(id));
        let vault_snapshot = transfer
            .vault_transfer_id
            .and_then(|id| self.vault_transfer_snapshot(id));
        let interrupted = vault_snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.state == VaultTransferState::Interrupted);
        let telemetry = vault_snapshot
            .as_ref()
            .filter(|snapshot| !snapshot.restored)
            .map(|snapshot| snapshot.telemetry.clone())
            .or_else(|| {
                runtime_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.telemetry.clone())
            });
        let unavailable = self.tr("transfer-value-unavailable");
        let remote_message_id = transfer
            .message_id
            .map(|id| id.to_string().into())
            .unwrap_or_else(|| unavailable.clone());
        let created_at = vault_snapshot
            .as_ref()
            .map(|snapshot| snapshot.queued_at_unix_ms)
            .or_else(|| {
                runtime_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.queued_at_unix_ms)
            })
            .map(|timestamp| format_unix_millis(self.locale(), timestamp).into())
            .unwrap_or_else(|| unavailable.clone());
        let started_at = vault_snapshot
            .as_ref()
            .and_then(|snapshot| {
                (snapshot.started_at_unix_ms > 0).then_some(snapshot.started_at_unix_ms)
            })
            .or_else(|| {
                runtime_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.started_at_unix_ms)
            })
            .map(|timestamp| format_unix_millis(self.locale(), timestamp).into())
            .unwrap_or_else(|| unavailable.clone());
        let finished_at = runtime_snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .finished_at_unix_ms
                .map(|timestamp| format_unix_millis(self.locale(), timestamp).into())
        });
        let elapsed_ms = runtime_snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot.duration_ms.or_else(|| {
                    snapshot.started_at_unix_ms.and_then(|started| {
                        current_unix_millis()
                            .and_then(|now| now.checked_sub(started))
                            .and_then(|elapsed| u64::try_from(elapsed).ok())
                    })
                })
            })
            .or_else(|| {
                vault_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.duration_ms)
            });
        let transfer_icon = match transfer.state {
            TransferState::Downloading => IconName::ArrowDown,
            TransferState::Uploading => IconName::ArrowUp,
            TransferState::Waiting => IconName::Calendar,
            TransferState::Paused => IconName::Dash,
            TransferState::Completed => IconName::CircleCheck,
            TransferState::Failed => IconName::CircleX,
            TransferState::Cancelled => IconName::CircleX,
        };
        let mut details = vec![
            (
                self.tr("detail-direction"),
                self.tr(if upload {
                    "detail-direction-upload"
                } else {
                    "detail-direction-download"
                }),
            ),
            (self.tr("detail-source-channel"), transfer.source.clone()),
            (self.tr("detail-message-id"), remote_message_id),
            (self.tr("detail-local-path"), transfer.destination.clone()),
            (self.tr("detail-speed"), transfer.speed.clone()),
            (
                self.tr("detail-transferred"),
                SharedString::from(format!("{} / {}", transfer.transferred, transfer.size)),
            ),
            (
                self.tr("detail-concurrency-limits"),
                transfer.connections.clone(),
            ),
            (
                self.tr("detail-retries"),
                vault_snapshot.as_ref().map_or_else(
                    || {
                        runtime_snapshot.as_ref().map_or_else(
                            || unavailable.clone(),
                            |snapshot| {
                                format_integer(
                                    self.locale(),
                                    u64::from(snapshot.attempts.saturating_sub(1)),
                                )
                                .into()
                            },
                        )
                    },
                    |_| unavailable.clone(),
                ),
            ),
            (self.tr("detail-created"), created_at),
            (self.tr("detail-started"), started_at),
        ];
        if completed && !upload && (runtime_backed || vault_backed) {
            details.insert(
                4,
                (
                    self.tr("local-file-status"),
                    self.local_presence_label(self.local_presence_for_path(std::path::Path::new(
                        transfer.destination.as_ref(),
                    ))),
                ),
            );
        }

        details.push((
            self.tr("detail-storage-format"),
            self.tr(if upload || vault_backed {
                "detail-storage-format-teleark"
            } else {
                "detail-storage-format-native"
            }),
        ));
        details.push((
            self.tr("detail-content-protection"),
            self.tr(
                if vault_backed || (upload && self.preferences.upload_encrypt_content) {
                    "detail-content-protection-aes"
                } else {
                    "detail-content-protection-none"
                },
            ),
        ));
        details.push((
            self.tr("detail-integrity-codec"),
            self.tr(if upload || vault_backed {
                "detail-integrity-teleark"
            } else {
                "detail-integrity-native"
            }),
        ));
        if upload || vault_backed {
            details.push((
                self.tr("upload-part-size"),
                if vault_backed {
                    format_bytes(
                        self.locale(),
                        teleark_runtime::encrypted_part_plaintext_limit(),
                    )
                    .into()
                } else {
                    self.tr_with(
                        "upload-part-size-mib",
                        MessageArgs::new().with(
                            "size",
                            format_integer(
                                self.locale(),
                                u64::from(self.preferences.upload_part_size_mib),
                            ),
                        ),
                    )
                },
            ));
            details.push((
                self.tr("detail-manifest-codec"),
                self.tr("detail-manifest-codec-value"),
            ));
        }
        if let Some(sent_at) = transfer.message_sent_at_unix_ms {
            details.push((
                self.tr("telegram-message-sent-at"),
                format_unix_millis(self.locale(), sent_at).into(),
            ));
        }
        if let Some(mime_type) = transfer.mime_type.clone() {
            details.push((self.tr("telegram-message-mime-type"), mime_type));
        }
        details.push((
            self.tr("telegram-message-caption"),
            transfer
                .caption
                .clone()
                .unwrap_or_else(|| self.tr("telegram-message-no-caption")),
        ));
        if let Some(snapshot) = runtime_snapshot.as_ref() {
            details.push((
                self.tr("detail-trace-id"),
                SharedString::from(format!("DL-{}", snapshot.id)),
            ));
            details.push((
                self.tr("detail-queue-wait"),
                snapshot
                    .queue_wait_ms
                    .map(|duration| format_duration_millis(self.locale(), duration).into())
                    .unwrap_or_else(|| unavailable.clone()),
            ));
            details.push((
                self.tr("detail-elapsed"),
                elapsed_ms
                    .map(|duration| format_duration_millis(self.locale(), duration).into())
                    .unwrap_or_else(|| unavailable.clone()),
            ));
            details.push((
                self.tr("detail-average-speed"),
                snapshot
                    .average_bytes_per_second
                    .map(|speed| format_speed(self.locale(), speed).into())
                    .unwrap_or_else(|| unavailable.clone()),
            ));
            if let Some(finished_at) = finished_at {
                details.push((self.tr("detail-finished"), finished_at));
            }
        }
        if let Some(snapshot) = vault_snapshot.as_ref() {
            details.push((
                self.tr("detail-trace-id"),
                SharedString::from(format!("VAULT-{}", snapshot.id)),
            ));
            details.push((
                self.tr("detail-elapsed"),
                elapsed_ms
                    .map(|duration| format_duration_millis(self.locale(), duration).into())
                    .unwrap_or_else(|| unavailable.clone()),
            ));
            details.push((
                self.tr("detail-average-speed"),
                snapshot
                    .average_bytes_per_second
                    .map(|speed| format_speed(self.locale(), speed).into())
                    .unwrap_or_else(|| unavailable.clone()),
            ));
            details.push((
                self.tr("detail-vault-lifecycle"),
                self.tr(if upload {
                    "detail-vault-lifecycle-durable-upload"
                } else {
                    "detail-vault-lifecycle-memory-only"
                }),
            ));
        }
        if let Some(telemetry) = telemetry.as_ref().filter(|_| !upload) {
            let parameters = telemetry.parameters;
            details.extend([
                (
                    self.tr("transfer-controller-phase"),
                    self.tr(controller_phase_message_id(telemetry.phase)),
                ),
                (
                    self.tr("transfer-controller-parameters"),
                    format_control_parameters(parameters).into(),
                ),
                (
                    self.tr("transfer-controller-goodput"),
                    format_speed(self.locale(), telemetry.goodput_bytes_per_second).into(),
                ),
                (
                    self.tr("transfer-controller-encryption-throughput"),
                    format_speed(self.locale(), telemetry.encryption_bytes_per_second).into(),
                ),
                (
                    self.tr("transfer-controller-disk-throughput"),
                    format_speed(self.locale(), telemetry.disk_bytes_per_second).into(),
                ),
                (
                    self.tr("transfer-controller-bdp"),
                    format_bytes(self.locale(), telemetry.estimated_bdp_bytes).into(),
                ),
                (
                    self.tr("transfer-controller-rtt"),
                    format_duration_millis(self.locale(), telemetry.round_trip_time_p95_millis)
                        .into(),
                ),
                (
                    self.tr("transfer-controller-inflight"),
                    self.tr_with(
                        "transfer-controller-inflight-value",
                        MessageArgs::new()
                            .with(
                                "current",
                                format_bytes(self.locale(), telemetry.inflight_bytes),
                            )
                            .with(
                                "target",
                                format_bytes(self.locale(), telemetry.target_inflight_bytes),
                            ),
                    ),
                ),
                (
                    self.tr("transfer-controller-cpu"),
                    format_percent(
                        self.locale(),
                        f64::from(telemetry.cpu_utilization_basis_points) / 10_000.0,
                        1,
                    )
                    .into(),
                ),
                (
                    self.tr("transfer-controller-bottleneck"),
                    self.tr(bottleneck_message_id(telemetry.bottleneck)),
                ),
                (
                    self.tr("transfer-controller-memory"),
                    self.tr_with(
                        "transfer-controller-memory-value",
                        MessageArgs::new()
                            .with(
                                "used",
                                format_bytes(self.locale(), telemetry.memory.total_bytes()),
                            )
                            .with(
                                "budget",
                                format_bytes(self.locale(), telemetry.memory_budget_bytes),
                            ),
                    ),
                ),
                (
                    self.tr("transfer-controller-part-map"),
                    self.tr_with(
                        "transfer-controller-part-map-value",
                        MessageArgs::new()
                            .with(
                                "completed",
                                format_integer(self.locale(), telemetry.parts.completed_parts),
                            )
                            .with(
                                "inflight",
                                format_integer(self.locale(), telemetry.parts.inflight_parts),
                            )
                            .with(
                                "retry",
                                format_integer(self.locale(), telemetry.parts.retry_parts),
                            )
                            .with(
                                "failed",
                                format_integer(self.locale(), telemetry.parts.failed_parts),
                            )
                            .with(
                                "missing",
                                format_integer(self.locale(), telemetry.parts.missing_parts),
                            ),
                    ),
                ),
            ]);
        }
        if let Some(path) = vault_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.session_log_path.as_ref())
            .or_else(|| {
                runtime_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.session_log_path.as_ref())
            })
        {
            details.push((
                self.tr("transfer-session-log"),
                path.to_string_lossy().into_owned().into(),
            ));
        }

        if vault_snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.restored)
        {
            details.push((
                self.tr("transfer-history-restored-label"),
                self.tr("transfer-history-restored-detail"),
            ));
        }
        let omitted_records = teleark_runtime::session_log_dropped_record_count();
        if omitted_records > 0 {
            details.push((
                self.tr("transfer-session-log-omitted-label"),
                self.tr_with(
                    "transfer-session-log-omitted-count",
                    MessageArgs::new()
                        .with("count", format_integer(self.locale(), omitted_records)),
                ),
            ));
        }

        if let Some(snapshot) = vault_snapshot.as_ref()
            && matches!(snapshot.state, VaultTransferState::Failed(_))
            && let Some(activity) = snapshot.upload_activity.as_ref()
        {
            details.push((
                self.tr("detail-failure-last-phase"),
                self.tr(upload_phase_message_id(activity.phase)),
            ));
        }

        let (verification, verification_tone, verification_icon) =
            if let Some(snapshot) = runtime_snapshot.as_ref() {
                match snapshot.verification {
                    ChannelDownloadVerification::Pending => (
                        self.tr("detail-verification-pending"),
                        Tone::Amber,
                        IconName::LoaderCircle,
                    ),
                    ChannelDownloadVerification::SizeChecked => (
                        self.tr("detail-verification-size-checked"),
                        Tone::Green,
                        IconName::CircleCheck,
                    ),
                    ChannelDownloadVerification::NotReached => (
                        self.tr("detail-verification-not-reached"),
                        Tone::Amber,
                        IconName::CircleX,
                    ),
                }
            } else if completed {
                (
                    self.tr("detail-verification-passed"),
                    Tone::Green,
                    IconName::CircleCheck,
                )
            } else if vault_snapshot
                .as_ref()
                .is_some_and(|s| s.state == VaultTransferState::Interrupted)
            {
                (
                    self.tr("detail-verification-not-reached"),
                    Tone::Amber,
                    IconName::CircleX,
                )
            } else if failed {
                (
                    self.tr("detail-verification-failed"),
                    Tone::Red,
                    IconName::CircleX,
                )
            } else {
                (
                    self.tr("detail-verification-pending"),
                    Tone::Amber,
                    IconName::LoaderCircle,
                )
            };
        let failure_reason = runtime_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.failure)
            .map(|failure| {
                (
                    self.tr(native_download_error_message_id(failure.kind)),
                    self.tr(if failure.retryable {
                        "detail-failure-retryable"
                    } else if failure.requires_user_action {
                        "detail-failure-user-action"
                    } else {
                        "detail-failure-terminal"
                    }),
                )
            })
            .or_else(|| {
                vault_snapshot.as_ref().and_then(|snapshot| {
                    if let VaultTransferState::Failed(kind) = snapshot.state {
                        Some((
                            self.tr(vault_transfer_error_message_id(
                                kind,
                                snapshot
                                    .upload_activity
                                    .as_ref()
                                    .map(|activity| activity.phase),
                            )),
                            self.tr(vault_recovery_guidance_message_id(snapshot, kind)),
                        ))
                    } else {
                        None
                    }
                })
            });

        components::inspector_panel("transfer-inspector", layout.transfer_inspector_width())
            .debug_selector(|| "transfer-inspector".to_owned())
            .child(
                div()
                    .p_5()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .items_start()
                            .child(
                                div()
                                    .size(px(42.0))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(theme::RADIUS_SMALL)
                                    .bg(tone.background())
                                    .child(Icon::new(transfer_icon).text_color(tone.foreground())),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .child(
                                        div()
                                            .truncate()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(transfer.name),
                                    )
                                    .child(
                                        div()
                                            .mt_1()
                                            .text_xs()
                                            .text_color(theme::text_muted())
                                            .child(transfer.size),
                                    ),
                            )
                            .child(
                                components::icon_button(
                                    "transfer-close-details",
                                    IconName::Close,
                                    self.tr("transfer-close-details"),
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.show_transfer_detail = false;
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                    .child(
                        div()
                            .mt_5()
                            .flex()
                            .justify_between()
                            .text_xs()
                            .child(self.tr("table-status"))
                            .child(components::badge(
                                transfer
                                    .activity
                                    .clone()
                                    .unwrap_or_else(|| self.tr(transfer.state.message_id())),
                                tone,
                            )),
                    )
                    .child(
                        div()
                            .mt_4()
                            .flex()
                            .justify_between()
                            .text_sm()
                            .child(self.tr("table-progress"))
                            .child(if transfer.activity.is_some() && transfer.progress == 0.0 {
                                self.tr("transfer-value-unavailable").to_string()
                            } else {
                                format_percent(
                                    self.locale(),
                                    f64::from(transfer.progress) / 100.0,
                                    1,
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_size(theme::LIST_TEXT_SIZE)
                            .text_color(theme::text_muted())
                            .child(self.tr("transfer-rate-basis")),
                    )
                    .when_some(transfer.activity_detail.clone(), |header, activity| {
                        header.child(
                            div()
                                .debug_selector(|| "transfer-activity-detail".to_owned())
                                .mt_2()
                                .text_xs()
                                .text_color(theme::text_secondary())
                                .child(activity),
                        )
                    })
                    .child(div().mt_2().child(components::progress(
                        transfer.progress,
                        tone,
                        self.tr("table-progress"),
                    )))
                    .when(active || waiting, |header| {
                        header.child(
                            div()
                                .mt_4()
                                .flex()
                                .justify_between()
                                .text_xs()
                                .text_color(theme::text_secondary())
                                .child(self.tr("detail-time-remaining"))
                                .child(transfer.eta),
                        )
                    }),
            )
            .child(
                div()
                    .h(px(42.0))
                    .px_5()
                    .flex()
                    .items_end()
                    .gap_5()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(detail_tab(self.tr("detail-tab-details"), true)),
            )
            .child(components::inspector_body(
                "transfer-detail-body",
                &self.transfer_detail_scroll,
                div()
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .when(interrupted, |details| {
                        details.child(
                            div()
                                .debug_selector(|| "upload-history-guidance".to_owned())
                                .p_3()
                                .rounded(theme::RADIUS_SMALL)
                                .bg(theme::red_soft())
                                .flex()
                                .flex_col()
                                .gap_2()
                                .text_xs()
                                .text_color(theme::text_secondary())
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme::red())
                                        .child(self.tr("transfer-upload-interrupted")),
                                )
                                .child(self.tr("transfer-upload-interrupted-reason"))
                                .child(self.tr("transfer-upload-interrupted-action")),
                        )
                    })
                    .when_some(failure_reason, |details, (reason, guidance)| {
                        details.child(
                            div()
                                .debug_selector(|| "transfer-recovery-guidance".to_owned())
                                .mt_2()
                                .p_3()
                                .rounded(theme::RADIUS_SMALL)
                                .bg(theme::red_soft())
                                .flex()
                                .flex_col()
                                .gap_1()
                                .text_xs()
                                .text_color(theme::red())
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(self.tr("detail-failure-reason")),
                                )
                                .child(reason)
                                .child(div().text_color(theme::text_secondary()).child(guidance)),
                        )
                    })
                    .when_some(
                        vault_snapshot
                            .as_ref()
                            .and_then(|snapshot| snapshot.upload_activity.as_ref()),
                        |details, activity| {
                            details.child(self.render_upload_activity(activity, active, cx))
                        },
                    )
                    .children(
                        details
                            .into_iter()
                            .map(|(label, value)| detail_row(label, value)),
                    )
                    .when(preview_upload, |details| {
                        details.child(
                            div()
                                .mt_1()
                                .p_3()
                                .rounded(theme::RADIUS_SMALL)
                                .bg(theme::amber_soft())
                                .text_xs()
                                .text_color(theme::text_secondary())
                                .child(self.tr("transfer-preview-upload-note")),
                        )
                    })
                    .child(
                        div()
                            .mt_1()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .text_xs()
                            .child(self.tr("detail-verification"))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_color(verification_tone.foreground())
                                    .child(
                                        Icon::new(verification_icon)
                                            .text_color(verification_tone.foreground()),
                                    )
                                    .child(verification),
                            ),
                    )
                    .when_some(
                        telemetry.filter(|_| {
                            vault_snapshot.as_ref().is_none_or(|snapshot| {
                                snapshot.direction != VaultTransferDirection::Upload
                            })
                        }),
                        |details, telemetry| {
                            details.child(self.render_transfer_telemetry(telemetry, active, cx))
                        },
                    )
                    .when_some(runtime_snapshot, |details, snapshot| {
                        let events = snapshot.events.iter().enumerate().map(|(index, event)| {
                            let label = self.tr(match event.kind {
                                ChannelDownloadEventKind::CleanupWaiting => {
                                    "native-cleanup-waiting"
                                }
                                ChannelDownloadEventKind::CleanupRemoving => {
                                    "native-cleanup-removing"
                                }
                                ChannelDownloadEventKind::CleanupFailed => "native-cleanup-failed",
                                ChannelDownloadEventKind::CleanupFinished => {
                                    "native-cleanup-finished"
                                }
                                ChannelDownloadEventKind::CleanupRetryRequested => {
                                    "native-cleanup-retry-waiting"
                                }
                                ChannelDownloadEventKind::Queued => "trace-event-queued",
                                ChannelDownloadEventKind::Started => "trace-event-started",
                                ChannelDownloadEventKind::Paused => "trace-event-paused",
                                ChannelDownloadEventKind::Resumed => "trace-event-resumed",
                                ChannelDownloadEventKind::Completed => "trace-event-completed",
                                ChannelDownloadEventKind::Failed => "trace-event-failed",
                                ChannelDownloadEventKind::Cancelled => "trace-event-cancelled",
                            });
                            let timestamp =
                                format_unix_millis(self.locale(), event.timestamp_unix_ms);
                            let elapsed = event
                                .elapsed_ms
                                .map(|duration| format_duration_millis(self.locale(), duration))
                                .unwrap_or_else(|| "—".to_owned());
                            components::list_summary(
                                ("transfer-trace-event", index),
                                format!("{label} · {timestamp} · {elapsed}"),
                            )
                        });
                        let shown_parts = snapshot.part_events.len().min(20);
                        let omitted_parts = snapshot.part_events.omitted().saturating_add(
                            snapshot.part_events.len().saturating_sub(shown_parts) as u64,
                        );
                        let part_retention = self.tr_with(
                            "transfer-part-retention",
                            MessageArgs::new()
                                .with("shown", format_integer(self.locale(), shown_parts as u64))
                                .with("omitted", format_integer(self.locale(), omitted_parts)),
                        );
                        let part_events = snapshot
                            .part_events
                            .iter()
                            .rev()
                            .take(20)
                            .enumerate()
                            .map(|(index, event)| {
                                let state = self.tr(match event.state {
                                    DownloadPartState::Inflight => "transfer-part-inflight",
                                    DownloadPartState::Completed => "transfer-part-completed",
                                    DownloadPartState::Retry => "transfer-part-retry",
                                    DownloadPartState::Failed => "transfer-part-failed",
                                });
                                components::list_summary(
                                    ("transfer-part-event", index),
                                    self.tr_with(
                                        "transfer-part-event-value",
                                        MessageArgs::new()
                                            .with(
                                                "part",
                                                format_integer(self.locale(), event.part_index),
                                            )
                                            .with(
                                                "offset",
                                                format_integer(self.locale(), event.offset_bytes),
                                            )
                                            .with(
                                                "length",
                                                format_bytes(self.locale(), event.length_bytes),
                                            )
                                            .with("state", state.to_string())
                                            .with(
                                                "attempt",
                                                format_integer(
                                                    self.locale(),
                                                    u64::from(event.attempt),
                                                ),
                                            )
                                            .with(
                                                "elapsed",
                                                format_duration_millis(
                                                    self.locale(),
                                                    event.elapsed_millis,
                                                ),
                                            ),
                                    ),
                                )
                            });
                        details
                            .child(
                                div()
                                    .mt_2()
                                    .pt_3()
                                    .border_t_1()
                                    .border_color(theme::border())
                                    .child(
                                        div()
                                            .mb_1()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(self.tr("detail-trace-timeline")),
                                    )
                                    .when(snapshot.event_history_omitted > 0, |timeline| {
                                        timeline.child(
                                            div().text_xs().text_color(theme::text_muted()).child(
                                                self.tr_with(
                                                    "transfer-lifecycle-history-omitted",
                                                    MessageArgs::new().with(
                                                        "count",
                                                        format_integer(
                                                            self.locale(),
                                                            snapshot.event_history_omitted,
                                                        ),
                                                    ),
                                                ),
                                            ),
                                        )
                                    })
                                    .children(events),
                            )
                            .child(
                                div()
                                    .mt_2()
                                    .pt_3()
                                    .border_t_1()
                                    .border_color(theme::border())
                                    .child(
                                        div()
                                            .mb_1()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(self.tr("transfer-part-timeline")),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme::text_muted())
                                            .child(part_retention),
                                    )
                                    .children(part_events),
                            )
                    }),
            ))
            .into_any_element()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TransferAction {
    Resume,
    Pause,
    Retry,
    Cancel,
    Delete,
}

impl TransferAction {
    fn supports_snapshot(self, snapshot: &ChannelDownloadSnapshot) -> bool {
        if let Some(cleanup) = snapshot.cleanup {
            if cleanup.phase
                == ChannelDownloadCleanupPhase::Failed(ApplicationErrorKind::InvalidRequest)
            {
                return false;
            }
            return self == Self::Retry
                && (matches!(cleanup.phase, ChannelDownloadCleanupPhase::Failed(_))
                    || !cleanup.retry_requested);
        }
        self.supports(snapshot.state)
    }
    fn supports(self, state: ChannelDownloadState) -> bool {
        self.supports_view(transfer_state(state))
    }
    fn supports_view(self, state: TransferState) -> bool {
        match self {
            Self::Resume => state == TransferState::Paused,
            Self::Pause => matches!(state, TransferState::Waiting | TransferState::Downloading),
            Self::Retry => matches!(state, TransferState::Failed | TransferState::Cancelled),
            Self::Cancel => matches!(
                state,
                TransferState::Waiting | TransferState::Downloading | TransferState::Paused
            ),
            Self::Delete => matches!(
                state,
                TransferState::Completed | TransferState::Failed | TransferState::Cancelled
            ),
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Resume => "action-resume",
            Self::Pause => "action-pause",
            Self::Retry => "action-retry",
            Self::Cancel => "action-cancel",
            Self::Delete => "action-delete-task",
        }
    }
    fn element_id(self) -> &'static str {
        match self {
            Self::Resume => "row-resume",
            Self::Pause => "row-pause",
            Self::Retry => "row-retry",
            Self::Cancel => "row-cancel",
            Self::Delete => "row-delete",
        }
    }
    fn icon(self) -> IconName {
        match self {
            Self::Resume => IconName::ArrowRight,
            Self::Pause => IconName::Dash,
            Self::Retry => IconName::Redo2,
            Self::Cancel => IconName::CircleX,
            Self::Delete => IconName::Delete,
        }
    }
}

fn execute_transfer_actions(
    action: TransferAction,
    ids: &[u64],
    cancelled: &std::sync::atomic::AtomicBool,
    mut apply: impl FnMut(u64) -> Result<(), teleark_core::ApplicationError>,
) -> (Vec<u64>, Option<teleark_core::ApplicationErrorKind>) {
    let mut deleted = Vec::new();
    let mut failure = None;
    for id in ids {
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            break;
        }
        match apply(*id) {
            Ok(()) if action == TransferAction::Delete => deleted.push(*id),
            Ok(()) => {}
            Err(error) => {
                failure.get_or_insert(error.kind());
            }
        }
    }
    (deleted, failure)
}

// A collapsed batch selects its native child tasks. A set prevents double dispatch
// when both the group and expanded children are selected. Hidden selections never
// turn a visible-list action into an operation on another filter's tasks.
#[cfg(test)]
fn transfer_scope_ids(
    rows: &[TransferRow],
    selected: &std::collections::BTreeSet<u64>,
    snapshots: &[(u64, Option<u64>)],
) -> std::collections::BTreeSet<u64> {
    let has_selection = rows
        .iter()
        .enumerate()
        .any(|(index, row)| selected.contains(&transfer_selection_key(row, index)));
    rows.iter()
        .enumerate()
        .filter(|(index, row)| {
            !has_selection || selected.contains(&transfer_selection_key(row, *index))
        })
        .flat_map(|(_, row)| {
            if let Some(id) = row.runtime_task_id {
                vec![id]
            } else if let Some(batch) = row.runtime_batch_id {
                snapshots
                    .iter()
                    .filter(|(_, batch_id)| *batch_id == Some(batch))
                    .map(|(id, _)| *id)
                    .collect()
            } else {
                Vec::new()
            }
        })
        .collect()
}

fn transfer_header(label: impl Into<SharedString>, width: Option<f32>) -> AnyElement {
    div()
        .when_some(width, |cell, width| cell.w(px(width)))
        .when(width.is_none(), |cell| cell.flex_1())
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme::text_secondary())
        .child(label.into())
        .into_any_element()
}

fn transfer_tone(state: TransferState) -> Tone {
    match state {
        TransferState::Downloading | TransferState::Uploading => Tone::Blue,
        TransferState::Waiting | TransferState::Paused => Tone::Amber,
        TransferState::Completed => Tone::Green,
        TransferState::Failed | TransferState::Cancelled => Tone::Red,
    }
}

fn format_control_parameters(parameters: TransferControlParameters) -> String {
    format!(
        "C={} · W={} · F={} · P={} · E={} · Qe={}",
        parameters.transfer_connection_count,
        parameters.inflight_rpcs_per_connection,
        parameters.active_file_count,
        parameters.inflight_parts_per_file,
        parameters.encryption_worker_count,
        parameters.encrypted_part_queue_depth,
    )
}

fn control_parameter_value(
    parameters: TransferControlParameters,
    parameter: TunableParameter,
) -> u16 {
    match parameter {
        TunableParameter::TransferConnections => parameters.transfer_connection_count,
        TunableParameter::InflightRpcsPerConnection => parameters.inflight_rpcs_per_connection,
        TunableParameter::ActiveFiles => parameters.active_file_count,
        TunableParameter::InflightPartsPerFile => parameters.inflight_parts_per_file,
        TunableParameter::EncryptionWorkers => parameters.encryption_worker_count,
        TunableParameter::EncryptedQueueDepth => parameters.encrypted_part_queue_depth,
    }
}

const fn controller_phase_message_id(phase: ControllerPhase) -> &'static str {
    match phase {
        ControllerPhase::Ramp => "transfer-phase-ramp",
        ControllerPhase::Probe => "transfer-phase-probe",
        ControllerPhase::Stable => "transfer-phase-stable",
        ControllerPhase::Recover => "transfer-phase-recover",
    }
}

const fn bottleneck_message_id(bottleneck: TransferBottleneck) -> &'static str {
    match bottleneck {
        TransferBottleneck::Unknown => "transfer-bottleneck-unknown",
        TransferBottleneck::EncryptionCpu => "transfer-bottleneck-encryption",
        TransferBottleneck::TelegramOrNetwork => "transfer-bottleneck-network",
        TransferBottleneck::Disk => "transfer-bottleneck-disk",
        TransferBottleneck::Memory => "transfer-bottleneck-memory",
    }
}

const fn parameter_message_id(parameter: TunableParameter) -> &'static str {
    match parameter {
        TunableParameter::TransferConnections => "transfer-parameter-connections",
        TunableParameter::InflightRpcsPerConnection => "transfer-parameter-rpcs",
        TunableParameter::ActiveFiles => "transfer-parameter-files",
        TunableParameter::InflightPartsPerFile => "transfer-parameter-parts",
        TunableParameter::EncryptionWorkers => "transfer-parameter-encryption-workers",
        TunableParameter::EncryptedQueueDepth => "transfer-parameter-encrypted-queue",
    }
}

const fn decision_outcome_message_id(outcome: ControllerDecisionOutcome) -> &'static str {
    match outcome {
        ControllerDecisionOutcome::Probe => "transfer-decision-probe",
        ControllerDecisionOutcome::Keep => "transfer-decision-keep",
        ControllerDecisionOutcome::Confirm => "transfer-decision-confirm",
        ControllerDecisionOutcome::Platform => "transfer-decision-platform",
        ControllerDecisionOutcome::Rollback => "transfer-decision-rollback",
        ControllerDecisionOutcome::Recover => "transfer-decision-recover",
        ControllerDecisionOutcome::RespectSoftLimit => "transfer-decision-respect-soft-limit",
        ControllerDecisionOutcome::OverrideSoftLimit => "transfer-decision-override-soft-limit",
        ControllerDecisionOutcome::IgnoreSoftLimit => "transfer-decision-ignore-soft-limit",
        ControllerDecisionOutcome::PauseLane => "transfer-decision-pause-lane",
        ControllerDecisionOutcome::ResumeLane => "transfer-decision-resume-lane",
    }
}

const fn decision_reason_message_id(reason: ControllerDecisionReason) -> &'static str {
    match reason {
        ControllerDecisionReason::UserSettings => "transfer-reason-user-settings",
        ControllerDecisionReason::InitialRamp => "transfer-reason-initial-ramp",
        ControllerDecisionReason::InflightBelowBdpTarget => "transfer-reason-bdp",
        ControllerDecisionReason::ThroughputImproved => "transfer-reason-improved",
        ControllerDecisionReason::ThroughputNeedsConfirmation => "transfer-reason-confirm",
        ControllerDecisionReason::ThroughputGainBelowThreshold => "transfer-reason-platform",
        ControllerDecisionReason::ThroughputRegressed => "transfer-reason-regressed",
        ControllerDecisionReason::EncryptionStarvedNetwork => "transfer-reason-encryption-starved",
        ControllerDecisionReason::NetworkBackpressuredEncryption => {
            "transfer-reason-network-backpressure"
        }
        ControllerDecisionReason::SmallFileQueueNeedsSlots => "transfer-reason-small-files",
        ControllerDecisionReason::LargeFilePipelineNeedsParts => "transfer-reason-large-file",
        ControllerDecisionReason::MemoryBudgetPressure => "transfer-reason-memory",
        ControllerDecisionReason::DiskLimited => "transfer-reason-disk",
        ControllerDecisionReason::PartRetryRequired => "transfer-reason-part-retry",
        ControllerDecisionReason::FloodWaitRequired => "transfer-reason-flood-wait",
        ControllerDecisionReason::FloodWaitExpired => "transfer-reason-flood-wait-expired",
        ControllerDecisionReason::TelegramSoftLimitConflict => "transfer-reason-soft-limit",
        ControllerDecisionReason::AllParametersAtPlatform => "transfer-reason-all-platform",
    }
}

fn transfer_selection_key(transfer: &TransferRow, index: usize) -> u64 {
    transfer.runtime_task_id.unwrap_or_else(|| {
        if let Some(id) = transfer.vault_transfer_id {
            return 0x2000_0000_0000_0000_u64 | id;
        }
        transfer
            .vault_batch_id
            .map(vault_batch_key)
            .or_else(|| {
                transfer
                    .runtime_batch_id
                    .map(|id| 0x4000_0000_0000_0000_u64 | id)
            })
            .unwrap_or(0x8000_0000_0000_0000_u64 | index as u64)
    })
}

fn native_cleanup_message_id(phase: ChannelDownloadCleanupPhase) -> &'static str {
    match phase {
        ChannelDownloadCleanupPhase::WaitingForWriter => "native-cleanup-waiting",
        ChannelDownloadCleanupPhase::RemovingPartial => "native-cleanup-removing",
        ChannelDownloadCleanupPhase::Failed(_) => "native-cleanup-failed",
    }
}

fn native_display_state(snapshot: &ChannelDownloadSnapshot) -> ChannelDownloadState {
    match snapshot.cleanup.map(|cleanup| cleanup.phase) {
        Some(ChannelDownloadCleanupPhase::Failed(kind)) => ChannelDownloadState::Failed(kind),
        Some(_) => ChannelDownloadState::Queued,
        None => snapshot.state,
    }
}

fn transfer_state(state: ChannelDownloadState) -> TransferState {
    match state {
        ChannelDownloadState::Queued => TransferState::Waiting,
        ChannelDownloadState::Running => TransferState::Downloading,
        ChannelDownloadState::Paused => TransferState::Paused,
        ChannelDownloadState::Completed => TransferState::Completed,
        ChannelDownloadState::Failed(_) => TransferState::Failed,
        ChannelDownloadState::Cancelled => TransferState::Cancelled,
    }
}

fn upload_phase_message_id(phase: VaultUploadPhase) -> &'static str {
    match phase {
        VaultUploadPhase::RestartingUnsealed => "upload-phase-restarting-unsealed",
        VaultUploadPhase::RestartingExpired => "transfer-upload-restarting-expired",
        VaultUploadPhase::UpgradingUpload => "transfer-upload-upgrading",
        VaultUploadPhase::CheckingStorage => "transfer-upload-checking-storage",
        VaultUploadPhase::CheckingTarget => "transfer-upload-checking-target",
        VaultUploadPhase::CheckingSource => "transfer-upload-checking-source",
        VaultUploadPhase::SavingRecovery => "transfer-upload-saving-recovery",
        VaultUploadPhase::Preparing => "transfer-upload-reading-encrypting",
        VaultUploadPhase::SealingContainer => "transfer-upload-sealing",
        VaultUploadPhase::WaitingForTelegram => "transfer-upload-waiting-telegram",
        VaultUploadPhase::Downloading => "transfer-download-receiving-blocks",
        VaultUploadPhase::Uploading => "transfer-upload-sending-bytes",
        VaultUploadPhase::SendingMessage => "transfer-upload-confirming-message",
        VaultUploadPhase::Verifying => "transfer-upload-verifying-bytes",
        VaultUploadPhase::Publishing => "transfer-upload-publishing-manifest",
        VaultUploadPhase::Persisting => "transfer-upload-saving-manifest",
    }
}

fn vault_display_bytes(snapshot: &VaultTransferSnapshot) -> u64 {
    snapshot
        .upload_activity
        .as_ref()
        .map_or(snapshot.transferred_bytes, |activity| {
            snapshot
                .transferred_bytes
                .max(activity.uploaded_bytes)
                .min(snapshot.size_bytes)
        })
}

fn transfer_progress(transferred: u64, total: u64, completed: bool) -> f32 {
    if total == 0 {
        if completed { 100.0 } else { 0.0 }
    } else {
        (transferred as f64 * 100.0 / total as f64).clamp(0.0, if completed { 100.0 } else { 99.0 })
            as f32
    }
}

fn aggregate_batch_state(items: &[&ChannelDownloadSnapshot]) -> TransferState {
    aggregate_download_states(items.iter().map(|item| native_display_state(item)))
}

fn aggregate_download_states(
    states: impl IntoIterator<Item = ChannelDownloadState>,
) -> TransferState {
    let states = states.into_iter().collect::<Vec<_>>();
    if states.contains(&ChannelDownloadState::Running) {
        TransferState::Downloading
    } else if states.contains(&ChannelDownloadState::Queued) {
        TransferState::Waiting
    } else if states.contains(&ChannelDownloadState::Paused) {
        TransferState::Paused
    } else if states
        .iter()
        .any(|state| matches!(state, ChannelDownloadState::Failed(_)))
    {
        TransferState::Failed
    } else if states.contains(&ChannelDownloadState::Cancelled) {
        TransferState::Cancelled
    } else {
        TransferState::Completed
    }
}

fn current_unix_millis() -> Option<i64> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    i64::try_from(millis).ok()
}

fn vault_recovery_guidance_message_id(
    snapshot: &VaultTransferSnapshot,
    kind: teleark_core::ApplicationErrorKind,
) -> &'static str {
    use teleark_core::ApplicationErrorKind as Error;
    use teleark_runtime::VaultRecoveryState as Recovery;
    match snapshot.recovery_state {
        Some(Recovery::Retryable) => "transfer-recovery-retry-guidance",
        Some(Recovery::Blocked) => match kind {
            Error::SourceMissing | Error::SourceChanged | Error::SourcePermissionDenied => {
                "transfer-recovery-source-guidance"
            }
            Error::Authorization | Error::VaultKeyUnavailable => "transfer-recovery-key-guidance",
            _ => "transfer-recovery-blocked-guidance",
        },
        _ if snapshot.restored => "transfer-recovery-legacy-guidance",
        _ => "detail-failure-terminal",
    }
}

fn vault_transfer_error_message_id(
    kind: teleark_core::ApplicationErrorKind,
    phase: Option<VaultUploadPhase>,
) -> &'static str {
    use teleark_core::ApplicationErrorKind;
    match kind {
        ApplicationErrorKind::SourcePermissionDenied => "vault-transfer-error-source-permission",
        ApplicationErrorKind::StorageAccessDenied => "storage-health-access",
        ApplicationErrorKind::StorageConfigurationUnsafe => "storage-health-unsafe",
        ApplicationErrorKind::StorageIdentityDamaged => "storage-health-repair",
        ApplicationErrorKind::StorageIdentityUnsupported => "storage-health-unsupported",
        ApplicationErrorKind::VaultKeyUnavailable => "vault-health-key-unavailable",
        ApplicationErrorKind::PermissionDenied
            if matches!(
                phase,
                Some(VaultUploadPhase::CheckingStorage | VaultUploadPhase::CheckingTarget)
            ) =>
        {
            "vault-transfer-error-target-permission"
        }
        ApplicationErrorKind::PermissionDenied => "vault-transfer-error-permission",
        ApplicationErrorKind::InvalidRequest => "vault-transfer-error-invalid-request",
        ApplicationErrorKind::Conflict => "vault-transfer-error-conflict",
        ApplicationErrorKind::SourceChanged => "vault-transfer-error-source-changed",
        ApplicationErrorKind::NotFound | ApplicationErrorKind::SourceMissing => {
            "vault-error-source-missing"
        }
        ApplicationErrorKind::Persistence => "vault-error-persistence",
        ApplicationErrorKind::Capacity => "vault-error-capacity",
        ApplicationErrorKind::Authorization => "error-transfer-authorization",
        ApplicationErrorKind::Network => "error-transfer-network",
        ApplicationErrorKind::Server => "telegram-error-server",
        ApplicationErrorKind::Cancelled => "error-transfer-cancelled",
        _ => "vault-error-persistence",
    }
}

fn native_download_error_message_id(kind: teleark_core::ApplicationErrorKind) -> &'static str {
    use teleark_core::ApplicationErrorKind;

    match kind {
        ApplicationErrorKind::InvalidRequest => "native-download-error-invalid-request",
        ApplicationErrorKind::NotFound => "native-download-error-not-found",
        ApplicationErrorKind::Conflict => "native-download-error-conflict",
        ApplicationErrorKind::Persistence => "native-download-error-persistence",
        ApplicationErrorKind::SourceMissing => "native-download-error-source-missing",
        ApplicationErrorKind::SourceChanged => "native-download-error-source-changed",
        ApplicationErrorKind::PermissionDenied => "native-download-error-permission-denied",
        ApplicationErrorKind::Capacity => "native-download-error-capacity",
        ApplicationErrorKind::Authorization => "native-download-error-authorization",
        ApplicationErrorKind::Network => "native-download-error-network",
        ApplicationErrorKind::Cancelled => "native-download-error-cancelled",
        _ => "native-download-error-unknown",
    }
}

fn toggle_visible_selection(
    selected: &mut std::collections::BTreeSet<u64>,
    visible: &[u64],
    all_visible_selected: bool,
) {
    if all_visible_selected {
        for key in visible {
            selected.remove(key);
        }
    } else {
        selected.extend(visible.iter().copied());
    }
}

#[cfg(test)]
fn visible_transfer_rows(
    rows: Vec<TransferRow>,
    selection: &str,
    query: &str,
    expanded: &std::collections::BTreeSet<u64>,
) -> Vec<TransferRow> {
    let reveal_matching_members =
        !query.is_empty() || matches!(selection, "nav-waiting" | "nav-completed" | "nav-failed");
    rows.into_iter()
        .filter(|row| {
            (!row.batch_child
                || reveal_matching_members
                || row
                    .runtime_batch_id
                    .is_some_and(|id| expanded.contains(&id))
                || row
                    .vault_batch_id
                    .is_some_and(|id| expanded.contains(&vault_batch_key(id))))
                && transfer_matches_nav(selection, row.state, row.direction)
                && (query.is_empty()
                    || row.name.to_lowercase().contains(query)
                    || row.source.to_lowercase().contains(query)
                    || row.destination.to_lowercase().contains(query))
        })
        .collect()
}

fn transfer_matches_nav(
    selection: &str,
    state: TransferState,
    direction: TransferDirection,
) -> bool {
    match selection {
        "nav-uploads" => direction == TransferDirection::Upload,
        "nav-downloads" => direction == TransferDirection::Download,
        "nav-waiting" => matches!(state, TransferState::Waiting | TransferState::Paused),
        "nav-completed" => state == TransferState::Completed,
        "nav-failed" => matches!(state, TransferState::Failed | TransferState::Cancelled),
        _ => true,
    }
}

fn detail_tab(label: impl Into<SharedString>, selected: bool) -> AnyElement {
    div()
        .h_full()
        .pb_2()
        .flex()
        .items_end()
        .text_sm()
        .text_color(if selected {
            theme::blue()
        } else {
            theme::text_secondary()
        })
        .when(selected, |tab| tab.border_b_2().border_color(theme::blue()))
        .child(label.into())
        .into_any_element()
}

fn detail_row(label: SharedString, value: SharedString) -> AnyElement {
    let tooltip = format!("{label}: {value}");
    components::list_row()
        .id(label.clone())
        .child(
            div()
                .w(px(128.0))
                .flex_none()
                .truncate()
                .text_color(theme::text_muted())
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(theme::text_secondary())
                .child(value),
        )
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
        })
        .into_any_element()
}

fn vault_batch_key(id: u64) -> u64 {
    0x6000_0000_0000_0000 | id
}

fn aggregate_transfer_states(states: &[TransferState]) -> TransferState {
    [
        TransferState::Uploading,
        TransferState::Downloading,
        TransferState::Waiting,
        TransferState::Paused,
        TransferState::Failed,
        TransferState::Cancelled,
    ]
    .into_iter()
    .find(|state| states.contains(state))
    .unwrap_or(TransferState::Completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui_kit::test]
    fn recovery_failure_guidance_is_visible_before_history_fields(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use teleark_core::ApplicationErrorKind as Error;
        use teleark_runtime::VaultRecoveryState as Recovery;
        let (app, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        for (width, height) in [(900.0, 600.0), (1440.0, 900.0)] {
            cx.simulate_resize(gpui_kit::size(px(width), px(height)));
            for (state, kind, message) in [
                (
                    Some(Recovery::Retryable),
                    Error::Network,
                    "transfer-recovery-retry-guidance",
                ),
                (
                    Some(Recovery::Blocked),
                    Error::SourceChanged,
                    "transfer-recovery-source-guidance",
                ),
                (
                    Some(Recovery::Blocked),
                    Error::VaultKeyUnavailable,
                    "transfer-recovery-key-guidance",
                ),
                (
                    None,
                    Error::InvalidRequest,
                    "transfer-recovery-legacy-guidance",
                ),
            ] {
                app.update(cx, |app, cx| {
                    let mut snapshot = vault_snapshot_fixture();
                    snapshot.account_id = app.telegram_account.as_ref().expect("account").id;
                    snapshot.batch_id = None;
                    snapshot.restored = true;
                    snapshot.state = VaultTransferState::Failed(kind);
                    snapshot.recovery_state = state;
                    assert_eq!(vault_recovery_guidance_message_id(&snapshot, kind), message);
                    let row = app.transfer_row_from_vault_snapshot(&snapshot);
                    app.focused_transfer_key = Some(transfer_selection_key(&row, 0));
                    app.preview_transfer_rows = vec![row];
                    app.nav_selection = "nav-uploads";
                    app.vault_locked = false;
                    app.vault_transfer_view.items = vec![std::sync::Arc::new(snapshot)].into();
                    app.native_transfer_view.items = Vec::new().into();
                    app.selected_file = 0;
                    app.show_transfer_detail = true;
                    cx.notify();
                });
                cx.run_until_parked();
                let panel = cx.debug_bounds("transfer-inspector").expect("inspector");
                let guidance = cx
                    .debug_bounds("transfer-recovery-guidance")
                    .expect("guidance");
                assert!(guidance.top() >= panel.top() && guidance.bottom() <= panel.bottom());
            }
        }
    }

    #[gpui_kit::test]
    fn restored_downloads_explain_unknown_progress_and_unavailable_context(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        app.update(cx, |app, _| {
            let mut snapshot = vault_snapshot_fixture();
            snapshot.direction = VaultTransferDirection::Download;
            snapshot.restored = true;
            snapshot.upload_activity = None;
            snapshot.state = VaultTransferState::Paused;
            let row = app.transfer_row_from_vault_snapshot(&snapshot);
            assert_eq!(row.transferred, app.tr("transfer-value-unavailable"));
            assert_eq!(
                row.activity_detail,
                Some(app.tr("transfer-recovery-verification-pending"))
            );
            assert_eq!(row.state, TransferState::Paused);
            snapshot.file_name.clear();
            snapshot.state =
                VaultTransferState::Failed(teleark_core::ApplicationErrorKind::InvalidRequest);
            let unavailable = app.transfer_row_from_vault_snapshot(&snapshot);
            assert_eq!(unavailable.name, app.tr("transfer-recovery-saved-download"));
            assert_eq!(unavailable.size, app.tr("transfer-value-unavailable"));
            assert_eq!(
                unavailable.activity_detail,
                Some(app.tr("transfer-recovery-unavailable-detail"))
            );
        });
    }

    #[gpui_kit::test]
    fn missing_key_does_not_hide_transfer_names_paths_progress_or_search(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        app.update(cx, |app, _| {
            let mut snapshot = vault_snapshot_fixture();
            snapshot.destination = Some("/synthetic/private-output.bin".into());
            snapshot.package_id = Some("private-package".into());
            snapshot.transferred_bytes = 4096;
            let item = TransferItem::Vault(std::sync::Arc::new(snapshot), false);
            app.vault_locked = false;
            let visible = item.row(app);
            app.vault_locked = true;
            let locked = item.row(app);
            assert_eq!(locked.name, visible.name);
            assert_eq!(locked.destination, visible.destination);
            assert_eq!(locked.caption, visible.caption);
            assert_eq!(locked.progress, visible.progress);
            assert_eq!(locked.activity, visible.activity);
            assert_eq!(locked.state, visible.state);
            let rows = projection::visible_items(
                std::slice::from_ref(&item),
                app,
                "nav-transfers-all",
                "fixture",
                &Default::default(),
            );
            assert_eq!(rows.len(), 1);
            app.vault_locked = false;
            assert_eq!(item.row(app).name, visible.name);
        });
    }

    fn vault_snapshot_fixture() -> VaultTransferSnapshot {
        VaultTransferSnapshot {
            recovery_state: None,
            restored: false,
            upload_activity: Some(VaultUploadActivity::new(VaultUploadPhase::Preparing)),
            id: 1,
            account_id: 7,
            chat_id: 90,
            batch_id: Some(1),
            queued_at_unix_ms: 1,
            direction: VaultTransferDirection::Upload,
            file_name: "京都 — fixture.bin".into(),
            package_id: None,
            size_bytes: 60 * 1024 * 1024,
            transferred_bytes: 0,
            completed_parts: 0,
            part_count: 1,
            started_at_unix_ms: 1,
            duration_ms: None,
            average_bytes_per_second: None,
            destination: None,
            session_log_path: None,
            telemetry: TransferTelemetrySnapshot {
                phase: ControllerPhase::Ramp,
                parameters: TransferControlParameters::conservative_upload(),
                goodput_bytes_per_second: 0,
                encryption_bytes_per_second: 0,
                disk_bytes_per_second: 0,
                round_trip_time_p95_millis: 0,
                estimated_bdp_bytes: 0,
                inflight_bytes: 0,
                target_inflight_bytes: 0,
                cpu_utilization_basis_points: 0,
                encrypted_queue_length: 0,
                network_waiting_for_encryption_millis: 0,
                encryption_waiting_for_network_millis: 0,
                bottleneck: TransferBottleneck::Unknown,
                parts: Default::default(),
                queues: Default::default(),
                memory: Default::default(),
                memory_budget_bytes: 512 * 1024 * 1024,
                lanes: vec![],
                decisions: vec![],
            },
            state: VaultTransferState::Running,
        }
    }

    fn native_snapshot_fixture(id: u64, account: i64) -> ChannelDownloadSnapshot {
        let vault = vault_snapshot_fixture();
        ChannelDownloadSnapshot {
            cleanup: None,
            account_id: Some(account),
            id,
            batch_id: None,
            chat_id: 90,
            message_id: id as i64,
            message_sent_at_unix_ms: Some(1),
            file_name: format!("京都 project file {id}.bin"),
            caption: Some("Synthetic benchmark caption".into()),
            mime_type: None,
            size_bytes: 1_024,
            transferred_bytes: 1_024,
            destination: format!("/tmp/teleark-preview/output-{id}.bin").into(),
            state: ChannelDownloadState::Completed,
            verification: ChannelDownloadVerification::SizeChecked,
            queued_at_unix_ms: 1,
            started_at_unix_ms: Some(1),
            finished_at_unix_ms: Some(2),
            queue_wait_ms: Some(0),
            duration_ms: Some(1),
            acknowledged_bytes: 0,
            current_bytes_per_second: None,
            average_bytes_per_second: None,
            eta_ms: None,
            attempts: 1,
            failure: None,
            events: vec![],
            event_history_omitted: 0,
            part_events: Default::default(),
            session_log_path: None,
            telemetry: vault.telemetry,
        }
    }

    fn populate_large_transfer_view(app: &mut TeleArkApp, count: u64) {
        app.visual_preview = false;
        let account = app.telegram_account.as_ref().expect("fixture account").id;
        app.native_transfer_view = teleark_runtime::TransferSnapshotView {
            omitted_items: 0,
            revision: 1,
            items: (0..count)
                .map(|id| std::sync::Arc::new(native_snapshot_fixture(id + 1, account)))
                .collect(),
            ..Default::default()
        };
    }

    #[gpui_kit::test]
    fn ten_thousand_transfers_format_only_the_visible_window(cx: &mut gpui_kit::TestAppContext) {
        let (entity, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        cx.simulate_resize(gpui_kit::size(px(900.0), px(600.0)));
        entity.update(cx, |app, cx| {
            populate_large_transfer_view(app, 10_000);
            let first = app.transfer_items();
            assert_eq!(first.len(), 10_000);
            assert!(std::sync::Arc::ptr_eq(&first, &app.transfer_items()));
            projection::reset_materialized_rows();
            cx.notify();
        });
        cx.run_until_parked();
        let formatted = projection::materialized_rows();
        assert!(
            formatted > 0 && formatted < 200,
            "formatted {formatted} rows for a compact viewport"
        );
    }

    #[gpui_kit::test]
    fn batch_eta_uses_all_live_receipt_rates_and_ignores_stale_snapshot_speed(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (entity, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        entity.update(cx, |app, _| {
            let account = app.telegram_account.as_ref().expect("account").id;
            let mut first = native_snapshot_fixture(1, account);
            first.size_bytes = 1000;
            first.transferred_bytes = 0;
            first.state = ChannelDownloadState::Running;
            first.current_bytes_per_second = Some(999_999);
            let mut second = first.clone();
            second.id = 2;
            let measured = teleark_runtime::TransferRate {
                bytes_per_second: Some(200),
                logical_bytes_per_second: Some(100),
                ..Default::default()
            };
            app.native_transfer_view.rates =
                std::sync::Arc::new([(1, measured), (2, measured)].into());
            let row = app.transfer_row_from_batch(1, &[&first, &second]);
            assert_eq!(row.speed, format_speed(app.locale(), 400));
            assert_eq!(row.eta, format_duration_millis(app.locale(), 10_000));
            app.native_transfer_view.rates = Default::default();
            first.state = ChannelDownloadState::Paused;
            second.state = ChannelDownloadState::Paused;
            let row = app.transfer_row_from_batch(1, &[&first, &second]);
            assert_eq!(row.speed, app.tr("transfer-value-unavailable"));
            assert_eq!(row.eta, app.tr("transfer-value-unavailable"));
        });
    }

    #[gpui_kit::test]
    fn rate_refresh_reuses_filtered_ten_thousand_row_projection_and_updates_visible_values(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (entity, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        entity.update(cx, |app, _| {
            populate_large_transfer_view(app, 10_000);
            let items = app.transfer_items();
            let first =
                app.transfer_projection_cache
                    .borrow_mut()
                    .presentation(app, items.clone(), "");
            for _ in 0..100 {
                let items = app.transfer_items();
                let next = app
                    .transfer_projection_cache
                    .borrow_mut()
                    .presentation(app, items, "");
                assert!(
                    std::rc::Rc::ptr_eq(&first, &next),
                    "ordinary samples reuse counts, keys, filters and bulk scopes"
                );
            }
            let original = app.native_transfer_view.items[0].clone();
            let mut changed = original.as_ref().clone();
            changed.transferred_bytes = 512;
            app.native_transfer_view.updates =
                std::sync::Arc::new([(changed.id, std::sync::Arc::new(changed))].into());
            app.native_transfer_view.revision += 1;
            let items = app.transfer_items();
            let next = app
                .transfer_projection_cache
                .borrow_mut()
                .presentation(app, items, "");
            assert!(std::rc::Rc::ptr_eq(&first, &next));
            assert_eq!(
                app.transfer_row_from_snapshot(&original, false).transferred,
                format_bytes(app.locale(), 512)
            );
        });
    }

    #[gpui_kit::test]
    fn lazy_transfer_filters_and_bulk_scope_match_displayed_rows(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (entity, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        entity.update(cx, |app, _| {
            populate_large_transfer_view(app, 3);
            let account = app.telegram_account.as_ref().expect("account").id;
            let mut native = (1..=3)
                .map(|id| native_snapshot_fixture(id, account))
                .collect::<Vec<_>>();
            native[0].batch_id = Some(9);
            native[1].batch_id = Some(9);
            native[1].state =
                ChannelDownloadState::Failed(teleark_core::ApplicationErrorKind::Network);
            app.native_transfer_view.items = native.into_iter().map(std::sync::Arc::new).collect();
            let mut vault = vault_snapshot_fixture();
            vault.account_id = account;
            app.vault_transfer_view.items = vec![std::sync::Arc::new(vault)].into();
            {
                let locale = teleark_i18n::SupportedLocale::EnUs;
                app.localizer.set_locale(locale);
                let items = app.transfer_items();
                let rows = app.transfer_rows();
                let source = app.tr("storage-channel-title").to_lowercase();
                for selection in [
                    "nav-transfers-all",
                    "nav-waiting",
                    "nav-completed",
                    "nav-failed",
                ] {
                    for query in ["", "京都", "output-3", source.as_ref()] {
                        for expanded in [
                            std::collections::BTreeSet::new(),
                            std::collections::BTreeSet::from([9, vault_batch_key(1)]),
                        ] {
                            let visible =
                                projection::visible_items(&items, app, selection, query, &expanded);
                            let legacy =
                                visible_transfer_rows(rows.clone(), selection, query, &expanded);
                            assert_eq!(
                                visible
                                    .iter()
                                    .enumerate()
                                    .map(|(i, item)| item.key(i))
                                    .collect::<Vec<_>>(),
                                legacy
                                    .iter()
                                    .enumerate()
                                    .map(|(i, row)| transfer_selection_key(row, i))
                                    .collect::<Vec<_>>()
                            );
                            let selected =
                                std::collections::BTreeSet::from([0x4000_0000_0000_0009, 2, 99]);
                            let tasks = [(1, Some(9)), (2, Some(9)), (3, None)];
                            assert_eq!(
                                projection::scope_ids(&visible, &selected, &tasks),
                                transfer_scope_ids(&legacy, &selected, &tasks)
                            );
                        }
                    }
                }
            }
        });
    }

    #[gpui_kit::test]
    #[ignore = "manual before/after performance measurement"]
    fn perf_transfer_virtualization(cx: &mut gpui_kit::TestAppContext) {
        use std::{hint::black_box, time::Instant};
        let (entity, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        entity.update(cx, |app, _| {
            populate_large_transfer_view(app, 10_000);
            let started = Instant::now();
            for _ in 0..10 {
                black_box(app.transfer_rows());
            }
            let before = started.elapsed();
            let started = Instant::now();
            for _ in 0..10 {
                let items = app.transfer_items();
                let visible = projection::visible_items(
                    &items,
                    app,
                    "nav-transfers-all",
                    "",
                    &std::collections::BTreeSet::new(),
                );
                for item in visible.iter().take(20) {
                    black_box(item.row(app));
                }
            }
            println!(
                "transfer_virtualization tasks=10000 frames=10 visible=20 old_us={} new_us={}",
                before.as_micros(),
                started.elapsed().as_micros()
            );
        });
    }

    #[gpui_kit::test]
    fn cached_transfer_projection_survives_unavailable_runtime_and_rejects_other_accounts(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (entity, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        entity.update(cx, |app, cx| {
            app.visual_preview = false;
            app.vault = None;
            let mut row = vault_snapshot_fixture();
            row.batch_id = None;
            row.account_id = app.telegram_account.as_ref().expect("fixture account").id;
            let mut other = row.clone();
            other.id = 2;
            other.account_id += 1;
            app.vault_transfer_view = teleark_runtime::TransferSnapshotView {
                omitted_items: 0,
                revision: 1,
                items: vec![std::sync::Arc::new(row.clone()), std::sync::Arc::new(other)].into(),
                ..Default::default()
            };
            let rows = app.transfer_rows();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].name.as_ref(), row.file_name);
            assert!(std::sync::Arc::ptr_eq(
                &app.vault_transfer_snapshot(1).expect("cached snapshot"),
                &app.vault_transfer_view.items[0]
            ));
            row.file_name = "Updated cached title".into();
            app.vault_transfer_view = teleark_runtime::TransferSnapshotView {
                omitted_items: 0,
                revision: 2,
                items: vec![std::sync::Arc::new(row)].into(),
                ..Default::default()
            };
            assert_eq!(app.transfer_rows()[0].name.as_ref(), "Updated cached title");
            cx.notify();
        });
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn upload_phases_remain_visible_in_collapsed_batches_and_terminal_rows_are_final(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        cx.simulate_resize(gpui_kit::size(px(900.0), px(600.0)));
        let mut completed = vault_snapshot_fixture();
        completed.state = VaultTransferState::Completed;
        completed.transferred_bytes = completed.size_bytes;
        let mut active = completed.clone();
        active.id = 2;
        active.state = VaultTransferState::Running;
        active.transferred_bytes = 0;
        for phase in [
            VaultUploadPhase::CheckingStorage,
            VaultUploadPhase::CheckingTarget,
            VaultUploadPhase::Preparing,
            VaultUploadPhase::WaitingForTelegram,
            VaultUploadPhase::Uploading,
            VaultUploadPhase::SendingMessage,
            VaultUploadPhase::Verifying,
            VaultUploadPhase::Publishing,
        ] {
            let mut activity = VaultUploadActivity::new(phase);
            activity.uploaded_bytes = active.size_bytes / 2;
            activity.bytes = 512 * 1024;
            activity.total = 64 * 1024 * 1024;
            active.upload_activity = Some(activity);
            app.update(cx, |app, cx| {
                let row = app.transfer_row_from_vault_snapshot(&active);
                assert_eq!(row.progress, 50.0);
                assert_eq!(row.activity, Some(app.tr(upload_phase_message_id(phase))));
                assert_eq!(
                    row.activity_detail
                        .as_ref()
                        .expect("phase detail")
                        .contains("Current container"),
                    phase == VaultUploadPhase::Uploading,
                    "only upload byte counts describe the current container"
                );
                let batch = app
                    .transfer_row_from_vault_batch(1, &[&completed, &active])
                    .expect("batch row");
                assert_eq!(batch.activity, row.activity);
                app.preview_transfer_rows = vec![batch];
                cx.notify();
            });
            cx.run_until_parked();
            assert!(cx.debug_bounds("upload-preflight-status").is_some());
        }
        app.update(cx, |app, cx| {
            active
                .upload_activity
                .as_mut()
                .expect("upload activity")
                .uploaded_bytes = active.size_bytes;
            assert_eq!(app.transfer_row_from_vault_snapshot(&active).progress, 99.0);
            active.state = VaultTransferState::Failed(teleark_core::ApplicationErrorKind::Network);
            let row = app.transfer_row_from_vault_snapshot(&active);
            assert!(row.activity.is_none());
            assert!(row.activity_detail.is_none());
            assert_eq!(row.state, TransferState::Failed);
            app.preview_transfer_rows = vec![row];
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("upload-preflight-status").is_none());
        assert_eq!(transfer_progress(100, 100, false), 99.0);
        assert_eq!(transfer_progress(100, 100, true), 100.0);
    }

    #[gpui_kit::test]
    fn restored_uploads_keep_batches_and_honest_interruption_details(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        cx.simulate_resize(gpui_kit::size(px(900.0), px(600.0)));
        for appearance in [
            teleark_runtime::AppearancePreference::Light,
            teleark_runtime::AppearancePreference::Dark,
        ] {
            app.update(cx, |app, cx| {
                assert_eq!(app.locale(), teleark_i18n::SupportedLocale::EnUs);
                app.preferences.appearance = appearance;
                app.preview_upload_history();
                let items = app.transfer_rows();
                assert_eq!(items.len(), 3);
                assert!(items[0].batch_summary.is_some());
                assert!(items[1].batch_child && items[2].batch_child);
                assert_eq!(items[1].progress, 100.0);
                assert_eq!(
                    items[2].activity,
                    Some(app.tr("transfer-upload-interrupted"))
                );
                assert_eq!(items[2].state, TransferState::Failed);
                assert!(items[2].progress < 100.0);
                assert_eq!(items[2].connections, app.tr("transfer-value-unavailable"));
                assert_eq!(app.vault_transfer_view.omitted_items, 12);
                assert!(app.vault_transfer_view.items.iter().all(|row| !matches!(
                    row.state,
                    VaultTransferState::Queued | VaultTransferState::Running
                )));
                // Real projection also filters historical account identity, independently of preview rows.
                app.visual_preview = false;
                assert_eq!(app.transfer_rows().len(), 3);
                app.telegram_account.as_mut().expect("account").id += 1;
                assert!(app.transfer_rows().is_empty());
                app.telegram_account.as_mut().expect("account").id -= 1;
                app.visual_preview = true;
                cx.notify();
            });
            cx.run_until_parked();
            let panel = cx.debug_bounds("transfer-inspector").expect("inspector");
            let guidance = cx
                .debug_bounds("upload-history-guidance")
                .expect("interruption guidance");
            assert!(
                guidance.top() >= panel.top() && guidance.bottom() <= panel.bottom(),
                "interruption guidance must be reachable without scrolling at 900x600"
            );
            assert!(
                cx.debug_bounds("upload-preflight-status").is_none(),
                "interrupted history must not look like ongoing preparation"
            );
        }
    }

    #[gpui_kit::test]
    fn batch_rows_are_34_pixels_and_file_rows_are_24_pixels(cx: &mut gpui_kit::TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        for (width, height) in [(900.0, 600.0), (1360.0, 760.0)] {
            cx.simulate_resize(gpui_kit::size(px(width), px(height)));
            {
                let locale = teleark_i18n::SupportedLocale::EnUs;
                for appearance in [
                    teleark_runtime::AppearancePreference::Light,
                    teleark_runtime::AppearancePreference::Dark,
                ] {
                    let keys = app.update(cx, |app, cx| {
                        app.localizer = teleark_i18n::Localizer::new(locale).expect("catalog");
                        app.preferences.appearance = appearance;
                        app.expanded_transfer_batches.insert(42);
                        cx.notify();
                        app.transfer_rows()
                            .iter()
                            .take(2)
                            .enumerate()
                            .map(|(index, row)| transfer_selection_key(row, index))
                            .collect::<Vec<_>>()
                    });
                    cx.run_until_parked();
                    let mut previous_bottom = None;
                    for (index, key) in keys.into_iter().enumerate() {
                        let selector: &'static str =
                            Box::leak(format!("transfer-row-{key}").into_boxed_str());
                        let bounds = cx.debug_bounds(selector).expect("rendered transfer row");
                        assert_eq!(
                            bounds.size.height,
                            if index == 0 {
                                theme::BATCH_ROW_HEIGHT
                            } else {
                                theme::ROW_HEIGHT
                            }
                        );
                        if let Some(bottom) = previous_bottom {
                            assert_eq!(bounds.top(), bottom);
                        }
                        previous_bottom = Some(bounds.bottom());
                    }
                    let progress = cx
                        .debug_bounds("batch-progress-4611686018427387946")
                        .expect("batch counts");
                    assert!(progress.size.width > px(100.0));
                    assert!(progress.size.height >= px(14.0));
                }
            }
        }
    }

    fn populate_native_batch(app: &mut TeleArkApp, count: u64) {
        populate_large_transfer_view(app, count);
        app.native_transfer_view.items = app
            .native_transfer_view
            .items
            .iter()
            .map(|row| {
                let mut row = (**row).clone();
                row.batch_id = Some(7);
                std::sync::Arc::new(row)
            })
            .collect();
    }

    #[gpui_kit::test]
    fn singleton_upload_and_download_have_no_batch_header(cx: &mut gpui_kit::TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        app.update(cx, |app, _| {
            populate_native_batch(app, 1);
            let mut upload = vault_snapshot_fixture();
            upload.account_id = app.telegram_account.as_ref().expect("account").id;
            app.vault_transfer_view.items = vec![std::sync::Arc::new(upload)].into();
            let rows = app.transfer_items();
            assert_eq!(rows.len(), 2);
            assert!(
                rows.iter()
                    .all(|row| !row.child() && row.batch_count() == 0)
            );
            assert!(rows.iter().all(|row| row.row(app).batch_summary.is_none()));
        });
    }

    #[gpui_kit::test]
    fn batch_click_routes_nine_inline_ten_to_one_live_window(cx: &mut gpui_kit::TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        cx.simulate_resize(gpui_kit::size(px(900.0), px(600.0)));
        app.update(cx, |app, cx| {
            populate_native_batch(app, 9);
            assert!(app.expanded_transfer_batches.is_empty());
            let items = app.transfer_items();
            let visible = projection::visible_items(
                &items,
                app,
                "nav-transfers-all",
                "",
                &app.expanded_transfer_batches,
            );
            assert_eq!(
                visible.len(),
                1,
                "new batches initially show only their header"
            );
            assert!(!visible[0].child());
            cx.notify();
        });
        cx.run_until_parked();
        let header = cx
            .debug_bounds("transfer-row-4611686018427387911")
            .expect("nine-file header");
        cx.simulate_click(header.center(), Default::default());
        cx.run_until_parked();
        app.update(cx, |app, _| {
            assert!(app.expanded_transfer_batches.contains(&7));
            assert!(!app.show_transfer_detail);
            assert!(app.transfer_batch_window.is_none());
        });
        assert_eq!(cx.windows().len(), 1);
        cx.simulate_click(header.center(), Default::default());
        cx.run_until_parked();
        app.update(cx, |app, cx| {
            assert!(!app.expanded_transfer_batches.contains(&7));
            populate_native_batch(app, 10);
            cx.notify();
        });
        cx.run_until_parked();
        cx.simulate_click(header.center(), Default::default());
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 2);
        let handle = app.update(cx, |app, _| {
            assert!(!app.expanded_transfer_batches.contains(&7));
            app.transfer_batch_window.expect("independent window").1
        });
        let mut child = gpui_kit::VisualTestContext::from_window(handle.into(), cx);
        assert!(child.debug_bounds("transfer-batch-window").is_some());
        let group = child
            .debug_bounds("transfer-batch-group")
            .expect("shared group surface");
        let heading = child
            .debug_bounds("transfer-batch-window-header")
            .expect("batch title");
        let rail = child
            .debug_bounds("transfer-batch-window-rail")
            .expect("continuous group marker");
        let first_member = child.debug_bounds("transfer-row-10").expect("first member");
        assert_eq!(heading.size.height, theme::BATCH_ROW_HEIGHT);
        assert_eq!(first_member.top(), heading.bottom());
        assert_eq!(first_member.size.height, theme::ROW_HEIGHT);
        assert!(rail.top() < heading.bottom() && rail.bottom() > first_member.bottom());
        assert_eq!(rail.size.width, theme::BATCH_RAIL_WIDTH);
        assert!(group.contains(&heading.origin) && group.contains(&first_member.origin));
        let child_bounds = child.update(|window, _| window.bounds());
        assert!(child_bounds.size.width < px(900.0));
        assert!(child_bounds.size.height < px(600.0));
        // Clicking the same group reuses its existing window.
        cx.simulate_click(header.center(), Default::default());
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 2);
        // A revised task projection is reflected in the already-open child.
        app.update(cx, |app, cx| {
            let mut rows = app.native_transfer_view.items.to_vec();
            let mut changed = (*rows[9]).clone();
            changed.id = 1010;
            changed.file_name = "updated-in-open-window.bin".into();
            rows[9] = std::sync::Arc::new(changed);
            app.native_transfer_view.items = rows.into();
            cx.notify();
        });
        child.run_until_parked();
        assert!(child.debug_bounds("transfer-row-1010").is_some());
        // Closing the auxiliary window does not remove or cancel tasks.
        child.update(|window, _| window.remove_window());
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1);
        app.update(cx, |app, _| {
            assert_eq!(app.native_transfer_view.items.len(), 10)
        });
        cx.simulate_click(header.center(), Default::default());
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 2);
        // Account changes must invalidate the detached view.
        app.update(cx, |app, cx| {
            app.telegram_account = None;
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1);
    }

    #[gpui_kit::test]
    fn large_batch_window_materializes_only_visible_members(cx: &mut gpui_kit::TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        cx.simulate_resize(gpui_kit::size(px(900.0), px(600.0)));
        app.update(cx, |app, cx| {
            populate_native_batch(app, 10_000);
            cx.notify();
        });
        cx.run_until_parked();
        projection::reset_materialized_rows();
        let header = cx
            .debug_bounds("transfer-row-4611686018427387911")
            .expect("large header");
        cx.simulate_click(header.center(), Default::default());
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 2);
        let count = projection::materialized_rows();
        assert!(
            count > 0 && count < 200,
            "formatted {count} rows for a 10,000-member batch"
        );
        // The main list remains collapsed despite the large number of children.
        app.update(cx, |app, _| {
            assert!(!app.expanded_transfer_batches.contains(&7))
        });
        let weak = app.downgrade();
        cx.update(|window, _| window.remove_window());
        drop(app);
        cx.run_until_parked();
        assert!(
            weak.upgrade().is_none(),
            "the parent owner must be released"
        );
        assert!(
            cx.windows().is_empty(),
            "the member window must not retain its parent owner"
        );
    }

    #[test]
    fn searching_or_filtering_reaches_members_of_collapsed_batches() {
        let mut group = transfers(false).remove(0);
        group.runtime_batch_id = Some(7);
        group.state = TransferState::Failed;
        let mut completed = group.clone();
        completed.runtime_task_id = Some(11);
        completed.batch_child = true;
        completed.name = "京都 — hidden member.pdf".into();
        completed.state = TransferState::Completed;
        let mut failed = completed.clone();
        failed.runtime_task_id = Some(12);
        failed.name = "failed member.pdf".into();
        failed.state = TransferState::Failed;
        let rows = vec![group, completed.clone(), failed];
        let collapsed = std::collections::BTreeSet::new();
        assert_eq!(
            visible_transfer_rows(rows.clone(), "nav-transfers-all", "", &collapsed).len(),
            1
        );
        let found = visible_transfer_rows(
            rows.clone(),
            "nav-transfers-all",
            "hidden member",
            &collapsed,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(transfer_selection_key(&found[0], 0), 11);
        let found = visible_transfer_rows(rows, "nav-completed", "", &collapsed);
        assert_eq!(found.len(), 1);
        assert_eq!(
            transfer_selection_key(&found[0], 0),
            transfer_selection_key(&completed, 7)
        );
    }

    #[test]
    fn bulk_commands_continue_after_failure_and_only_clear_successful_deletions() {
        use teleark_core::{ApplicationError, ApplicationErrorKind};
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let mut called = Vec::new();
        let (deleted, error) =
            execute_transfer_actions(TransferAction::Delete, &[1, 2, 3], &cancelled, |id| {
                called.push(id);
                if id == 2 {
                    Err(ApplicationError::new(ApplicationErrorKind::Persistence))
                } else {
                    Ok(())
                }
            });
        assert_eq!(called, vec![1, 2, 3]);
        assert_eq!(deleted, vec![1, 3]);
        assert_eq!(error, Some(ApplicationErrorKind::Persistence));
    }

    #[test]
    fn command_owner_cancellation_stops_at_task_boundaries() {
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let mut called = Vec::new();
        let (deleted, error) =
            execute_transfer_actions(TransferAction::Pause, &[1, 2, 3], &cancelled, |id| {
                called.push(id);
                cancelled.store(true, std::sync::atomic::Ordering::Release);
                Ok(())
            });
        assert_eq!(called, vec![1]);
        assert!(deleted.is_empty());
        assert_eq!(error, None);
    }

    #[test]
    fn batch_selection_expands_children_once_and_ignores_hidden_selection() {
        let mut group = transfers(false).remove(0);
        group.runtime_task_id = None;
        group.runtime_batch_id = Some(7);
        let mut child = group.clone();
        child.runtime_task_id = Some(11);
        child.batch_child = true;
        let mut single = child.clone();
        single.runtime_task_id = Some(13);
        single.runtime_batch_id = None;
        single.batch_child = false;
        let rows = vec![group, child, single];
        let tasks = [(11, Some(7)), (12, Some(7)), (13, None), (99, None)];
        let group_key = transfer_selection_key(&rows[0], 0);
        let selected = std::collections::BTreeSet::from([group_key, 11, 99]);
        assert_eq!(
            transfer_scope_ids(&rows, &selected, &tasks),
            std::collections::BTreeSet::from([11, 12])
        );
        assert_eq!(
            transfer_scope_ids(&rows, &std::collections::BTreeSet::from([13]), &tasks),
            std::collections::BTreeSet::from([13])
        );
        assert_eq!(
            transfer_scope_ids(&rows, &std::collections::BTreeSet::from([99]), &tasks),
            std::collections::BTreeSet::from([11, 12, 13])
        );
        assert!(transfer_scope_ids(&[], &selected, &tasks).is_empty());
    }

    #[test]
    fn action_availability_matches_native_lifecycle() {
        use teleark_core::ApplicationErrorKind;
        for state in [
            ChannelDownloadState::Queued,
            ChannelDownloadState::Running,
            ChannelDownloadState::Paused,
            ChannelDownloadState::Completed,
            ChannelDownloadState::Failed(ApplicationErrorKind::Network),
            ChannelDownloadState::Cancelled,
        ] {
            assert_eq!(
                TransferAction::Pause.supports(state),
                matches!(
                    state,
                    ChannelDownloadState::Queued | ChannelDownloadState::Running
                )
            );
            assert_eq!(
                TransferAction::Resume.supports(state),
                state == ChannelDownloadState::Paused
            );
            assert_eq!(
                TransferAction::Retry.supports(state),
                matches!(
                    state,
                    ChannelDownloadState::Failed(_) | ChannelDownloadState::Cancelled
                )
            );
            assert_ne!(
                TransferAction::Delete.supports(state),
                TransferAction::Cancel.supports(state)
            );
        }
    }

    #[test]
    fn vault_selection_identity_survives_reordering_and_is_disjoint() {
        let mut row = transfers(false).remove(0);
        row.vault_transfer_id = Some(42);
        assert_eq!(
            transfer_selection_key(&row, 0),
            transfer_selection_key(&row, 9)
        );
        assert_ne!(transfer_selection_key(&row, 0), 42);
        assert_ne!(transfer_selection_key(&row, 0), 0x4000_0000_0000_002a);
    }

    #[test]
    fn transfer_sidebar_facets_match_only_the_requested_state() {
        assert!(transfer_matches_nav(
            "nav-completed",
            TransferState::Completed,
            TransferDirection::Download,
        ));
        assert!(!transfer_matches_nav(
            "nav-completed",
            TransferState::Failed,
            TransferDirection::Download,
        ));
        assert!(transfer_matches_nav(
            "nav-failed",
            TransferState::Failed,
            TransferDirection::Download,
        ));
        assert!(transfer_matches_nav(
            "nav-transfers-all",
            TransferState::Waiting,
            TransferDirection::Upload,
        ));
        assert!(transfer_matches_nav(
            "nav-uploads",
            TransferState::Waiting,
            TransferDirection::Upload,
        ));
        assert!(!transfer_matches_nav(
            "nav-uploads",
            TransferState::Downloading,
            TransferDirection::Download,
        ));
    }

    #[test]
    fn transfer_selection_keys_are_stable_for_runtime_tasks_and_disjoint_for_preview_rows() {
        let mut rows = transfers(false);
        assert_eq!(transfer_selection_key(&rows[0], 3), 0x8000_0000_0000_0003);
        rows[0].runtime_task_id = Some(42);
        assert_eq!(transfer_selection_key(&rows[0], 3), 42);
        rows[0].runtime_task_id = None;
        rows[0].runtime_batch_id = Some(42);
        assert_eq!(transfer_selection_key(&rows[0], 3), 0x4000_0000_0000_002a);
    }

    #[test]
    fn batch_state_uses_active_then_actionable_then_terminal_precedence() {
        use teleark_core::ApplicationErrorKind;

        assert_eq!(
            aggregate_download_states([
                ChannelDownloadState::Completed,
                ChannelDownloadState::Running,
                ChannelDownloadState::Failed(ApplicationErrorKind::Network),
            ]),
            TransferState::Downloading
        );
        assert_eq!(
            aggregate_download_states([
                ChannelDownloadState::Completed,
                ChannelDownloadState::Paused,
                ChannelDownloadState::Failed(ApplicationErrorKind::Network),
            ]),
            TransferState::Paused
        );
        assert_eq!(
            aggregate_download_states([
                ChannelDownloadState::Completed,
                ChannelDownloadState::Failed(ApplicationErrorKind::Network),
            ]),
            TransferState::Failed
        );
        assert_eq!(
            aggregate_download_states([
                ChannelDownloadState::Completed,
                ChannelDownloadState::Completed,
            ]),
            TransferState::Completed
        );
    }

    #[test]
    fn select_all_toggles_only_visible_rows_and_preserves_hidden_selection() {
        let mut selected = std::collections::BTreeSet::from([99]);
        toggle_visible_selection(&mut selected, &[1, 2], false);
        assert_eq!(selected, std::collections::BTreeSet::from([1, 2, 99]));
        toggle_visible_selection(&mut selected, &[1, 2], true);
        assert_eq!(selected, std::collections::BTreeSet::from([99]));
    }

    #[test]
    fn vault_failures_do_not_claim_a_native_download_destination() {
        use teleark_core::ApplicationErrorKind as Kind;
        use teleark_i18n::{Localizer, MessageId, SupportedLocale};
        for kind in [
            Kind::InvalidRequest,
            Kind::NotFound,
            Kind::Conflict,
            Kind::Persistence,
            Kind::SourceMissing,
            Kind::SourceChanged,
            Kind::PermissionDenied,
            Kind::Capacity,
            Kind::Authorization,
            Kind::Network,
            Kind::Server,
            Kind::Cancelled,
        ] {
            for phase in [
                None,
                Some(VaultUploadPhase::CheckingStorage),
                Some(VaultUploadPhase::CheckingTarget),
                Some(VaultUploadPhase::Preparing),
                Some(VaultUploadPhase::Uploading),
            ] {
                let id = vault_transfer_error_message_id(kind, phase);
                assert!(!id.starts_with("native-download-"));
                for locale in SupportedLocale::ALL {
                    assert!(
                        Localizer::new(locale)
                            .expect("localizer")
                            .contains(locale, MessageId::new(id))
                    );
                }
            }
        }
        assert_eq!(
            vault_transfer_error_message_id(
                Kind::PermissionDenied,
                Some(VaultUploadPhase::CheckingTarget)
            ),
            "vault-transfer-error-target-permission"
        );
        assert_eq!(
            vault_transfer_error_message_id(
                Kind::PermissionDenied,
                Some(VaultUploadPhase::Uploading)
            ),
            "vault-transfer-error-permission"
        );
    }

    #[test]
    fn every_native_download_failure_has_a_localized_specific_reason() {
        use teleark_core::ApplicationErrorKind;
        use teleark_i18n::{Localizer, MessageId, SupportedLocale};

        let kinds = [
            ApplicationErrorKind::InvalidRequest,
            ApplicationErrorKind::NotFound,
            ApplicationErrorKind::Conflict,
            ApplicationErrorKind::Persistence,
            ApplicationErrorKind::SourceMissing,
            ApplicationErrorKind::SourceChanged,
            ApplicationErrorKind::PermissionDenied,
            ApplicationErrorKind::Capacity,
            ApplicationErrorKind::Authorization,
            ApplicationErrorKind::Network,
            ApplicationErrorKind::Cancelled,
        ];
        for locale in SupportedLocale::ALL {
            let localizer = Localizer::new(locale).expect("localizer");
            for kind in kinds {
                let id = MessageId::new(native_download_error_message_id(kind));
                assert!(localizer.contains(locale, id));
            }
        }
    }

    #[gpui_kit::test]
    fn native_cleanup_explains_waiting_and_failure_with_only_supported_actions(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (entity, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        for (width, height) in [(900.0, 600.0), (1440.0, 900.0)] {
            cx.simulate_resize(gpui_kit::size(px(width), px(height)));
            for failed in [false, true] {
                entity.update(cx, |app, cx| {
                    app.preview_native_cleanup(failed);
                    assert_eq!(app.native_transfer_view.omitted_items, 0);
                    assert_eq!(app.vault_transfer_view.omitted_items, 0);
                    let snapshot = &app.native_transfer_view.items[0];
                    let row = app.transfer_row_from_snapshot(snapshot, false);
                    assert_eq!(
                        row.state,
                        if failed {
                            TransferState::Failed
                        } else {
                            TransferState::Waiting
                        }
                    );
                    assert_eq!(
                        row.activity,
                        Some(app.tr(if failed {
                            "native-cleanup-failed"
                        } else {
                            "native-cleanup-waiting"
                        }))
                    );
                    assert!(TransferAction::Retry.supports_snapshot(snapshot));
                    for action in [
                        TransferAction::Delete,
                        TransferAction::Pause,
                        TransferAction::Resume,
                        TransferAction::Cancel,
                    ] {
                        assert!(!action.supports_snapshot(snapshot));
                    }
                    let batch = app.transfer_row_from_batch(1, &[snapshot]);
                    assert!(batch.activity.is_some());
                    assert_eq!(
                        batch.batch_summary.expect("batch").failed,
                        usize::from(failed)
                    );
                    cx.notify();
                });
                cx.run_until_parked();
                let inspector = cx.debug_bounds("transfer-inspector").expect("inspector");
                let explanation = cx
                    .debug_bounds("transfer-activity-detail")
                    .expect("cleanup explanation");
                assert!(
                    explanation.top() >= inspector.top()
                        && explanation.bottom() <= inspector.bottom()
                );
            }
        }
    }
}
