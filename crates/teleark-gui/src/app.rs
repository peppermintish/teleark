use gpui::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    Icon, IconName,
    input::{InputEvent, InputState},
    scroll::ScrollableElement as _,
};
use teleark_core::{
    ApplicationError, FileKind, LibraryFilter, LibraryPage, LibraryQuery, LibrarySort,
    LibraryStatistics,
};
use teleark_i18n::{
    Localizer, MessageArgs, MessageId, SupportedLocale,
    format::{format_bytes, format_integer},
};
use teleark_runtime::DesktopLibrary;

use crate::{
    DismissOverlay,
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
enum LocaleOverrideChoice {
    SystemDefault,
    Explicit(SupportedLocale),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocaleStartup {
    pub system_locale: SupportedLocale,
    pub follows_system_locale: bool,
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
    pub(crate) vault_locked: bool,
    pub(crate) recovery_visible: bool,
    pub(crate) index_paused: bool,
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
    library: Option<DesktopLibrary>,
    library_query_generation: u64,
    pending_locale_override: Option<LocaleOverrideChoice>,
    library_task: Option<Task<()>>,
    library_more_task: Option<Task<()>>,
    import_task: Option<Task<()>>,
    locale_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl TeleArkApp {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        localizer: Localizer,
        library: Result<DesktopLibrary, ApplicationError>,
        page: Page,
        show_upload: bool,
        locale_startup: LocaleStartup,
    ) -> Self {
        let placeholder = localizer.translate_or_id(MessageId::new("search-placeholder"));
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
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
            Page::Channel => "channel-cinema",
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
            vault_locked: true,
            recovery_visible: false,
            index_paused: false,
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
            library,
            library_query_generation: 0,
            pending_locale_override: None,
            library_task: None,
            library_more_task: None,
            import_task: None,
            locale_task: None,
            _subscriptions: vec![search_subscription],
        };
        if app.library.is_some() {
            app.refresh_library(cx);
        }
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
        self.page = page;
        self.show_upload = false;
        if page == Page::Library {
            self.refresh_library(cx);
        } else {
            cx.notify();
        }
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
        let placeholder = self.tr("search-placeholder");
        self.search_input.update(cx, |input, input_cx| {
            input.set_placeholder(placeholder, window, input_cx);
        });
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

    fn render_header(&self, layout: LayoutPolicy, cx: &mut Context<Self>) -> AnyElement {
        let preview_backed = self.show_upload
            || matches!(
                self.page,
                Page::Transfers | Page::Vault | Page::Channel | Page::Settings
            )
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
                        .child(div().size_2().rounded_full().bg(theme::green()))
                        .child(self.tr("connection-connected")),
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
                        .child(self.tr("accounts-count")),
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
        let library_size = self.library_content.snapshot().map_or_else(
            || SharedString::from("—"),
            |snapshot| {
                SharedString::from(format_bytes(
                    self.locale(),
                    snapshot.statistics.logical_bytes,
                ))
            },
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
                    .gap_3()
                    .child(
                        div()
                            .size(px(38.0))
                            .rounded(theme::RADIUS_SMALL)
                            .bg(theme::blue_soft())
                            .text_color(theme::blue())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_lg()
                            .child("▰"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .text_sm()
                                    .child(self.tr("storage-local"))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme::green())
                                            .child(self.tr("status-healthy")),
                                    ),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .text_xs()
                                    .text_color(theme::text_muted())
                                    .child(library_size),
                            ),
                    ),
            );

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
            .child(self.source_item(
                "channel-saved",
                "S",
                "Saved Messages",
                "184,521",
                Page::Channel,
                cx,
            ))
            .child(self.source_item(
                "channel-cinema",
                "4K",
                "Cinema 4K",
                "392,104",
                Page::Channel,
                cx,
            ))
            .child(self.source_item(
                "channel-design",
                "D",
                "Design Assets",
                "262,341",
                Page::Channel,
                cx,
            ))
            .child(self.source_item(
                "channel-software",
                "S",
                "Software",
                "53,221",
                Page::Channel,
                cx,
            ))
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
            ))
            .child(div().flex_1())
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
        components::button(id, self.tr(label_id), Some(icon), selected)
            .flex_none()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.nav_selection = nav_selection;
                this.selected_file = 0;
                this.set_page(target, cx);
            }))
            .into_any_element()
    }

    fn render_compact_navigation(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
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
            .overflow_x_scrollbar()
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
                    nav_selection: "channel-cinema",
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
            .on_action(cx.listener(|this, _: &DismissOverlay, _, cx| {
                if this.show_upload {
                    this.show_upload = false;
                    cx.notify();
                }
            }))
            .child(self.render_header(layout, cx))
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
    }
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
}
