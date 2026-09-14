//! Platform window commands share the native close/quit lifecycle gate.
use crate::{
    app::{TeleArkApp, lifecycle::TransitionAction},
    *,
};
use gpui_kit::{AnyWindowHandle, App, Global, KeyBinding, WeakEntity};

#[derive(Default)]
struct MainWindow(Option<(AnyWindowHandle, WeakEntity<TeleArkApp>)>);
impl Global for MainWindow {}

pub(crate) fn register(window: AnyWindowHandle, app: WeakEntity<TeleArkApp>, cx: &mut App) {
    cx.set_global(MainWindow(Some((window, app))));
}

pub(crate) fn init(cx: &mut App) {
    cx.set_global(MainWindow::default());
    cx.bind_keys(bindings(cfg!(target_os = "macos")));
    cx.on_action(close_window);
    cx.on_action(quit);
    cx.on_action(minimize_window);
    cx.on_action(toggle_fullscreen);
    cx.on_action(zoom_window);
}

fn bindings(macos: bool) -> Vec<KeyBinding> {
    let mut keys = vec![KeyBinding::new("escape", DismissOverlay, None)];
    macro_rules! key {
        ($mac:literal, $other:literal, $action:expr) => {
            keys.push(KeyBinding::new(
                if macos { $mac } else { $other },
                $action,
                None,
            ));
        };
    }
    key!("cmd-w", "ctrl-w", CloseWindow);
    key!("cmd-q", "ctrl-q", Quit);
    key!("cmd-,", "ctrl-,", ShowSettings);
    key!("cmd-1", "ctrl-1", ShowTransfers);
    key!("cmd-2", "ctrl-2", ShowStorage);
    key!("cmd-u", "ctrl-u", UploadFile);
    key!("cmd-f", "ctrl-f", FocusSearch);
    key!("ctrl-cmd-f", "f11", ToggleFullscreen);
    if macos {
        keys.push(KeyBinding::new("cmd-m", MinimizeWindow, None));
    }
    // Alt+F4, Win+Down, Alt+F9 and text editing remain owned by the OS/input control.
    keys
}

fn close_window(_: &CloseWindow, cx: &mut App) {
    let Some(active) = cx.active_window() else {
        return;
    };
    let main = cx.global::<MainWindow>().0.clone();
    if let Some((main, app)) = main
        && active == main
    {
        let _ = app.update(cx, |app, cx| {
            app.request_transition(TransitionAction::Quit, cx)
        });
    } else {
        cx.defer(move |cx| {
            let _ = active.update(cx, |_, window, _| window.remove_window());
        });
    }
}
fn quit(_: &Quit, cx: &mut App) {
    if let Some((_, app)) = cx.global::<MainWindow>().0.clone()
        && app
            .update(cx, |app, cx| {
                app.request_transition(TransitionAction::Quit, cx)
            })
            .is_ok()
    {
        return;
    }
    // Startup also responds before the workspace and its runtime are installed.
    cx.quit();
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Action as _, Focusable as _};
    use gpui_kit as gpui;

    #[test]
    fn platform_shortcuts_do_not_steal_text_editing_or_system_minimize() {
        for macos in [false, true] {
            let keys = bindings(macos);
            let key_for = |name: &str| {
                keys.iter()
                    .find(|k| k.action().name() == name)
                    .expect("command has a binding")
                    .keystrokes()[0]
                    .clone()
            };
            let expected =
                KeyBinding::new(if macos { "cmd-w" } else { "ctrl-w" }, CloseWindow, None);
            assert_eq!(key_for(CloseWindow.name()), expected.keystrokes()[0]);
            let fullscreen = KeyBinding::new(
                if macos { "ctrl-cmd-f" } else { "f11" },
                ToggleFullscreen,
                None,
            );
            assert_eq!(key_for(ToggleFullscreen.name()), fullscreen.keystrokes()[0]);
            assert_eq!(
                keys.iter()
                    .any(|k| k.action().name() == MinimizeWindow.name()),
                macos
            );
            for editing in [
                "ctrl-a", "ctrl-c", "ctrl-v", "ctrl-x", "ctrl-z", "cmd-a", "cmd-c", "cmd-v",
                "cmd-x", "cmd-z",
            ] {
                let edit = KeyBinding::new(editing, DismissOverlay, None);
                assert!(keys.iter().all(|k| k.keystrokes() != edit.keystrokes()));
            }
        }
    }

    #[gpui::test]
    fn closing_an_auxiliary_window_keeps_the_main_window_and_tasks(cx: &mut gpui::TestAppContext) {
        struct Auxiliary;
        impl gpui::Render for Auxiliary {
            fn render(
                &mut self,
                _: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                gpui::div()
            }
        }
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        app.update(cx, |app, _| {
            app.visual_preview = false;
            app.upload_in_flight = true;
        });
        let child = cx.update(|_, cx| {
            cx.open_window(gpui::WindowOptions::default(), |_, cx| {
                cx.new(|_| Auxiliary)
            })
            .expect("auxiliary")
        });
        cx.update(|_, cx| {
            child
                .update(cx, |_, window, cx| {
                    window.activate_window();
                    window.dispatch_action(Box::new(CloseWindow), cx);
                })
                .expect("close auxiliary");
        });
        cx.run_until_parked();
        cx.update(|_, cx| assert!(child.update(cx, |_, _, _| ()).is_err()));
        app.read_with(cx, |app, _| {
            assert!(app.transition.is_none());
            assert!(app.upload_in_flight);
        });
    }

    #[gpui::test]
    fn close_shortcuts_reach_the_real_exit_gate_from_a_locked_pin_input(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        app.update(cx, |app, cx| {
            app.visual_preview = false;
            app.app_lock.locked = true;
            app.upload_in_flight = true;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.activate_window();
            app.read(cx)
                .app_lock
                .pin
                .read(cx)
                .focus_handle(cx)
                .focus(window, cx);
        });
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "ctrl-cmd-f"
        } else {
            "f11"
        });
        cx.run_until_parked();
        cx.update(|window, _| {
            assert!(
                window.is_fullscreen(),
                "global window command runs after dispatch"
            )
        });
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-w"
        } else {
            "ctrl-w"
        });
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            assert!(
                app.transition
                    .as_ref()
                    .is_some_and(|t| t.action == TransitionAction::Quit
                        && t.phase == crate::app::lifecycle::TransitionPhase::Confirm)
            )
        });
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            assert!(app.transition.is_none());
            assert!(app.upload_in_flight, "cancel leaves work untouched");
            assert!(app.app_lock.locked);
        });
        cx.update(|window, _| {
            assert!(
                window.is_fullscreen(),
                "dialog Escape preserves full screen"
            )
        });
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, _| assert!(!window.is_fullscreen()));
    }
}
