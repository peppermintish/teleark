//! Paint an acknowledgment before opening or upgrading persistent state.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use gpui_kit::StatefulInteractiveElement as _;
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    Styled, Task, Window, div, px,
};
use teleark_core::{ApplicationError, ApplicationErrorKind};
use teleark_i18n::{
    Localizer, MessageArgs, MessageId, SupportedLocale, format::format_duration_millis,
};
use teleark_runtime::{
    DesktopLibrary, DesktopTelegram, DesktopTransfers, DesktopVault, MigrationProgress,
    initialize_diagnostics,
};

use crate::{
    LaunchOptions,
    app::{AppStartup, LocaleStartup, RuntimeConfiguration, RuntimeStartup, TeleArkApp},
    apply_persisted_locale, components, menus, theme,
};

struct Progress {
    events: VecDeque<(MigrationProgress, Instant)>,
    dropped: u64,
}
impl Progress {
    fn new() -> Self {
        Self {
            events: VecDeque::from([(MigrationProgress::Detecting, Instant::now())]),
            dropped: 0,
        }
    }
    fn publish(&mut self, phase: MigrationProgress) {
        if self.events.back().is_some_and(|(old, _)| *old == phase) {
            return;
        }
        if self.events.len() == 64 {
            self.events.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.events.push_back((phase, Instant::now()));
    }
}

pub(crate) struct StartupView {
    localizer: Localizer,
    launch: LaunchOptions,
    system_locale: SupportedLocale,
    visual_preview: bool,
    scheduled: bool,
    progress: Arc<Mutex<Progress>>,
    failure: Option<ApplicationErrorKind>,
    ready: Option<(RuntimeStartup, Option<String>)>,
    view: Option<Entity<TeleArkApp>>,
    task: Option<Task<()>>,
    clock: Option<Task<()>>,
}

impl StartupView {
    pub(crate) fn new(
        localizer: Localizer,
        launch: LaunchOptions,
        system_locale: SupportedLocale,
        visual_preview: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        theme::apply_appearance(teleark_runtime::AppearancePreference::System, window, cx);
        Self {
            localizer,
            launch,
            system_locale,
            visual_preview,
            scheduled: false,
            progress: Arc::new(Mutex::new(Progress::new())),
            failure: None,
            ready: None,
            view: None,
            task: None,
            clock: None,
        }
    }
    fn start(&mut self, cx: &mut Context<Self>) {
        self.failure = None;
        self.progress = Arc::new(Mutex::new(Progress::new()));
        let progress = Arc::clone(&self.progress);
        let preview = self.visual_preview;
        let work = cx.background_spawn(async move { prepare(preview, progress) });
        self.task = Some(cx.spawn(async move |this, cx| {
            let (runtime, locale) = work.await;
            let Some(entity) = this.upgrade() else { return };
            entity.update(cx, |this, cx| {
                this.clock = None;
                if !this.visual_preview
                    && let Err(error) = &runtime.library
                {
                    this.failure = Some(error.kind());
                } else {
                    this.ready = Some((runtime, locale));
                }
                cx.notify();
            });
        }));
        self.clock = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let Some(entity) = this.upgrade() else { break };
                entity.update(cx, |_, cx| cx.notify());
            }
        }));
        cx.notify();
    }
    fn tr(&self, id: &'static str) -> String {
        self.localizer.translate_or_id(MessageId::new(id))
    }
    fn phase_label(&self, phase: MigrationProgress) -> String {
        let (id, version) = match phase {
            MigrationProgress::Detecting => ("startup-detecting", 0),
            MigrationProgress::Preparing { to, .. } => ("startup-preparing", to),
            MigrationProgress::Converting { version } => ("startup-converting", version),
            MigrationProgress::Verifying { version } => ("startup-verifying", version),
            MigrationProgress::Completed => ("startup-completed", 0),
        };
        self.localizer
            .translate_with(
                MessageId::new(id),
                &MessageArgs::new().with("version", version.to_string()),
            )
            .unwrap_or_else(|_| self.tr(id))
    }
}

impl Render for StartupView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.scheduled {
            self.scheduled = true;
            let entity = cx.entity();
            window.on_next_frame(move |_, cx| entity.update(cx, |this, cx| this.start(cx)));
            window.request_animation_frame();
        }
        if let Some((runtime, locale)) = self.ready.take() {
            let follows = apply_persisted_locale(&mut self.launch, locale.as_deref());
            if let Ok(localizer) = Localizer::new(self.launch.locale) {
                cx.set_menus(menus::application_menus(&localizer));
                let startup = AppStartup {
                    page: self.launch.page,
                    visual_preview: self.visual_preview,
                    show_upload: self.launch.show_upload,
                    locale: LocaleStartup {
                        system_locale: self.system_locale,
                        follows_system_locale: follows,
                    },
                };
                self.view =
                    Some(cx.new(|cx| TeleArkApp::new(window, cx, localizer, runtime, startup)));
            } else {
                self.failure = Some(ApplicationErrorKind::InvalidRequest);
            }
        }
        if let Some(view) = &self.view {
            return div().size_full().child(view.clone());
        }
        let mut body = div().w(px(560.0)).flex().flex_col().gap_3().child(
            div()
                .id("startup-acknowledgment")
                .debug_selector(|| "startup-acknowledgment".into())
                .text_xl()
                .text_color(theme::text_primary())
                .child(self.tr("startup-title")),
        );
        if let Ok(progress) = self.progress.lock() {
            if let Some((phase, at)) = progress.events.back() {
                let duration = format_duration_millis(
                    self.localizer.locale(),
                    u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX),
                );
                let timing = self
                    .localizer
                    .translate_with(
                        MessageId::new("channel-sync-timing"),
                        &MessageArgs::new()
                            .with("duration", duration.clone())
                            .with("activity", duration),
                    )
                    .unwrap_or_default();
                body = body
                    .child(self.phase_label(*phase))
                    .child(div().text_xs().child(timing));
            }
            // A retained bounded timeline includes the exact conversion and
            // validation phases even when they finish between rendered frames.
            let mut timeline = div()
                .id("startup-timeline")
                .debug_selector(|| "startup-timeline".into())
                .max_h(px(260.0))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_1()
                .text_xs();
            for (phase, _) in &progress.events {
                timeline = timeline.child(self.phase_label(*phase));
            }
            body = body.child(timeline);
            if progress.dropped > 0 {
                body = body.child(self.tr("startup-truncated"));
            }
        }
        if self.failure.is_some() {
            body = body.child(self.tr("startup-failed")).child(
                components::button(
                    "startup-retry",
                    self.tr("telegram-files-retry-action"),
                    None,
                    true,
                )
                .debug_selector(|| "startup-retry".into())
                .on_click(cx.listener(|this, _, _, cx| this.start(cx))),
            );
        }
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme::canvas())
            .text_color(theme::text_secondary())
            .child(body)
    }
}

fn prepare(preview: bool, progress: Arc<Mutex<Progress>>) -> (RuntimeStartup, Option<String>) {
    let library = if preview {
        Err(ApplicationError::new(ApplicationErrorKind::NotFound))
    } else {
        DesktopLibrary::open_default_with_progress(move |phase| {
            if let Ok(mut progress) = progress.lock() {
                progress.publish(phase);
            }
        })
    };
    let _diagnostics = library
        .as_ref()
        .map_err(|error| ApplicationError::new(error.kind()))
        .and_then(|library| library.managed_directories())
        .and_then(|directories| initialize_diagnostics(&directories.logs));
    tracing::info!(
        event = "application.starting",
        version = env!("CARGO_PKG_VERSION"),
        "TeleArk application starting"
    );
    let telegram = if preview {
        Err(ApplicationError::new(ApplicationErrorKind::NotFound))
    } else {
        library
            .as_ref()
            .map_err(|error| ApplicationError::new(error.kind()))
            .and_then(DesktopTelegram::open_default_configured)
    };
    let transfers = match (telegram.as_ref(), library.as_ref()) {
        (Ok(telegram), Ok(library)) => DesktopTransfers::new(telegram.clone(), library.clone()),
        (Err(error), _) | (_, Err(error)) => Err(ApplicationError::new(error.kind())),
    };
    let vault = match (telegram.as_ref(), library.as_ref()) {
        (Ok(telegram), Ok(library)) => DesktopVault::new(telegram.clone(), library.clone()),
        (Err(error), _) | (_, Err(error)) => Err(ApplicationError::new(error.kind())),
    };
    let locale = library
        .as_ref()
        .ok()
        .and_then(|library| library.locale_override().ok())
        .flatten();
    (
        RuntimeStartup {
            configuration: Some(RuntimeConfiguration::read(&library, &telegram)),
            library,
            telegram,
            transfers,
            vault,
        },
        locale,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit as gpui;

    #[gpui::test]
    fn startup_paints_before_work_and_retains_readable_failure_phases(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = StartupView::new(
                Localizer::new(SupportedLocale::EnUs).expect("catalog"),
                LaunchOptions::from_args(["--locale=en-US"], SupportedLocale::EnUs),
                SupportedLocale::EnUs,
                true,
                window,
                cx,
            );
            assert!(
                view.task.is_none(),
                "constructing the view cannot open a database"
            );
            // Hold an injected blocked phase without touching real runtime state.
            view.scheduled = true;
            view.progress
                .lock()
                .expect("progress")
                .publish(MigrationProgress::Verifying { version: 11 });
            view.failure = Some(ApplicationErrorKind::Persistence);
            view
        });
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("startup-acknowledgment").is_some());
        let retry = cx
            .debug_bounds("startup-retry")
            .expect("retry remains reachable");
        assert!(retry.bottom() <= px(600.0));
        assert!(cx.debug_bounds("startup-timeline").is_some());
        view.update(cx, |view, _| {
            assert!(view.task.is_none());
            assert_eq!(view.progress.lock().expect("progress").events.len(), 2);
        });
    }

    #[test]
    fn startup_timeline_bounds_preserve_the_final_phase() {
        let mut progress = Progress::new();
        for version in 1..=100 {
            progress.publish(MigrationProgress::Converting { version });
        }
        progress.publish(MigrationProgress::Completed);
        assert_eq!(progress.events.len(), 64);
        assert_eq!(progress.dropped, 38);
        assert_eq!(
            progress.events.back().expect("last").0,
            MigrationProgress::Completed
        );
    }

    #[gpui::test]
    fn preview_startup_finishes_without_opening_persistent_runtime(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            StartupView::new(
                Localizer::new(SupportedLocale::EnUs).expect("catalog"),
                LaunchOptions::from_args(["--screen=channel"], SupportedLocale::EnUs),
                SupportedLocale::EnUs,
                true,
                window,
                cx,
            )
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("startup-acknowledgment").is_some());
        cx.update(|window, cx| {
            assert!(window.simulate_next_frame(cx) > 0);
        });
        cx.run_until_parked();
        view.update(cx, |view, _| {
            assert!(view.failure.is_none());
            assert!(
                view.view.is_some(),
                "the frame callback and background completion must mount the app"
            );
            assert!(
                view.clock.is_none(),
                "startup polling must stop after completion"
            );
        });
        let (runtime, _) = prepare(true, Arc::new(Mutex::new(Progress::new())));
        assert!(
            runtime.library.is_err()
                && runtime.telegram.is_err()
                && runtime.transfers.is_err()
                && runtime.vault.is_err()
        );
    }
}
