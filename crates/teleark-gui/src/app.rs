mod app_lock;
mod auth;
mod background;
mod browser;
mod channel_layout;
mod channel_sync;
mod dialogs;
mod library;
pub(crate) mod lifecycle;
mod local_files;
mod managed_projection;
mod navigation;
mod preferences;
mod preview;
mod speed_limits;
mod status_bar;
mod sync_history;
mod sync_time;
mod workspace_view;
#[cfg(test)]
pub(crate) use preview::completed_storage_maintenance_preview;
pub(crate) mod proxy;
pub(crate) mod storage;
mod vault;

use std::{collections::BTreeSet, time::Duration};

use gpui_kit::component::{
    Icon, IconName, WindowExt as _,
    input::{InputEvent, InputState},
    notification::Notification,
    resizable::ResizableState,
    table::{TableEvent, TableState},
};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Subscription, Task,
    Window, div, prelude::FluentBuilder as _, px,
};
use teleark_core::{
    ApplicationError, FileKind, LibraryFilter, LibraryQuery, LibrarySort, LibraryStatistics,
};
use teleark_i18n::{
    Localizer, MessageArgs, MessageId, SupportedLocale,
    format::{format_bytes, format_speed},
};
use teleark_runtime::{
    AppearancePreference, ChannelDownloadRequest, ChannelDownloadState, DesktopLibrary,
    DesktopPreferences, DesktopTelegram, DesktopTransfers, DesktopVault, ManagedVaultFile,
    TelegramAuthState, TelegramChatSummary, TelegramCredentialSource, TelegramFilePage,
    TelegramFileSummary, TelegramIndexPage, TelegramScanCancellation, VaultStatus,
};
use teleark_runtime::{TelegramAccount, TelegramChatKind};

use crate::{
    DismissOverlay, FocusSearch, MinimizeWindow, ShowAbout, ShowSettings, ShowStorage,
    ShowTransfers, ToggleFullscreen, UploadFile, ZoomWindow,
    components::{self, Tone},
    layout::LayoutPolicy,
    library_state::{
        ImportActivity, ImportFeedback, LibraryContent, LibraryPageCursor, LibrarySnapshot,
        LibraryView,
    },
    screens::{
        self,
        channel::{
            CHANNEL_FILE_INITIAL_SCAN, CHANNEL_FILE_LIST_CAPACITY, CHANNEL_FILE_LOAD_MORE_SCAN,
            CHANNEL_FILE_SCAN_CHUNK, ChannelFileTableDelegate,
        },
    },
    theme,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Page {
    Account,
    Storage,
    LegacyRecovery,
    Library,
    Transfers,
    FileDetail,
    Channel,
    Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnlockIntent {
    Browse,
    Upload,
    Download(u64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LocalePersistence {
    Idle,
    Saving,
    Saved,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TelegramActivity {
    Idle,
    Working,
    Failed(teleark_core::ApplicationErrorKind),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum StorageView {
    #[default]
    Files,
    RawFiles,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ChannelBatchPeriod {
    #[default]
    AnyTime,
    Past24Hours,
    Past7Days,
    Past30Days,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ChannelBatchActivity {
    Idle,
    Preparing,
    Queued { batch_id: u64, count: usize },
    NoMatches,
    Failed(teleark_core::ApplicationErrorKind),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TelegramApiIdPersistence {
    Idle,
    Saving,
    Saved,
    Removed,
    Failed(teleark_core::ApplicationErrorKind),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PreferencePersistence {
    Idle,
    Saving,
    Saved,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SettingsSection {
    #[default]
    General,
    Accounts,
    Network,
    Storage,
    Downloads,
    Uploads,
    KeyVault,
    Indexing,
    Notifications,
    Appearance,
    About,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EscapeBehavior {
    ExitFullscreen,
    DismissUnlock,
    DismissUpload,
    DismissTelegramApiIdPrompt,
    DismissStorageDetails,
    Ignore,
}

fn escape_behavior(
    fullscreen: bool,
    unlock_visible: bool,
    upload_visible: bool,
    telegram_api_id_prompt_visible: bool,
    storage_details_visible: bool,
) -> EscapeBehavior {
    if unlock_visible {
        EscapeBehavior::DismissUnlock
    } else if upload_visible {
        EscapeBehavior::DismissUpload
    } else if telegram_api_id_prompt_visible {
        EscapeBehavior::DismissTelegramApiIdPrompt
    } else if storage_details_visible {
        EscapeBehavior::DismissStorageDetails
    } else if fullscreen {
        EscapeBehavior::ExitFullscreen
    } else {
        EscapeBehavior::Ignore
    }
}

fn parse_telegram_api_id(value: &str) -> Result<i32, teleark_core::ApplicationErrorKind> {
    value
        .parse::<i32>()
        .ok()
        .filter(|api_id| *api_id > 0)
        .ok_or(teleark_core::ApplicationErrorKind::InvalidRequest)
}

fn validate_telegram_api_hash(value: &str) -> Result<(), teleark_core::ApplicationErrorKind> {
    if value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(teleark_core::ApplicationErrorKind::InvalidRequest)
    }
}

pub(crate) fn telegram_login_controls_enabled(configured_api_id: Option<i32>) -> bool {
    configured_api_id.is_some_and(|api_id| api_id > 0)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LocaleOverrideChoice {
    SystemDefault,
    Explicit(SupportedLocale),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocaleStartup {
    pub system_locale: SupportedLocale,
    pub follows_system_locale: bool,
}

pub struct AppStartup {
    pub page: Page,
    pub visual_preview: bool,
    pub show_upload: bool,
    pub locale: LocaleStartup,
}

pub struct RuntimeConfiguration {
    app_pin: Result<Option<teleark_runtime::AppPinRecord>, teleark_core::ApplicationErrorKind>,
    network_route: Result<teleark_runtime::NetworkRoute, teleark_core::ApplicationErrorKind>,
    credential_status: Result<
        Option<teleark_runtime::TelegramCredentialsStatus>,
        teleark_core::ApplicationErrorKind,
    >,
    preferences: Result<DesktopPreferences, teleark_core::ApplicationErrorKind>,
}

impl RuntimeConfiguration {
    pub(crate) fn read(
        library: &Result<DesktopLibrary, ApplicationError>,
        telegram: &Result<DesktopTelegram, ApplicationError>,
    ) -> Self {
        Self {
            app_pin: library
                .as_ref()
                .map_err(|e| e.kind())
                .and_then(|library| library.app_pin().map_err(|e| e.kind())),
            network_route: library
                .as_ref()
                .map_err(|e| e.kind())
                .and_then(|library| library.proxy_configuration().map_err(|e| e.kind())),
            credential_status: library
                .as_ref()
                .map_err(|error| error.kind())
                .and_then(|library| {
                    telegram
                        .as_ref()
                        .map_err(|error| error.kind())?
                        .effective_credentials_status(library)
                        .map_err(|error| error.kind())
                }),
            preferences: library
                .as_ref()
                .map_err(|error| error.kind())
                .and_then(|library| library.preferences().map_err(|error| error.kind())),
        }
    }
}

pub struct RuntimeStartup {
    pub configuration: Option<RuntimeConfiguration>,
    pub library: Result<DesktopLibrary, ApplicationError>,
    pub telegram: Result<DesktopTelegram, ApplicationError>,
    pub transfers: Result<DesktopTransfers, ApplicationError>,
    pub vault: Result<DesktopVault, ApplicationError>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum VaultActivity {
    #[default]
    Idle,
    Working,
    Succeeded,
    Failed(teleark_core::ApplicationErrorKind),
}

pub struct TeleArkApp {
    pub(crate) app_lock: app_lock::AppLockUi,
    pub(crate) transition: Option<lifecycle::Transition>,
    shutdown_task: Option<Task<()>>,
    main_window: gpui_kit::AnyWindowHandle,
    pub(crate) proxy: proxy::ProxyUi,
    pub(crate) page: Page,
    pub(crate) storage_status: teleark_runtime::StorageChannelStatus,
    pub(crate) storage_loading: bool,
    pub(crate) storage_details_expanded: bool,
    pub(crate) storage_confirmation: Option<storage::StorageAction>,
    pub(crate) storage_maintenance: Option<teleark_runtime::StorageMaintenance>,
    pub(crate) storage_maintenance_preview: Option<teleark_runtime::StorageMaintenanceSnapshot>,
    storage_maintenance_presentation: Option<Task<()>>,
    pub(crate) storage_notice: Option<&'static str>,
    storage_retry_task: Option<Task<()>>,
    pub(crate) storage_error: Option<teleark_core::ApplicationErrorKind>,
    pub(crate) show_storage_guide: bool,
    pub(crate) settings_advanced_expanded: bool,
    pub(crate) about_show_licenses: bool,
    pub(crate) upload_advanced_expanded: bool,
    pub(crate) vault_advanced_expanded: bool,
    pub(crate) vault_new_epoch_confirmation: bool,
    pub(crate) vault_key_progress: Option<teleark_runtime::VaultKeyProgress>,
    vault_key_presentation: Option<Task<()>>,
    pub(crate) account_restoring: bool,
    account_restore_retry_at: Option<std::time::Instant>,
    account_restore_event_at: Option<std::time::Instant>,
    pub(crate) transfers_account_ready: bool,
    pub(crate) account_avatar: Option<std::sync::Arc<gpui_kit::Image>>,
    pub(crate) show_account_switch: bool,
    pub(crate) confirm_account_switch: bool,
    pub(crate) phone_login: bool,
    pub(crate) unlock_intent: Option<UnlockIntent>,
    main_focus: gpui_kit::FocusHandle,
    modal_was_open: bool,
    pub(crate) modal_focus: gpui_kit::FocusHandle,
    storage_task: Option<Task<()>>,
    avatar_task: Option<Task<()>>,
    pub(crate) visual_preview: bool,
    pub(crate) localizer: Localizer,
    pub(crate) search_input: Entity<InputState>,
    pub(crate) show_upload: bool,
    pub(crate) upload_queued: bool,
    pub(crate) upload_in_flight: bool,
    upload_draft_generation: u64,
    pub(crate) selected_file: usize,
    pub(crate) selected_transfer_keys: BTreeSet<u64>,
    pub(crate) pending_transfer_delete: Option<u64>,
    pub(crate) pending_transfer_bulk_delete: Vec<u64>,
    pub(crate) transfer_action_error: Option<teleark_core::ApplicationErrorKind>,
    pub(crate) transfer_projection_cache:
        std::cell::RefCell<screens::transfers::TransferProjectionCache>,
    pub(crate) transfer_action_job: Option<screens::transfers::TransferActionJob>,
    pub(crate) show_transfer_detail: bool,
    pub(crate) transfer_controls_expanded: bool,
    pub(crate) focused_transfer_key: Option<u64>,
    pub(crate) transfer_scroll: gpui_kit::ListState,
    pub(crate) transfer_list_keys: std::cell::RefCell<Vec<u64>>,
    pub(crate) transfer_detail_scroll: gpui_kit::ScrollHandle,
    pub(crate) raw_detail_scroll: gpui_kit::ScrollHandle,
    pub(crate) upload_body_scroll: gpui_kit::ScrollHandle,
    pub(crate) batch_detail_scroll: gpui_kit::UniformListScrollHandle,
    pub(crate) expanded_transfer_batches: BTreeSet<u64>,
    pub(crate) transfer_batch_window_request: Option<u64>,
    pub(crate) transfer_batch_window:
        Option<(u64, gpui_kit::WindowHandle<gpui_kit::component::Root>)>,
    pub(crate) transfer_inspector_replay: bool,
    pub(crate) transfer_replay_cursor: usize,
    pub(crate) vault_locked: bool,
    pub(crate) recovery_visible: bool,
    pub(crate) vault_status: VaultStatus,
    pub(crate) vault_activity: VaultActivity,
    pub(crate) vault_recovery_secret: Option<String>,
    pub(crate) vault_password: Entity<InputState>,
    pub(crate) vault_new_password: Entity<InputState>,
    pub(crate) vault_recovery_key: Entity<InputState>,
    pub(crate) managed_vault_files: std::sync::Arc<Vec<ManagedVaultFile>>,
    pub(crate) managed_projection: std::cell::RefCell<managed_projection::ManagedProjection>,
    pub(crate) managed_vault_rejected: usize,
    pub(crate) upload_sources: Vec<teleark_runtime::VaultUploadSource>,
    pub(crate) upload_preparing: bool,
    pub(crate) upload_source_total_bytes: u64,
    pub(crate) upload_selection_progress: Option<teleark_runtime::VaultUploadSelectionProgress>,
    pub(crate) upload_preparation_progress: Option<teleark_runtime::VaultUploadSelectionProgress>,
    upload_selection_presentation: Option<Task<()>>,
    upload_preparation_presentation: Option<Task<()>>,
    pub(crate) nav_selection: &'static str,
    pub(crate) storage_view: StorageView,
    pub(crate) library_content: LibraryContent,
    pub(crate) library_batch_cancellation: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub(crate) library_selection: Vec<crate::library_state::LibraryRowId>,
    pub(crate) library_view: LibraryView,
    pub(crate) library_kind_selection: &'static str,
    library_scan_cancellation: teleark_runtime::LocalLibraryCancellation,
    pub(crate) import_activity: ImportActivity,
    pub(crate) import_feedback: Option<ImportFeedback>,
    pub(crate) library_loading_more: bool,
    pub(crate) library_load_more_error: Option<teleark_core::ApplicationErrorKind>,
    pub(crate) follows_system_locale: bool,
    pub(crate) system_locale: SupportedLocale,
    pub(crate) locale_persistence: LocalePersistence,
    pub(crate) telegram_auth: TelegramAuthState,
    pub(crate) telegram_activity: TelegramActivity,
    pub(crate) qr_login_error: Option<teleark_core::ApplicationErrorKind>,
    pub(crate) login_proxy_open: bool,
    pub(crate) dialogs: dialogs::DialogLoad,
    dialogs_task: Option<Task<()>>,
    pub(crate) telegram_account: Option<TelegramAccount>,
    pub(crate) telegram_chats: Vec<TelegramChatSummary>,
    pub(crate) selected_chat_id: Option<i64>,
    pub(crate) last_channel_id: Option<i64>,
    pub(crate) preview_transfer_rows: Vec<crate::mock::TransferRow>,
    pub(crate) telegram_index: Option<TelegramIndexPage>,
    pub(crate) telegram_files: Vec<TelegramFileSummary>,
    channel_sync: Option<teleark_runtime::ChannelSync>,
    channel_sources_revision: u64,
    pub(crate) channel_history_request: Option<(i64, u64)>,
    pub(crate) channel_history_failed: bool,
    pub(crate) channel_sync_snapshot: Option<teleark_runtime::ChannelSyncSnapshot>,
    channel_sync_task: Option<Task<()>>,
    channel_display_revision: i64,
    workspace_view: Option<Entity<workspace_view::WorkspaceView>>,
    page_view: Option<Entity<workspace_view::ContentView>>,
    source_view: Option<Entity<workspace_view::ContentView>>,
    sync_inspector: Option<Entity<workspace_view::SyncInspector>>,
    sync_time_anchor: sync_time::TimeAnchor,
    sync_history: Option<Entity<sync_history::SyncHistory>>,
    managed_display_revision: i64,
    managed_upload_receipts:
        std::collections::VecDeque<(i64, i64, teleark_runtime::ManagedVaultFile)>,
    pub(crate) managed_catalog_pending: bool,
    pub(crate) managed_catalog_limited: bool,
    pub(crate) managed_health_checked: Option<usize>,
    channel_local_read_failed: bool,
    channel_view_cache: std::collections::VecDeque<channel_sync::CachedChannelView>,
    channel_history_armed: bool,
    channel_loaded_scope: Option<(i64, i64)>,
    pub(crate) channel_sync_details: bool,
    channel_sync_private_expanded: bool,
    pub(crate) channel_sync_scroll: gpui_kit::ScrollHandle,
    pub(crate) telegram_files_next: Option<i64>,
    pub(crate) telegram_files_exhausted: bool,
    pub(crate) telegram_files_loading: bool,
    pub(crate) telegram_files_scanned: u64,
    pub(crate) telegram_files_scan_target: u64,
    pub(crate) telegram_files_slow: bool,
    pub(crate) telegram_files_retry_append: bool,
    pub(crate) telegram_download: Option<(u64, ChannelDownloadState)>,
    pub(crate) selected_telegram_message_id: Option<i64>,
    pub(crate) show_channel_detail: bool,
    pub(crate) selected_channel_message_ids: BTreeSet<i64>,
    pub(crate) channel_batch_period: ChannelBatchPeriod,
    pub(crate) channel_batch_kinds: std::collections::HashSet<FileKind>,
    pub(crate) channel_batch_activity: ChannelBatchActivity,
    pub(crate) channel_batch_expanded: bool,
    pub(crate) channel_file_table: Entity<TableState<ChannelFileTableDelegate>>,
    pub(crate) telegram_api_id: Entity<InputState>,
    pub(crate) telegram_api_hash: Entity<InputState>,
    pub(crate) telegram_phone: Entity<InputState>,
    pub(crate) telegram_code: Entity<InputState>,
    pub(crate) telegram_password: Entity<InputState>,
    pub(crate) configured_telegram_api_id: Option<i32>,
    pub(crate) telegram_credential_source: Option<TelegramCredentialSource>,
    pub(crate) telegram_api_id_persistence: TelegramApiIdPersistence,
    pub(crate) show_telegram_api_id_prompt: bool,
    pub(crate) custom_telegram_credentials_enabled: bool,
    pub(crate) settings_section: SettingsSection,
    pub(crate) speed_limits: speed_limits::SpeedLimitsUi,
    pub(crate) preferences: DesktopPreferences,
    channel_layout: Entity<ResizableState>,
    channel_layout_geometry: Option<(gpui_kit::Pixels, bool)>,
    pub(crate) preference_persistence: PreferencePersistence,
    pub(crate) volume_space: Option<teleark_runtime::VolumeSpace>,
    library: Option<DesktopLibrary>,
    telegram: Option<DesktopTelegram>,
    pub(crate) transfers: Option<DesktopTransfers>,
    pub(crate) vault: Option<DesktopVault>,
    library_query_generation: u64,
    library_sync_dirty: bool,
    pub(crate) library_sync_loading: bool,
    library_sync_started: Option<std::time::Instant>,
    library_sync_error: Option<teleark_core::ApplicationErrorKind>,
    library_sync_task: Option<Task<()>>,
    pending_locale_override: Option<LocaleOverrideChoice>,
    library_action_task: Option<Task<()>>,
    pub(crate) library_action_busy: bool,
    pub(crate) library_action_error: Option<teleark_core::ApplicationErrorKind>,
    library_task: Option<Task<()>>,
    library_more_task: Option<Task<()>>,
    import_task: Option<Task<()>>,
    locale_task: Option<Task<()>>,
    telegram_api_id_task: Option<Task<()>>,
    telegram_task: Option<Task<()>>,
    telegram_file_task: Option<Task<()>>,
    telegram_file_slow_task: Option<Task<()>>,
    telegram_file_cancellation: Option<TelegramScanCancellation>,
    telegram_download_task: Option<Task<()>>,
    telegram_batch_task: Option<Task<()>>,
    preference_task: Option<Task<()>>,
    preference_picker_task: Option<Task<()>>,
    volume_space_task: Option<Task<()>>,
    local_files_task: Option<Task<()>>,
    pub(crate) local_downloads: local_files::LocalDownloadCache,
    transfer_monitor_task: Option<Task<()>>,
    transfer_refresh_task: Option<Task<()>>,
    vault_refresh_task: Option<Task<()>>,
    transfer_clock_task: Option<Task<()>>,
    pub(crate) native_transfer_view:
        teleark_runtime::TransferSnapshotView<teleark_runtime::ChannelDownloadSnapshot>,
    pub(crate) vault_transfer_view:
        teleark_runtime::TransferSnapshotView<teleark_runtime::VaultTransferSnapshot>,
    status_rate_cache: std::cell::RefCell<status_bar::RateCache>,
    vault_task: Option<Task<()>>,
    vault_upload_task: Option<Task<()>>,
    vault_recovery_task: Option<Task<()>>,
    pub(crate) vault_transfer_jobs: std::collections::BTreeMap<(i64, u64, bool), Task<()>>,
    vault_recovery_scope: Option<(u64, u64)>,
    vault_session_generation: u64,
    vault_download_in_flight: bool,
    pub(crate) managed_scan_loading: bool,
    managed_scan_generation: u64,
    managed_projection_scope: Option<(i64, i64)>,
    managed_view_before_legacy: Option<vault::ManagedViewCache>,
    managed_scan_cancellation: Option<TelegramScanCancellation>,
    vault_scan_task: Option<Task<()>>,
    vault_download_task: Option<Task<()>>,
    upload_picker_task: Option<Task<()>>,
    qr_poll_task: Option<Task<()>>,
    pub(crate) telegram_login_generation: u64,
    telegram_file_generation: u64,
    telegram_file_auto_load: bool,
    _subscriptions: Vec<Subscription>,
}

impl TeleArkApp {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        localizer: Localizer,
        runtime: RuntimeStartup,
        startup: AppStartup,
    ) -> Self {
        let AppStartup {
            page,
            visual_preview,
            show_upload,
            locale: locale_startup,
        } = startup;
        let RuntimeStartup {
            configuration,
            library,
            telegram,
            transfers,
            vault,
        } = runtime;
        let configuration =
            configuration.unwrap_or_else(|| RuntimeConfiguration::read(&library, &telegram));
        let app_lock = app_lock::AppLockUi::new(configuration.app_pin, visual_preview, window, cx);
        let proxy = proxy::ProxyUi::new(configuration.network_route, visual_preview, window, cx);
        let credential_status = configuration.credential_status;
        let (configured_telegram_api_id, telegram_credential_source, telegram_api_id_persistence) =
            match credential_status {
                Ok(status) => (
                    status.map(|status| status.api_id),
                    status.map(|status| status.source),
                    TelegramApiIdPersistence::Idle,
                ),
                Err(kind) => (None, None, TelegramApiIdPersistence::Failed(kind)),
            };
        let (preferences, preference_persistence) = match configuration.preferences {
            Ok(preferences) => (preferences, PreferencePersistence::Idle),
            Err(_) => (DesktopPreferences::default(), PreferencePersistence::Failed),
        };
        let speed_limits = speed_limits::SpeedLimitsUi::new(preferences.speed_limits, window, cx);
        theme::apply_appearance(preferences.appearance, window, cx);
        let appearance_subscription = cx.observe_window_appearance(window, |this, window, cx| {
            if this.preferences.appearance == AppearancePreference::System {
                theme::apply_appearance(AppearancePreference::System, window, cx);
                cx.notify();
            }
        });
        let activation_subscription = cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.reset_channel_layout(cx);
            }
        });
        let placeholder = localizer.translate_or_id(MessageId::new("search-placeholder"));
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let telegram_api_id = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder(
                localizer.translate_or_id(MessageId::new("telegram-api-id-placeholder")),
            );
            if let Some(api_id) = configured_telegram_api_id {
                input.set_value(api_id.to_string(), window, cx);
            }
            input
        });
        let telegram_api_hash = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(
                    localizer.translate_or_id(MessageId::new("telegram-api-hash-placeholder")),
                )
                .masked(true)
        });
        let telegram_phone = cx.new(|cx| {
            InputState::new(window, cx).placeholder(
                localizer.translate_or_id(MessageId::new("telegram-phone-placeholder")),
            )
        });
        let telegram_code = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(localizer.translate_or_id(MessageId::new("telegram-code-placeholder")))
        });
        let telegram_password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(
                    localizer.translate_or_id(MessageId::new("telegram-password-placeholder")),
                )
                .masked(true)
        });
        let vault_password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(
                    localizer.translate_or_id(MessageId::new("vault-password-placeholder")),
                )
                .masked(true)
        });
        let vault_new_password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(
                    localizer.translate_or_id(MessageId::new("vault-new-password-placeholder")),
                )
                .masked(true)
        });
        let vault_recovery_key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(
                    localizer.translate_or_id(MessageId::new("vault-recovery-placeholder")),
                )
                .masked(true)
        });
        let channel_file_table = cx.new(|cx| {
            TableState::new(ChannelFileTableDelegate::new(), window, cx)
                .sortable(false)
                .col_movable(false)
                .col_resizable(false)
                .col_selectable(false)
        });
        let search_subscription = cx.subscribe(&search_input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) && this.page == Page::Library {
                this.refresh_library(cx);
            } else {
                cx.notify();
            }
        });
        let mut form_subscriptions = Vec::new();
        for input in [&vault_password, &vault_new_password] {
            form_subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. })
                        && this.unlock_intent.is_some()
                        && this.vault_recovery_secret.is_none()
                    {
                        if this.vault_status.configured {
                            this.unlock_vault_with_password(window, cx);
                        } else {
                            this.initialize_vault(window, cx);
                        }
                    }
                },
            ));
        }
        form_subscriptions.push(cx.subscribe_in(
            &vault_recovery_key,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. })
                    && this.unlock_intent.is_some()
                    && this.vault_recovery_secret.is_none()
                {
                    if this.vault_status.configured {
                        this.unlock_vault_with_recovery(window, cx);
                    } else {
                        this.restore_vault_with_recovery(window, cx);
                    }
                }
            },
        ));
        let channel_table_subscription = cx.subscribe(
            &channel_file_table,
            |this, table, event: &TableEvent, cx| {
                if let TableEvent::SelectRow(row) = event {
                    let selected = table.read(cx).delegate().message_id_at(*row);
                    if this.selected_telegram_message_id != selected {
                        this.raw_detail_scroll
                            .set_offset(gpui_kit::point(px(0.0), px(0.0)));
                    }
                    this.selected_telegram_message_id = selected;
                    this.show_channel_detail = true;
                    this.channel_sync_details = false;
                    cx.notify();
                }
            },
        );
        let nav_selection = match page {
            Page::Account => "nav-account",
            Page::Storage | Page::LegacyRecovery => "nav-storage",
            Page::Library | Page::FileDetail => "nav-all",
            Page::Transfers => "nav-transfers-all",
            Page::Channel => "nav-channel",
            Page::Settings => "nav-settings",
        };
        let (library, library_content, locale_persistence) = match library {
            Ok(library) => (
                Some(library),
                LibraryContent::Loading,
                LocalePersistence::Idle,
            ),
            Err(error) => (
                None,
                LibraryContent::Failed(error.kind()),
                LocalePersistence::Failed,
            ),
        };

        let vault_status = vault
            .as_ref()
            .map(DesktopVault::status)
            .unwrap_or(VaultStatus {
                active_key_locked: true,
                historical_key_unlocked: false,
                active_vault_id: None,
                configured: false,
                locked: true,
                created_at_unix_ms: None,
                password_generation: None,
                recovery_generation: None,
            });
        let mut app = Self {
            app_lock,
            transition: None,
            shutdown_task: None,
            main_window: window.window_handle(),
            proxy,
            page,
            storage_status: teleark_runtime::StorageChannelStatus::Missing,
            storage_loading: false,
            storage_details_expanded: false,
            storage_confirmation: None,
            storage_maintenance: None,
            storage_maintenance_preview: None,
            storage_maintenance_presentation: None,
            storage_notice: None,
            storage_retry_task: None,
            storage_error: None,
            show_storage_guide: false,
            settings_advanced_expanded: false,
            about_show_licenses: false,
            upload_advanced_expanded: false,
            vault_advanced_expanded: false,
            vault_new_epoch_confirmation: false,
            vault_key_progress: None,
            vault_key_presentation: None,
            account_restoring: false,
            account_restore_retry_at: None,
            account_restore_event_at: None,
            transfers_account_ready: false,
            account_avatar: None,
            show_account_switch: false,
            confirm_account_switch: false,
            phone_login: false,
            unlock_intent: None,
            main_focus: cx.focus_handle(),
            modal_was_open: false,
            modal_focus: cx.focus_handle(),
            storage_task: None,
            avatar_task: None,
            visual_preview,
            localizer,
            search_input,
            show_upload,
            upload_queued: false,
            upload_in_flight: false,
            upload_draft_generation: 0,
            selected_file: 0,
            selected_transfer_keys: BTreeSet::new(),
            pending_transfer_delete: None,
            pending_transfer_bulk_delete: Vec::new(),
            transfer_action_error: None,
            transfer_projection_cache: Default::default(),
            transfer_action_job: None,
            show_transfer_detail: false,
            transfer_controls_expanded: false,
            focused_transfer_key: None,
            transfer_scroll: gpui_kit::ListState::new(0, gpui_kit::ListAlignment::Top, px(200.0)),
            transfer_list_keys: Default::default(),
            transfer_detail_scroll: gpui_kit::ScrollHandle::new(),
            raw_detail_scroll: gpui_kit::ScrollHandle::new(),
            upload_body_scroll: gpui_kit::ScrollHandle::new(),
            batch_detail_scroll: gpui_kit::UniformListScrollHandle::new(),
            expanded_transfer_batches: BTreeSet::new(),
            transfer_batch_window: None,
            transfer_batch_window_request: None,
            transfer_inspector_replay: false,
            transfer_replay_cursor: 0,
            vault_locked: vault_status.locked,
            recovery_visible: false,
            vault_status,
            vault_activity: if vault.is_ok() {
                VaultActivity::Idle
            } else {
                VaultActivity::Failed(teleark_core::ApplicationErrorKind::Persistence)
            },
            vault_recovery_secret: None,
            vault_password,
            vault_new_password,
            vault_recovery_key,
            managed_vault_files: Default::default(),
            managed_projection: Default::default(),
            managed_vault_rejected: 0,
            upload_sources: Vec::new(),
            upload_preparing: false,
            upload_source_total_bytes: 0,
            upload_selection_progress: None,
            upload_preparation_progress: None,
            upload_selection_presentation: None,
            upload_preparation_presentation: None,
            nav_selection,
            storage_view: StorageView::Files,
            library_content,
            library_batch_cancellation: Default::default(),
            library_selection: Vec::new(),
            library_view: LibraryView::Local,
            library_kind_selection: "library-types-all",
            library_scan_cancellation: Default::default(),
            import_activity: ImportActivity::Idle,
            import_feedback: None,
            library_loading_more: false,
            library_load_more_error: None,
            follows_system_locale: locale_startup.follows_system_locale,
            system_locale: locale_startup.system_locale,
            locale_persistence,
            telegram_auth: TelegramAuthState::Disconnected,
            qr_login_error: None,
            login_proxy_open: false,
            telegram_activity: if telegram.is_ok() {
                TelegramActivity::Idle
            } else {
                TelegramActivity::Failed(teleark_core::ApplicationErrorKind::Persistence)
            },
            dialogs: dialogs::DialogLoad::default(),
            dialogs_task: None,
            telegram_account: None,
            telegram_chats: Vec::new(),
            selected_chat_id: None,
            last_channel_id: None,
            preview_transfer_rows: Vec::new(),
            telegram_index: None,
            telegram_files: Vec::new(),
            channel_sync: None,
            channel_sources_revision: 0,
            channel_history_request: None,
            channel_history_failed: false,
            channel_sync_snapshot: None,
            channel_sync_task: None,
            channel_display_revision: 0,
            workspace_view: None,
            page_view: None,
            source_view: None,
            sync_inspector: None,
            sync_time_anchor: sync_time::TimeAnchor::new(),
            sync_history: None,
            managed_display_revision: 0,
            managed_upload_receipts: std::collections::VecDeque::new(),
            managed_catalog_pending: false,
            managed_catalog_limited: false,
            managed_health_checked: None,
            channel_local_read_failed: false,
            channel_view_cache: Default::default(),
            channel_history_armed: false,
            channel_loaded_scope: None,
            channel_sync_details: false,
            channel_sync_private_expanded: false,
            channel_sync_scroll: gpui_kit::ScrollHandle::new(),
            telegram_files_next: None,
            telegram_files_exhausted: false,
            telegram_files_loading: false,
            telegram_files_scanned: 0,
            telegram_files_scan_target: 0,
            telegram_files_slow: false,
            telegram_files_retry_append: false,
            telegram_download: None,
            selected_telegram_message_id: None,
            show_channel_detail: false,
            selected_channel_message_ids: BTreeSet::new(),
            channel_batch_period: ChannelBatchPeriod::AnyTime,
            channel_batch_kinds: std::collections::HashSet::new(),
            channel_batch_activity: ChannelBatchActivity::Idle,
            channel_batch_expanded: false,
            channel_file_table,
            telegram_api_id,
            telegram_api_hash,
            telegram_phone,
            telegram_code,
            telegram_password,
            configured_telegram_api_id,
            telegram_credential_source,
            telegram_api_id_persistence,
            show_telegram_api_id_prompt: false,
            custom_telegram_credentials_enabled: telegram_credential_source
                == Some(TelegramCredentialSource::User),
            settings_section: SettingsSection::General,
            speed_limits,
            preferences,
            channel_layout: cx.new(|_| ResizableState::default()),
            channel_layout_geometry: None,
            preference_persistence,
            volume_space: None,
            library,
            telegram: telegram.ok(),
            transfers: transfers.ok(),
            vault: vault.ok(),
            library_query_generation: 0,
            library_sync_dirty: false,
            library_sync_loading: false,
            library_sync_started: None,
            library_sync_error: None,
            library_sync_task: None,
            pending_locale_override: None,
            library_action_task: None,
            library_action_busy: false,
            library_action_error: None,
            library_task: None,
            library_more_task: None,
            import_task: None,
            locale_task: None,
            telegram_api_id_task: None,
            telegram_task: None,
            telegram_file_task: None,
            telegram_file_slow_task: None,
            telegram_file_cancellation: None,
            telegram_download_task: None,
            telegram_batch_task: None,
            preference_task: None,
            preference_picker_task: None,
            volume_space_task: None,
            local_files_task: None,
            local_downloads: Default::default(),
            transfer_monitor_task: None,
            transfer_refresh_task: None,
            vault_refresh_task: None,
            transfer_clock_task: None,
            native_transfer_view: Default::default(),
            vault_transfer_view: Default::default(),
            status_rate_cache: Default::default(),
            vault_task: None,
            vault_upload_task: None,
            vault_recovery_task: None,
            vault_transfer_jobs: Default::default(),
            vault_recovery_scope: None,
            vault_session_generation: 0,
            vault_download_in_flight: false,
            managed_scan_loading: false,
            managed_scan_generation: 0,
            managed_projection_scope: None,
            managed_view_before_legacy: None,
            managed_scan_cancellation: None,
            vault_scan_task: None,
            vault_download_task: None,
            upload_picker_task: None,
            qr_poll_task: None,
            telegram_login_generation: 0,
            telegram_file_generation: 0,
            telegram_file_auto_load: false,
            _subscriptions: vec![
                search_subscription,
                appearance_subscription,
                activation_subscription,
                channel_table_subscription,
            ],
        };
        app._subscriptions.extend(form_subscriptions);
        let pin_input = app.app_lock.pin.clone();
        app._subscriptions.push(cx.subscribe_in(
            &pin_input,
            window,
            |app, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) && app.app_is_locked() {
                    app.submit_app_pin(app_lock::PinOperation::Unlock, window, cx);
                }
            },
        ));
        app.install_lifecycle(window, cx);
        if app.library.is_some() && matches!(app.page, Page::Library | Page::FileDetail) {
            app.refresh_library(cx);
        }
        if app.visual_preview {
            app.initialize_preview(window, cx);
        }
        app.start_transfer_refresh(cx);
        app.start_volume_space_refresh(cx);
        app.start_local_file_refresh(cx);
        app.start_network_observer(cx);
        app.start_bandwidth_observer(cx);
        app.restore_telegram_session(cx);
        app
    }

    pub(crate) fn tr(&self, id: &'static str) -> SharedString {
        self.localizer.translate_or_id(MessageId::new(id)).into()
    }

    pub(crate) fn tr_with(&self, id: &'static str, args: MessageArgs) -> SharedString {
        match self.localizer.translate_with(MessageId::new(id), &args) {
            Ok(message) => message.into(),
            Err(_) => self.tr(id),
        }
    }

    pub(crate) fn locale(&self) -> SupportedLocale {
        self.localizer.locale()
    }

    pub(crate) fn set_page(&mut self, page: Page, cx: &mut Context<Self>) {
        if page != Page::Account && !self.telegram_is_authorized() {
            return;
        }
        self.channel_history_armed = false;
        self.show_transfer_detail = false;
        self.pending_transfer_delete = None;
        self.pending_transfer_bulk_delete.clear();
        self.selected_transfer_keys.clear();
        if matches!(
            self.page,
            Page::Channel | Page::Storage | Page::LegacyRecovery
        ) && self.page != page
        {
            self.cancel_channel_history();
            self.cancel_telegram_file_load(cx);
        }
        let leaving_legacy = self.page == Page::LegacyRecovery && self.page != page;
        if leaving_legacy {
            self.cancel_managed_scan();
        }
        if self.page == Page::Library && page != Page::Library {
            self.library_scan_cancellation.cancel();
            self.library_query_generation = self.library_query_generation.wrapping_add(1);
        }
        self.page = page;
        if leaving_legacy {
            self.restore_managed_view_after_legacy(cx);
        }
        self.show_upload = false;
        if page == Page::Account {
            self.ensure_telegram_qr_login(cx);
        }
        if page == Page::Library {
            self.refresh_library(cx);
        } else {
            cx.notify();
        }
    }

    pub(crate) fn set_settings_section(
        &mut self,
        section: SettingsSection,
        cx: &mut Context<Self>,
    ) {
        self.settings_section = section;
        cx.notify();
    }
}

fn library_kind_for_selection(selection: &str) -> Option<FileKind> {
    match selection {
        "nav-videos" => Some(FileKind::Video),
        "nav-docs" => Some(FileKind::Document),
        "nav-archives" => Some(FileKind::Archive),
        "nav-images" => Some(FileKind::Image),
        "nav-audio" => Some(FileKind::Audio),
        "nav-disk-images" => Some(FileKind::DiskImage),
        "nav-other" => Some(FileKind::Other),
        _ => None,
    }
}

fn safe_suggested_file_name(remote_name: &str, message_id: i64) -> String {
    remote_name
        .rsplit(['/', '\\'])
        .next()
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
        .map(str::to_owned)
        .unwrap_or_else(|| format!("telegram-document-{message_id}"))
}

fn next_reserved_download_destination(
    library: &DesktopLibrary,
    suggested_name: &str,
    reserved: &mut BTreeSet<std::path::PathBuf>,
) -> Result<std::path::PathBuf, ApplicationError> {
    for suffix in 0_u32..10_000 {
        let candidate_name = if suffix == 0 {
            suggested_name.to_owned()
        } else {
            append_file_name_suffix(suggested_name, suffix)
        };
        let destination = library.next_download_destination(&candidate_name)?;
        if reserved.insert(destination.clone()) {
            return Ok(destination);
        }
    }
    Err(ApplicationError::new(
        teleark_core::ApplicationErrorKind::Capacity,
    ))
}

fn append_file_name_suffix(file_name: &str, suffix: u32) -> String {
    let path = std::path::Path::new(file_name);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(file_name);
    path.extension()
        .and_then(|extension| extension.to_str())
        .map_or_else(
            || format!("{stem} ({suffix})"),
            |extension| format!("{stem} ({suffix}).{extension}"),
        )
}

fn merge_telegram_file_page(
    files: &mut Vec<TelegramFileSummary>,
    page: TelegramFilePage,
    append: bool,
) -> (Option<i64>, bool) {
    if !append {
        *files = page.files;
    } else {
        for file in page.files {
            if let Some(existing) = files
                .iter_mut()
                .find(|existing| existing.message_id == file.message_id)
            {
                *existing = file;
            } else {
                files.push(file);
            }
        }
    }
    files.sort_by(|left, right| {
        right
            .sent_at_unix_ms
            .cmp(&left.sent_at_unix_ms)
            .then_with(|| right.message_id.cmp(&left.message_id))
    });
    (page.next_before_message_id, page.exhausted)
}

pub(crate) fn is_preview_library_selection(selection: &str) -> bool {
    matches!(selection, "collection-mac" | "collection-course")
}

impl Drop for TeleArkApp {
    fn drop(&mut self) {
        self.library_batch_cancellation
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.library_scan_cancellation.cancel();
        if let Some(vault) = self.vault.as_ref() {
            let batches = vault.active_upload_batches();
            for (account, batch) in batches {
                let _ = vault.stop_upload_batch(account, batch);
            }
        }
        self.cancel_managed_scan();
        if let Some(cancellation) = self.telegram_file_cancellation.take() {
            cancellation.cancel();
        }
    }
}

impl TeleArkApp {
    fn render_workspace_frame(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if !self.channel_sync_details {
            self.sync_history = None;
            self.sync_inspector = None;
        }
        let channel_geometry = (
            window.viewport_size().width,
            self.preferences.sidebar_collapsed,
        );
        if self.page == Page::Channel && self.channel_layout_geometry != Some(channel_geometry) {
            // Reapply the saved width against the new available space. Kit's
            // proportional resizing would otherwise shrink the preferred list.
            self.reset_channel_layout(cx);
            self.channel_layout_geometry = Some(channel_geometry);
        }
        if self.page != Page::Channel && !self.channel_layout.read(cx).sizes().is_empty() {
            self.reset_channel_layout(cx);
        }
        let layout = LayoutPolicy::from_window(window)
            .with_sidebar_collapsed(self.preferences.sidebar_collapsed)
            .with_channel_sidebar_width(
                self.channel_layout
                    .read(cx)
                    .sizes()
                    .first()
                    .map_or(f32::from(self.preferences.channel_sidebar_width), |width| {
                        f32::from(*width)
                    }),
            );
        let modal_open = self.transition.is_some()
            || self.show_upload
            || self.show_telegram_api_id_prompt
            || self.unlock_intent.is_some()
            || self.confirm_account_switch;
        if modal_open != self.modal_was_open {
            window.focus(
                if modal_open {
                    &self.modal_focus
                } else {
                    &self.main_focus
                },
                cx,
            );
            self.modal_was_open = modal_open;
        } else if window.focused(cx).is_none() {
            window.focus(&self.main_focus, cx);
        }

        div()
            .size_full()
            .track_focus(&self.main_focus)
            .relative()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .font_family(".SystemUIFont")
            .text_color(theme::text_primary())
            .on_action(cx.listener(|this, _: &DismissOverlay, window, cx| {
                if this.transition.is_some() {
                    this.cancel_transition(cx);
                    return;
                }
                if this.confirm_account_switch {
                    return;
                }
                match escape_behavior(
                    window.is_fullscreen(),
                    this.unlock_intent.is_some(),
                    this.show_upload,
                    this.show_telegram_api_id_prompt,
                    this.page == Page::Storage && this.storage_details_expanded,
                ) {
                    EscapeBehavior::ExitFullscreen => window.toggle_fullscreen(),
                    EscapeBehavior::DismissUnlock => {
                        if this.vault_activity != VaultActivity::Working {
                            this.dismiss_unlock(window, cx);
                        }
                    }
                    EscapeBehavior::DismissUpload => {
                        this.show_upload = false;
                        cx.notify();
                    }
                    EscapeBehavior::DismissTelegramApiIdPrompt => {
                        this.skip_telegram_api_id_prompt(cx);
                    }
                    EscapeBehavior::DismissStorageDetails => {
                        this.storage_details_expanded = false;
                        this.main_focus.focus(window, cx);
                        cx.notify();
                    }
                    EscapeBehavior::Ignore => {
                        this.dialogs.details = false;
                        this.channel_sync_details = false;
                        this.show_transfer_detail = false;
                        this.pending_transfer_delete = None;
                        this.pending_transfer_bulk_delete.clear();
                        cx.notify();
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &ShowAbout, _, cx| {
                this.set_settings_section(SettingsSection::About, cx);
                this.set_page(Page::Settings, cx);
            }))
            .on_action(
                cx.listener(|this, _: &ShowSettings, _, cx| this.set_page(Page::Settings, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowTransfers, _, cx| {
                this.nav_selection = "nav-transfers-all";
                this.set_page(Page::Transfers, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowStorage, _, cx| {
                this.page = Page::Storage;
                this.select_storage(StorageView::Files, cx);
            }))
            .on_action(cx.listener(|this, _: &UploadFile, _, cx| {
                this.page = Page::Storage;
                if this.storage_channel_id().is_some() {
                    this.request_vault_unlock(UnlockIntent::Upload, cx);
                } else {
                    this.select_storage(StorageView::Files, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.search_input
                    .update(cx, |input, cx| input.focus(window, cx))
            }))
            .on_action(cx.listener(|_, _: &MinimizeWindow, window, _| {
                window.minimize_window();
            }))
            .on_action(cx.listener(|_, _: &ToggleFullscreen, window, _| {
                window.toggle_fullscreen();
            }))
            .on_action(cx.listener(|_, _: &ZoomWindow, window, _| {
                window.zoom_window();
            }))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(self.page != Page::Account, |body| {
                        body.child(self.render_sidebar(layout, cx))
                    })
                    .child(self.render_workspace(window, layout, cx)),
            )
            .when(self.proxy.show_banner(), |root| {
                root.child(self.render_network_banner(cx))
            })
            .when(
                self.storage_loading
                    && self.storage_maintenance.is_some()
                    && self.page != Page::Storage,
                |root| root.child(self.render_storage_maintenance(cx)),
            )
            .when(
                self.vault_activity == VaultActivity::Working
                    && self.vault_key_progress.is_some()
                    && self.unlock_intent.is_none(),
                |root| root.child(self.render_vault_key_progress(cx)),
            )
            .when(
                self.upload_in_flight
                    || (self.page == Page::Transfers && self.upload_selection_progress.is_some()),
                |root| root.child(self.render_upload_selection_progress(false, cx)),
            )
            .when(self.upload_preparing && !self.show_upload, |root| {
                root.child(self.render_upload_selection_progress(true, cx))
            })
            .child(self.render_bandwidth_status(cx))
            .child(self.render_status_bar(cx))
            .when(self.dialogs.details, |root| {
                root.child(self.render_dialog_details(cx))
            })
            .when(
                self.confirm_account_switch && self.transition.is_none(),
                |root| root.child(self.render_account_switch_dialog(cx)),
            )
            .when(
                self.page == Page::Storage && self.storage_details_expanded,
                |root| root.child(self.render_storage_details_dialog(window, cx)),
            )
            .when(self.unlock_intent.is_some(), |root| {
                root.child(self.render_unlock_dialog(layout, cx))
            })
            .when(self.show_upload, |root| {
                root.child(screens::upload::render_upload_overlay(self, layout, cx))
            })
            .when(self.speed_limits.open, |root| {
                root.child(self.render_speed_limits(cx))
            })
            .when(self.show_telegram_api_id_prompt, |root| {
                root.child(screens::settings::render_telegram_api_id_prompt(self, cx))
            })
    }
}

fn write_recovery_key_file(path: &std::path::Path, secret: &str) -> Result<(), ApplicationError> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|error| {
        ApplicationError::new(match error.kind() {
            std::io::ErrorKind::AlreadyExists => teleark_core::ApplicationErrorKind::Conflict,
            std::io::ErrorKind::PermissionDenied => {
                teleark_core::ApplicationErrorKind::PermissionDenied
            }
            _ => teleark_core::ApplicationErrorKind::Persistence,
        })
    })?;
    file.write_all(secret.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(|_| ApplicationError::new(teleark_core::ApplicationErrorKind::Persistence))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_export_is_private_and_never_overwrites() -> Result<(), Box<dyn std::error::Error>> {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "teleark-gui-recovery-export-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&directory)?;
        let path = directory.join("recovery.txt");
        write_recovery_key_file(&path, "secret-bundle")?;
        assert_eq!(std::fs::read_to_string(&path)?, "secret-bundle\n");
        assert!(write_recovery_key_file(&path, "replacement").is_err());
        assert_eq!(std::fs::read_to_string(&path)?, "secret-bundle\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&path)?.permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_file(path)?;
        std::fs::remove_dir(directory)?;
        Ok(())
    }

    #[test]
    fn every_file_kind_facet_maps_to_exactly_one_core_kind() {
        let facets = [
            ("nav-videos", FileKind::Video),
            ("nav-docs", FileKind::Document),
            ("nav-archives", FileKind::Archive),
            ("nav-images", FileKind::Image),
            ("nav-audio", FileKind::Audio),
            ("nav-disk-images", FileKind::DiskImage),
            ("nav-other", FileKind::Other),
        ];

        for (selection, expected) in facets {
            assert_eq!(library_kind_for_selection(selection), Some(expected));
        }
        assert_eq!(library_kind_for_selection("nav-all"), None);
        assert_ne!(
            library_kind_for_selection("nav-other"),
            Some(FileKind::DiskImage)
        );
    }

    #[test]
    fn demo_collections_are_explicit_preview_selections() {
        assert!(is_preview_library_selection("collection-mac"));
        assert!(is_preview_library_selection("collection-course"));
        assert!(!is_preview_library_selection("nav-all"));
        assert!(!is_preview_library_selection("nav-other"));
    }

    #[test]
    fn escape_dismisses_the_topmost_modal_before_leaving_fullscreen() {
        assert_eq!(
            escape_behavior(true, false, false, false, true),
            EscapeBehavior::DismissStorageDetails
        );
        assert_eq!(
            escape_behavior(true, true, true, true, false),
            EscapeBehavior::DismissUnlock
        );
        assert_eq!(
            escape_behavior(true, false, true, true, false),
            EscapeBehavior::DismissUpload
        );
        assert_eq!(
            escape_behavior(false, false, true, true, false),
            EscapeBehavior::DismissUpload
        );
        assert_eq!(
            escape_behavior(false, false, false, true, false),
            EscapeBehavior::DismissTelegramApiIdPrompt
        );
        assert_eq!(
            escape_behavior(false, false, false, false, false),
            EscapeBehavior::Ignore
        );
        assert_eq!(
            escape_behavior(true, false, false, false, false),
            EscapeBehavior::ExitFullscreen
        );
    }

    #[test]
    fn suggested_download_name_cannot_escape_the_selected_directory() {
        assert_eq!(
            safe_suggested_file_name("folder/report.pdf", 9),
            "report.pdf"
        );
        assert_eq!(safe_suggested_file_name("..\\secret.zip", 9), "secret.zip");
        assert_eq!(safe_suggested_file_name("..", 9), "telegram-document-9");
    }

    #[test]
    fn channel_file_pages_replace_append_and_deduplicate_by_message() {
        let file = |message_id, name: &str| TelegramFileSummary {
            message_id,
            sent_at_unix_ms: 1,
            modified_at_unix_ms: 1,
            file_name: name.to_owned(),
            caption: String::new(),
            mime_type: None,
            size_bytes: 10,
        };
        let mut files = vec![file(99, "stale")];
        let first = TelegramFilePage {
            files: vec![file(20, "new"), file(19, "older")],
            next_before_message_id: Some(18),
            exhausted: false,
            examined_messages: 200,
        };
        assert_eq!(
            merge_telegram_file_page(&mut files, first, false),
            (Some(18), false)
        );
        assert_eq!(
            files.iter().map(|file| file.message_id).collect::<Vec<_>>(),
            vec![20, 19]
        );

        let second = TelegramFilePage {
            files: vec![file(19, "duplicate"), file(18, "oldest")],
            next_before_message_id: None,
            exhausted: true,
            examined_messages: 2,
        };
        assert_eq!(
            merge_telegram_file_page(&mut files, second, true),
            (None, true)
        );
        assert_eq!(
            files.iter().map(|file| file.message_id).collect::<Vec<_>>(),
            vec![20, 19, 18]
        );
        assert_eq!(files[1].file_name, "duplicate");
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub(crate) fn preview_app(
        cx: &mut gpui_kit::TestAppContext,
        page: Page,
    ) -> (
        gpui_kit::Entity<TeleArkApp>,
        &mut gpui_kit::VisualTestContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::window_commands::init(cx);
        });
        cx.add_window_view(move |window, cx| {
            let unavailable =
                || ApplicationError::new(teleark_core::ApplicationErrorKind::Authorization);
            TeleArkApp::new(
                window,
                cx,
                Localizer::new(SupportedLocale::EnUs).expect("catalog"),
                RuntimeStartup {
                    configuration: None,
                    library: Err(unavailable()),
                    telegram: Err(unavailable()),
                    transfers: Err(unavailable()),
                    vault: Err(unavailable()),
                },
                AppStartup {
                    page,
                    visual_preview: true,
                    show_upload: false,
                    locale: LocaleStartup {
                        system_locale: SupportedLocale::EnUs,
                        follows_system_locale: false,
                    },
                },
            )
        })
    }
}

impl Render for TeleArkApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_access_menus(cx);
        self.schedule_pin_work(window, cx);
        if self.app_is_locked() {
            return self.render_app_lock_screen(window, cx);
        }
        if !self.telegram_is_authorized() {
            return self.render_signed_out_screen(window, cx);
        }
        if !self.channel_sync_details {
            self.sync_history = None;
            self.sync_inspector = None;
        }
        let workspace = self
            .workspace_view
            .get_or_insert_with(|| {
                let owner = cx.entity();
                cx.new(|cx| workspace_view::WorkspaceView::new(owner, cx))
            })
            .clone();
        div()
            .size_full()
            .relative()
            .child(workspace)
            .when(self.channel_sync_details, |root| {
                let inspector = self
                    .sync_inspector
                    .get_or_insert_with(|| {
                        let owner = cx.weak_entity();
                        cx.new(|_| workspace_view::SyncInspector::new(owner))
                    })
                    .clone();
                root.child(inspector)
            })
            .when(self.transition.is_some(), |root| {
                root.child(self.render_transition_dialog(cx))
            })
            .into_any_element()
    }
}
