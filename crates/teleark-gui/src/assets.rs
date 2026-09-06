//! Small original vector symbols alongside GPUI Kit's bundled icon set.
use gpui_kit::component::IconNamed;
use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

#[derive(Clone, Copy)]
pub(crate) enum Symbol {
    Lock,
    Unlock,
    Layers,
    Hash,
    Help,
}
impl IconNamed for Symbol {
    fn path(self) -> SharedString {
        match self {
            Self::Lock => "teleark/lock.svg",
            Self::Unlock => "teleark/unlock.svg",
            Self::Layers => "teleark/layers.svg",
            Self::Hash => "teleark/hash.svg",
            Self::Help => "teleark/help.svg",
        }
        .into()
    }
}
const SYMBOLS: [(&str, &[u8]); 5] = [
    ("teleark/lock.svg", br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="5" y="10" width="14" height="11" rx="3"/><path d="M8 10V7a4 4 0 0 1 8 0v3m-4 5v2"/></svg>"#),
    ("teleark/unlock.svg", br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="5" y="10" width="14" height="11" rx="3"/><path d="M8 10V7a4 4 0 0 1 7.5-2M12 15v2"/></svg>"#),
    ("teleark/layers.svg", br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="m3 8 9-5 9 5-9 5-9-5Zm0 4 9 5 9-5M3 16l9 5 9-5"/></svg>"#),
    ("teleark/hash.svg", br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><path d="m10 4-3 16M17 4l-3 16M4 9h17M3 15h17"/></svg>"#),
    ("teleark/help.svg", br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><circle cx="12" cy="12" r="9"/><path d="M9.5 8.5c.5-3 6-2.5 5 1-.5 1.5-2.5 1.5-2.5 3M12 16h.01"/></svg>"#),
];
pub(crate) struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = SYMBOLS.iter().find(|(name, _)| *name == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit::assets::Assets.load(path)
    }
    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(
            SYMBOLS
                .iter()
                .filter(|(name, _)| name.starts_with(path))
                .map(|(name, _)| SharedString::from(*name)),
        );
        Ok(paths)
    }
}
