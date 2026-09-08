//! One egress route for every Telegram datacenter, including reconnects and media.
//!
//! The sender pool talks only to an authenticated, bounded loopback gateway.
//! The gateway's immutable policy chooses exactly one outbound endpoint. A proxy
//! failure is terminal for that attempt: there is no direct fallback or DNS lookup.
use std::{
    collections::VecDeque,
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
    sync::watch,
    task::{JoinHandle, JoinSet},
};
use zeroize::Zeroizing;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_TUNNELS: usize = 32;
const HISTORY_LIMIT: usize = 64;
const MAX_HTTP_HEADER: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProxyProtocol {
    Socks5,
    HttpConnect,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ProxyConfig {
    pub protocol: ProxyProtocol,
    pub address: SocketAddr,
    username: String,
    password: Zeroizing<String>,
}

impl fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyConfig")
            .field("protocol", &self.protocol)
            .field("address", &self.address)
            .field("authentication", &!self.username.is_empty())
            .finish_non_exhaustive()
    }
}

impl ProxyConfig {
    pub fn new(
        protocol: ProxyProtocol,
        address: SocketAddr,
        username: String,
        password: String,
    ) -> Result<Self, ProxyFailure> {
        if address.port() == 0
            || address.ip().is_unspecified()
            || address.ip().is_multicast()
            || username.len() > 255
            || password.len() > 255
            || (username.is_empty() && !password.is_empty())
            || (protocol == ProxyProtocol::HttpConnect && username.contains(':'))
        {
            return Err(ProxyFailure::Configuration);
        }
        Ok(Self {
            protocol,
            address,
            username,
            password: Zeroizing::new(password),
        })
    }

    pub fn username(&self) -> &str {
        &self.username
    }
    pub fn password(&self) -> &str {
        &self.password
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum NetworkRoute {
    #[default]
    Direct,
    Proxy(ProxyConfig),
}

impl NetworkRoute {
    pub fn is_proxy(&self) -> bool {
        matches!(self, Self::Proxy(_))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProxyFailure {
    Unreachable,
    Timeout,
    Authentication,
    Rejected,
    Protocol,
    Disconnected,
    Configuration,
    Persistence,
    Capacity,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkPhase {
    Direct,
    ProxyReady,
    Applying,
    Connecting,
    Connected,
    TestQueued,
    Testing,
    TestSucceeded,
    Blocked(ProxyFailure),
}

#[derive(Clone, Debug)]
pub struct NetworkEvent {
    pub phase: NetworkPhase,
    pub at: Instant,
}

#[derive(Clone, Debug)]
pub struct NetworkSnapshot {
    pub revision: u64,
    pub generation: u64,
    pub proxy_enabled: bool,
    pub phase: NetworkPhase,
    pub phase_started: Instant,
    pub last_activity: Instant,
    pub events: VecDeque<NetworkEvent>,
    pub omitted_events: u64,
}

/// Cheap retained projection; changes wake the frontend without a database poll.
#[derive(Clone)]
pub struct NetworkMonitor {
    inner: Arc<Mutex<NetworkSnapshot>>,
    changed: watch::Sender<u64>,
}

impl NetworkMonitor {
    pub fn new(route: &NetworkRoute) -> Self {
        let now = Instant::now();
        let phase = if route.is_proxy() {
            NetworkPhase::ProxyReady
        } else {
            NetworkPhase::Direct
        };
        let (changed, _) = watch::channel(0);
        Self {
            inner: Arc::new(Mutex::new(NetworkSnapshot {
                revision: 0,
                generation: 0,
                proxy_enabled: route.is_proxy(),
                phase,
                phase_started: now,
                last_activity: now,
                events: VecDeque::from([NetworkEvent { phase, at: now }]),
                omitted_events: 0,
            })),
            changed,
        }
    }

    pub fn snapshot(&self) -> NetworkSnapshot {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn subscribe(&self) -> NetworkUpdates {
        NetworkUpdates {
            monitor: self.clone(),
            receiver: self.changed.subscribe(),
        }
    }

    /// Invalidates old callbacks before any potentially slow shutdown/persistence.
    pub fn begin_change(&self, route: &NetworkRoute) -> u64 {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.generation = state.generation.wrapping_add(1);
        state.proxy_enabled = route.is_proxy();
        Self::transition(&mut state, NetworkPhase::Applying);
        self.changed.send_replace(state.revision);
        state.generation
    }

    pub fn publish(&self, generation: u64, phase: NetworkPhase) {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if state.generation != generation {
            return;
        }
        if state.phase == phase {
            return;
        }
        // A healthy concurrent tunnel must not erase a failure notification.
        // Only an explicit test/apply may acknowledge and clear it.
        if matches!(
            state.phase,
            NetworkPhase::Blocked(_) | NetworkPhase::TestQueued | NetworkPhase::Testing
        ) && matches!(
            phase,
            NetworkPhase::Connecting | NetworkPhase::Connected | NetworkPhase::ProxyReady
        ) {
            return;
        }
        Self::transition(&mut state, phase);
        self.changed.send_replace(state.revision);
    }

    fn transition(state: &mut NetworkSnapshot, phase: NetworkPhase) {
        let now = Instant::now();
        state.phase = phase;
        state.phase_started = now;
        state.last_activity = now;
        state.revision = state.revision.wrapping_add(1);
        if state.events.len() == HISTORY_LIMIT {
            state.events.pop_front();
            state.omitted_events = state.omitted_events.saturating_add(1);
        }
        state.events.push_back(NetworkEvent { phase, at: now });
    }

    pub(crate) fn transport_closed(&self, generation: u64) {
        if self.snapshot().proxy_enabled {
            self.publish(
                generation,
                NetworkPhase::Blocked(ProxyFailure::Disconnected),
            );
        }
    }
}

pub struct NetworkUpdates {
    monitor: NetworkMonitor,
    receiver: watch::Receiver<u64>,
}
impl NetworkUpdates {
    pub async fn changed(&mut self) -> Option<NetworkSnapshot> {
        self.receiver.changed().await.ok()?;
        Some(self.monitor.snapshot())
    }
}

/// The only place production code opens an outbound TCP socket.
/// Literal addresses prevent both target-DNS and proxy-DNS leaks.
async fn dial(route: &NetworkRoute, target: SocketAddr) -> Result<TcpStream, ProxyFailure> {
    let operation = async {
        let endpoint = match route {
            NetworkRoute::Direct => target,
            NetworkRoute::Proxy(proxy) => proxy.address,
        };
        let mut stream = TcpStream::connect(endpoint)
            .await
            .map_err(|_| ProxyFailure::Unreachable)?;
        stream
            .set_nodelay(true)
            .map_err(|_| ProxyFailure::Unreachable)?;
        if let NetworkRoute::Proxy(proxy) = route {
            match proxy.protocol {
                ProxyProtocol::Socks5 => socks_connect(&mut stream, proxy, target).await?,
                ProxyProtocol::HttpConnect => http_connect(&mut stream, proxy, target).await?,
            }
        }
        Ok(stream)
    };
    tokio::time::timeout(CONNECT_TIMEOUT, operation)
        .await
        .map_err(|_| ProxyFailure::Timeout)?
}

async fn socks_connect(
    stream: &mut TcpStream,
    proxy: &ProxyConfig,
    target: SocketAddr,
) -> Result<(), ProxyFailure> {
    let authenticated = !proxy.username.is_empty();
    let method = if authenticated { 2 } else { 0 };
    stream
        .write_all(&[5, 1, method])
        .await
        .map_err(|_| ProxyFailure::Disconnected)?;
    let mut response = [0; 2];
    stream
        .read_exact(&mut response)
        .await
        .map_err(|_| ProxyFailure::Disconnected)?;
    if response != [5, method] {
        return Err(ProxyFailure::Authentication);
    }
    if authenticated {
        let mut auth = Zeroizing::new(Vec::with_capacity(513));
        auth.extend_from_slice(&[1, proxy.username.len() as u8]);
        auth.extend_from_slice(proxy.username.as_bytes());
        auth.push(proxy.password.len() as u8);
        auth.extend_from_slice(proxy.password.as_bytes());
        stream
            .write_all(&auth)
            .await
            .map_err(|_| ProxyFailure::Disconnected)?;
        stream
            .read_exact(&mut response)
            .await
            .map_err(|_| ProxyFailure::Disconnected)?;
        if response != [1, 0] {
            return Err(ProxyFailure::Authentication);
        }
    }
    let mut request = vec![5, 1, 0];
    append_address(&mut request, target);
    stream
        .write_all(&request)
        .await
        .map_err(|_| ProxyFailure::Disconnected)?;
    let mut header = [0; 4];
    stream
        .read_exact(&mut header)
        .await
        .map_err(|_| ProxyFailure::Disconnected)?;
    if header[0] != 5 || header[2] != 0 {
        return Err(ProxyFailure::Protocol);
    }
    if header[1] != 0 {
        return Err(ProxyFailure::Rejected);
    }
    consume_bound_address(stream, header[3]).await
}

fn append_address(bytes: &mut Vec<u8>, target: SocketAddr) {
    match target.ip() {
        IpAddr::V4(ip) => {
            bytes.push(1);
            bytes.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            bytes.push(4);
            bytes.extend_from_slice(&ip.octets());
        }
    }
    bytes.extend_from_slice(&target.port().to_be_bytes());
}

async fn consume_bound_address(stream: &mut TcpStream, kind: u8) -> Result<(), ProxyFailure> {
    let length = match kind {
        1 => 4,
        4 => 16,
        3 => usize::from(
            stream
                .read_u8()
                .await
                .map_err(|_| ProxyFailure::Disconnected)?,
        ),
        _ => return Err(ProxyFailure::Protocol),
    };
    let mut bytes = [0; 257];
    stream
        .read_exact(&mut bytes[..length + 2])
        .await
        .map_err(|_| ProxyFailure::Disconnected)?;
    Ok(())
}

async fn http_connect(
    stream: &mut TcpStream,
    proxy: &ProxyConfig,
    target: SocketAddr,
) -> Result<(), ProxyFailure> {
    let mut header = Zeroizing::new(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n"));
    if !proxy.username.is_empty() {
        let pair = Zeroizing::new(format!("{}:{}", proxy.username, proxy.password.as_str()));
        let encoded = Zeroizing::new(STANDARD.encode(pair.as_bytes()));
        header.push_str("Proxy-Authorization: Basic ");
        header.push_str(&encoded);
        header.push_str("\r\n");
    }
    header.push_str("\r\n");
    stream
        .write_all(header.as_bytes())
        .await
        .map_err(|_| ProxyFailure::Disconnected)?;
    let mut response = Vec::with_capacity(256);
    while !response.ends_with(b"\r\n\r\n") {
        if response.len() == MAX_HTTP_HEADER {
            return Err(ProxyFailure::Protocol);
        }
        response.push(
            stream
                .read_u8()
                .await
                .map_err(|_| ProxyFailure::Disconnected)?,
        );
    }
    let first = response
        .split(|byte| *byte == b'\n')
        .next()
        .ok_or(ProxyFailure::Protocol)?;
    let line = std::str::from_utf8(first).map_err(|_| ProxyFailure::Protocol)?;
    let mut fields = line.split_ascii_whitespace();
    if !matches!(fields.next(), Some("HTTP/1.0" | "HTTP/1.1")) {
        return Err(ProxyFailure::Protocol);
    }
    match fields.next().and_then(|status| status.parse::<u16>().ok()) {
        Some(200..=299) => Ok(()),
        Some(407) => Err(ProxyFailure::Authentication),
        Some(_) => Err(ProxyFailure::Rejected),
        None => Err(ProxyFailure::Protocol),
    }
}

pub(crate) struct Gateway {
    pub(crate) proxy_url: String,
    pub(crate) task: JoinHandle<()>,
}

impl Gateway {
    pub(crate) async fn start(
        route: NetworkRoute,
        monitor: NetworkMonitor,
        generation: u64,
    ) -> Result<Self, ProxyFailure> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| ProxyFailure::Unreachable)?;
        let address = listener
            .local_addr()
            .map_err(|_| ProxyFailure::Unreachable)?;
        let mut random = [0; 32];
        getrandom::fill(&mut random).map_err(|_| ProxyFailure::Configuration)?;
        let token: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let proxy_url = format!("socks5://teleark:{token}@{address}");
        let task = tokio::spawn(async move {
            let token = Arc::new(Zeroizing::new(token));
            let route = Arc::new(route);
            let mut tunnels = JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    _ = tunnels.join_next(), if !tunnels.is_empty() => {},
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        if tunnels.len() >= MAX_TUNNELS {
                            if route.is_proxy() { monitor.publish(generation, NetworkPhase::Blocked(ProxyFailure::Capacity)); }
                            drop(stream);
                            continue;
                        }
                        let (route, monitor, token) = (Arc::clone(&route), monitor.clone(), Arc::clone(&token));
                        tunnels.spawn(async move { serve(stream, &route, &token, &monitor, generation).await; });
                    }
                }
            }
            tunnels.shutdown().await;
        });
        Ok(Self { proxy_url, task })
    }
}

async fn read_gateway_target(
    stream: &mut TcpStream,
    token: &str,
) -> Result<SocketAddr, ProxyFailure> {
    let mut header = [0; 2];
    stream
        .read_exact(&mut header)
        .await
        .map_err(|_| ProxyFailure::Disconnected)?;
    if header[0] != 5 || header[1] == 0 {
        return Err(ProxyFailure::Protocol);
    }
    let mut methods = [0; 255];
    stream
        .read_exact(&mut methods[..usize::from(header[1])])
        .await
        .map_err(|_| ProxyFailure::Disconnected)?;
    if !methods[..usize::from(header[1])].contains(&2) {
        return Err(ProxyFailure::Authentication);
    }
    stream
        .write_all(&[5, 2])
        .await
        .map_err(|_| ProxyFailure::Disconnected)?;
    stream
        .read_exact(&mut header)
        .await
        .map_err(|_| ProxyFailure::Disconnected)?;
    if header[0] != 1 {
        return Err(ProxyFailure::Authentication);
    }
    let mut username = [0; 255];
    let user_len = usize::from(header[1]);
    stream
        .read_exact(&mut username[..user_len])
        .await
        .map_err(|_| ProxyFailure::Disconnected)?;
    let pass_len = usize::from(
        stream
            .read_u8()
            .await
            .map_err(|_| ProxyFailure::Disconnected)?,
    );
    let mut password = Zeroizing::new([0; 255]);
    stream
        .read_exact(&mut password[..pass_len])
        .await
        .map_err(|_| ProxyFailure::Disconnected)?;
    if &username[..user_len] != b"teleark" || &password[..pass_len] != token.as_bytes() {
        return Err(ProxyFailure::Authentication);
    }
    stream
        .write_all(&[1, 0])
        .await
        .map_err(|_| ProxyFailure::Disconnected)?;
    let mut request = [0; 4];
    stream
        .read_exact(&mut request)
        .await
        .map_err(|_| ProxyFailure::Disconnected)?;
    if request[..3] != [5, 1, 0] {
        return Err(ProxyFailure::Protocol);
    }
    let ip = match request[3] {
        1 => {
            let mut bytes = [0; 4];
            stream
                .read_exact(&mut bytes)
                .await
                .map_err(|_| ProxyFailure::Disconnected)?;
            IpAddr::V4(Ipv4Addr::from(bytes))
        }
        4 => {
            let mut bytes = [0; 16];
            stream
                .read_exact(&mut bytes)
                .await
                .map_err(|_| ProxyFailure::Disconnected)?;
            IpAddr::V6(Ipv6Addr::from(bytes))
        }
        // Never resolve a hostname, even when a client requests one.
        _ => return Err(ProxyFailure::Protocol),
    };
    let port = stream
        .read_u16()
        .await
        .map_err(|_| ProxyFailure::Disconnected)?;
    if port == 0 || ip.is_unspecified() || ip.is_multicast() {
        return Err(ProxyFailure::Protocol);
    }
    Ok(SocketAddr::new(ip, port))
}

async fn serve(
    mut local: TcpStream,
    route: &NetworkRoute,
    token: &str,
    monitor: &NetworkMonitor,
    generation: u64,
) {
    let target =
        match tokio::time::timeout(CONNECT_TIMEOUT, read_gateway_target(&mut local, token)).await {
            Ok(Ok(target)) => target,
            _ => return, // An unauthenticated local caller cannot alter app network status.
        };
    if route.is_proxy() {
        monitor.publish(generation, NetworkPhase::Connecting);
    }
    let mut remote = match dial(route, target).await {
        Ok(stream) => stream,
        Err(reason) => {
            if route.is_proxy() {
                monitor.publish(generation, NetworkPhase::Blocked(reason));
            }
            let _ = local.write_all(&[5, 1, 0, 1, 0, 0, 0, 0, 0, 0]).await;
            return;
        }
    };
    if local
        .write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])
        .await
        .is_err()
    {
        return;
    }
    if route.is_proxy() {
        monitor.publish(generation, NetworkPhase::Connected);
    }
    // Bounded per-tunnel copy buffers; a remote close is visible even while idle.
    let (mut local_read, mut local_write) = local.split();
    let (mut remote_read, mut remote_write) = remote.split();
    let remote_closed = tokio::select! {
        _ = tokio::io::copy(&mut local_read, &mut remote_write) => false,
        _ = tokio::io::copy(&mut remote_read, &mut local_write) => true,
    };
    if remote_closed && route.is_proxy() {
        monitor.transport_closed(generation);
    }
}

/// Tests a real tunnel to Telegram via the already-applied policy. It does not
/// claim sign-in success or send user data. A failed test never changes policy.
pub async fn test_proxy(
    route: &NetworkRoute,
    monitor: &NetworkMonitor,
    generation: u64,
    cancellation: &crate::ScanCancellation,
) -> Result<Duration, ProxyFailure> {
    if !route.is_proxy() {
        return Err(ProxyFailure::Configuration);
    }
    use grammers_session::{Session as _, storages::MemorySession};
    let session = MemorySession::default();
    let dc = session
        .home_dc_id()
        .map_err(|_| ProxyFailure::Configuration)?;
    let target = session
        .dc_option(dc)
        .map_err(|_| ProxyFailure::Configuration)?
        .ok_or(ProxyFailure::Configuration)?
        .ipv4
        .into();
    monitor.publish(generation, NetworkPhase::Testing);
    let start = Instant::now();
    let result = tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(ProxyFailure::Cancelled),
        result = dial(route, target) => result,
    };
    match result {
        Ok(stream) => {
            drop(stream);
            monitor.publish(generation, NetworkPhase::TestSucceeded);
            Ok(start.elapsed())
        }
        Err(reason) => {
            monitor.publish(generation, NetworkPhase::Blocked(reason));
            Err(reason)
        }
    }
}

#[cfg(test)]
mod tests;
