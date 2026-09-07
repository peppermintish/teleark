mod auth;
mod background;
mod browser;
mod library;
mod local_files;
mod navigation;
mod preferences;
mod preview;
mod storage;
mod vault;

use std::{collections::BTreeSet, time::Duration};

use gpui_kit::component::{
    Icon, IconName, WindowExt as _,
    input::{InputEvent, InputState},
    notification::Notification,
    table::{TableEvent, TableState},
};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Subscription, Task,
    Window, div, prelude::FluentBuilder as _, px,
};
use teleark_core::{
    ApplicationError, FileKind, LibraryFilter, LibraryPage, LibraryQuery, LibrarySort,
    LibraryStatistics,
};
use teleark_i18n::{
    Localizer, MessageArgs, MessageId, SupportedLocale,
    format::{format_bytes, format_speed},
};
use teleark_runtime::{
    AppearancePreference, ChannelDownloadRequest, ChannelDownloadState, DesktopLibrary,
    DesktopPreferences, DesktopTelegram, DesktopTransfers, DesktopVault, ManagedStorageMetrics,
    ManagedVaultFile, TelegramAuthState, TelegramChatSummary, TelegramCredentialSource,
    TelegramFilePage, TelegramFileSummary, TelegramIndexPage, TelegramScanCancellation,
    VaultStatus,
};
use teleark_runtime::{TelegramAccount, TelegramChatKind};

use crate::{
    DismissOverlay, FocusSearch, MinimizeWindow, RefreshPage, ShowAbout, ShowSettings, ShowStorage,
    ShowTransfers, ToggleFullscreen, UploadFile, ZoomWindow,
    components::{self, Tone},
    layout::LayoutPolicy,
    library_state::{ImportActivity, ImportFeedback, LibraryContent, LibrarySnapshot},
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
    Ignore,
}

fn escape_behavior(
    fullscreen: bool,
    unlock_visible: bool,
    upload_visible: bool,
    telegram_api_id_prompt_visible: bool,
) -> EscapeBehavior {
    if unlock_visible {
        EscapeBehavior::DismissUnlock
    } else if upload_visible {
        EscapeBehavior::DismissUpload
    } else if telegram_api_id_prompt_visible {
        EscapeBehavior::DismissTelegramApiIdPrompt
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

pub struct RuntimeStartup {
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
    pub(crate) page: Page,
    pub(crate) storage_status: teleark_runtime::StorageChannelStatus,
    pub(crate) storage_loading: bool,
    pub(crate) storage_error: Option<teleark_core::ApplicationErrorKind>,
    pub(crate) show_storage_guide: bool,
    pub(crate) settings_advanced_expanded: bool,
    pub(crate) about_show_licenses: bool,
    pub(crate) upload_advanced_expanded: bool,
    pub(crate) vault_advanced_expanded: bool,
    pub(crate) account_restoring: bool,
    pub(crate) transfers_account_ready: bool,
    pub(crate) account_avatar: Option<std::sync::Arc<gpui_kit::Image>>,
    pub(crate) show_account_switch: bool,
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
    pub(crate) selected_file: usize,
    pub(crate) selected_transfer_keys: BTreeSet<u64>,
    pub(crate) pending_transfer_delete: Option<u64>,
    pub(crate) pending_transfer_bulk_delete: Vec<u64>,
    pub(crate) transfer_action_error: Option<teleark_core::ApplicationErrorKind>,
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
    pub(crate) managed_vault_files: Vec<ManagedVaultFile>,
    pub(crate) managed_vault_rejected: usize,
    pub(crate) upload_sources: Vec<teleark_runtime::VaultUploadSource>,
    pub(crate) upload_preparing: bool,
    pub(crate) nav_selection: &'static str,
    pub(crate) storage_view: StorageView,
    pub(crate) library_content: LibraryContent,
    pub(crate) import_activity: ImportActivity,
    pub(crate) import_feedback: Option<ImportFeedback>,
    pub(crate) library_loading_more: bool,
    pub(crate) library_load_more_error: Option<teleark_core::ApplicationErrorKind>,
    pub(crate) follows_system_locale: bool,
    pub(crate) system_locale: SupportedLocale,
    pub(crate) locale_persistence: LocalePersistence,
    pub(crate) telegram_auth: TelegramAuthState,
    pub(crate) telegram_activity: TelegramActivity,
    pub(crate) telegram_account: Option<TelegramAccount>,
    pub(crate) telegram_chats: Vec<TelegramChatSummary>,
    pub(crate) selected_chat_id: Option<i64>,
    pub(crate) last_channel_id: Option<i64>,
    pub(crate) preview_transfer_rows: Vec<crate::mock::TransferRow>,
    pub(crate) telegram_index: Option<TelegramIndexPage>,
    pub(crate) telegram_files: Vec<TelegramFileSummary>,
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
    pub(crate) settings_section: SettingsSection,
    pub(crate) preferences: DesktopPreferences,
    pub(crate) preference_persistence: PreferencePersistence,
    pub(crate) volume_space: Option<teleark_runtime::VolumeSpace>,
    pub(crate) overall_storage_metrics: Option<ManagedStorageMetrics>,
    library: Option<DesktopLibrary>,
    telegram: Option<DesktopTelegram>,
    pub(crate) transfers: Option<DesktopTransfers>,
    pub(crate) vault: Option<DesktopVault>,
    library_query_generation: u64,
    pending_locale_override: Option<LocaleOverrideChoice>,
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
    storage_metrics_task: Option<Task<()>>,
    volume_space_task: Option<Task<()>>,
    local_files_task: Option<Task<()>>,
    pub(crate) local_downloads:
        std::collections::BTreeMap<std::path::PathBuf, local_files::LocalDownloadObservation>,
    transfer_monitor_task: Option<Task<()>>,
    transfer_refresh_task: Option<Task<()>>,
    vault_task: Option<Task<()>>,
    pub(crate) managed_scan_loading: bool,
    managed_scan_generation: u64,
    managed_scan_cancellation: Option<TelegramScanCancellation>,
    vault_scan_task: Option<Task<()>>,
    vault_download_task: Option<Task<()>>,
    upload_picker_task: Option<Task<()>>,
    qr_poll_task: Option<Task<()>>,
    telegram_login_generation: u64,
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
            library,
            telegram,
            transfers,
            vault,
        } = runtime;
        let credential_status =
            library
                .as_ref()
                .map_err(|error| error.kind())
                .and_then(|library| {
                    telegram
                        .as_ref()
                        .map_err(|error| error.kind())?
                        .effective_credentials_status(library)
                        .map_err(|error| error.kind())
                });
        let (configured_telegram_api_id, telegram_credential_source, telegram_api_id_persistence) =
            match credential_status {
                Ok(status) => (
                    status.map(|status| status.api_id),
                    status.map(|status| status.source),
                    TelegramApiIdPersistence::Idle,
                ),
                Err(kind) => (None, None, TelegramApiIdPersistence::Failed(kind)),
            };
        let (preferences, preference_persistence) = match library.as_ref() {
            Ok(library) => match library.preferences() {
                Ok(preferences) => (preferences, PreferencePersistence::Idle),
                Err(_) => (DesktopPreferences::default(), PreferencePersistence::Failed),
            },
            Err(_) => (DesktopPreferences::default(), PreferencePersistence::Failed),
        };
        theme::apply_appearance(preferences.appearance, window, cx);
        let appearance_subscription = cx.observe_window_appearance(window, |this, window, cx| {
            if this.preferences.appearance == AppearancePreference::System {
                theme::apply_appearance(AppearancePreference::System, window, cx);
                cx.notify();
            }
        });
        let activation_subscription = cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active()
                && !window.has_active_prompt()
                && this.preferences.lock_vault_when_hidden
            {
                this.clear_vault_inputs(window, cx);
                this.lock_vault(cx);
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
                configured: false,
                locked: true,
                created_at_unix_ms: None,
                password_generation: None,
                recovery_generation: None,
            });
        let mut app = Self {
            page,
            storage_status: teleark_runtime::StorageChannelStatus::Missing,
            storage_loading: false,
            storage_error: None,
            show_storage_guide: false,
            settings_advanced_expanded: false,
            about_show_licenses: false,
            upload_advanced_expanded: false,
            vault_advanced_expanded: false,
            account_restoring: false,
            transfers_account_ready: false,
            account_avatar: None,
            show_account_switch: false,
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
            selected_file: 0,
            selected_transfer_keys: BTreeSet::new(),
            pending_transfer_delete: None,
            pending_transfer_bulk_delete: Vec::new(),
            transfer_action_error: None,
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
            managed_vault_files: Vec::new(),
            managed_vault_rejected: 0,
            upload_sources: Vec::new(),
            upload_preparing: false,
            nav_selection,
            storage_view: StorageView::Files,
            library_content,
            import_activity: ImportActivity::Idle,
            import_feedback: None,
            library_loading_more: false,
            library_load_more_error: None,
            follows_system_locale: locale_startup.follows_system_locale,
            system_locale: locale_startup.system_locale,
            locale_persistence,
            telegram_auth: TelegramAuthState::Disconnected,
            telegram_activity: if telegram.is_ok() {
                TelegramActivity::Idle
            } else {
                TelegramActivity::Failed(teleark_core::ApplicationErrorKind::Persistence)
            },
            telegram_account: None,
            telegram_chats: Vec::new(),
            selected_chat_id: None,
            last_channel_id: None,
            preview_transfer_rows: Vec::new(),
            telegram_index: None,
            telegram_files: Vec::new(),
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
            settings_section: SettingsSection::General,
            preferences,
            preference_persistence,
            volume_space: None,
            overall_storage_metrics: None,
            library,
            telegram: telegram.ok(),
            transfers: transfers.ok(),
            vault: vault.ok(),
            library_query_generation: 0,
            pending_locale_override: None,
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
            storage_metrics_task: None,
            volume_space_task: None,
            local_files_task: None,
            local_downloads: Default::default(),
            transfer_monitor_task: None,
            transfer_refresh_task: None,
            vault_task: None,
            managed_scan_loading: false,
            managed_scan_generation: 0,
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
        if app.library.is_some() && matches!(app.page, Page::Library | Page::FileDetail) {
            app.refresh_library(cx);
        }
        if app.visual_preview {
            app.initialize_preview(window, cx);
        }
        app.start_transfer_refresh(cx);
        app.start_storage_metrics_refresh(cx);
        app.start_local_file_refresh(cx);
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
        self.show_transfer_detail = false;
        self.pending_transfer_delete = None;
        self.pending_transfer_bulk_delete.clear();
        self.selected_transfer_keys.clear();
        if matches!(
            self.page,
            Page::Channel | Page::Storage | Page::LegacyRecovery
        ) && self.page != page
        {
            self.cancel_telegram_file_load(cx);
        }
        if self.page != page {
            self.cancel_managed_scan();
        }
        self.page = page;
        self.show_upload = false;
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
        if let Some(vault) = self.vault.as_ref() {
            let batches = vault
                .transfers()
                .into_iter()
                .filter_map(|item| item.batch_id.map(|batch| (item.account_id, batch)))
                .collect::<std::collections::BTreeSet<_>>();
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

impl Render for TeleArkApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = LayoutPolicy::from_window(window)
            .with_sidebar_collapsed(self.preferences.sidebar_collapsed);
        let modal_open =
            self.show_upload || self.show_telegram_api_id_prompt || self.unlock_intent.is_some();
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
                match escape_behavior(
                    window.is_fullscreen(),
                    this.unlock_intent.is_some(),
                    this.show_upload,
                    this.show_telegram_api_id_prompt,
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
                    EscapeBehavior::Ignore => {
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
            .on_action(cx.listener(|this, _: &RefreshPage, _, cx| match this.page {
                Page::Storage | Page::LegacyRecovery if this.storage_view == StorageView::Files => {
                    this.scan_managed_vault_files(cx)
                }
                Page::Storage | Page::LegacyRecovery | Page::Channel => {
                    this.load_selected_telegram_files(false, cx)
                }
                Page::Library => this.refresh_library(cx),
                _ => this.load_telegram_dialogs(cx),
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
                    .when(self.page == Page::Channel, |body| {
                        body.child(self.render_channels_sidebar(cx))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .when(self.page != Page::Account, |body| {
                                body.child(self.render_header(window, layout, cx))
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .child(self.render_page(window, layout, cx)),
                            )
                            .when(self.page != Page::Account, |body| {
                                body.child(self.render_status_bar(cx))
                            }),
                    ),
            )
            .when(self.unlock_intent.is_some(), |root| {
                root.child(self.render_unlock_dialog(layout, cx))
            })
            .when(self.show_upload, |root| {
                root.child(screens::upload::render_upload_overlay(self, layout, cx))
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
            escape_behavior(true, true, true, true),
            EscapeBehavior::DismissUnlock
        );
        assert_eq!(
            escape_behavior(true, false, true, true),
            EscapeBehavior::DismissUpload
        );
        assert_eq!(
            escape_behavior(false, false, true, true),
            EscapeBehavior::DismissUpload
        );
        assert_eq!(
            escape_behavior(false, false, false, true),
            EscapeBehavior::DismissTelegramApiIdPrompt
        );
        assert_eq!(
            escape_behavior(false, false, false, false),
            EscapeBehavior::Ignore
        );
        assert_eq!(
            escape_behavior(true, false, false, false),
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
