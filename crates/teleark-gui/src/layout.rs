//! Responsive layout decisions shared by every TeleArk screen.
//!
//! Keep the breakpoint math independent from GPUI widgets so the supported
//! window sizes can be checked deterministically without opening a window.

use gpui::Window;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WindowClass {
    Compact,
    Standard,
    Spacious,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LayoutPolicy {
    class: WindowClass,
    width: f32,
    height: f32,
}

impl LayoutPolicy {
    const COMPACT_MAX_WIDTH: f32 = 1_299.0;
    const SPACIOUS_MIN_WIDTH: f32 = 1_600.0;
    #[cfg(test)]
    const HEADER_HEIGHT: f32 = 58.0;
    #[cfg(test)]
    const COMPACT_NAV_HEIGHT: f32 = 46.0;
    const PANEL_GAP: f32 = 16.0;
    #[cfg(test)]
    const LIBRARY_BASE_COLUMNS: f32 = 494.0;
    #[cfg(test)]
    const TABLE_HORIZONTAL_INSET: f32 = 32.0;

    pub(crate) fn from_window(window: &Window) -> Self {
        let viewport = window.viewport_size();
        Self::from_size(viewport.width.into(), viewport.height.into())
    }

    pub(crate) fn from_size(width: f32, height: f32) -> Self {
        let class = if width <= Self::COMPACT_MAX_WIDTH {
            WindowClass::Compact
        } else if width >= Self::SPACIOUS_MIN_WIDTH {
            WindowClass::Spacious
        } else {
            WindowClass::Standard
        };
        Self {
            class,
            width,
            height,
        }
    }

    pub(crate) fn is_compact(self) -> bool {
        self.class == WindowClass::Compact
    }

    pub(crate) fn is_spacious(self) -> bool {
        self.class == WindowClass::Spacious
    }

    pub(crate) fn shows_global_sidebar(self) -> bool {
        !self.is_compact()
    }

    pub(crate) fn shows_full_header_status(self) -> bool {
        !self.is_compact()
    }

    pub(crate) fn sidebar_width(self) -> f32 {
        if self.is_spacious() { 240.0 } else { 216.0 }
    }

    pub(crate) fn header_brand_width(self) -> f32 {
        if self.is_compact() {
            156.0
        } else {
            self.sidebar_width()
        }
    }

    pub(crate) fn content_padding(self) -> f32 {
        match self.class {
            WindowClass::Compact => 12.0,
            WindowClass::Standard => 20.0,
            WindowClass::Spacious => 24.0,
        }
    }

    pub(crate) fn channel_compact_file_list_height(self) -> f32 {
        (self.height - 220.0).clamp(380.0, 560.0)
    }

    pub(crate) fn transfer_inspector_width(self) -> f32 {
        match self.class {
            WindowClass::Compact => 400.0,
            WindowClass::Standard => 420.0,
            WindowClass::Spacious => 440.0,
        }
    }

    pub(crate) fn shows_transfer_source(self) -> bool {
        self.is_spacious()
    }

    pub(crate) fn shows_transfer_speed(self) -> bool {
        true
    }

    pub(crate) fn local_navigation_width(self) -> f32 {
        match self.class {
            WindowClass::Compact => 184.0,
            WindowClass::Standard => 220.0,
            WindowClass::Spacious => 236.0,
        }
    }

    pub(crate) fn properties_width(self) -> f32 {
        match self.class {
            WindowClass::Compact => 280.0,
            WindowClass::Standard => 320.0,
            WindowClass::Spacious => 360.0,
        }
    }

    pub(crate) fn upload_dialog_width(self) -> f32 {
        (self.width - 32.0).clamp(720.0, 920.0)
    }

    pub(crate) fn upload_dialog_height(self) -> f32 {
        (self.height - 40.0).clamp(520.0, 720.0)
    }

    pub(crate) fn upload_preview_width(self) -> f32 {
        if self.is_compact() { 176.0 } else { 210.0 }
    }

    pub(crate) fn transfer_status_width(self) -> f32 {
        if self.is_spacious() { 108.0 } else { 118.0 }
    }

    pub(crate) fn library_status_width(self) -> f32 {
        200.0
    }

    pub(crate) fn shows_library_encryption_parts(self) -> bool {
        !self.is_compact()
    }

    pub(crate) fn file_detail_part_columns(self) -> u16 {
        if self.is_compact() || self.file_detail_parts_width() < 750.0 {
            3
        } else {
            5
        }
    }

    pub(crate) fn route_width(self) -> f32 {
        self.width
            - if self.shows_global_sidebar() {
                self.sidebar_width()
            } else {
                0.0
            }
    }

    #[cfg(test)]
    pub(crate) fn route_height(self) -> f32 {
        self.height
            - Self::HEADER_HEIGHT
            - if self.is_compact() {
                Self::COMPACT_NAV_HEIGHT
            } else {
                0.0
            }
    }

    #[cfg(test)]
    pub(crate) fn library_filename_width(self) -> f32 {
        let optional_columns = if self.shows_library_encryption_parts() {
            72.0 + 54.0
        } else {
            0.0
        };
        self.route_width()
            - self.content_padding() * 2.0
            - Self::TABLE_HORIZONTAL_INSET
            - Self::LIBRARY_BASE_COLUMNS
            - self.library_status_width()
            - optional_columns
    }

    #[cfg(test)]
    pub(crate) fn library_visible_rows_height(self) -> f32 {
        self.route_height()
            - 58.0
            - 42.0
            - self.content_padding()
            - 36.0
            - if self.is_compact() { 58.0 } else { 46.0 }
    }

    #[cfg(test)]
    pub(crate) fn transfer_main_width(self) -> f32 {
        self.route_width()
    }

    #[cfg(test)]
    pub(crate) fn transfer_filename_width(self) -> f32 {
        let optional_columns = if self.shows_transfer_source() {
            108.0 + 76.0
        } else {
            0.0
        } + if self.shows_transfer_speed() {
            82.0 + if self.is_compact() { 0.0 } else { 64.0 }
        } else {
            0.0
        };
        let fixed_columns =
            28.0 + 76.0 + 112.0 + 152.0 + self.transfer_status_width() + optional_columns;
        self.transfer_main_width() - self.content_padding() * 2.0 - 24.0 - fixed_columns
    }

    #[cfg(test)]
    pub(crate) fn transfer_visible_rows_height(self) -> f32 {
        let summary = 94.0;
        let bottom = self.content_padding();
        let toolbar = 50.0;
        let table_chrome = 34.0 + if self.is_compact() { 58.0 } else { 38.0 };
        self.route_height() - summary - bottom - toolbar - table_chrome
    }

    #[cfg(test)]
    pub(crate) fn transfer_inspector_scroll_height(self) -> f32 {
        // Only header/status and tabs surround the scrollable diagnostic panel.
        self.route_height() - 236.0 - 42.0
    }

    pub(crate) fn file_detail_parts_width(self) -> f32 {
        self.route_width()
            - self.content_padding() * 2.0
            - self.properties_width()
            - Self::PANEL_GAP
    }

    #[cfg(test)]
    pub(crate) fn file_detail_visible_rows_height(self) -> f32 {
        // Toolbar, a conservatively wrapped hero, outer content padding, and
        // the tabs/header/footer surrounding the independently scrolling rows.
        self.route_height() - 58.0 - 112.0 - self.content_padding() * 2.0 - 130.0
    }

    #[cfg(test)]
    pub(crate) fn local_settings_content_width(self) -> f32 {
        self.route_width()
            - self.content_padding() * 2.0
            - self.local_navigation_width()
            - Self::PANEL_GAP
    }

    #[cfg(test)]
    pub(crate) fn local_settings_viewport_height(self) -> f32 {
        self.route_height() - 58.0 - self.content_padding()
    }

    #[cfg(test)]
    pub(crate) fn channel_compact_panel_width(self) -> f32 {
        self.route_width() - self.content_padding() * 2.0
    }

    #[cfg(test)]
    pub(crate) fn channel_job_viewport_height(self) -> f32 {
        self.route_height() - 58.0 - 98.0 - 98.0 - self.content_padding() - 16.0
    }

    #[cfg(test)]
    pub(crate) fn upload_form_width(self) -> f32 {
        self.upload_dialog_width() - self.upload_preview_width()
    }

    #[cfg(test)]
    pub(crate) fn upload_preview_content_width(self) -> f32 {
        self.upload_preview_width() - if self.is_compact() { 32.0 } else { 40.0 }
    }

    #[cfg(test)]
    pub(crate) fn upload_scroll_height(self) -> f32 {
        self.upload_dialog_height() - 52.0 - 58.0
    }
}

#[cfg(test)]
mod tests {
    use teleark_i18n::{Localizer, MessageId, SupportedLocale};

    use super::*;

    const SUPPORTED_VIEWPORTS: [(f32, f32); 4] = [
        (900.0, 600.0),
        (960.0, 640.0),
        (1360.0, 760.0),
        (1920.0, 1080.0),
    ];

    #[test]
    fn target_window_sizes_select_expected_layouts() {
        let compact = LayoutPolicy::from_size(960.0, 640.0);
        let default = LayoutPolicy::from_size(1360.0, 760.0);
        let large = LayoutPolicy::from_size(1920.0, 1080.0);

        assert_eq!(compact.class, WindowClass::Compact);
        assert_eq!(default.class, WindowClass::Standard);
        assert_eq!(large.class, WindowClass::Spacious);
        assert!(!compact.shows_global_sidebar());
        assert!(default.shows_global_sidebar());
        assert!(large.shows_global_sidebar());
    }

    #[test]
    fn compact_channel_file_list_grows_with_available_window_height() {
        let minimum = LayoutPolicy::from_size(900.0, 600.0);
        let common = LayoutPolicy::from_size(960.0, 640.0);
        let tall = LayoutPolicy::from_size(1_200.0, 960.0);

        assert_eq!(minimum.channel_compact_file_list_height(), 380.0);
        assert_eq!(common.channel_compact_file_list_height(), 420.0);
        assert_eq!(tall.channel_compact_file_list_height(), 560.0);
    }

    #[test]
    fn dialogs_and_inspectors_fit_each_supported_target() {
        for (width, height) in SUPPORTED_VIEWPORTS {
            let policy = LayoutPolicy::from_size(width, height);
            assert!(policy.upload_dialog_width() <= width - 32.0);
            assert!(policy.upload_dialog_height() <= height - 32.0);
            assert!(policy.transfer_inspector_width() < width / 2.0);
            assert!(policy.properties_width() < width / 2.0);
        }
    }

    #[test]
    fn transfer_columns_add_detail_as_space_increases() {
        let compact = LayoutPolicy::from_size(960.0, 640.0);
        let default = LayoutPolicy::from_size(1360.0, 760.0);
        let large = LayoutPolicy::from_size(1920.0, 1080.0);

        assert!(!compact.shows_transfer_source());
        assert!(compact.shows_transfer_speed());
        assert!(!default.shows_transfer_source());
        assert!(default.shows_transfer_speed());
        assert!(large.shows_transfer_source());
        assert!(large.shows_transfer_speed());
    }

    #[test]
    fn breakpoint_edges_never_collapse_primary_columns() {
        for (width, expected_class) in [
            (1_299.0, WindowClass::Compact),
            (1_300.0, WindowClass::Standard),
            (1_341.0, WindowClass::Standard),
            (1_342.0, WindowClass::Standard),
            (1_599.0, WindowClass::Standard),
            (1_600.0, WindowClass::Spacious),
        ] {
            let policy = LayoutPolicy::from_size(width, 760.0);
            assert_eq!(policy.class, expected_class, "{width}");
            assert!(policy.library_filename_width() >= 150.0, "library {width}");
            assert!(
                policy.transfer_filename_width() >= 240.0,
                "transfer {width}"
            );
            assert!(
                policy.file_detail_parts_width() / f32::from(policy.file_detail_part_columns())
                    >= 150.0,
                "file detail {width}"
            );
        }
    }

    #[test]
    fn every_route_keeps_a_usable_primary_content_budget() {
        for (width, height) in SUPPORTED_VIEWPORTS {
            let policy = LayoutPolicy::from_size(width, height);

            assert!(policy.route_height() >= 496.0, "{width}x{height}");
            assert!(
                policy.library_filename_width() >= 116.0,
                "library at {width}x{height}"
            );
            assert!(
                policy.library_visible_rows_height() >= 284.0,
                "library rows at {width}x{height}"
            );
            assert!(
                policy.transfer_filename_width() >= 240.0,
                "transfers at {width}x{height}"
            );
            assert!(
                policy.transfer_visible_rows_height() >= 240.0,
                "transfer table at {width}x{height}"
            );
            assert!(
                policy.transfer_inspector_scroll_height() >= 150.0,
                "transfer inspector at {width}x{height}"
            );
            assert!(
                policy.file_detail_parts_width() >= 580.0,
                "file detail at {width}x{height}"
            );
            assert!(
                policy.file_detail_visible_rows_height() >= 172.0,
                "file detail rows at {width}x{height}"
            );
            assert!(
                policy.local_settings_content_width() >= 676.0,
                "settings/vault at {width}x{height}"
            );
            assert!(
                policy.local_settings_viewport_height() >= 426.0,
                "settings/vault viewport at {width}x{height}"
            );
            assert!(
                9.0 * 38.0 + 16.0 + 38.0 <= policy.local_settings_viewport_height(),
                "settings navigation at {width}x{height}"
            );
            assert!(
                9.0 * 36.0 + 16.0 <= policy.local_settings_viewport_height(),
                "vault navigation at {width}x{height}"
            );
            assert!(
                policy.channel_compact_panel_width() >= 876.0,
                "channel at {width}x{height}"
            );
            if policy.is_compact() {
                assert!(
                    policy.channel_compact_file_list_height() >= 380.0,
                    "channel files at {width}x{height}"
                );
            }
            if !policy.is_compact() {
                assert!(
                    policy.channel_job_viewport_height() >= 400.0,
                    "channel job at {width}x{height}"
                );
            }
            assert!(
                policy.upload_form_width() >= 692.0,
                "upload form at {width}x{height}"
            );
            assert!(
                policy.upload_scroll_height() >= 450.0,
                "upload scroll region at {width}x{height}"
            );
        }
    }

    #[test]
    fn compact_file_detail_progressively_hides_technical_columns() {
        let compact = LayoutPolicy::from_size(900.0, 600.0);
        let default = LayoutPolicy::from_size(1360.0, 760.0);

        assert_eq!(compact.file_detail_part_columns(), 3);
        assert_eq!(default.file_detail_part_columns(), 5);
        assert!(compact.file_detail_parts_width() / 3.0 >= 190.0);
        assert!(default.file_detail_parts_width() / 5.0 >= 150.0);
    }

    #[test]
    fn localized_primary_controls_fit_all_route_budgets() {
        let compact = LayoutPolicy::from_size(900.0, 600.0);
        let default = LayoutPolicy::from_size(1360.0, 760.0);

        for locale in [
            SupportedLocale::EnUs,
            SupportedLocale::ZhCn,
            SupportedLocale::JaJp,
        ] {
            let localizer = Localizer::new(locale).expect("embedded locale");

            let library_toolbar = 220.0
                + icon_button_width(&localizer, "action-import-files")
                + 12.0
                + compact.content_padding() * 2.0;
            assert!(library_toolbar <= compact.route_width(), "{locale:?}");

            let transfer_toolbar = icon_button_width(&localizer, "action-start-all")
                + icon_button_width(&localizer, "action-pause-all")
                + 5.0 * 34.0
                + 6.0 * 8.0
                + compact.content_padding() * 2.0;
            assert!(
                transfer_toolbar <= compact.transfer_main_width(),
                "{locale:?}"
            );

            let upload_footer = text_button_width(&localizer, "action-cancel")
                + icon_button_width(&localizer, "upload-add-to-queue")
                + 8.0
                + 40.0;
            assert!(upload_footer <= compact.upload_form_width(), "{locale:?}");
            assert!(
                icon_button_width(&localizer, "upload-change-file")
                    <= compact.upload_preview_content_width(),
                "{locale:?} upload preview action"
            );

            let vault_actions = icon_button_width(&localizer, "vault-change-password")
                + icon_button_width(&localizer, "vault-unlock-to-view")
                + 8.0;
            assert!(
                vault_actions <= compact.local_settings_content_width() - 40.0,
                "{locale:?}"
            );

            let channel_toolbar = estimated_message_width(&localizer, "index-channel-title")
                + icon_button_width(&localizer, "index-options")
                + compact.content_padding() * 2.0;
            assert!(channel_toolbar <= compact.route_width(), "{locale:?}");

            let settings_toolbar = estimated_message_width(&localizer, "settings-title")
                + estimated_message_width(&localizer, "settings-preferences-failed")
                + 28.0
                + compact.content_padding() * 2.0;
            assert!(settings_toolbar <= compact.route_width(), "{locale:?}");

            let longest_local_nav = [
                "settings-general",
                "settings-accounts",
                "settings-storage",
                "settings-downloads",
                "settings-uploads",
                "settings-key-vault",
                "settings-indexing",
                "settings-notifications",
                "settings-appearance",
            ]
            .into_iter()
            .map(|id| estimated_message_width(&localizer, id))
            .fold(0.0_f32, f32::max);
            assert!(
                longest_local_nav <= compact.local_navigation_width() - 60.0,
                "{locale:?}"
            );

            for status in [
                "transfer-state-downloading",
                "transfer-state-uploading",
                "transfer-state-verifying",
                "transfer-state-completed",
            ] {
                assert!(
                    estimated_message_width(&localizer, status) + 16.0
                        <= compact.transfer_status_width(),
                    "{locale:?} {status}"
                );
            }

            for label in [
                "detail-source-channel",
                "detail-message-id",
                "detail-local-path",
                "detail-speed",
                "detail-transferred",
                "detail-active-connections",
                "detail-workers",
                "detail-retries",
                "detail-created",
                "detail-started",
            ] {
                assert!(
                    estimated_message_width(&localizer, label) <= 128.0,
                    "{locale:?} transfer inspector {label}"
                );
            }

            for label in [
                "table-name",
                "table-size",
                "table-type",
                "table-source",
                "file-detail-modified-at",
                "table-parts",
                "file-detail-encryption",
                "file-detail-package-id",
                "file-detail-hash",
            ] {
                assert!(
                    estimated_message_width(&localizer, label) <= 116.0,
                    "{locale:?} file inspector {label}"
                );
            }

            assert!(
                estimated_message_width(&localizer, "connection-client") <= 82.0,
                "{locale:?} connection client"
            );
            assert!(
                estimated_message_width(&localizer, "connection-latency") <= 72.0,
                "{locale:?} connection latency"
            );
            let connection_card_width = compact.route_width().min(440.0);
            assert!(connection_card_width - 24.0 - 70.0 - 70.0 - 82.0 - 72.0 >= 120.0);

            for status in [
                "file-state-local",
                "file-state-uploading",
                "file-state-uploaded",
                "file-state-verifying",
                "file-state-verification-failed",
                "file-state-remote-missing",
            ] {
                assert!(
                    estimated_message_width(&localizer, status) + 16.0
                        <= compact.library_status_width(),
                    "{locale:?} {status}"
                );
            }

            let technical_headers = [
                "file-detail-part-index",
                "table-size",
                "table-status",
                "file-detail-remote-id",
                "file-detail-upload-time",
            ];
            let default_column_width =
                default.file_detail_parts_width() / default.file_detail_part_columns() as f32;
            for header in technical_headers {
                assert!(
                    estimated_message_width(&localizer, header) <= default_column_width,
                    "{locale:?} {header}"
                );
            }
        }
    }

    fn icon_button_width(localizer: &Localizer, id: &'static str) -> f32 {
        estimated_message_width(localizer, id) + 48.0
    }

    fn text_button_width(localizer: &Localizer, id: &'static str) -> f32 {
        estimated_message_width(localizer, id) + 32.0
    }

    fn estimated_message_width(localizer: &Localizer, id: &'static str) -> f32 {
        localizer
            .translate_or_id(MessageId::new(id))
            .chars()
            .map(|character| {
                if character.is_ascii_whitespace() {
                    4.0
                } else if character.is_ascii() {
                    7.0
                } else {
                    13.0
                }
            })
            .sum()
    }
}
