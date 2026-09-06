//! Owns bounded background observations; no filesystem work runs on the UI thread.
use super::*;
use teleark_runtime::{DownloadedFileRecord, DownloadedFilesCursor, LocalFilePresence};

#[derive(Clone, Debug)]
pub(crate) struct LocalDownloadObservation {
    pub file: DownloadedFileRecord,
    pub presence: LocalFilePresence,
}

impl TeleArkApp {
    pub(super) fn start_local_file_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(library) = self.library.clone() else {
            return;
        };
        self.local_files_task = Some(cx.spawn(async move |this, cx| {
            let mut observed_account = None;
            let mut cursor = None;
            loop {
                let Some(entity) = this.upgrade() else { return };
                let account = entity.update(cx, |this, _| {
                    this.telegram_account.as_ref().map(|account| account.id)
                });
                if account != observed_account {
                    observed_account = account;
                    cursor = None;
                    entity.update(cx, |this, cx| {
                        this.local_downloads.clear();
                        cx.notify();
                    });
                }
                if let Some(account_id) = account {
                    let library = library.clone();
                    let result = cx
                        .background_spawn(async move {
                            let page = library.downloaded_files_page(account_id, cursor)?;
                            let next = page.last().map(|file| file.cursor);
                            // Refresh the newest outputs of both kinds each pass while
                            // an independent cursor revisits older history in bounded pages.
                            let mut files = library.downloaded_files_page(account_id, None)?;
                            files.extend(library.downloaded_files_page(
                                account_id,
                                Some(DownloadedFilesCursor { kind: 0, id: 0 }),
                            )?);
                            files.extend(page);
                            let mut unique = std::collections::BTreeMap::new();
                            for file in files {
                                unique
                                    .entry(file.destination.clone())
                                    .and_modify(|previous: &mut DownloadedFileRecord| {
                                        if file.completed_at_unix_ms > previous.completed_at_unix_ms
                                        {
                                            *previous = file.clone();
                                        }
                                    })
                                    .or_insert(file);
                            }
                            let observations = unique
                                .into_values()
                                .map(|file| {
                                    let presence = teleark_runtime::local_file_presence(
                                        &file.destination,
                                        file.size_bytes,
                                    );
                                    LocalDownloadObservation { file, presence }
                                })
                                .collect::<Vec<_>>();
                            Ok::<_, ApplicationError>((next, observations))
                        })
                        .await;
                    let Some(entity) = this.upgrade() else { return };
                    entity.update(cx, |this, cx| {
                        if this.telegram_account.as_ref().map(|account| account.id)
                            != Some(account_id)
                        {
                            return;
                        }
                        if let Ok((next, observations)) = result {
                            cursor = next;
                            for observation in observations {
                                this.local_downloads
                                    .insert(observation.file.destination.clone(), observation);
                            }
                            // This presentation cache is bounded; the durable inventory remains complete.
                            if this.local_downloads.len() > 10_000 {
                                let mut age = this
                                    .local_downloads
                                    .iter()
                                    .map(|(path, item)| {
                                        (item.file.completed_at_unix_ms, path.clone())
                                    })
                                    .collect::<Vec<_>>();
                                age.sort_unstable();
                                for (_, path) in
                                    age.into_iter().take(this.local_downloads.len() - 10_000)
                                {
                                    this.local_downloads.remove(&path);
                                }
                            }
                        } else {
                            for item in this.local_downloads.values_mut() {
                                item.presence = LocalFilePresence::Unavailable;
                            }
                        }
                        this.channel_file_table.update(cx, |_, cx| cx.notify());
                        cx.notify();
                    });
                }
                cx.background_executor().timer(Duration::from_secs(3)).await;
            }
        }));
    }

    pub(crate) fn local_download_actions(
        &self,
        observation: &LocalDownloadObservation,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::component::{Disableable as _, IconName};
        use gpui_kit::{IntoElement as _, ParentElement as _, Styled as _, div};
        let available = observation.presence == LocalFilePresence::Present && !self.visual_preview;
        let open = observation.file.destination.clone();
        let reveal = open.clone();
        div()
            .mt_3()
            .flex()
            .flex_wrap()
            .gap_2()
            .child(
                crate::components::button(
                    "local-output-open",
                    self.tr("action-open-file"),
                    Some(IconName::ArrowRight),
                    false,
                )
                .disabled(!available)
                .on_click(move |_, _, cx| cx.open_with_system(&open)),
            )
            .child(
                crate::components::button(
                    "local-output-reveal",
                    self.tr("action-show-in-folder"),
                    Some(IconName::FolderOpen),
                    false,
                )
                .disabled(!available)
                .on_click(move |_, _, cx| cx.reveal_path(&reveal)),
            )
            .into_any_element()
    }

    pub(crate) fn local_presence_for_path(
        &self,
        path: &std::path::Path,
    ) -> Option<LocalFilePresence> {
        self.local_downloads.get(path).map(|item| item.presence)
    }

    pub(crate) fn local_download_for_source(
        &self,
        chat_id: i64,
        message_id: Option<i64>,
        package_id: Option<&str>,
    ) -> Option<&LocalDownloadObservation> {
        self.local_downloads
            .values()
            .filter(|item| {
                item.file.chat_id == chat_id
                    && match package_id {
                        Some(package_id) => item.file.package_id.as_deref() == Some(package_id),
                        None => message_id.is_some() && item.file.message_id == message_id,
                    }
            })
            .max_by_key(|item| {
                (
                    item.presence == LocalFilePresence::Present,
                    item.file.completed_at_unix_ms,
                )
            })
    }

    pub(crate) fn local_presence_label(&self, presence: Option<LocalFilePresence>) -> SharedString {
        self.tr(match presence {
            Some(LocalFilePresence::Present) => "local-file-present",
            Some(LocalFilePresence::Missing) => "local-file-missing",
            Some(LocalFilePresence::SizeChanged) => "local-file-size-changed",
            Some(LocalFilePresence::Unavailable) => "local-file-unavailable",
            None => "local-file-checking",
        })
    }
}
