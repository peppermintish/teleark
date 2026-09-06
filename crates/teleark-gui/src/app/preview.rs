//! Synthetic, isolated fixtures. Called only after --preview-ui disables every runtime owner.
use super::*;

impl TeleArkApp {
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
                id: 1000 + index,
                name: format!(
                    "{} {index:03}",
                    ["Design Library", "旅行摄影", "映像資料", "Archives"][index as usize % 4]
                ),
                username: None,
                kind: TelegramChatKind::Channel,
            })
            .collect();
        let storage = TelegramChatSummary {
            id: 9000,
            name: "TeleArk".into(),
            username: None,
            kind: TelegramChatKind::Channel,
        };
        self.storage_status = teleark_runtime::StorageChannelStatus::Ready(storage);
        self.selected_chat_id = Some(9000);
        self.vault_status.configured = true;
        self.vault_status.locked = state == "locked" || state == "unlock";
        self.vault_locked = self.vault_status.locked;
        let names = [
            "Coastal Journey.mov",
            "Project Aurora — 设计稿.zip",
            "京都の春 • 写真集.zip",
            "Annual Report.pdf",
            "Field Recordings.wav",
            "Workshop Notes.md",
        ];
        self.managed_vault_files = names
            .iter()
            .enumerate()
            .map(|(index, name)| ManagedVaultFile {
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
            .collect();
        self.telegram_files = (0..5000)
            .map(|index| TelegramFileSummary {
                message_id: 5000 - index,
                sent_at_unix_ms: 1_788_624_000_000,
                modified_at_unix_ms: 1_788_624_000_000,
                file_name: format!("{index:04} {}", names[index as usize % names.len()]),
                caption: String::new(),
                mime_type: None,
                size_bytes: 1024 * 1024 * 123,
            })
            .collect();
        self.telegram_files_exhausted = true;
        self.refresh_channel_file_table(cx);
        let rows = names
            .iter()
            .enumerate()
            .map(|(index, name)| crate::library_state::LibraryRowView {
                id: teleark_core::LogicalFileId::new(index as u64 + 1),
                name: (*name).into(),
                size_bytes: 1024 * 1024 * 123,
                kind: FileKind::Document,
                source_name: Some("TeleArk".into()),
                local_source_path: None,
                source_chat_id: Some(9000),
                modified_at_unix_ms: Some(1_788_624_000_000),
                remote_state: teleark_core::RemoteState::Uploaded,
                encryption_state: teleark_core::EncryptionState::Encrypted,
                verification_state: teleark_core::VerificationState::Verified,
                package_id: None,
                part_count: 3,
            })
            .collect();
        self.library_content = LibraryContent::Ready(LibrarySnapshot {
            rows,
            total_matching: names.len() as u64,
            next_cursor: None,
            statistics: LibraryStatistics::default(),
        });
        match state.as_str() {
            "returning" => self.page = Page::Account,
            "setup" => {
                self.page = Page::Storage;
                self.storage_status = teleark_runtime::StorageChannelStatus::Missing;
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
