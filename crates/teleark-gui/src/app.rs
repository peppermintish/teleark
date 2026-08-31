use std::collections::BTreeSet;

use gpui::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Timer, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    Icon, IconName, WindowExt as _,
    input::{InputEvent, InputState},
    notification::Notification,
    scroll::ScrollableElement as _,
};
use teleark_core::{
    ApplicationError, FileKind, LibraryFilter, LibraryPage, LibraryQuery, LibrarySort,
    LibraryStatistics,
};
use teleark_i18n::{
    Localizer, MessageArgs, MessageId, SupportedLocale,
    format::{format_bytes, format_integer, format_speed},
};
use teleark_runtime::{
    AppearancePreference, ChannelDownloadRequest, ChannelDownloadState, DesktopLibrary,
    DesktopPreferences, DesktopTelegram, DesktopTransfers, ManagedStorageMetrics,
    TelegramAuthState, TelegramChatSummary, TelegramCredentialSource, TelegramFileFilter,
    TelegramFilePage, TelegramFileSummary, TelegramIndexPage,
};
use teleark_telegram::TelegramAccount;

use crate::{
    DismissOverlay, MinimizeWindow, ToggleFullscreen, ZoomWindow,
    components::{self, Tone},
    layout::LayoutPolicy,
    library_state::{ImportActivity, ImportFeedback, LibraryContent, LibrarySnapshot},
    screens, theme,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Page {
    Library,
    Transfers,
    FileDetail,
    Vault,
    Channel,
    Settings,
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EscapeBehavior {
    ExitFullscreen,
    DismissUpload,
    DismissTelegramApiIdPrompt,
    Ignore,
}

fn escape_behavior(
    fullscreen: bool,
    upload_visible: bool,
    telegram_api_id_prompt_visible: bool,
) -> EscapeBehavior {
    if fullscreen {
        EscapeBehavior::ExitFullscreen
    } else if upload_visible {
        EscapeBehavior::DismissUpload
    } else if telegram_api_id_prompt_visible {
        EscapeBehavior::DismissTelegramApiIdPrompt
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

fn should_show_telegram_credentials_prompt(
    skip_prompt: bool,
    configured_api_id: Option<i32>,
) -> bool {
    !skip_prompt && !telegram_login_controls_enabled(configured_api_id)
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
    pub show_upload: bool,
    pub skip_telegram_api_id_prompt: bool,
    pub locale: LocaleStartup,
}

pub struct RuntimeStartup {
    pub library: Result<DesktopLibrary, ApplicationError>,
    pub telegram: Result<DesktopTelegram, ApplicationError>,
    pub transfers: Result<DesktopTransfers, ApplicationError>,
}

struct CompactNavItem {
    id: &'static str,
    label_id: &'static str,
    icon: IconName,
    target: Page,
    nav_selection: &'static str,
}

pub struct TeleArkApp {
    pub(crate) page: Page,
    pub(crate) localizer: Localizer,
    pub(crate) search_input: Entity<InputState>,
    pub(crate) show_upload: bool,
    pub(crate) upload_queued: bool,
    pub(crate) transfer_paused: bool,
    pub(crate) selected_file: usize,
    pub(crate) selected_transfer_keys: BTreeSet<u64>,
    pub(crate) expanded_transfer_batches: BTreeSet<u64>,
    pub(crate) vault_locked: bool,
    pub(crate) recovery_visible: bool,
    pub(crate) nav_selection: &'static str,
    pub(crate) selected_source: &'static str,
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
    pub(crate) telegram_index: Option<TelegramIndexPage>,
    pub(crate) telegram_files: Vec<TelegramFileSummary>,
    pub(crate) telegram_files_next: Option<i64>,
    pub(crate) telegram_files_exhausted: bool,
    pub(crate) telegram_files_loading: bool,
    pub(crate) telegram_download: Option<(u64, ChannelDownloadState)>,
    pub(crate) selected_telegram_message_id: Option<i64>,
    pub(crate) channel_batch_period: ChannelBatchPeriod,
    pub(crate) channel_batch_kind: Option<FileKind>,
    pub(crate) channel_batch_activity: ChannelBatchActivity,
    pub(crate) channel_batch_expanded: bool,
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
    pub(crate) overall_storage_metrics: Option<ManagedStorageMetrics>,
    library: Option<DesktopLibrary>,
    telegram: Option<DesktopTelegram>,
    pub(crate) transfers: Option<DesktopTransfers>,
    library_query_generation: u64,
    pending_locale_override: Option<LocaleOverrideChoice>,
    library_task: Option<Task<()>>,
    library_more_task: Option<Task<()>>,
    import_task: Option<Task<()>>,
    locale_task: Option<Task<()>>,
    telegram_api_id_task: Option<Task<()>>,
    telegram_task: Option<Task<()>>,
    telegram_file_task: Option<Task<()>>,
    telegram_download_task: Option<Task<()>>,
    telegram_batch_task: Option<Task<()>>,
    preference_task: Option<Task<()>>,
    preference_picker_task: Option<Task<()>>,
    storage_metrics_task: Option<Task<()>>,
    transfer_monitor_task: Option<Task<()>>,
    transfer_refresh_task: Option<Task<()>>,
    qr_poll_task: Option<Task<()>>,
    telegram_login_generation: u64,
    telegram_file_generation: u64,
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
            show_upload,
            skip_telegram_api_id_prompt,
            locale: locale_startup,
        } = startup;
        let RuntimeStartup {
            library,
            telegram,
            transfers,
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
        let search_subscription = cx.subscribe(&search_input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) && this.page == Page::Library {
                this.refresh_library(cx);
            } else {
                cx.notify();
            }
        });
        let nav_selection = match page {
            Page::Library | Page::FileDetail => "nav-all",
            Page::Transfers => "nav-transfers-all",
            Page::Vault => "nav-vault",
            Page::Channel => "nav-telegram-sources",
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

        let mut app = Self {
            page,
            localizer,
            search_input,
            show_upload,
            upload_queued: false,
            transfer_paused: false,
            selected_file: 0,
            selected_transfer_keys: BTreeSet::new(),
            expanded_transfer_batches: BTreeSet::new(),
            vault_locked: true,
            recovery_visible: false,
            nav_selection,
            selected_source: "Cinema 4K",
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
            telegram_index: None,
            telegram_files: Vec::new(),
            telegram_files_next: None,
            telegram_files_exhausted: false,
            telegram_files_loading: false,
            telegram_download: None,
            selected_telegram_message_id: None,
            channel_batch_period: ChannelBatchPeriod::AnyTime,
            channel_batch_kind: None,
            channel_batch_activity: ChannelBatchActivity::Idle,
            channel_batch_expanded: false,
            telegram_api_id,
            telegram_api_hash,
            telegram_phone,
            telegram_code,
            telegram_password,
            configured_telegram_api_id,
            telegram_credential_source,
            telegram_api_id_persistence,
            show_telegram_api_id_prompt: should_show_telegram_credentials_prompt(
                skip_telegram_api_id_prompt,
                configured_telegram_api_id,
            ),
            settings_section: SettingsSection::General,
            preferences,
            preference_persistence,
            overall_storage_metrics: None,
            library,
            telegram: telegram.ok(),
            transfers: transfers.ok(),
            library_query_generation: 0,
            pending_locale_override: None,
            library_task: None,
            library_more_task: None,
            import_task: None,
            locale_task: None,
            telegram_api_id_task: None,
            telegram_task: None,
            telegram_file_task: None,
            telegram_download_task: None,
            telegram_batch_task: None,
            preference_task: None,
            preference_picker_task: None,
            storage_metrics_task: None,
            transfer_monitor_task: None,
            transfer_refresh_task: None,
            qr_poll_task: None,
            telegram_login_generation: 0,
            telegram_file_generation: 0,
            _subscriptions: vec![search_subscription, appearance_subscription],
        };
        if app.library.is_some() {
            app.refresh_library(cx);
        }
        app.start_transfer_refresh(cx);
        app.start_storage_metrics_refresh(cx);
        app
    }

    pub(crate) fn begin_telegram_login(&mut self, cx: &mut Context<Self>) {
        if self.telegram_activity == TelegramActivity::Working {
            return;
        }
        let Some(telegram) = self.telegram.clone() else {
            self.telegram_activity =
                TelegramActivity::Failed(teleark_core::ApplicationErrorKind::Persistence);
            cx.notify();
            return;
        };
        let Some(library) = self.library.clone() else {
            self.telegram_activity =
                TelegramActivity::Failed(teleark_core::ApplicationErrorKind::Persistence);
            cx.notify();
            return;
        };
        let phone = self.telegram_phone.read(cx).value().to_string();
        self.telegram_activity = TelegramActivity::Working;
        cx.notify();
        let work = cx.background_spawn(async move {
            match telegram.connect_configured(&library)? {
                TelegramAuthState::Authorized(account) => {
                    Ok::<_, ApplicationError>(TelegramAuthState::Authorized(account))
                }
                TelegramAuthState::Unauthorized => {
                    telegram.request_login_code_configured(&library, phone)
                }
                _ => Err(ApplicationError::new(
                    teleark_core::ApplicationErrorKind::Conflict,
                )),
            }
        });
        self.telegram_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| this.apply_telegram_auth_result(result, cx))
                .ok();
        }));
    }

    pub(crate) fn begin_telegram_qr_login(&mut self, cx: &mut Context<Self>) {
        if self.telegram_activity == TelegramActivity::Working {
            return;
        }
        let Some(telegram) = self.telegram.clone() else {
            self.telegram_activity =
                TelegramActivity::Failed(teleark_core::ApplicationErrorKind::Persistence);
            cx.notify();
            return;
        };
        let Some(library) = self.library.clone() else {
            self.telegram_activity =
                TelegramActivity::Failed(teleark_core::ApplicationErrorKind::Persistence);
            cx.notify();
            return;
        };
        self.telegram_login_generation = self.telegram_login_generation.wrapping_add(1);
        let generation = self.telegram_login_generation;
        self.telegram_activity = TelegramActivity::Working;
        cx.notify();
        let work = cx.background_spawn(async move {
            match telegram.connect_configured(&library)? {
                TelegramAuthState::Authorized(account) => {
                    Ok::<_, ApplicationError>(TelegramAuthState::Authorized(account))
                }
                TelegramAuthState::Unauthorized => telegram.begin_qr_login_configured(&library),
                _ => Err(ApplicationError::new(
                    teleark_core::ApplicationErrorKind::Conflict,
                )),
            }
        });
        self.telegram_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.telegram_login_generation == generation {
                    this.apply_telegram_auth_result(result, cx);
                }
            })
            .ok();
        }));
    }

    pub(crate) fn refresh_telegram_qr_login(&mut self, cx: &mut Context<Self>) {
        if self.telegram_activity == TelegramActivity::Working {
            return;
        }
        let Some(telegram) = self.telegram.clone() else {
            return;
        };
        let Some(library) = self.library.clone() else {
            self.telegram_activity =
                TelegramActivity::Failed(teleark_core::ApplicationErrorKind::Persistence);
            cx.notify();
            return;
        };
        self.telegram_activity = TelegramActivity::Working;
        cx.notify();
        let work = cx.background_spawn(async move { telegram.begin_qr_login_configured(&library) });
        self.telegram_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| this.apply_telegram_auth_result(result, cx))
                .ok();
        }));
    }

    pub(crate) fn submit_telegram_code(&mut self, cx: &mut Context<Self>) {
        let code = self.telegram_code.read(cx).value().to_string();
        self.submit_telegram_secret(code.into_bytes(), false, cx);
    }

    pub(crate) fn submit_telegram_password(&mut self, cx: &mut Context<Self>) {
        let password = self.telegram_password.read(cx).value().to_string();
        self.submit_telegram_secret(password.into_bytes(), true, cx);
    }

    fn submit_telegram_secret(&mut self, secret: Vec<u8>, password: bool, cx: &mut Context<Self>) {
        if self.telegram_activity == TelegramActivity::Working {
            return;
        }
        let Some(telegram) = self.telegram.clone() else {
            return;
        };
        self.telegram_activity = TelegramActivity::Working;
        cx.notify();
        let work = cx.background_spawn(async move {
            if password {
                telegram.submit_password(secret)
            } else {
                telegram.submit_code(String::from_utf8_lossy(&secret).into_owned())
            }
        });
        self.telegram_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| this.apply_telegram_auth_result(result, cx))
                .ok();
        }));
    }

    fn apply_telegram_auth_result(
        &mut self,
        result: Result<TelegramAuthState, ApplicationError>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(state) => {
                self.telegram_activity = TelegramActivity::Idle;
                if let TelegramAuthState::Authorized(account) = &state {
                    self.telegram_account = Some(account.clone());
                }
                self.telegram_auth = state;
                if matches!(self.telegram_auth, TelegramAuthState::QrCode { .. }) {
                    self.schedule_qr_login_poll(cx);
                } else if matches!(self.telegram_auth, TelegramAuthState::Authorized(_)) {
                    self.load_telegram_dialogs(cx);
                }
            }
            Err(error) => self.telegram_activity = TelegramActivity::Failed(error.kind()),
        }
        cx.notify();
    }

    fn schedule_qr_login_poll(&mut self, cx: &mut Context<Self>) {
        let Some(telegram) = self.telegram.clone() else {
            return;
        };
        let generation = self.telegram_login_generation;
        self.qr_poll_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(std::time::Duration::from_millis(750)).await;
            let poll = cx.background_spawn(async move { telegram.poll_qr_login() });
            let result = poll.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.telegram_login_generation == generation
                    && matches!(this.telegram_auth, TelegramAuthState::QrCode { .. })
                {
                    this.apply_telegram_auth_result(result, cx);
                }
            })
            .ok();
        }));
    }

    fn load_telegram_dialogs(&mut self, cx: &mut Context<Self>) {
        let (Some(telegram), Some(library), Some(account)) = (
            self.telegram.clone(),
            self.library.clone(),
            self.telegram_account.clone(),
        ) else {
            return;
        };
        self.telegram_activity = TelegramActivity::Working;
        let work = cx.background_spawn(async move {
            let chats = telegram.list_dialogs()?;
            library.save_telegram_sources(&account, &chats)?;
            Ok::<_, ApplicationError>(chats)
        });
        self.telegram_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                match result {
                    Ok(chats) => {
                        this.selected_chat_id = chats.first().map(|chat| chat.id);
                        this.telegram_chats = chats;
                        this.telegram_activity = TelegramActivity::Idle;
                        this.telegram_files.clear();
                        this.telegram_files_next = None;
                        this.telegram_files_exhausted = false;
                        this.telegram_file_generation =
                            this.telegram_file_generation.wrapping_add(1);
                        if let Some(transfers) = this.transfers.as_ref() {
                            let _ = transfers.activate_pending_downloads();
                        }
                        this.load_selected_telegram_files(false, cx);
                    }
                    Err(error) => this.telegram_activity = TelegramActivity::Failed(error.kind()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(crate) fn select_telegram_chat(&mut self, chat_id: i64, cx: &mut Context<Self>) {
        self.selected_chat_id = Some(chat_id);
        self.telegram_index = None;
        self.telegram_files.clear();
        self.telegram_files_next = None;
        self.telegram_files_exhausted = false;
        self.telegram_files_loading = false;
        self.telegram_file_generation = self.telegram_file_generation.wrapping_add(1);
        self.telegram_download = None;
        self.selected_telegram_message_id = None;
        self.channel_batch_activity = ChannelBatchActivity::Idle;
        self.channel_batch_expanded = false;
        self.load_selected_telegram_files(false, cx);
    }

    pub(crate) fn load_selected_telegram_files(&mut self, append: bool, cx: &mut Context<Self>) {
        if self.telegram_files_loading || (append && self.telegram_files_exhausted) {
            return;
        }
        let (Some(telegram), Some(chat_id)) = (self.telegram.clone(), self.selected_chat_id) else {
            return;
        };
        let before = append.then_some(self.telegram_files_next).flatten();
        let generation = self.telegram_file_generation;
        self.telegram_files_loading = true;
        self.telegram_activity = TelegramActivity::Working;
        cx.notify();
        let work =
            cx.background_spawn(async move { telegram.scan_file_page(chat_id, before, 200) });
        self.telegram_file_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.selected_chat_id != Some(chat_id)
                    || this.telegram_file_generation != generation
                {
                    return;
                }
                this.telegram_files_loading = false;
                match result {
                    Ok(page) => {
                        let (next, exhausted) =
                            merge_telegram_file_page(&mut this.telegram_files, page, append);
                        this.telegram_files_next = next;
                        this.telegram_files_exhausted = exhausted;
                        this.telegram_activity = TelegramActivity::Idle;
                    }
                    Err(error) => {
                        this.telegram_activity = TelegramActivity::Failed(error.kind());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(crate) fn download_telegram_file(&mut self, message_id: i64, cx: &mut Context<Self>) {
        let (Some(library), Some(transfers), Some(chat_id), Some(file)) = (
            self.library.clone(),
            self.transfers.clone(),
            self.selected_chat_id,
            self.telegram_files
                .iter()
                .find(|file| file.message_id == message_id)
                .cloned(),
        ) else {
            self.telegram_activity =
                TelegramActivity::Failed(teleark_core::ApplicationErrorKind::InvalidRequest);
            cx.notify();
            return;
        };
        let suggested_name = safe_suggested_file_name(&file.file_name, file.message_id);
        let destination_work = cx.background_spawn({
            let suggested_name = suggested_name.clone();
            async move { library.next_download_destination(&suggested_name) }
        });
        self.telegram_download_task = Some(cx.spawn(async move |this, cx| {
            let destination = match destination_work.await {
                Ok(destination) => destination,
                Err(error) => {
                    let Some(this) = this.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.telegram_activity = TelegramActivity::Failed(error.kind());
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                this.enqueue_telegram_download(
                    transfers,
                    chat_id,
                    file,
                    suggested_name,
                    destination,
                    cx,
                );
            })
            .ok();
        }));
    }

    fn enqueue_telegram_download(
        &mut self,
        transfers: DesktopTransfers,
        chat_id: i64,
        file: TelegramFileSummary,
        file_name: String,
        destination: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        match transfers.enqueue_channel_download(ChannelDownloadRequest {
            chat_id,
            message_id: file.message_id,
            message_sent_at_unix_ms: Some(file.sent_at_unix_ms),
            file_name,
            caption: (!file.caption.is_empty()).then_some(file.caption),
            mime_type: file.mime_type,
            size_bytes: file.size_bytes,
            destination,
        }) {
            Ok(id) => {
                self.telegram_download = Some((id, ChannelDownloadState::Queued));
                self.telegram_activity = TelegramActivity::Idle;
                self.monitor_channel_download(id, cx);
            }
            Err(error) => {
                self.telegram_activity = TelegramActivity::Failed(error.kind());
            }
        }
        cx.notify();
    }

    fn monitor_channel_download(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(transfers) = self.transfers.clone() else {
            return;
        };
        self.transfer_monitor_task = Some(cx.spawn(async move |this, cx| {
            loop {
                Timer::after(std::time::Duration::from_millis(250)).await;
                let snapshots = transfers.snapshots();
                let Some(entity) = this.upgrade() else { return };
                let outcome = entity
                    .update(cx, |this, cx| {
                        let snapshot = snapshots.ok().and_then(|snapshots| {
                            snapshots.into_iter().find(|snapshot| snapshot.id == id)
                        });
                        let state = snapshot.as_ref().map(|snapshot| snapshot.state).unwrap_or(
                            ChannelDownloadState::Failed(
                                teleark_core::ApplicationErrorKind::Persistence,
                            ),
                        );
                        this.telegram_download = Some((id, state));
                        let file_name = snapshot
                            .as_ref()
                            .map(|snapshot| snapshot.file_name.clone())
                            .unwrap_or_else(|| this.tr("transfer-value-unavailable").to_string());
                        let notification = match state {
                            ChannelDownloadState::Completed
                                if this.preferences.notify_download_completed =>
                            {
                                Some((
                                    true,
                                    this.tr_with(
                                        "notification-download-completed",
                                        MessageArgs::new().with("name", file_name.clone()),
                                    ),
                                ))
                            }
                            ChannelDownloadState::Failed(_)
                                if this.preferences.notify_download_failed =>
                            {
                                Some((
                                    false,
                                    this.tr_with(
                                        "notification-download-failed",
                                        MessageArgs::new().with("name", file_name),
                                    ),
                                ))
                            }
                            _ => None,
                        };
                        let reveal_path = (state == ChannelDownloadState::Completed
                            && this.preferences.reveal_completed_downloads)
                            .then(|| snapshot.map(|snapshot| snapshot.destination))
                            .flatten();
                        cx.notify();
                        (
                            matches!(
                                state,
                                ChannelDownloadState::Completed
                                    | ChannelDownloadState::Failed(_)
                                    | ChannelDownloadState::Cancelled
                            ),
                            notification,
                            reveal_path,
                        )
                    })
                    .unwrap_or((true, None, None));
                let (terminal, notification, reveal_path) = outcome;
                if notification.is_some() || reveal_path.is_some() {
                    cx.update(|cx| {
                        if let Some(path) = reveal_path {
                            cx.reveal_path(&path);
                        }
                        if let Some((success, message)) = notification
                            && let Some(handle) = cx.active_window()
                        {
                            let _ = handle.update(cx, |_, window, cx| {
                                window.push_notification(
                                    if success {
                                        Notification::success(message)
                                    } else {
                                        Notification::error(message)
                                    },
                                    cx,
                                );
                            });
                        }
                    })
                    .ok();
                }
                if terminal {
                    break;
                }
            }
        }));
    }

    fn start_transfer_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(transfers) = self.transfers.clone() else {
            return;
        };
        self.transfer_refresh_task = Some(cx.spawn(async move |this, cx| {
            let mut previous = Vec::new();
            loop {
                Timer::after(std::time::Duration::from_millis(250)).await;
                let Some(entity) = this.upgrade() else { return };
                let authorized = entity
                    .update(cx, |this, _cx| {
                        matches!(this.telegram_auth, TelegramAuthState::Authorized(_))
                    })
                    .unwrap_or(false);
                if authorized {
                    let _ = transfers.activate_pending_downloads();
                }
                let snapshots = transfers.snapshots().unwrap_or_default();
                let signature: Vec<_> = snapshots
                    .iter()
                    .map(|snapshot| {
                        (
                            snapshot.id,
                            snapshot.state,
                            snapshot.transferred_bytes,
                            snapshot.current_bytes_per_second,
                            snapshot.eta_ms,
                        )
                    })
                    .collect();
                if signature == previous {
                    continue;
                }
                previous = signature;
                if entity.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
        }));
    }

    fn start_storage_metrics_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(library) = self.library.clone() else {
            return;
        };
        self.storage_metrics_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let library = library.clone();
                let metrics = cx
                    .background_spawn(async move { library.managed_storage_metrics() })
                    .await
                    .ok();
                let Some(entity) = this.upgrade() else { return };
                if entity
                    .update(cx, |this, cx| {
                        this.overall_storage_metrics = metrics;
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                Timer::after(std::time::Duration::from_secs(15)).await;
            }
        }));
    }

    pub(crate) fn index_selected_telegram_chat(&mut self, cx: &mut Context<Self>) {
        if self.telegram_activity == TelegramActivity::Working {
            return;
        }
        let (Some(telegram), Some(library), Some(account), Some(chat_id)) = (
            self.telegram.clone(),
            self.library.clone(),
            self.telegram_account.clone(),
            self.selected_chat_id,
        ) else {
            return;
        };
        let before = self
            .telegram_index
            .as_ref()
            .and_then(|page| page.next_before_message_id);
        let batch_size = usize::from(self.preferences.index_batch_size);
        self.telegram_activity = TelegramActivity::Working;
        let work = cx.background_spawn(async move {
            library.index_telegram_page(&telegram, account.id, chat_id, before, batch_size)
        });
        self.telegram_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| match result {
                Ok(page) => {
                    this.telegram_index = Some(page);
                    this.telegram_activity = TelegramActivity::Idle;
                    this.refresh_library(cx);
                }
                Err(error) => {
                    this.telegram_activity = TelegramActivity::Failed(error.kind());
                    cx.notify();
                }
            })
            .ok();
        }));
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

    pub(crate) fn download_filtered_telegram_files(&mut self, cx: &mut Context<Self>) {
        if self.channel_batch_activity == ChannelBatchActivity::Preparing {
            return;
        }
        let (Some(telegram), Some(library), Some(transfers), Some(chat_id)) = (
            self.telegram.clone(),
            self.library.clone(),
            self.transfers.clone(),
            self.selected_chat_id,
        ) else {
            return;
        };
        let now = current_system_unix_millis().unwrap_or(i64::MAX);
        let after_unix_ms = match self.channel_batch_period {
            ChannelBatchPeriod::AnyTime => None,
            ChannelBatchPeriod::Past24Hours => Some(now.saturating_sub(24 * 60 * 60 * 1_000)),
            ChannelBatchPeriod::Past7Days => Some(now.saturating_sub(7 * 24 * 60 * 60 * 1_000)),
            ChannelBatchPeriod::Past30Days => Some(now.saturating_sub(30 * 24 * 60 * 60 * 1_000)),
        };
        let filter = TelegramFileFilter {
            after_unix_ms,
            before_unix_ms: None,
            kind: self.channel_batch_kind,
        };
        self.channel_batch_activity = ChannelBatchActivity::Preparing;
        let work = cx.background_spawn(async move {
            let files = telegram.scan_filtered_files(chat_id, filter)?;
            if files.is_empty() {
                return Ok(None);
            }
            let mut reserved = BTreeSet::new();
            let mut requests = Vec::with_capacity(files.len());
            for file in files {
                let suggested_name = safe_suggested_file_name(&file.file_name, file.message_id);
                let destination =
                    next_reserved_download_destination(&library, &suggested_name, &mut reserved)?;
                requests.push(ChannelDownloadRequest {
                    chat_id,
                    message_id: file.message_id,
                    message_sent_at_unix_ms: Some(file.sent_at_unix_ms),
                    file_name: suggested_name,
                    caption: (!file.caption.is_empty()).then_some(file.caption),
                    mime_type: file.mime_type,
                    size_bytes: file.size_bytes,
                    destination,
                });
            }
            let count = requests.len();
            transfers
                .enqueue_channel_download_batch(requests)
                .map(|batch_id| Some((batch_id, count)))
        });
        self.telegram_batch_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                this.channel_batch_activity = match result {
                    Ok(Some((batch_id, count))) => {
                        this.expanded_transfer_batches.insert(batch_id);
                        ChannelBatchActivity::Queued { batch_id, count }
                    }
                    Ok(None) => ChannelBatchActivity::NoMatches,
                    Err(error) => ChannelBatchActivity::Failed(error.kind()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    pub(crate) fn locale(&self) -> SupportedLocale {
        self.localizer.locale()
    }

    pub(crate) fn set_page(&mut self, page: Page, cx: &mut Context<Self>) {
        if self.page == Page::Vault
            && page != Page::Vault
            && self.preferences.lock_vault_when_hidden
        {
            self.vault_locked = true;
            self.recovery_visible = false;
        }
        self.page = page;
        self.show_upload = false;
        if page == Page::Library {
            self.refresh_library(cx);
        } else {
            cx.notify();
        }
    }

    pub(crate) fn open_telegram_api_id_settings(&mut self, cx: &mut Context<Self>) {
        self.show_telegram_api_id_prompt = false;
        self.settings_section = SettingsSection::Accounts;
        self.nav_selection = "nav-settings";
        self.set_page(Page::Settings, cx);
    }

    pub(crate) fn set_settings_section(
        &mut self,
        section: SettingsSection,
        cx: &mut Context<Self>,
    ) {
        self.settings_section = section;
        cx.notify();
    }

    pub(crate) fn persist_preferences(&mut self, cx: &mut Context<Self>) {
        if self.preference_persistence == PreferencePersistence::Saving {
            return;
        }
        let Some(library) = self.library.clone() else {
            self.preference_persistence = PreferencePersistence::Failed;
            cx.notify();
            return;
        };
        let preferences = self.preferences.clone();
        self.preference_persistence = PreferencePersistence::Saving;
        cx.notify();
        let work = cx.background_spawn(async move { library.set_preferences(&preferences) });
        self.preference_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                this.preference_persistence = if result.is_ok() {
                    PreferencePersistence::Saved
                } else {
                    PreferencePersistence::Failed
                };
                cx.notify();
            })
            .ok();
        }));
    }

    pub(crate) fn set_appearance_preference(
        &mut self,
        appearance: AppearancePreference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.preference_persistence == PreferencePersistence::Saving {
            return;
        }
        self.preferences.appearance = appearance;
        theme::apply_appearance(appearance, window, cx);
        self.persist_preferences(cx);
    }

    pub(crate) fn choose_managed_files_root(&mut self, cx: &mut Context<Self>) {
        if self.preference_persistence == PreferencePersistence::Saving {
            return;
        }
        let selected = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(self.tr("settings-managed-root-picker")),
        });
        self.preference_picker_task = Some(cx.spawn(async move |this, cx| {
            let path = match selected.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) => None,
                Ok(Err(_)) | Err(_) => {
                    let Some(this) = this.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.preference_persistence = PreferencePersistence::Failed;
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let Some(path) = path else { return };
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                this.preferences.managed_files_root = Some(path);
                this.persist_preferences(cx);
            })
            .ok();
        }));
    }

    pub(crate) fn skip_telegram_api_id_prompt(&mut self, cx: &mut Context<Self>) {
        self.show_telegram_api_id_prompt = false;
        cx.notify();
    }

    pub(crate) fn save_telegram_credentials(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.telegram_api_id_persistence == TelegramApiIdPersistence::Saving {
            return;
        }
        let api_id = match parse_telegram_api_id(self.telegram_api_id.read(cx).value().as_ref()) {
            Ok(api_id) => api_id,
            Err(kind) => {
                self.telegram_api_id_persistence = TelegramApiIdPersistence::Failed(kind);
                cx.notify();
                return;
            }
        };
        let api_hash = self.telegram_api_hash.read(cx).value().to_string();
        if let Err(kind) = validate_telegram_api_hash(api_hash.trim()) {
            self.telegram_api_id_persistence = TelegramApiIdPersistence::Failed(kind);
            cx.notify();
            return;
        }
        let Some(library) = self.library.clone() else {
            self.telegram_api_id_persistence =
                TelegramApiIdPersistence::Failed(teleark_core::ApplicationErrorKind::Persistence);
            cx.notify();
            return;
        };
        self.telegram_api_hash.update(cx, |input, cx| {
            input.set_value(String::new(), window, cx);
        });
        self.telegram_api_id_persistence = TelegramApiIdPersistence::Saving;
        cx.notify();
        let save = cx.background_spawn(async move {
            library.set_telegram_credentials(api_id, api_hash.trim())
        });
        self.telegram_api_id_task = Some(cx.spawn(async move |this, cx| {
            let result = save.await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.configured_telegram_api_id = Some(api_id);
                        this.telegram_credential_source = Some(TelegramCredentialSource::User);
                        this.telegram_api_id_persistence = TelegramApiIdPersistence::Saved;
                        this.show_telegram_api_id_prompt = false;
                    }
                    Err(error) => {
                        this.telegram_api_id_persistence =
                            TelegramApiIdPersistence::Failed(error.kind());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(crate) fn clear_telegram_credentials(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.telegram_api_id_persistence == TelegramApiIdPersistence::Saving {
            return;
        }
        let (Some(library), Some(telegram)) = (self.library.clone(), self.telegram.clone()) else {
            self.telegram_api_id_persistence =
                TelegramApiIdPersistence::Failed(teleark_core::ApplicationErrorKind::Persistence);
            cx.notify();
            return;
        };
        self.telegram_api_id.update(cx, |input, cx| {
            input.set_value(String::new(), window, cx);
        });
        self.telegram_api_hash.update(cx, |input, cx| {
            input.set_value(String::new(), window, cx);
        });
        self.telegram_api_id_persistence = TelegramApiIdPersistence::Saving;
        cx.notify();
        let clear = cx.background_spawn(async move {
            library.clear_telegram_credentials()?;
            telegram.effective_credentials_status(&library)
        });
        self.telegram_api_id_task = Some(cx.spawn(async move |this, cx| {
            let result = clear.await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                match result {
                    Ok(status) => {
                        this.configured_telegram_api_id = status.map(|status| status.api_id);
                        this.telegram_credential_source = status.map(|status| status.source);
                        this.telegram_api_id_persistence = TelegramApiIdPersistence::Removed;
                        this.show_telegram_api_id_prompt = false;
                    }
                    Err(error) => {
                        this.telegram_api_id_persistence =
                            TelegramApiIdPersistence::Failed(error.kind());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(crate) fn refresh_library(&mut self, cx: &mut Context<Self>) {
        if is_preview_library_selection(self.nav_selection) {
            self.library_query_generation = self.library_query_generation.wrapping_add(1);
            self.library_loading_more = false;
            self.library_load_more_error = None;
            self.selected_file = 0;
            cx.notify();
            return;
        }
        let Some(library) = self.library.clone() else {
            cx.notify();
            return;
        };
        let query = self.library_query(cx);
        self.library_query_generation = self.library_query_generation.wrapping_add(1);
        let generation = self.library_query_generation;
        self.library_content = LibraryContent::Loading;
        self.library_loading_more = false;
        self.library_load_more_error = None;
        self.selected_file = 0;
        cx.notify();

        let load = cx.background_spawn(async move {
            let page = library.search(&query)?;
            let statistics = library.statistics()?;
            Ok::<(LibraryPage, LibraryStatistics), ApplicationError>((page, statistics))
        });
        self.library_task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                if this.library_query_generation != generation {
                    return;
                }
                this.library_content = match result {
                    Ok((page, statistics)) => {
                        LibraryContent::from_snapshot(LibrarySnapshot::from_core(page, statistics))
                    }
                    Err(error) => LibraryContent::Failed(error.kind()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    pub(crate) fn load_more_library(&mut self, cx: &mut Context<Self>) {
        if self.library_loading_more {
            return;
        }
        let Some(after) = self
            .library_content
            .snapshot()
            .and_then(|snapshot| snapshot.next_cursor.clone())
        else {
            return;
        };
        let Some(library) = self.library.clone() else {
            return;
        };
        let mut query = self.library_query(cx);
        query.after = Some(after);
        let generation = self.library_query_generation;
        self.library_loading_more = true;
        self.library_load_more_error = None;
        cx.notify();

        let load = cx.background_spawn(async move { library.search(&query) });
        self.library_more_task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                if this.library_query_generation != generation {
                    return;
                }
                this.library_loading_more = false;
                match result {
                    Ok(page) => {
                        if let Some(snapshot) = this.library_content.snapshot_mut() {
                            snapshot.append_page(page);
                        }
                    }
                    Err(error) => this.library_load_more_error = Some(error.kind()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(crate) fn choose_library_files(&mut self, cx: &mut Context<Self>) {
        if self.import_activity != ImportActivity::Idle {
            return;
        }
        let Some(library) = self.library.clone() else {
            self.import_feedback = Some(ImportFeedback::PickerFailed);
            cx.notify();
            return;
        };

        self.import_activity = ImportActivity::Picking;
        self.import_feedback = None;
        cx.notify();
        let selected_paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(self.tr("library-file-picker-prompt")),
        });
        let background = cx.background_executor().clone();
        self.import_task = Some(cx.spawn(async move |this, cx| {
            let paths = match selected_paths.await {
                Ok(Ok(Some(paths))) if paths.is_empty() => {
                    if let Some(this) = this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.import_activity = ImportActivity::Idle;
                            this.import_feedback = Some(ImportFeedback::NoFilesSelected);
                            cx.notify();
                        })
                        .ok();
                    }
                    return;
                }
                Ok(Ok(Some(paths))) => paths,
                Ok(Ok(None)) => {
                    if let Some(this) = this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.import_activity = ImportActivity::Idle;
                            cx.notify();
                        })
                        .ok();
                    }
                    return;
                }
                Ok(Err(_)) | Err(_) => {
                    if let Some(this) = this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.import_activity = ImportActivity::Idle;
                            this.import_feedback = Some(ImportFeedback::PickerFailed);
                            cx.notify();
                        })
                        .ok();
                    }
                    return;
                }
            };

            if let Some(entity) = this.upgrade() {
                entity
                    .update(cx, |this, cx| {
                        this.import_activity = ImportActivity::Importing;
                        cx.notify();
                    })
                    .ok();
            }
            let results = background
                .spawn(async move { library.import_paths(paths) })
                .await;
            let feedback =
                ImportFeedback::from_results(&results).or(Some(ImportFeedback::NoFilesSelected));
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                this.import_activity = ImportActivity::Idle;
                this.import_feedback = feedback;
                this.refresh_library(cx);
            })
            .ok();
        }));
    }

    fn library_query(&self, cx: &Context<Self>) -> LibraryQuery {
        let kind = library_kind_for_selection(self.nav_selection);
        LibraryQuery {
            text: self.search_input.read(cx).value().to_string(),
            filter: LibraryFilter {
                kinds: kind.into_iter().collect(),
                ..LibraryFilter::default()
            },
            sort: LibrarySort::ModifiedNewest,
            page_size: 200,
            after: None,
        }
    }

    pub(crate) fn set_locale(
        &mut self,
        locale: SupportedLocale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.follows_system_locale = false;
        self.apply_locale(locale, window, cx);
        self.persist_locale_override(LocaleOverrideChoice::Explicit(locale), cx);
    }

    pub(crate) fn use_system_locale(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.follows_system_locale = true;
        self.apply_locale(self.system_locale, window, cx);
        self.persist_locale_override(LocaleOverrideChoice::SystemDefault, cx);
    }

    fn apply_locale(
        &mut self,
        locale: SupportedLocale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.localizer.set_locale(locale);
        let placeholders = [
            (self.search_input.clone(), self.tr("search-placeholder")),
            (
                self.telegram_api_id.clone(),
                self.tr("telegram-api-id-placeholder"),
            ),
            (
                self.telegram_api_hash.clone(),
                self.tr("telegram-api-hash-placeholder"),
            ),
            (
                self.telegram_phone.clone(),
                self.tr("telegram-phone-placeholder"),
            ),
            (
                self.telegram_code.clone(),
                self.tr("telegram-code-placeholder"),
            ),
            (
                self.telegram_password.clone(),
                self.tr("telegram-password-placeholder"),
            ),
        ];
        for (input, placeholder) in placeholders {
            input.update(cx, |input, input_cx| {
                input.set_placeholder(placeholder, window, input_cx);
            });
        }
        cx.set_menus(crate::menus::application_menus(&self.localizer));
        cx.notify();
    }

    fn persist_locale_override(&mut self, choice: LocaleOverrideChoice, cx: &mut Context<Self>) {
        if self.locale_persistence == LocalePersistence::Saving {
            self.pending_locale_override = Some(choice);
            return;
        }
        let Some(library) = self.library.clone() else {
            self.locale_persistence = LocalePersistence::Failed;
            cx.notify();
            return;
        };
        self.locale_persistence = LocalePersistence::Saving;
        let locale = match choice {
            LocaleOverrideChoice::SystemDefault => None,
            LocaleOverrideChoice::Explicit(locale) => Some(locale.as_str().to_owned()),
        };
        let save =
            cx.background_spawn(async move { library.set_locale_override(locale.as_deref()) });
        self.locale_task = Some(cx.spawn(async move |this, cx| {
            let result = save.await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                this.locale_persistence = if result.is_ok() {
                    LocalePersistence::Saved
                } else {
                    LocalePersistence::Failed
                };
                if let Some(pending) = this.pending_locale_override.take() {
                    this.persist_locale_override(pending, cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn render_header(
        &self,
        window: &mut Window,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let preview_backed = self.show_upload
            || self.page == Page::Vault
            || (self.page == Page::Transfers && self.transfers.is_none())
            || (self.page == Page::Library && is_preview_library_selection(self.nav_selection));
        let brand = div()
            .h_full()
            .w(px(layout.header_brand_width()))
            .flex_none()
            .flex()
            .items_center()
            .pl(px(if layout.is_compact() { 20.0 } else { 54.0 }))
            .pr_4()
            .gap_2()
            .border_r_1()
            .border_color(theme::border())
            .child(
                div()
                    .size(px(28.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme::RADIUS_SMALL)
                    .bg(theme::blue())
                    .text_color(theme::surface())
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("T"),
            )
            .child(
                div()
                    .text_size(px(15.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme::text_primary())
                    .child("TeleArk"),
            );

        let search = div()
            .flex_1()
            .min_w(px(180.0))
            .max_w(px(if layout.is_spacious() { 760.0 } else { 660.0 }))
            .h(px(36.0))
            .mx(px(if layout.is_compact() { 12.0 } else { 20.0 }))
            .child(
                gpui_component::input::Input::new(&self.search_input)
                    .prefix(IconName::Search)
                    .h(px(36.0)),
            );

        let connected = matches!(self.telegram_auth, TelegramAuthState::Authorized(_));
        let fullscreen = window.is_fullscreen();
        let status = div()
            .h_full()
            .flex()
            .items_center()
            .justify_end()
            .gap(if layout.is_compact() {
                px(8.0)
            } else {
                px(20.0)
            })
            .pr(px(if layout.is_compact() { 12.0 } else { 20.0 }))
            .text_sm()
            .text_color(theme::text_secondary())
            .when(preview_backed, |status| {
                status.child(components::badge(
                    self.tr("prototype-demo-badge"),
                    Tone::Amber,
                ))
            })
            .when(layout.shows_full_header_status(), |status| {
                status.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().size_2().rounded_full().bg(if connected {
                            theme::green()
                        } else {
                            theme::amber()
                        }))
                        .child(self.tr(if connected {
                            "telegram-status-connected"
                        } else {
                            "telegram-status-not-connected"
                        })),
                )
            })
            .when(!connected, |status| {
                status.child(
                    components::button(
                        "header-telegram-login",
                        self.tr("telegram-header-login-action"),
                        Some(IconName::ArrowRight),
                        true,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.nav_selection = "nav-telegram-sources";
                        this.set_page(Page::Channel, cx);
                    })),
                )
            })
            .when(layout.shows_full_header_status(), |status| {
                status.child(
                    div()
                        .id("header-accounts")
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .hover(|style| style.text_color(theme::blue()))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.nav_selection = "nav-settings";
                            this.set_page(Page::Settings, cx);
                        }))
                        .child(Icon::new(IconName::CircleUser).text_color(theme::text_secondary()))
                        .child(self.telegram_account.as_ref().map_or_else(
                            || self.tr("settings-accounts"),
                            |account| SharedString::from(account.display_name.clone()),
                        )),
                )
            })
            .when(fullscreen, |status| {
                status.child(
                    components::icon_button(
                        "header-exit-fullscreen",
                        IconName::Minimize,
                        self.tr("window-exit-fullscreen-action"),
                    )
                    .on_click(|_, window, _| window.toggle_fullscreen()),
                )
            })
            .child(
                div()
                    .id("header-settings")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.nav_selection = "nav-settings";
                        this.set_page(Page::Settings, cx);
                    }))
                    .child(Icon::new(IconName::Settings).text_color(theme::text_secondary())),
            );

        div()
            .h(theme::HEADER_HEIGHT)
            .w_full()
            .flex()
            .items_center()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(brand)
            .child(search)
            .child(div().flex_1())
            .child(status)
            .into_any_element()
    }

    fn nav_item(
        &self,
        id: &'static str,
        label_id: &'static str,
        icon: IconName,
        count: Option<SharedString>,
        target: Page,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.nav_selection == id;
        div()
            .id(id)
            .h(px(32.0))
            .mx_2()
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .rounded(theme::RADIUS_SMALL)
            .cursor_pointer()
            .focusable()
            .tab_index(0)
            .text_sm()
            .text_color(if selected {
                theme::blue()
            } else {
                theme::text_primary()
            })
            .when(selected, |style| style.bg(theme::blue_soft()))
            .hover(|style| style.bg(theme::blue_pale()))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.nav_selection = id;
                this.selected_file = 0;
                this.set_page(target, cx);
            }))
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.nav_selection = id;
                    this.selected_file = 0;
                    this.set_page(target, cx);
                }
            }))
            .child(Icon::new(icon).text_color(if selected {
                theme::blue()
            } else {
                theme::text_secondary()
            }))
            .child(div().flex_1().min_w_0().truncate().child(self.tr(label_id)))
            .when_some(count, |row, count| {
                row.child(div().text_xs().text_color(theme::text_muted()).child(count))
            })
            .into_any_element()
    }

    fn source_item(
        &self,
        id: &'static str,
        glyph: &'static str,
        title: &'static str,
        count: &'static str,
        target: Page,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.nav_selection == id;
        div()
            .id(id)
            .h(px(30.0))
            .mx_2()
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .rounded(theme::RADIUS_SMALL)
            .cursor_pointer()
            .focusable()
            .tab_index(0)
            .text_sm()
            .when(selected, |style| style.bg(theme::blue_soft()))
            .hover(|style| style.bg(theme::blue_pale()))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.nav_selection = id;
                this.selected_source = title;
                this.selected_file = 0;
                this.set_page(target, cx);
            }))
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.nav_selection = id;
                    this.selected_source = title;
                    this.selected_file = 0;
                    this.set_page(target, cx);
                }
            }))
            .child(
                div()
                    .size(px(18.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(theme::purple_soft())
                    .text_color(theme::purple())
                    .text_xs()
                    .child(glyph),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(theme::text_primary())
                    .child(title),
            )
            .child(div().text_xs().text_color(theme::text_muted()).child(count))
            .into_any_element()
    }

    fn group_label(&self, label_id: &'static str) -> AnyElement {
        div()
            .h(px(28.0))
            .px_5()
            .flex()
            .items_end()
            .pb_1()
            .text_size(px(10.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme::text_muted())
            .child(self.tr(label_id))
            .into_any_element()
    }

    fn render_sidebar(&self, layout: LayoutPolicy, cx: &mut Context<Self>) -> AnyElement {
        let library_count = self.library_content.snapshot().map(|snapshot| {
            SharedString::from(format_integer(
                self.locale(),
                snapshot.statistics.logical_file_count,
            ))
        });
        let rates = self
            .transfers
            .as_ref()
            .and_then(|transfers| transfers.current_rates().ok())
            .unwrap_or_default();
        let download_speed = format_speed(self.locale(), rates.download_bytes_per_second);
        let upload_speed = format_speed(self.locale(), rates.upload_bytes_per_second);
        let free_space = self.overall_storage_metrics.map_or_else(
            || SharedString::from("—"),
            |metrics| SharedString::from(format_bytes(self.locale(), metrics.available_bytes)),
        );
        let app_usage = self.overall_storage_metrics.map_or_else(
            || SharedString::from("—"),
            |metrics| SharedString::from(format_bytes(self.locale(), metrics.app_used_bytes)),
        );
        let storage_card = div()
            .mx_3()
            .mt_3()
            .p_3()
            .rounded(theme::RADIUS_MEDIUM)
            .border_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .size(px(32.0))
                            .rounded(theme::RADIUS_SMALL)
                            .bg(theme::blue_soft())
                            .text_color(theme::blue())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_sm()
                            .child("▰"),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            div()
                                .flex()
                                .text_sm()
                                .text_xs()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(self.tr("overall-title")),
                        ),
                    ),
            )
            .child(sidebar_metric_row(
                self.tr("overall-download-speed"),
                download_speed.into(),
                theme::blue(),
            ))
            .child(sidebar_metric_row(
                self.tr("overall-upload-speed"),
                upload_speed.into(),
                theme::green(),
            ))
            .child(sidebar_metric_row(
                self.tr("overall-free-space"),
                free_space,
                theme::text_secondary(),
            ))
            .child(sidebar_metric_row(
                self.tr("overall-app-usage"),
                app_usage,
                theme::text_secondary(),
            ));

        let navigation =
            div()
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .child(self.group_label("nav-library"))
                .child(self.nav_item(
                    "nav-all",
                    "nav-all-files",
                    IconName::FolderOpen,
                    library_count,
                    Page::Library,
                    cx,
                ))
                .child(self.nav_item(
                    "nav-recent",
                    "nav-recent",
                    IconName::Calendar,
                    None,
                    Page::Library,
                    cx,
                ))
                .child(self.nav_item(
                    "nav-videos",
                    "nav-videos",
                    IconName::GalleryVerticalEnd,
                    None,
                    Page::Library,
                    cx,
                ))
                .child(self.nav_item(
                    "nav-docs",
                    "nav-documents",
                    IconName::File,
                    None,
                    Page::Library,
                    cx,
                ))
                .child(self.nav_item(
                    "nav-archives",
                    "nav-archives",
                    IconName::Inbox,
                    None,
                    Page::Library,
                    cx,
                ))
                .child(self.nav_item(
                    "nav-images",
                    "library-images",
                    IconName::File,
                    None,
                    Page::Library,
                    cx,
                ))
                .child(self.nav_item(
                    "nav-audio",
                    "library-audio",
                    IconName::File,
                    None,
                    Page::Library,
                    cx,
                ))
                .child(self.nav_item(
                    "nav-disk-images",
                    "library-disk-images",
                    IconName::File,
                    None,
                    Page::Library,
                    cx,
                ))
                .child(self.nav_item(
                    "nav-other",
                    "library-other",
                    IconName::File,
                    None,
                    Page::Library,
                    cx,
                ))
                .child(self.group_label("nav-channels"))
                .child(
                    self.nav_item(
                        "nav-telegram-sources",
                        "telegram-library-title",
                        IconName::Inbox,
                        self.telegram_chats.len().try_into().ok().map(|count: u64| {
                            SharedString::from(format_integer(self.locale(), count))
                        }),
                        Page::Channel,
                        cx,
                    ),
                )
                .child(self.group_label("nav-collections"))
                .child(self.source_item(
                    "collection-mac",
                    "M",
                    "Mac Backup",
                    "12,040",
                    Page::Library,
                    cx,
                ))
                .child(self.source_item(
                    "collection-course",
                    "A",
                    "AI Course",
                    "876",
                    Page::Library,
                    cx,
                ))
                .child(self.group_label("nav-transfers"))
                .child(self.nav_item(
                    "nav-transfers-all",
                    "nav-all-transfers",
                    IconName::ArrowDown,
                    Some("176".into()),
                    Page::Transfers,
                    cx,
                ))
                .child(self.nav_item(
                    "nav-completed",
                    "nav-completed",
                    IconName::CircleCheck,
                    Some("3,842".into()),
                    Page::Transfers,
                    cx,
                ))
                .child(self.nav_item(
                    "nav-failed",
                    "nav-failed",
                    IconName::TriangleAlert,
                    Some("12".into()),
                    Page::Transfers,
                    cx,
                ))
                .child(self.group_label("nav-storage"))
                .child(self.nav_item(
                    "nav-vault",
                    "nav-key-vault",
                    IconName::Asterisk,
                    None,
                    Page::Vault,
                    cx,
                ))
                .child(self.nav_item(
                    "nav-settings",
                    "nav-settings",
                    IconName::Settings,
                    None,
                    Page::Settings,
                    cx,
                ));

        div()
            .w(px(layout.sidebar_width()))
            .h_full()
            .min_h_0()
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme::sidebar())
            .border_r_1()
            .border_color(theme::border())
            .child(navigation)
            .child(storage_card)
            .child(div().h(px(12.0)))
            .into_any_element()
    }

    fn compact_nav_item(
        &self,
        item: CompactNavItem,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let CompactNavItem {
            id,
            label_id,
            icon,
            target,
            nav_selection,
        } = item;
        div()
            .id(id)
            .h(px(34.0))
            .px_3()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .rounded(theme::RADIUS_SMALL)
            .cursor_pointer()
            .focusable()
            .tab_index(0)
            .when(selected, |item| {
                item.bg(theme::blue()).text_color(theme::surface())
            })
            .when(!selected, |item| {
                item.bg(theme::sidebar()).text_color(theme::text_primary())
            })
            .hover(|item| {
                item.bg(if selected {
                    theme::blue()
                } else {
                    theme::blue_pale()
                })
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.nav_selection = nav_selection;
                this.selected_file = 0;
                this.set_page(target, cx);
            }))
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.nav_selection = nav_selection;
                    this.selected_file = 0;
                    this.set_page(target, cx);
                }
            }))
            .child(Icon::new(icon))
            .child(self.tr(label_id))
            .into_any_element()
    }

    fn render_compact_navigation(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("compact-navigation")
            .h(px(46.0))
            .w_full()
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .overflow_x_scroll()
            .child(self.compact_nav_item(
                CompactNavItem {
                    id: "compact-library",
                    label_id: "nav-all-files",
                    icon: IconName::FolderOpen,
                    target: Page::Library,
                    nav_selection: "nav-all",
                },
                matches!(self.page, Page::Library | Page::FileDetail),
                cx,
            ))
            .child(self.compact_nav_item(
                CompactNavItem {
                    id: "compact-transfers",
                    label_id: "nav-all-transfers",
                    icon: IconName::ArrowDown,
                    target: Page::Transfers,
                    nav_selection: "nav-transfers-all",
                },
                self.page == Page::Transfers,
                cx,
            ))
            .child(self.compact_nav_item(
                CompactNavItem {
                    id: "compact-channel",
                    label_id: "nav-channels",
                    icon: IconName::Search,
                    target: Page::Channel,
                    nav_selection: "nav-telegram-sources",
                },
                self.page == Page::Channel,
                cx,
            ))
            .child(self.compact_nav_item(
                CompactNavItem {
                    id: "compact-vault",
                    label_id: "nav-key-vault",
                    icon: IconName::Asterisk,
                    target: Page::Vault,
                    nav_selection: "nav-vault",
                },
                self.page == Page::Vault,
                cx,
            ))
            .child(self.compact_nav_item(
                CompactNavItem {
                    id: "compact-settings",
                    label_id: "nav-settings",
                    icon: IconName::Settings,
                    target: Page::Settings,
                    nav_selection: "nav-settings",
                },
                self.page == Page::Settings,
                cx,
            ))
            .into_any_element()
    }

    fn render_page(
        &self,
        window: &mut Window,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.page {
            Page::Library => self.render_library(window, layout, cx),
            Page::Transfers => self.render_transfers(window, layout, cx),
            Page::FileDetail => self.render_file_detail(window, layout, cx),
            Page::Vault => self.render_vault(window, layout, cx),
            Page::Channel => self.render_channel(window, layout, cx),
            Page::Settings => self.render_settings(window, layout, cx),
        }
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

fn current_system_unix_millis() -> Option<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
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
            if !files
                .iter()
                .any(|existing| existing.message_id == file.message_id)
            {
                files.push(file);
            }
        }
    }
    (page.next_before_message_id, page.exhausted)
}

pub(crate) fn is_preview_library_selection(selection: &str) -> bool {
    matches!(selection, "collection-mac" | "collection-course")
}

impl Render for TeleArkApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = LayoutPolicy::from_window(window);
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .font_family(".SystemUIFont")
            .text_color(theme::text_primary())
            .on_action(cx.listener(|this, _: &DismissOverlay, window, cx| {
                match escape_behavior(
                    window.is_fullscreen(),
                    this.show_upload,
                    this.show_telegram_api_id_prompt,
                ) {
                    EscapeBehavior::ExitFullscreen => window.toggle_fullscreen(),
                    EscapeBehavior::DismissUpload => {
                        this.show_upload = false;
                        cx.notify();
                    }
                    EscapeBehavior::DismissTelegramApiIdPrompt => {
                        this.skip_telegram_api_id_prompt(cx);
                    }
                    EscapeBehavior::Ignore => {}
                }
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
            .child(self.render_header(window, layout, cx))
            .when(layout.is_compact(), |root| {
                root.child(self.render_compact_navigation(cx))
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(layout.shows_global_sidebar(), |body| {
                        body.child(self.render_sidebar(layout, cx))
                    })
                    .child(self.render_page(window, layout, cx)),
            )
            .when(self.show_upload, |root| {
                root.child(screens::upload::render_upload_overlay(self, layout, cx))
            })
            .when(self.show_telegram_api_id_prompt, |root| {
                root.child(screens::settings::render_telegram_api_id_prompt(self, cx))
            })
    }
}

fn sidebar_metric_row(
    label: SharedString,
    value: SharedString,
    value_color: gpui::Rgba,
) -> AnyElement {
    div()
        .mt_2()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .text_xs()
        .child(div().min_w_0().text_color(theme::text_muted()).child(label))
        .child(
            div()
                .flex_none()
                .text_color(value_color)
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(value),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn escape_prioritizes_leaving_fullscreen_before_dismissing_an_overlay() {
        assert_eq!(
            escape_behavior(true, true, true),
            EscapeBehavior::ExitFullscreen
        );
        assert_eq!(
            escape_behavior(false, true, true),
            EscapeBehavior::DismissUpload
        );
        assert_eq!(
            escape_behavior(false, false, true),
            EscapeBehavior::DismissTelegramApiIdPrompt
        );
        assert_eq!(escape_behavior(false, false, false), EscapeBehavior::Ignore);
    }

    #[test]
    fn telegram_credential_validation_controls_startup_and_login_availability() {
        assert_eq!(parse_telegram_api_id("12345"), Ok(12_345));
        assert_eq!(
            parse_telegram_api_id("0"),
            Err(teleark_core::ApplicationErrorKind::InvalidRequest)
        );
        assert_eq!(
            parse_telegram_api_id("not-a-number"),
            Err(teleark_core::ApplicationErrorKind::InvalidRequest)
        );
        assert!(!telegram_login_controls_enabled(None));
        assert!(!telegram_login_controls_enabled(Some(0)));
        assert!(telegram_login_controls_enabled(Some(12_345)));
        assert!(should_show_telegram_credentials_prompt(false, None));
        assert!(!should_show_telegram_credentials_prompt(true, None));
        assert!(!should_show_telegram_credentials_prompt(
            false,
            Some(12_345)
        ));
        assert_eq!(
            validate_telegram_api_hash("0123456789abcdef0123456789abcdef"),
            Ok(())
        );
        for invalid in ["", "short", "z123456789abcdef0123456789abcdef"] {
            assert_eq!(
                validate_telegram_api_hash(invalid),
                Err(teleark_core::ApplicationErrorKind::InvalidRequest)
            );
        }
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
    }
}
