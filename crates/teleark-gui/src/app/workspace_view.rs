//! Retain the workspace independently of presentation-only inspector ticks.
use super::*;
use gpui_kit::WeakEntity;

pub(super) struct WorkspaceView {
    owner: WeakEntity<TeleArkApp>,
    _changes: Subscription,
    #[cfg(test)]
    pub renders: usize,
}
impl WorkspaceView {
    pub fn new(owner: Entity<TeleArkApp>, cx: &mut Context<Self>) -> Self {
        let changes = cx.observe(&owner, |_, _, cx| cx.notify());
        Self {
            owner: owner.downgrade(),
            _changes: changes,
            #[cfg(test)]
            renders: 0,
        }
    }
}
impl Render for WorkspaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.renders += 1;
        }
        self.owner
            .update(cx, |app, cx| {
                app.render_workspace_frame(window, cx).into_any_element()
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
}

/// Keep the inspector's dispatch/render ancestry separate from the workspace.
pub(super) struct SyncInspector {
    owner: WeakEntity<TeleArkApp>,
    clock: Option<Task<()>>,
}
impl SyncInspector {
    pub fn new(owner: WeakEntity<TeleArkApp>) -> Self {
        Self { owner, clock: None }
    }

    fn needs_clock(&self, cx: &gpui_kit::App) -> bool {
        self.owner.upgrade().is_some_and(|owner| {
            let app = owner.read(cx);
            app.channel_sync_details
                && app.channel_sync_snapshot.as_ref().is_some_and(|s| {
                    !s.active.is_empty()
                        || s.managed_scan.as_ref().is_some_and(|s| s.active())
                        || matches!(
                            s.phase,
                            teleark_runtime::ChannelSyncPhase::Waiting
                                | teleark_runtime::ChannelSyncPhase::RateLimited
                                | teleark_runtime::ChannelSyncPhase::Pacing
                        )
                })
        })
    }
}
impl Render for SyncInspector {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.clock.is_none() && self.needs_clock(cx) {
            self.clock = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let Ok(keep) = this.update(cx, |this, cx| {
                        if this.needs_clock(cx) {
                            cx.notify();
                            true
                        } else {
                            this.clock = None;
                            false
                        }
                    }) else {
                        return;
                    };
                    if !keep {
                        return;
                    }
                }
            }));
        }
        self.owner
            .update(cx, |app, cx| app.render_channel_sync_details(window, cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}

/// Content projections have their own cache boundary. Header input widgets may
/// invalidate themselves after paint; that must not rebuild files or sources.
pub(super) struct ContentView {
    owner: WeakEntity<TeleArkApp>,
    layout: LayoutPolicy,
    channels: bool,
    _changes: Subscription,
    #[cfg(test)]
    pub renders: usize,
}
impl ContentView {
    pub fn new(
        owner: Entity<TeleArkApp>,
        layout: LayoutPolicy,
        channels: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let changes = cx.observe(&owner, |_, _, cx| cx.notify());
        Self {
            owner: owner.downgrade(),
            layout,
            channels,
            _changes: changes,
            #[cfg(test)]
            renders: 0,
        }
    }
    pub fn set_layout(&mut self, layout: LayoutPolicy, cx: &mut Context<Self>) {
        if self.layout != layout {
            self.layout = layout;
            cx.notify();
        }
    }
}
impl Render for ContentView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.renders += 1;
        }
        let content = self
            .owner
            .update(cx, |app, cx| {
                if self.channels {
                    app.render_channels_sidebar(cx)
                } else {
                    app.render_page(window, self.layout, cx)
                }
            })
            .unwrap_or_else(|_| div().into_any_element());
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(content)
    }
}
