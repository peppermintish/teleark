//! Preferences presentation owner. Business operations stay in the runtime.

use super::*;

impl TeleArkApp {
    pub(crate) fn persist_preferences(&mut self, cx: &mut Context<Self>) {
        if self.visual_preview {
            self.preference_persistence = PreferencePersistence::Idle;
            cx.notify();
            return;
        }
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
        let saved_preferences = preferences.clone();
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
                if result.is_ok() && this.preferences != saved_preferences {
                    this.persist_preferences(cx);
                }
                cx.notify();
            });
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
                    });
                    return;
                }
            };
            let Some(path) = path else { return };
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                this.preferences.managed_files_root = Some(path);
                this.persist_preferences(cx);
            });
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
                        if matches!(this.telegram_auth, TelegramAuthState::Unauthorized)
                            && this.telegram_activity != TelegramActivity::Working
                        {
                            this.telegram_activity = TelegramActivity::Idle;
                        }
                        this.ensure_telegram_qr_login(cx);
                    }
                    Err(error) => {
                        this.telegram_api_id_persistence =
                            TelegramApiIdPersistence::Failed(error.kind());
                    }
                }
                cx.notify();
            });
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
            });
        }));
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

    pub(super) fn apply_locale(
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
        self.refresh_channel_file_table(cx);
        cx.set_menus(crate::menus::application_menus(&self.localizer));
        cx.notify();
    }

    pub(super) fn persist_locale_override(
        &mut self,
        choice: LocaleOverrideChoice,
        cx: &mut Context<Self>,
    ) {
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
            });
        }));
    }
}
