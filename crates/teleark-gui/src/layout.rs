//! Geometry for the permanent sidebar and progressively disclosed details.
use gpui_kit::Window;
use teleark_runtime::{
    CHANNEL_SIDEBAR_DEFAULT_WIDTH, CHANNEL_SIDEBAR_MAX_WIDTH, CHANNEL_SIDEBAR_MIN_WIDTH,
};

pub(crate) const CHANNEL_CONTENT_MIN_WIDTH: f32 = 500.0;
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TransferColumnWidths {
    pub bytes: f32,
    pub eta: f32,
    pub progress: f32,
    pub actions: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LayoutPolicy {
    width: f32,
    height: f32,
    sidebar_collapsed: bool,
    channel_sidebar_width: f32,
}
impl LayoutPolicy {
    pub(crate) fn from_window(window: &Window) -> Self {
        let size = window.viewport_size();
        Self::from_size(size.width.into(), size.height.into())
    }
    pub(crate) fn from_size(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            sidebar_collapsed: true,
            channel_sidebar_width: f32::from(CHANNEL_SIDEBAR_DEFAULT_WIDTH),
        }
    }
    pub(crate) fn is_compact(self) -> bool {
        self.width < 1300.0
    }
    pub(crate) fn is_spacious(self) -> bool {
        self.width >= 1600.0
    }
    pub(crate) fn sidebar_width(self) -> f32 {
        if self.sidebar_collapsed { 64.0 } else { 184.0 }
    }
    pub(crate) fn with_sidebar_collapsed(mut self, collapsed: bool) -> Self {
        self.sidebar_collapsed = collapsed;
        self
    }
    pub(crate) fn with_channel_sidebar_width(mut self, width: f32) -> Self {
        self.channel_sidebar_width = width;
        self
    }
    pub(crate) fn channel_sidebar_max_width(self) -> f32 {
        (self.width - self.sidebar_width() - CHANNEL_CONTENT_MIN_WIDTH).clamp(
            f32::from(CHANNEL_SIDEBAR_MIN_WIDTH),
            f32::from(CHANNEL_SIDEBAR_MAX_WIDTH),
        )
    }
    pub(crate) fn channel_sidebar_width(self) -> f32 {
        self.channel_sidebar_width.clamp(
            f32::from(CHANNEL_SIDEBAR_MIN_WIDTH),
            self.channel_sidebar_max_width(),
        )
    }
    pub(crate) fn content_padding(self) -> f32 {
        if self.is_spacious() {
            24.0
        } else if self.is_compact() {
            12.0
        } else {
            20.0
        }
    }
    pub(crate) fn raw_table_width(self, channel_context: bool) -> f32 {
        self.width
            - self.sidebar_width()
            - 2.0 * self.content_padding()
            - 26.0
            - if channel_context {
                self.channel_sidebar_width()
            } else {
                0.0
            }
    }
    pub(crate) fn transfer_inspector_width(self) -> f32 {
        if self.is_compact() { 380.0 } else { 420.0 }
    }
    pub(crate) fn docks_transfer_inspector(self) -> bool {
        self.width
            - self.sidebar_width()
            - self.transfer_inspector_width()
            - 2.0 * self.content_padding()
            >= 900.0
    }
    pub(crate) fn transfer_columns(self) -> TransferColumnWidths {
        if self.is_compact() {
            TransferColumnWidths {
                bytes: 112.0,
                eta: 100.0,
                progress: 228.0,
                actions: 100.0,
            }
        } else {
            TransferColumnWidths {
                bytes: 140.0,
                eta: 112.0,
                progress: 280.0,
                actions: 116.0,
            }
        }
    }
    pub(crate) fn properties_width(self) -> f32 {
        if self.is_compact() { 250.0 } else { 300.0 }
    }
    pub(crate) fn upload_dialog_height(self) -> f32 {
        (self.height - 64.0).clamp(480.0, 720.0)
    }
    pub(crate) fn library_status_width(self) -> f32 {
        200.0
    }
    pub(crate) fn shows_library_encryption_parts(self) -> bool {
        !self.is_compact()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn channel_width_reserves_content_and_drives_table_geometry() {
        for collapsed in [false, true] {
            for width in [900.0, 1300.0, 1920.0] {
                let layout = LayoutPolicy::from_size(width, 600.0)
                    .with_sidebar_collapsed(collapsed)
                    .with_channel_sidebar_width(720.0);
                assert!(
                    width - layout.sidebar_width() - layout.channel_sidebar_width()
                        >= CHANNEL_CONTENT_MIN_WIDTH
                );
                assert_eq!(
                    layout.raw_table_width(false) - layout.raw_table_width(true),
                    layout.channel_sidebar_width()
                );
            }
        }
    }
    #[test]
    fn primary_navigation_and_content_fit_supported_sizes_and_breakpoints() {
        for width in [900.0, 960.0, 1299.0, 1300.0, 1360.0, 1599.0, 1600.0, 1920.0] {
            let policy = LayoutPolicy::from_size(width, 600.0);
            let content = width - policy.sidebar_width() - 2.0 * policy.content_padding();
            let columns = policy.transfer_columns();
            // Transfer rows: checkbox, processed bytes, ETA, progress, actions and padding.
            assert!(
                content
                    - 28.0
                    - columns.bytes
                    - columns.eta
                    - columns.progress
                    - columns.actions
                    - 64.0
                    >= 180.0
            );
            let library_fixed = if policy.is_compact() {
                90.0
            } else {
                494.0 + 126.0
            };
            assert!(content - 32.0 - library_fixed - policy.library_status_width() >= 190.0);
            assert!(policy.upload_dialog_height() <= 600.0 - 64.0);
            assert!(policy.transfer_inspector_width() < content);
        }
    }
}
