//! Replacing a route retires its entire reactor before a new one can connect.
//! Joining and persistence are caller-owned background work; no lock spans either.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use teleark_telegram::network::{
    NetworkMonitor, NetworkPhase, NetworkRoute, NetworkSnapshot, NetworkUpdates, ProxyFailure,
};

pub(super) struct Endpoint {
    pub(super) sender: tokio::sync::mpsc::Sender<TelegramRequest>,
    pub(super) generation: u64,
    pub(super) stop: ScanCancellation,
    pub(super) join: Option<JoinHandle<()>>,
}

impl Endpoint {
    fn spawn(
        route: NetworkRoute,
        monitor: NetworkMonitor,
        generation: u64,
        bandwidth: teleark_telegram::TransferBandwidth,
        lifecycle: lifecycle::Lifecycle,
    ) -> Result<Self, ApplicationError> {
        Self::spawn_with(route, monitor, generation, move |receiver, mut state| {
            state.bandwidth = bandwidth;
            state.lifecycle = lifecycle;
            telegram_loop(receiver, state)
        })
    }

    fn spawn_with<F, Fut>(
        route: NetworkRoute,
        monitor: NetworkMonitor,
        generation: u64,
        run: F,
    ) -> Result<Self, ApplicationError>
    where
        F: FnOnce(tokio::sync::mpsc::Receiver<TelegramRequest>, WorkerState) -> Fut
            + Send
            + 'static,
        Fut: std::future::Future<Output = ()>,
    {
        let (sender, receiver) = tokio::sync::mpsc::channel(TELEGRAM_QUEUE_CAPACITY);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let stop = ScanCancellation::default();
        let cancellation = stop.clone();
        let join = thread::Builder::new()
            .name("teleark-telegram".to_owned())
            .spawn(move || {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => {
                        let _ = ready_sender.send(Ok(()));
                        runtime.block_on(async move {
                            let state = WorkerState {
                                network_route: route,
                                network_monitor: monitor,
                                network_generation: generation,
                                ..WorkerState::default()
                            };
                            tokio::select! {
                                biased;
                                _ = cancellation.cancelled() => {},
                                _ = run(receiver, state) => {},
                            }
                        });
                        // Dropping this dedicated runtime closes every sender and gateway
                        // socket, including tasks retained by stale read/transfer snapshots.
                        drop(runtime);
                    }
                    Err(_) => {
                        let _ = ready_sender
                            .send(Err(ApplicationError::new(ApplicationErrorKind::Network)));
                    }
                }
            })
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Network))?;
        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                sender,
                generation,
                stop,
                join: Some(join),
            }),
            _ => {
                let _ = join.join();
                Err(ApplicationError::new(ApplicationErrorKind::Network))
            }
        }
    }

    fn retire(mut self) -> Result<(), ApplicationError> {
        self.stop.cancel();
        if let Some(join) = self.join.take() {
            join.join()
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Network))?;
        }
        Ok(())
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        let _ = self.sender.try_send(TelegramRequest::Shutdown);
        self.stop.cancel();
        if let Some(join) = self.join.take()
            && join.is_finished()
        {
            let _ = join.join();
        }
    }
}

struct Changing<'a>(&'a AtomicBool);
impl Drop for Changing<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl DesktopTelegram {
    /// For isolated callers with an explicitly direct policy. The application
    /// must use `open_configured` so saved policy is loaded before any network.
    pub fn open_direct(session_path: impl AsRef<Path>) -> Result<Self, ApplicationError> {
        Self::open_with_route(
            session_path,
            NetworkRoute::Direct,
            teleark_telegram::TransferBandwidth::default(),
        )
    }

    pub fn open_configured(
        session_path: impl AsRef<Path>,
        library: &DesktopLibrary,
    ) -> Result<Self, ApplicationError> {
        // Preserve library access for repair, but never start traffic with unreadable limits.
        library.preferences()?;
        let route = library.proxy_configuration()?;
        Self::open_with_route(session_path, route, library.bandwidth.clone())
    }

    fn open_with_route(
        session_path: impl AsRef<Path>,
        route: NetworkRoute,
        bandwidth: teleark_telegram::TransferBandwidth,
    ) -> Result<Self, ApplicationError> {
        let session_path = session_path.as_ref().to_owned();
        if session_path.as_os_str().is_empty() {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        }
        let monitor = NetworkMonitor::new(&route);
        let lifecycle = lifecycle::Lifecycle::default();
        let endpoint = Endpoint::spawn(
            route.clone(),
            monitor.clone(),
            0,
            bandwidth.clone(),
            lifecycle.clone(),
        )?;
        Ok(Self {
            #[cfg(test)]
            test_vault_remote: None,
            inner: Arc::new(TelegramWorkerInner {
                lifecycle,
                bandwidth,
                endpoint: Mutex::new(Some(endpoint)),
                changing: AtomicBool::new(false),
                route: Mutex::new(route),
                monitor,
                active_probe: Mutex::new(None),
                session_path,
            }),
        })
    }

    pub fn open_default_configured(library: &DesktopLibrary) -> Result<Self, ApplicationError> {
        let path = default_telegram_session_path()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        Self::open_configured(path, library)
    }

    pub fn network_snapshot(&self) -> NetworkSnapshot {
        self.inner.monitor.snapshot()
    }
    pub fn network_updates(&self) -> NetworkUpdates {
        self.inner.monitor.subscribe()
    }
    pub fn network_route(&self) -> NetworkRoute {
        self.inner
            .route
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Background-only synchronous boundary. As soon as it starts, admission
    /// closes and old callbacks are invalidated. Failures leave admission closed;
    /// only a successful explicit apply can create another owner.
    pub fn apply_network_route(
        &self,
        library: &DesktopLibrary,
        route: NetworkRoute,
    ) -> Result<(), ApplicationError> {
        if self
            .inner
            .changing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        let _changing = Changing(&self.inner.changing);
        let generation = self.inner.monitor.begin_change(&route);
        self.inner.lifecycle.publish(generation, None, None);
        *self.inner.route.lock().unwrap_or_else(|e| e.into_inner()) = route.clone();
        let old = self
            .inner
            .endpoint
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(old) = old
            && let Err(error) = old.retire()
        {
            self.inner.monitor.publish(
                generation,
                NetworkPhase::Blocked(ProxyFailure::Disconnected),
            );
            return Err(error);
        }
        if let Err(error) = library.set_proxy_configuration(&route) {
            self.inner
                .monitor
                .publish(generation, NetworkPhase::Blocked(ProxyFailure::Persistence));
            return Err(error);
        }
        let endpoint = match Endpoint::spawn(
            route.clone(),
            self.inner.monitor.clone(),
            generation,
            self.inner.bandwidth.clone(),
            self.inner.lifecycle.clone(),
        ) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                self.inner
                    .monitor
                    .publish(generation, NetworkPhase::Blocked(ProxyFailure::Unreachable));
                return Err(error);
            }
        };
        *self
            .inner
            .endpoint
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(endpoint);
        self.inner.monitor.publish(
            generation,
            if route.is_proxy() {
                NetworkPhase::ProxyReady
            } else {
                NetworkPhase::Direct
            },
        );
        Ok(())
    }

    pub fn test_proxy(&self) -> Result<Duration, ApplicationError> {
        if !self.network_route().is_proxy() {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        }
        let generation = self.network_snapshot().generation;
        let cancellation = ScanCancellation::default();
        {
            let mut active = self
                .inner
                .active_probe
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if active.as_ref().is_some_and(|(g, _)| *g == generation) {
                return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
            }
            if let Some((_, old)) = active.take() {
                old.cancel();
            }
            *active = Some((generation, cancellation.clone()));
        }
        self.inner
            .monitor
            .publish(generation, NetworkPhase::TestQueued);
        let result = self.request("test_proxy", |reply| TelegramRequest::TestProxy {
            cancellation,
            reply,
        });
        if result.is_err()
            && matches!(
                self.network_snapshot().phase,
                NetworkPhase::TestQueued | NetworkPhase::Testing
            )
        {
            // A dispatcher barrier can cancel a probe before its own future
            // publishes a terminal phase. Never leave a phantom testing state.
            self.inner
                .monitor
                .publish(generation, NetworkPhase::Blocked(ProxyFailure::Cancelled));
        }
        let mut active = self
            .inner
            .active_probe
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if active.as_ref().is_some_and(|(g, _)| *g == generation) {
            *active = None;
        }
        result
    }

    /// Nonblocking; cancels only the probe, keeping the applied proxy policy.
    pub fn cancel_proxy_test(&self) {
        let cancellation = self
            .inner
            .active_probe
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|(_, token)| token.clone());
        if let Some(cancellation) = cancellation {
            cancellation.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{ErrorKind, Read as _};
    use teleark_telegram::network::{ProxyConfig, ProxyProtocol};

    fn proxy_route() -> NetworkRoute {
        // Deliberately closed local port: a proxy outage cannot restore direct.
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("controlled proxy fixture");
        NetworkRoute::Proxy(
            ProxyConfig::new(
                ProxyProtocol::Socks5,
                listener.local_addr().expect("controlled proxy fixture"),
                String::new(),
                String::new(),
            )
            .expect("controlled proxy fixture"),
        )
    }

    #[test]
    fn switch_retires_active_sockets_before_saving_and_publishing_new_owner() {
        let directory = tempfile::tempdir().expect("controlled proxy fixture");
        let library = DesktopLibrary::open(directory.path().join("library"))
            .expect("controlled proxy fixture");
        let telegram = DesktopTelegram::open_configured(directory.path().join("session"), &library)
            .expect("controlled proxy fixture");
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("controlled proxy fixture");
        let target = listener.local_addr().expect("controlled proxy fixture");
        let (entered, started) = mpsc::sync_channel(1);
        let old = Endpoint::spawn_with(
            NetworkRoute::Direct,
            telegram.inner.monitor.clone(),
            0,
            move |_, _| async move {
                let _socket = tokio::net::TcpStream::connect(target)
                    .await
                    .expect("controlled proxy fixture");
                entered.send(()).expect("controlled proxy fixture");
                // A retained child socket must also be dropped, not just the root.
                let child = tokio::spawn(async move {
                    let _socket = tokio::net::TcpStream::connect(target)
                        .await
                        .expect("controlled proxy fixture");
                    std::future::pending::<()>().await;
                });
                let _ = child.await;
            },
        )
        .expect("controlled proxy fixture");
        let original = telegram
            .inner
            .endpoint
            .lock()
            .expect("controlled proxy fixture")
            .replace(old)
            .expect("controlled proxy fixture");
        original.retire().expect("controlled proxy fixture");
        started.recv().expect("controlled proxy fixture");
        let (mut first, _) = listener.accept().expect("controlled proxy fixture");
        let (mut second, _) = listener.accept().expect("controlled proxy fixture");
        let route = proxy_route();
        telegram
            .apply_network_route(&library, route.clone())
            .expect("controlled proxy fixture");
        for stream in [&mut first, &mut second] {
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .expect("controlled proxy fixture");
            assert_eq!(
                stream.read(&mut [0]).expect("controlled proxy fixture"),
                0,
                "old direct socket must be closed before apply returns"
            );
        }
        assert_eq!(
            library
                .proxy_configuration()
                .expect("controlled proxy fixture"),
            route
        );
        assert_eq!(telegram.network_route(), route);
        assert!(telegram.test_proxy().is_err());
        assert!(telegram.network_snapshot().proxy_enabled);
        assert!(matches!(
            telegram.network_snapshot().phase,
            NetworkPhase::Blocked(_)
        ));
        // Policy survives process/owner restart even after the proxy test fails.
        drop(telegram);
        let reopened = DesktopTelegram::open_configured(directory.path().join("session"), &library)
            .expect("controlled proxy fixture");
        assert_eq!(reopened.network_route(), route);
        listener
            .set_nonblocking(true)
            .expect("controlled proxy fixture");
        assert_eq!(
            listener
                .accept()
                .expect_err("operation must fail closed")
                .kind(),
            ErrorKind::WouldBlock
        );
    }

    #[test]
    fn storage_failure_keeps_network_blocked_and_does_not_recreate_direct_owner() {
        let directory = tempfile::tempdir().expect("controlled proxy fixture");
        let library = DesktopLibrary::open(directory.path().join("library"))
            .expect("controlled proxy fixture");
        let telegram = DesktopTelegram::open_configured(directory.path().join("session"), &library)
            .expect("controlled proxy fixture");
        library
            .worker
            .inner
            .sender
            .send(crate::StorageRequest::Shutdown)
            .expect("controlled proxy fixture");
        // Drain synchronization through the same FIFO: this fails only after
        // the shutdown has closed the storage worker's receiver.
        assert!(library.proxy_configuration().is_err());
        let route = proxy_route();
        assert!(
            telegram
                .apply_network_route(&library, route.clone())
                .is_err()
        );
        assert!(
            telegram
                .inner
                .endpoint
                .lock()
                .expect("controlled proxy fixture")
                .is_none()
        );
        assert_eq!(telegram.network_route(), route);
        assert_eq!(
            telegram.network_snapshot().phase,
            NetworkPhase::Blocked(ProxyFailure::Persistence)
        );
        assert!(telegram.connect(12345).is_err());
    }

    #[test]
    fn only_explicit_successful_disable_reenables_direct_policy() {
        let directory = tempfile::tempdir().expect("controlled proxy fixture");
        let library = DesktopLibrary::open(directory.path().join("library"))
            .expect("controlled proxy fixture");
        let telegram = DesktopTelegram::open_configured(directory.path().join("session"), &library)
            .expect("controlled proxy fixture");
        telegram
            .apply_network_route(&library, proxy_route())
            .expect("controlled proxy fixture");
        let old_generation = telegram.network_snapshot().generation;
        assert!(telegram.test_proxy().is_err());
        assert!(
            library
                .proxy_configuration()
                .expect("controlled proxy fixture")
                .is_proxy()
        );
        telegram
            .apply_network_route(&library, NetworkRoute::Direct)
            .expect("controlled proxy fixture");
        telegram.inner.monitor.publish(
            old_generation,
            NetworkPhase::Blocked(ProxyFailure::Unreachable),
        );
        assert_eq!(telegram.network_snapshot().phase, NetworkPhase::Direct);
        assert_eq!(
            library
                .proxy_configuration()
                .expect("controlled proxy fixture"),
            NetworkRoute::Direct
        );
        let reopened =
            DesktopTelegram::open_configured(directory.path().join("other-session"), &library)
                .expect("controlled proxy fixture");
        assert_eq!(reopened.network_route(), NetworkRoute::Direct);
    }
    #[test]
    fn speed_limits_upgrade_missing_keys_persist_and_update_live_owners() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("library.sqlite3");
        let library = DesktopLibrary::open(&path).expect("legacy empty preferences");
        assert_eq!(
            library.preferences().expect("defaults").speed_limits,
            crate::TransferSpeedLimits::default()
        );
        let telegram = DesktopTelegram::open_configured(directory.path().join("session"), &library)
            .expect("owner");
        let mut preferences = library.preferences().expect("preferences");
        preferences.speed_limits = crate::TransferSpeedLimits {
            upload: 1024,
            download: 4096,
        };
        library.set_preferences(&preferences).expect("save limits");
        assert_eq!(
            telegram.inner.bandwidth.upload.snapshot().bytes_per_second,
            1024
        );
        assert_eq!(
            telegram
                .inner
                .bandwidth
                .download
                .snapshot()
                .bytes_per_second,
            4096
        );
        let reopened = DesktopLibrary::open(&path).expect("restart");
        assert_eq!(reopened.bandwidth_snapshot().0.bytes_per_second, 1024);
        assert_eq!(
            reopened.preferences().expect("restored").speed_limits,
            preferences.speed_limits
        );
        telegram
            .apply_network_route(&library, proxy_route())
            .expect("replace network owner");
        assert_eq!(
            telegram.inner.bandwidth.upload.snapshot().bytes_per_second,
            1024
        );
        preferences.speed_limits = crate::TransferSpeedLimits::default();
        library
            .set_preferences(&preferences)
            .expect("disable limits");
        assert_eq!(
            telegram.inner.bandwidth.upload.snapshot().bytes_per_second,
            0
        );
        assert_eq!(
            telegram
                .inner
                .bandwidth
                .download
                .snapshot()
                .bytes_per_second,
            0
        );
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use std::io::Read as _;
    use teleark_telegram::network::{ProxyConfig, ProxyProtocol};

    #[test]
    fn cancelling_a_stalled_probe_closes_it_without_disabling_the_proxy() {
        let directory = tempfile::tempdir().expect("controlled proxy fixture");
        let library = DesktopLibrary::open(directory.path().join("library"))
            .expect("controlled proxy fixture");
        let telegram = DesktopTelegram::open_configured(directory.path().join("session"), &library)
            .expect("controlled proxy fixture");
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("controlled proxy fixture");
        let route = NetworkRoute::Proxy(
            ProxyConfig::new(
                ProxyProtocol::Socks5,
                listener.local_addr().expect("controlled proxy fixture"),
                String::new(),
                String::new(),
            )
            .expect("controlled proxy fixture"),
        );
        telegram
            .apply_network_route(&library, route.clone())
            .expect("controlled proxy fixture");
        let caller = telegram.clone();
        let probe = thread::spawn(move || caller.test_proxy());
        let (mut stream, _) = listener.accept().expect("controlled proxy fixture");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("controlled proxy fixture");
        let mut hello = [0; 3];
        stream
            .read_exact(&mut hello)
            .expect("controlled proxy fixture");
        assert_eq!(hello, [5, 1, 0]);
        telegram.cancel_proxy_test();
        assert_eq!(
            probe
                .join()
                .expect("controlled proxy fixture")
                .expect_err("operation must fail closed")
                .kind(),
            ApplicationErrorKind::Cancelled
        );
        assert_eq!(stream.read(&mut [0]).expect("controlled proxy fixture"), 0);
        assert_eq!(
            library
                .proxy_configuration()
                .expect("controlled proxy fixture"),
            route
        );
        assert_eq!(
            telegram.network_snapshot().phase,
            NetworkPhase::Blocked(ProxyFailure::Cancelled)
        );
    }
}
