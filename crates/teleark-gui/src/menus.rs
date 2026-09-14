use gpui_kit::{Menu, MenuItem, SystemMenuType};
use teleark_i18n::{Localizer, MessageId};

#[cfg(not(target_os = "macos"))]
use crate::ToggleFullscreen;
use crate::{
    CloseWindow, MinimizeWindow, Quit, ShowAbout, ShowSettings, ShowStorage, ShowTransfers,
    UploadFile, ZoomWindow,
};

pub(crate) fn application_menus(localizer: &Localizer) -> Vec<Menu> {
    let tr = |id| localizer.translate_or_id(MessageId::new(id));
    vec![
        Menu {
            disabled: false,
            name: "TeleArk".into(),
            items: vec![
                MenuItem::action(tr("menu-application-about"), ShowAbout),
                MenuItem::action(tr("menu-application-settings"), ShowSettings),
                MenuItem::separator(),
                MenuItem::os_submenu(tr("menu-application-services"), SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action(tr("menu-application-quit"), Quit),
            ],
        },
        Menu {
            disabled: false,
            name: tr("menu-view-title").into(),
            // AppKit inserts its native Enter/Exit Full Screen command here.
            // Keeping that selector system-owned is what preserves the menu-bar
            // reveal and traffic-light behavior in a full-screen Space.
            items: vec![
                MenuItem::action(tr("menu-view-transfers"), ShowTransfers),
                MenuItem::action(tr("menu-view-storage"), ShowStorage),
                MenuItem::action(tr("menu-file-upload"), UploadFile),
                #[cfg(not(target_os = "macos"))]
                MenuItem::action(tr("menu-view-toggle-fullscreen"), ToggleFullscreen),
            ],
        },
        Menu {
            disabled: false,
            name: tr("menu-window-title").into(),
            items: vec![
                MenuItem::action(tr("menu-window-close"), CloseWindow),
                MenuItem::action(tr("menu-window-minimize"), MinimizeWindow),
                MenuItem::action(tr("menu-window-zoom"), ZoomWindow),
            ],
        },
    ]
}

pub(crate) fn menus_for_access(localizer: &Localizer, locked: bool) -> Vec<Menu> {
    let mut menus = application_menus(localizer);
    if locked {
        // Keep Quit and native window controls; workspace commands have no lock-screen role.
        menus[0].items.drain(..3);
        menus[1].items.drain(..3);
    }
    menus
}

#[cfg(test)]
mod tests {
    use teleark_i18n::SupportedLocale;

    use super::*;

    #[test]
    fn english_builds_a_native_menu_with_fullscreen_and_window_recovery() {
        {
            let locale = SupportedLocale::EnUs;
            let localizer = Localizer::new(locale).expect("locale catalog");
            let menus = application_menus(&localizer);
            assert_eq!(menus.len(), 3);
            assert_eq!(menus[0].name.as_ref(), "TeleArk");
            assert_eq!(menus[0].items.len(), 6);
            #[cfg(target_os = "macos")]
            assert_eq!(menus[1].items.len(), 3);
            #[cfg(not(target_os = "macos"))]
            assert_eq!(menus[1].items.len(), 4);
            assert_eq!(menus[2].items.len(), 3);
        }
    }
}
