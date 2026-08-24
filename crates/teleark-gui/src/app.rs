use gpui::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{Icon, IconName, input::InputState};
use teleark_i18n::{Localizer, MessageArgs, MessageId, SupportedLocale};

use crate::{
    DismissOverlay,
    components::{self, Tone},
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
    _subscriptions: Vec<Subscription>,
}

impl TeleArkApp {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        localizer: Localizer,
        page: Page,
        show_upload: bool,
    ) -> Self {
        let placeholder = localizer.translate_or_id(MessageId::new("search-placeholder"));
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let search_subscription = cx.observe(&search_input, |_, _, cx| cx.notify());
        let nav_selection = match page {
            Page::Library | Page::FileDetail => "nav-all",
            Page::Transfers => "nav-transfers-all",
            Page::Vault => "nav-vault",
            Page::Channel => "channel-cinema",
            Page::Settings => "nav-settings",
        };

        Self {
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
            _subscriptions: vec![search_subscription],
        }
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
        cx.notify();
    }

    pub(crate) fn set_locale(
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

    fn render_header(&self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let brand = div()
            .h_full()
            .w(theme::SIDEBAR_WIDTH)
            .flex()
            .items_center()
            .pl(px(72.0))
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

        let search = div().flex_1().max_w(px(660.0)).h(px(36.0)).mx_5().child(
            gpui_component::input::Input::new(&self.search_input)
                .prefix(IconName::Search)
                .h(px(36.0)),
        );

        let status = div()
            .h_full()
            .flex()
            .items_center()
            .justify_end()
            .gap_5()
            .pr_5()
            .text_sm()
            .text_color(theme::text_secondary())
            .child(components::badge(
                self.tr("prototype-demo-badge"),
                Tone::Amber,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().size_2().rounded_full().bg(theme::green()))
                    .child(self.tr("connection-connected")),
            )
            .child(
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
        count: Option<&'static str>,
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
            .child(div().flex_1().child(self.tr(label_id)))
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

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
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
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
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
                                    .child("2.05 TB / 9.14 TB"),
                            ),
                    ),
            )
            .child(
                div()
                    .mt_3()
                    .h(px(4.0))
                    .rounded_full()
                    .bg(theme::border_subtle())
                    .child(div().h_full().w(px(45.0)).rounded_full().bg(theme::blue())),
            );

        div()
            .w(theme::SIDEBAR_WIDTH)
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme::sidebar())
            .border_r_1()
            .border_color(theme::border())
            .overflow_hidden()
            .child(self.group_label("nav-library"))
            .child(self.nav_item(
                "nav-all",
                "nav-all-files",
                IconName::FolderOpen,
                Some("2,851,233"),
                Page::Library,
                cx,
            ))
            .child(self.nav_item(
                "nav-recent",
                "nav-recent",
                IconName::Calendar,
                Some("45,721"),
                Page::Library,
                cx,
            ))
            .child(self.nav_item(
                "nav-videos",
                "nav-videos",
                IconName::GalleryVerticalEnd,
                Some("1,203,421"),
                Page::Library,
                cx,
            ))
            .child(self.nav_item(
                "nav-docs",
                "nav-documents",
                IconName::File,
                Some("621,334"),
                Page::Library,
                cx,
            ))
            .child(self.nav_item(
                "nav-archives",
                "nav-archives",
                IconName::Inbox,
                Some("472,113"),
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
                Some("176"),
                Page::Transfers,
                cx,
            ))
            .child(self.nav_item(
                "nav-completed",
                "nav-completed",
                IconName::CircleCheck,
                Some("3,842"),
                Page::Transfers,
                cx,
            ))
            .child(self.nav_item(
                "nav-failed",
                "nav-failed",
                IconName::TriangleAlert,
                Some("12"),
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

    fn render_page(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.page {
            Page::Library => self.render_library(window, cx),
            Page::Transfers => self.render_transfers(window, cx),
            Page::FileDetail => self.render_file_detail(window, cx),
            Page::Vault => self.render_vault(window, cx),
            Page::Channel => self.render_channel(window, cx),
            Page::Settings => self.render_settings(window, cx),
        }
    }
}

impl Render for TeleArkApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .child(self.render_header(window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_sidebar(cx))
                    .child(self.render_page(window, cx)),
            )
            .when(self.show_upload, |root| {
                root.child(screens::upload::render_upload_overlay(self, cx))
            })
    }
}
