//! Proxy editor and retained network feedback. No sockets or persistence on UI.
use super::*;
use std::{
    net::{IpAddr, SocketAddr},
    time::Instant,
};
use teleark_runtime::{NetworkPhase, NetworkRoute, NetworkSnapshot, ProxyConfig, ProxyProtocol};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProxyAction {
    Idle,
    Applying,
    Testing,
    Succeeded,
    Failed,
}
impl ProxyAction {
    pub(crate) fn busy(self) -> bool {
        matches!(self, Self::Applying | Self::Testing)
    }
}

pub(crate) struct ProxyUi {
    pub enabled: bool,
    pub protocol: ProxyProtocol,
    pub host: Entity<InputState>,
    pub port: Entity<InputState>,
    pub username: Entity<InputState>,
    pub password: Entity<InputState>,
    pub load_failed: bool,
    pub invalid: bool,
    pub action: ProxyAction,
    pub started: Instant,
    pub snapshot: Option<NetworkSnapshot>,
    pub test_elapsed: Option<Duration>,
    pub history_expanded: bool,
    task: Option<Task<()>>,
    updates: Option<Task<()>>,
}

impl ProxyUi {
    pub(super) fn new(
        route: Result<NetworkRoute, teleark_core::ApplicationErrorKind>,
        preview: bool,
        window: &mut Window,
        cx: &mut Context<TeleArkApp>,
    ) -> Self {
        let load_failed = route.is_err() && !preview;
        let config = match route {
            Ok(NetworkRoute::Proxy(config)) => Some(config),
            _ => None,
        };
        let mut input = |value: String, masked| {
            cx.new(|cx| {
                let mut input = InputState::new(window, cx).masked(masked);
                input.set_value(value, window, cx);
                input
            })
        };
        Self {
            enabled: config.is_some(),
            protocol: config
                .as_ref()
                .map_or(ProxyProtocol::Socks5, |c| c.protocol),
            host: input(
                config
                    .as_ref()
                    .map_or_else(|| "127.0.0.1".into(), |c| c.address.ip().to_string()),
                false,
            ),
            port: input(
                config
                    .as_ref()
                    .map_or_else(|| "1080".into(), |c| c.address.port().to_string()),
                false,
            ),
            username: input(
                config
                    .as_ref()
                    .map_or_else(String::new, |c| c.username().to_owned()),
                false,
            ),
            password: input(
                config
                    .as_ref()
                    .map_or_else(String::new, |c| c.password().to_owned()),
                true,
            ),
            load_failed,
            invalid: false,
            action: ProxyAction::Idle,
            started: Instant::now(),
            snapshot: None,
            test_elapsed: None,
            history_expanded: false,
            task: None,
            updates: None,
        }
    }

    pub(crate) fn waiting(&self) -> bool {
        self.action.busy()
            || self.snapshot.as_ref().is_some_and(|s| {
                matches!(
                    s.phase,
                    NetworkPhase::Applying
                        | NetworkPhase::Connecting
                        | NetworkPhase::TestQueued
                        | NetworkPhase::Testing
                )
            })
    }

    pub(crate) fn show_banner(&self) -> bool {
        self.load_failed
            || self.waiting()
            || self
                .snapshot
                .as_ref()
                .is_some_and(|s| matches!(s.phase, NetworkPhase::Blocked(_)))
    }
}

impl TeleArkApp {
    pub(super) fn start_network_observer(&mut self, cx: &mut Context<Self>) {
        let Some(telegram) = &self.telegram else {
            return;
        };
        let mut updates = telegram.network_updates();
        self.proxy.snapshot = Some(telegram.network_snapshot());
        self.proxy.updates = Some(cx.spawn(async move |this, cx| {
            while let Some(snapshot) = updates.changed().await {
                let Some(this) = this.upgrade() else { break };
                this.update(cx, |this, cx| {
                    this.proxy.snapshot = Some(snapshot);
                    cx.notify();
                });
            }
        }));
    }

    fn proxy_draft(&self, cx: &Context<Self>) -> Result<NetworkRoute, ()> {
        if !self.proxy.enabled {
            return Ok(NetworkRoute::Direct);
        }
        let host: IpAddr = self
            .proxy
            .host
            .read(cx)
            .value()
            .trim()
            .trim_matches(['[', ']'])
            .parse()
            .map_err(|_| ())?;
        let port: u16 = self
            .proxy
            .port
            .read(cx)
            .value()
            .trim()
            .parse()
            .map_err(|_| ())?;
        ProxyConfig::new(
            self.proxy.protocol,
            SocketAddr::new(host, port),
            self.proxy.username.read(cx).value().to_string(),
            self.proxy.password.read(cx).value().to_string(),
        )
        .map(NetworkRoute::Proxy)
        .map_err(|_| ())
    }

    pub(crate) fn apply_proxy(&mut self, cx: &mut Context<Self>) {
        if self.proxy.action.busy() || self.proxy.load_failed {
            return;
        }
        if self.proxy_draft(cx).is_err() {
            self.proxy.invalid = true;
            cx.notify();
            return;
        }
        self.request_transition(super::lifecycle::TransitionAction::ApplyProxy, cx);
    }

    pub(super) fn apply_proxy_after_drain(&mut self, cx: &mut Context<Self>) {
        if self.proxy.action.busy() || self.proxy.load_failed {
            return;
        }
        let route = match self.proxy_draft(cx) {
            Ok(route) => route,
            Err(()) => {
                self.proxy.invalid = true;
                cx.notify();
                return;
            }
        };
        self.proxy.invalid = false;
        let (Some(telegram), Some(library)) = (self.telegram.clone(), self.library.clone()) else {
            self.proxy.action = ProxyAction::Failed;
            cx.notify();
            return;
        };
        self.proxy.action = ProxyAction::Applying;
        if let Some(snapshot) = &mut self.proxy.snapshot {
            snapshot.phase = NetworkPhase::Applying;
            snapshot.phase_started = Instant::now();
        }
        self.proxy.started = Instant::now();
        self.proxy.test_elapsed = None;
        // Retire callbacks associated with the old network generation.
        self.telegram_login_generation = self.telegram_login_generation.wrapping_add(1);
        self.telegram_task = None;
        self.transfers_account_ready = false;
        self.qr_poll_task = None;
        self.avatar_task = None;
        self.cancel_dialog_load(cx);
        self.cancel_telegram_file_load(cx);
        if let Some(sync) = self.channel_sync.take() {
            sync.stop();
        }
        self.channel_sync_task = None;
        self.telegram_activity = TelegramActivity::Working;
        cx.notify();
        let work = cx.background_spawn(async move {
            telegram.apply_network_route(&library, route.clone())?;
            if route.is_proxy() {
                telegram.test_proxy().map(Some)
            } else {
                Ok(None)
            }
        });
        self.proxy.task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                this.proxy.action = if result.is_ok() {
                    ProxyAction::Succeeded
                } else {
                    ProxyAction::Failed
                };
                this.proxy.test_elapsed = result.as_ref().ok().copied().flatten();
                this.telegram_activity = TelegramActivity::Idle;
                this.account_restoring = false;
                if result.is_ok() {
                    this.restore_telegram_session(cx);
                }
                cx.notify();
            });
        }));
    }

    pub(crate) fn test_applied_proxy(&mut self, cx: &mut Context<Self>) {
        if self.proxy.action.busy() {
            return;
        }
        let Some(telegram) = self.telegram.clone() else {
            return;
        };
        if !telegram.network_route().is_proxy() {
            return;
        }
        self.proxy.action = ProxyAction::Testing;
        if let Some(snapshot) = &mut self.proxy.snapshot {
            snapshot.phase = NetworkPhase::TestQueued;
            snapshot.phase_started = Instant::now();
        }
        self.proxy.started = Instant::now();
        self.proxy.test_elapsed = None;
        cx.notify();
        let work = cx.background_spawn(async move { telegram.test_proxy() });
        self.proxy.task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                this.proxy.action = if result.is_ok() {
                    ProxyAction::Succeeded
                } else {
                    ProxyAction::Failed
                };
                this.proxy.test_elapsed = result.ok();
                if this.proxy.test_elapsed.is_some() {
                    this.restore_telegram_session(cx);
                }
                cx.notify();
            });
        }));
    }

    pub(crate) fn cancel_proxy_test(&self) {
        if let Some(telegram) = &self.telegram {
            telegram.cancel_proxy_test();
        }
    }

    pub(crate) fn restrict_external_links(&self) -> bool {
        self.proxy.enabled
            || self.proxy.load_failed
            || self.proxy.action.busy()
            || self
                .telegram
                .as_ref()
                .is_some_and(|t| t.network_snapshot().proxy_enabled)
            || self
                .proxy
                .snapshot
                .as_ref()
                .is_some_and(|s| s.proxy_enabled)
    }

    pub(crate) fn telegram_api_panel_action_id(&self) -> &'static str {
        if self.restrict_external_links() {
            "proxy-copy-api-link"
        } else {
            "settings-telegram-api-panel-action"
        }
    }

    pub(crate) fn open_telegram_api_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        const URL: &str = "https://my.telegram.org/apps";
        if self.restrict_external_links() {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(URL.to_owned()));
            window.push_notification(Notification::success(self.tr("proxy-link-copied")), cx);
        } else {
            cx.open_url(URL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit as gpui;
    use teleark_runtime::ProxyFailure;

    fn snapshot(phase: NetworkPhase) -> NetworkSnapshot {
        NetworkSnapshot {
            revision: 1,
            generation: 1,
            proxy_enabled: true,
            phase,
            phase_started: Instant::now(),
            last_activity: Instant::now(),
            events: Default::default(),
            omitted_events: 0,
        }
    }

    #[gpui::test]
    fn proxy_editor_requires_apply_and_never_opens_external_browser_when_enabled(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Settings);
        cx.simulate_resize(gpui::size(px(1360.0), px(760.0)));
        app.update(cx, |app, cx| {
            app.settings_section = SettingsSection::Network;
            cx.notify();
        });
        cx.run_until_parked();
        let enable = cx
            .debug_bounds("proxy-enable")
            .expect("controlled proxy fixture")
            .center();
        cx.simulate_click(enable, gpui::Modifiers::default());
        app.update(cx, |app, _| {
            assert!(app.proxy.enabled);
            assert_eq!(app.proxy.action, ProxyAction::Idle);
            assert!(app.restrict_external_links());
            assert_eq!(app.telegram_api_panel_action_id(), "proxy-copy-api-link");
        });
        let host = app.update(cx, |app, _| app.proxy.host.clone());
        cx.update(|window, cx| {
            host.update(cx, |host, cx| host.set_value("proxy.example", window, cx))
        });
        app.update(cx, |app, cx| {
            app.apply_proxy(cx);
            assert!(app.proxy.invalid, "hostnames cannot cause a DNS request");
            assert_eq!(app.proxy.action, ProxyAction::Idle);
            assert!(app.proxy.task.is_none());
        });
    }

    #[gpui::test]
    fn proxy_failure_and_waiting_stay_visible_after_navigation_and_busy_cannot_disable(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Settings);
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        app.update(cx, |app, cx| {
            app.settings_section = SettingsSection::Network;
            app.proxy.enabled = true;
            app.proxy.action = ProxyAction::Testing;
            app.proxy.snapshot = Some(snapshot(NetworkPhase::Testing));
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("network-proxy-banner").is_some());
        let disable = cx
            .debug_bounds("proxy-disable")
            .expect("controlled proxy fixture")
            .center();
        cx.simulate_click(disable, gpui::Modifiers::default());
        app.update(cx, |app, cx| {
            assert!(app.proxy.enabled);
            app.proxy.action = ProxyAction::Failed;
            app.proxy.snapshot = Some(snapshot(NetworkPhase::Blocked(ProxyFailure::Timeout)));
            app.page = Page::Channel;
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("network-proxy-banner").is_some());
        app.update(cx, |app, _| {
            // Editing a draft to direct is not permission to launch a browser
            // before the applied proxy is explicitly disabled successfully.
            app.proxy.enabled = false;
            assert!(app.restrict_external_links());
        });
    }

    #[gpui::test]
    fn unreadable_policy_blocks_editor_and_external_links(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Settings);
        cx.simulate_resize(gpui::size(px(1360.0), px(760.0)));
        app.update(cx, |app, cx| {
            app.settings_section = SettingsSection::Network;
            app.proxy.load_failed = true;
            app.apply_proxy(cx);
            assert!(app.proxy.task.is_none());
            assert!(app.restrict_external_links());
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("network-proxy-banner").is_some());
    }
}
