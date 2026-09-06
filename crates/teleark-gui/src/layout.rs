//! Geometry for the permanent sidebar and progressively disclosed details.
use gpui_kit::Window;
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LayoutPolicy {
    width: f32,
    height: f32,
}
impl LayoutPolicy {
    pub(crate) fn from_window(window: &Window) -> Self {
        let size = window.viewport_size();
        Self::from_size(size.width.into(), size.height.into())
    }
    pub(crate) fn from_size(width: f32, height: f32) -> Self {
        Self { width, height }
    }
    pub(crate) fn is_compact(self) -> bool {
        self.width < 1300.0
    }
    pub(crate) fn is_spacious(self) -> bool {
        self.width >= 1600.0
    }
    pub(crate) fn sidebar_width(self) -> f32 {
        if self.is_spacious() {
            232.0
        } else if self.is_compact() {
            190.0
        } else {
            214.0
        }
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
    pub(crate) fn transfer_inspector_width(self) -> f32 {
        if self.is_compact() { 380.0 } else { 420.0 }
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
    pub(crate) fn file_detail_part_columns(self) -> u16 {
        if self.is_compact() { 3 } else { 5 }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn primary_navigation_and_content_fit_supported_sizes_and_breakpoints() {
        for width in [900.0, 960.0, 1299.0, 1300.0, 1360.0, 1599.0, 1600.0, 1920.0] {
            let policy = LayoutPolicy::from_size(width, 600.0);
            let content = width - policy.sidebar_width() - 2.0 * policy.content_padding();
            // Transfer rows: checkbox, progress, actions, padding/gaps.
            assert!(content - 28.0 - 188.0 - 116.0 - 64.0 >= 260.0);
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
