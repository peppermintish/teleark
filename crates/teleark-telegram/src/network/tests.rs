use super::*;

#[test]
fn monitor_rejects_stale_generations_and_bounds_history() {
    let route = NetworkRoute::Direct;
    let monitor = NetworkMonitor::new(&route);
    let generation = monitor.begin_change(&route);
    monitor.publish(0, NetworkPhase::Connected);
    assert_eq!(monitor.snapshot().phase, NetworkPhase::Applying);
    for _ in 0..100 {
        monitor.publish(generation, NetworkPhase::Testing);
        monitor.publish(generation, NetworkPhase::Blocked(ProxyFailure::Timeout));
    }
    assert_eq!(monitor.snapshot().events.len(), HISTORY_LIMIT);
    assert!(monitor.snapshot().omitted_events > 0);
}

use std::io::ErrorKind;
use tokio::sync::oneshot;

/// A live, reachable destination. Any forbidden direct dial remains in its
/// accept queue even if the buggy client immediately closes the connection.
fn trap(ip: IpAddr) -> std::net::TcpListener {
    let listener =
        std::net::TcpListener::bind(SocketAddr::new(ip, 0)).expect("controlled proxy fixture");
    listener
        .set_nonblocking(true)
        .expect("controlled proxy fixture");
    listener
}
fn untouched(listener: &std::net::TcpListener) {
    match listener.accept() {
        Err(error) if error.kind() == ErrorKind::WouldBlock => {}
        other => panic!("forbidden direct connection reached the target: {other:?}"),
    }
}

#[derive(Clone, Copy, Debug)]
enum Behavior {
    Echo,
    RejectAuth,
    RejectTunnel,
    Malformed,
    Disconnect,
}

async fn socks_request(stream: &mut TcpStream, target: SocketAddr, auth: bool, behavior: Behavior) {
    let mut hello = [0; 3];
    stream
        .read_exact(&mut hello)
        .await
        .expect("controlled proxy fixture");
    assert_eq!(hello, [5, 1, if auth { 2 } else { 0 }]);
    if matches!(behavior, Behavior::RejectAuth) {
        stream
            .write_all(&[5, 255])
            .await
            .expect("controlled proxy fixture");
        return;
    }
    stream
        .write_all(&[5, hello[2]])
        .await
        .expect("controlled proxy fixture");
    if auth {
        assert_eq!(stream.read_u8().await.expect("controlled proxy fixture"), 1);
        let count = usize::from(stream.read_u8().await.expect("controlled proxy fixture"));
        let mut name = vec![0; count];
        stream
            .read_exact(&mut name)
            .await
            .expect("controlled proxy fixture");
        assert_eq!(name, b"test@user");
        let count = usize::from(stream.read_u8().await.expect("controlled proxy fixture"));
        let mut password = vec![0; count];
        stream
            .read_exact(&mut password)
            .await
            .expect("controlled proxy fixture");
        assert_eq!(password, b"synthetic:/@password");
        stream
            .write_all(&[1, 0])
            .await
            .expect("controlled proxy fixture");
    }
    let mut header = [0; 4];
    stream
        .read_exact(&mut header)
        .await
        .expect("controlled proxy fixture");
    assert_eq!(&header[..3], &[5, 1, 0]);
    let ip = match header[3] {
        1 => {
            let mut bytes = [0; 4];
            stream
                .read_exact(&mut bytes)
                .await
                .expect("controlled proxy fixture");
            IpAddr::V4(bytes.into())
        }
        4 => {
            let mut bytes = [0; 16];
            stream
                .read_exact(&mut bytes)
                .await
                .expect("controlled proxy fixture");
            IpAddr::V6(bytes.into())
        }
        _ => panic!("destination must be an IP, never a DNS name"),
    };
    let port = stream.read_u16().await.expect("controlled proxy fixture");
    assert_eq!(SocketAddr::new(ip, port), target);
    let response = match behavior {
        Behavior::RejectTunnel => [5, 5, 0, 1, 0, 0, 0, 0, 0, 0],
        Behavior::Malformed => [4, 0, 0, 1, 0, 0, 0, 0, 0, 0],
        _ => [5, 0, 0, 1, 0, 0, 0, 0, 0, 0],
    };
    if !matches!(behavior, Behavior::Disconnect) {
        stream
            .write_all(&response)
            .await
            .expect("controlled proxy fixture");
    }
}

async fn http_request(stream: &mut TcpStream, target: SocketAddr, auth: bool, behavior: Behavior) {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        assert!(bytes.len() < 2048);
        bytes.push(stream.read_u8().await.expect("controlled proxy fixture"));
    }
    let header = String::from_utf8(bytes).expect("controlled proxy fixture");
    assert!(header.starts_with(&format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n")));
    if auth {
        assert!(header.contains(&format!(
            "Proxy-Authorization: Basic {}\r\n",
            STANDARD.encode(b"test@user:synthetic:/@password")
        )));
    } else {
        assert!(!header.contains("Proxy-Authorization"));
    }
    let response: &[u8] = match behavior {
        Behavior::Echo => b"HTTP/1.1 200 Connection established\r\n\r\n",
        Behavior::RejectAuth => b"HTTP/1.1 407 Authentication required\r\n\r\n",
        // A redirect must never cause a second direct HTTP request.
        Behavior::RejectTunnel => b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1/\r\n\r\n",
        Behavior::Malformed => b"NOTHTTP 200 Invalid\r\n\r\n",
        Behavior::Disconnect => return,
    };
    stream
        .write_all(response)
        .await
        .expect("controlled proxy fixture");
}

async fn fake_proxy(
    protocol: ProxyProtocol,
    target: SocketAddr,
    auth: bool,
    behavior: Behavior,
    count: usize,
    ipv6: bool,
) -> (NetworkRoute, JoinHandle<()>) {
    let ip: IpAddr = if ipv6 {
        Ipv6Addr::LOCALHOST.into()
    } else {
        Ipv4Addr::LOCALHOST.into()
    };
    let listener = TcpListener::bind(SocketAddr::new(ip, 0))
        .await
        .expect("controlled proxy fixture");
    let config = ProxyConfig::new(
        protocol,
        listener.local_addr().expect("controlled proxy fixture"),
        if auth {
            "test@user".into()
        } else {
            String::new()
        },
        if auth {
            "synthetic:/@password".into()
        } else {
            String::new()
        },
    )
    .expect("controlled proxy fixture");
    let task = tokio::spawn(async move {
        for _ in 0..count {
            let (mut stream, _) = listener.accept().await.expect("controlled proxy fixture");
            match protocol {
                ProxyProtocol::Socks5 => socks_request(&mut stream, target, auth, behavior).await,
                ProxyProtocol::HttpConnect => {
                    http_request(&mut stream, target, auth, behavior).await
                }
            }
            if matches!(behavior, Behavior::Echo) {
                let mut payload = [0; 10];
                stream
                    .read_exact(&mut payload)
                    .await
                    .expect("controlled proxy fixture");
                assert_eq!(&payload, b"proxy-only");
                stream
                    .write_all(&payload)
                    .await
                    .expect("controlled proxy fixture");
            }
        }
    });
    (NetworkRoute::Proxy(config), task)
}

#[tokio::test]
async fn both_protocols_route_real_payloads_and_raw_credentials_only_to_the_proxy() {
    for protocol in [ProxyProtocol::Socks5, ProxyProtocol::HttpConnect] {
        for auth in [false, true] {
            for ipv6 in [false, true] {
                let destination = trap(if ipv6 {
                    Ipv6Addr::LOCALHOST.into()
                } else {
                    Ipv4Addr::LOCALHOST.into()
                });
                let target = destination.local_addr().expect("controlled proxy fixture");
                let (route, proxy) =
                    fake_proxy(protocol, target, auth, Behavior::Echo, 2, ipv6).await;
                // Repeated connections must obey the same route, not only the first.
                for _ in 0..2 {
                    let mut stream = dial(&route, target)
                        .await
                        .expect("controlled proxy fixture");
                    stream
                        .write_all(b"proxy-only")
                        .await
                        .expect("controlled proxy fixture");
                    let mut echo = [0; 10];
                    stream
                        .read_exact(&mut echo)
                        .await
                        .expect("controlled proxy fixture");
                    assert_eq!(&echo, b"proxy-only");
                }
                proxy.await.expect("controlled proxy fixture");
                untouched(&destination);
            }
        }
    }
}

#[tokio::test]
async fn authentication_refusal_redirect_malformed_and_drop_never_try_direct_even_on_retry() {
    for protocol in [ProxyProtocol::Socks5, ProxyProtocol::HttpConnect] {
        for behavior in [
            Behavior::RejectAuth,
            Behavior::RejectTunnel,
            Behavior::Malformed,
            Behavior::Disconnect,
        ] {
            let destination = trap(Ipv4Addr::LOCALHOST.into());
            let target = destination.local_addr().expect("controlled proxy fixture");
            let (route, proxy) = fake_proxy(protocol, target, true, behavior, 3, false).await;
            for _ in 0..3 {
                let result = dial(&route, target).await;
                let expected = match behavior {
                    Behavior::RejectAuth => ProxyFailure::Authentication,
                    Behavior::RejectTunnel => ProxyFailure::Rejected,
                    Behavior::Malformed => ProxyFailure::Protocol,
                    Behavior::Disconnect => ProxyFailure::Disconnected,
                    Behavior::Echo => unreachable!(),
                };
                assert_eq!(result.expect_err("operation must fail closed"), expected);
            }
            proxy.await.expect("controlled proxy fixture");
            untouched(&destination);
        }
    }
}

#[tokio::test]
async fn refused_proxy_port_never_falls_back_to_reachable_destination() {
    let destination = trap(Ipv4Addr::LOCALHOST.into());
    let proxy = trap(Ipv4Addr::LOCALHOST.into());
    let address = proxy.local_addr().expect("controlled proxy fixture");
    drop(proxy);
    for protocol in [ProxyProtocol::Socks5, ProxyProtocol::HttpConnect] {
        let route = NetworkRoute::Proxy(
            ProxyConfig::new(protocol, address, String::new(), String::new())
                .expect("controlled proxy fixture"),
        );
        for _ in 0..3 {
            assert_eq!(
                dial(
                    &route,
                    destination.local_addr().expect("controlled proxy fixture")
                )
                .await
                .expect_err("operation must fail closed"),
                ProxyFailure::Unreachable
            );
        }
    }
    untouched(&destination);
}

#[tokio::test]
async fn stalled_proxy_has_bounded_timeout_without_direct_fallback() {
    for protocol in [ProxyProtocol::Socks5, ProxyProtocol::HttpConnect] {
        let destination = trap(Ipv4Addr::LOCALHOST.into());
        let target = destination.local_addr().expect("controlled proxy fixture");
        let proxy = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("controlled proxy fixture");
        let route = NetworkRoute::Proxy(
            ProxyConfig::new(
                protocol,
                proxy.local_addr().expect("controlled proxy fixture"),
                String::new(),
                String::new(),
            )
            .expect("controlled proxy fixture"),
        );
        let call = tokio::spawn(async move { dial(&route, target).await });
        let (mut accepted, _) = proxy.accept().await.expect("controlled proxy fixture");
        let mut byte = [0];
        accepted
            .read_exact(&mut byte)
            .await
            .expect("controlled proxy fixture");
        // Controlled time starts after the handshake has reached the proxy.
        tokio::time::pause();
        tokio::time::advance(CONNECT_TIMEOUT + Duration::from_secs(1)).await;
        assert_eq!(
            call.await
                .expect("controlled proxy fixture")
                .expect_err("operation must fail closed"),
            ProxyFailure::Timeout
        );
        tokio::time::resume();
        untouched(&destination);
    }
}

#[tokio::test]
async fn gateway_enforces_upstream_route_and_emits_a_persistent_failure() {
    let destination = trap(Ipv4Addr::LOCALHOST.into());
    let target = destination.local_addr().expect("controlled proxy fixture");
    let (route, upstream) = fake_proxy(
        ProxyProtocol::Socks5,
        target,
        true,
        Behavior::RejectTunnel,
        1,
        false,
    )
    .await;
    let monitor = NetworkMonitor::new(&route);
    let gateway = Gateway::start(route, monitor.clone(), 0)
        .await
        .expect("controlled proxy fixture");
    let (auth, address) = gateway
        .proxy_url
        .strip_prefix("socks5://")
        .expect("controlled proxy fixture")
        .split_once('@')
        .expect("controlled proxy fixture");
    let (username, password) = auth.split_once(':').expect("controlled proxy fixture");
    let local = NetworkRoute::Proxy(
        ProxyConfig::new(
            ProxyProtocol::Socks5,
            address.parse().expect("controlled proxy fixture"),
            username.into(),
            password.into(),
        )
        .expect("controlled proxy fixture"),
    );
    assert_eq!(
        dial(&local, target)
            .await
            .expect_err("operation must fail closed"),
        ProxyFailure::Rejected
    );
    upstream.await.expect("controlled proxy fixture");
    assert_eq!(
        monitor.snapshot().phase,
        NetworkPhase::Blocked(ProxyFailure::Rejected)
    );
    monitor.publish(0, NetworkPhase::Connected);
    assert_eq!(
        monitor.snapshot().phase,
        NetworkPhase::Blocked(ProxyFailure::Rejected)
    );
    gateway.task.abort();
    let _ = gateway.task.await;
    untouched(&destination);
}

#[tokio::test]
async fn unauthenticated_gateway_client_cannot_open_any_upstream_socket() {
    let upstream = trap(Ipv4Addr::LOCALHOST.into());
    let route = NetworkRoute::Proxy(
        ProxyConfig::new(
            ProxyProtocol::Socks5,
            upstream.local_addr().expect("controlled proxy fixture"),
            String::new(),
            String::new(),
        )
        .expect("controlled proxy fixture"),
    );
    let monitor = NetworkMonitor::new(&route);
    let gateway = Gateway::start(route, monitor.clone(), 0)
        .await
        .expect("controlled proxy fixture");
    let address: SocketAddr = gateway
        .proxy_url
        .rsplit_once('@')
        .expect("controlled proxy fixture")
        .1
        .parse()
        .expect("controlled proxy fixture");
    let mut client = TcpStream::connect(address)
        .await
        .expect("controlled proxy fixture");
    client
        .write_all(&[5, 1, 0])
        .await
        .expect("controlled proxy fixture");
    let mut bytes = [0];
    assert_eq!(
        client
            .read(&mut bytes)
            .await
            .expect("controlled proxy fixture"),
        0
    );
    assert_eq!(monitor.snapshot().phase, NetworkPhase::ProxyReady);
    gateway.task.abort();
    let _ = gateway.task.await;
    untouched(&upstream);
}

#[tokio::test]
async fn production_pool_routes_all_datacenters_and_reconnections_through_gateway() {
    use grammers_session::{Session as _, storages::MemorySession};
    let destination = trap(Ipv4Addr::LOCALHOST.into());
    let target = destination.local_addr().expect("controlled proxy fixture");
    let (route, proxy) = fake_proxy(
        ProxyProtocol::HttpConnect,
        target,
        false,
        Behavior::RejectTunnel,
        10,
        false,
    )
    .await;
    let monitor = NetworkMonitor::new(&route);
    let gateway = Gateway::start(route, monitor.clone(), 0)
        .await
        .expect("controlled proxy fixture");
    let session = Arc::new(MemorySession::default());
    for dc in 1..=5 {
        let mut option = session
            .dc_option(dc)
            .expect("controlled proxy fixture")
            .expect("controlled proxy fixture");
        option.ipv4 = match target {
            SocketAddr::V4(address) => address,
            _ => unreachable!(),
        };
        session
            .set_dc_option(&option)
            .await
            .expect("controlled proxy fixture");
    }
    let pool = crate::sender_pool(session, 12345, gateway.proxy_url);
    let task = tokio::spawn(pool.runner.run());
    for _ in 0..2 {
        for dc in 1..=5 {
            // The real pool must attempt auth/bootstrap over the same gateway
            // before it can service any API, media or sync RPC in this DC.
            assert!(pool.handle.invoke_in_dc(dc, vec![0; 4]).await.is_err());
            pool.handle.disconnect_from_dc(dc);
        }
    }
    pool.handle.quit();
    task.await.expect("controlled proxy fixture");
    proxy.await.expect("controlled proxy fixture");
    gateway.task.abort();
    let _ = gateway.task.await;
    untouched(&destination);
}

#[tokio::test]
async fn real_plaintext_and_authenticated_mtproto_bootstrap_bytes_reach_only_proxy() {
    use grammers_session::{Session as _, storages::MemorySession};
    for authenticated in [false, true] {
        let destination = trap(Ipv4Addr::LOCALHOST.into());
        let target = destination.local_addr().expect("controlled proxy fixture");
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("controlled proxy fixture");
        let route = NetworkRoute::Proxy(
            ProxyConfig::new(
                ProxyProtocol::Socks5,
                listener.local_addr().expect("controlled proxy fixture"),
                String::new(),
                String::new(),
            )
            .expect("controlled proxy fixture"),
        );
        let (observed, bytes_seen) = oneshot::channel();
        let proxy = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("controlled proxy fixture");
            socks_request(&mut stream, target, false, Behavior::Echo).await;
            // Inspect the actual length-prefixed Full transport packet emitted
            // by grammers, not a test-only configuration string.
            let size = stream
                .read_u32_le()
                .await
                .expect("controlled proxy fixture") as usize;
            assert!((12..=4096).contains(&size));
            let mut packet = vec![0; size - 4];
            stream
                .read_exact(&mut packet)
                .await
                .expect("controlled proxy fixture");
            observed.send(packet).expect("controlled proxy fixture");
            // Drop after receiving a complete packet: the RPC must fail, not
            // retry on a direct socket after the proxy connection disappears.
        });
        let monitor = NetworkMonitor::new(&route);
        let gateway = Gateway::start(route, monitor.clone(), 0)
            .await
            .expect("controlled proxy fixture");
        let session = Arc::new(MemorySession::default());
        let dc = session.home_dc_id().expect("controlled proxy fixture");
        let mut option = session
            .dc_option(dc)
            .expect("controlled proxy fixture")
            .expect("controlled proxy fixture");
        option.ipv4 = match target {
            SocketAddr::V4(address) => address,
            _ => unreachable!(),
        };
        option.auth_key = authenticated.then_some([42; 256]);
        session
            .set_dc_option(&option)
            .await
            .expect("controlled proxy fixture");
        let pool = crate::sender_pool(session, 12345, gateway.proxy_url);
        let task = tokio::spawn(pool.runner.run());
        assert!(pool.handle.invoke_in_dc(dc, vec![0; 4]).await.is_err());
        let packet = bytes_seen.await.expect("controlled proxy fixture");
        // seqno occupies four bytes; the next eight are MTProto auth_key_id.
        assert_eq!(packet[4..12] == [0; 8], !authenticated);
        proxy.await.expect("controlled proxy fixture");
        pool.handle.quit();
        task.await.expect("controlled proxy fixture");
        gateway.task.abort();
        let _ = gateway.task.await;
        untouched(&destination);
    }
}

#[tokio::test]
async fn remote_proxy_close_is_visible_without_waiting_for_another_rpc() {
    let destination = trap(Ipv4Addr::LOCALHOST.into());
    let target = destination.local_addr().expect("controlled proxy fixture");
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("controlled proxy fixture");
    let route = NetworkRoute::Proxy(
        ProxyConfig::new(
            ProxyProtocol::HttpConnect,
            listener.local_addr().expect("controlled proxy fixture"),
            String::new(),
            String::new(),
        )
        .expect("controlled proxy fixture"),
    );
    let (close, closed) = oneshot::channel();
    let proxy = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("controlled proxy fixture");
        http_request(&mut stream, target, false, Behavior::Echo).await;
        closed.await.expect("controlled proxy fixture");
    });
    let monitor = NetworkMonitor::new(&route);
    let gateway = Gateway::start(route, monitor.clone(), 0)
        .await
        .expect("controlled proxy fixture");
    let (auth, address) = gateway
        .proxy_url
        .strip_prefix("socks5://")
        .expect("controlled proxy fixture")
        .split_once('@')
        .expect("controlled proxy fixture");
    let (user, password) = auth.split_once(':').expect("controlled proxy fixture");
    let local = NetworkRoute::Proxy(
        ProxyConfig::new(
            ProxyProtocol::Socks5,
            address.parse().expect("controlled proxy fixture"),
            user.into(),
            password.into(),
        )
        .expect("controlled proxy fixture"),
    );
    let _stream = dial(&local, target)
        .await
        .expect("controlled proxy fixture");
    let mut updates = monitor.subscribe();
    close.send(()).expect("controlled proxy fixture");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if matches!(
                monitor.snapshot().phase,
                NetworkPhase::Blocked(ProxyFailure::Disconnected)
            ) {
                break;
            }
            updates.changed().await.expect("controlled proxy fixture");
        }
    })
    .await
    .expect("controlled proxy fixture");
    proxy.await.expect("controlled proxy fixture");
    gateway.task.abort();
    let _ = gateway.task.await;
    untouched(&destination);
}

#[tokio::test]
async fn oversized_http_response_fails_closed_with_bounded_memory() {
    let destination = trap(Ipv4Addr::LOCALHOST.into());
    let target = destination.local_addr().expect("controlled proxy fixture");
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("controlled proxy fixture");
    let route = NetworkRoute::Proxy(
        ProxyConfig::new(
            ProxyProtocol::HttpConnect,
            listener.local_addr().expect("controlled proxy fixture"),
            String::new(),
            String::new(),
        )
        .expect("controlled proxy fixture"),
    );
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("controlled proxy fixture");
        let mut first = [0];
        stream
            .read_exact(&mut first)
            .await
            .expect("controlled proxy fixture");
        let _ = stream.write_all(&vec![b'A'; MAX_HTTP_HEADER + 1]).await;
        let _ = stream.read_to_end(&mut Vec::new()).await;
    });
    assert_eq!(
        dial(&route, target)
            .await
            .expect_err("operation must fail closed"),
        ProxyFailure::Protocol
    );
    task.await.expect("controlled proxy fixture");
    untouched(&destination);
}

#[test]
fn environment_proxy_bypass_variables_cannot_override_explicit_routing() {
    let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", "network::tests::both_protocols_route_real_payloads_and_raw_credentials_only_to_the_proxy", "--nocapture"])
        .env("NO_PROXY", "*").env("no_proxy", "*")
        .env("ALL_PROXY", "http://127.0.0.1:1").env("all_proxy", "http://127.0.0.1:1")
        .env("HTTP_PROXY", "http://127.0.0.1:1").env("HTTPS_PROXY", "http://127.0.0.1:1")
        .output().expect("run isolated environment routing test");
    assert!(
        output.status.success(),
        "routing must ignore environment bypass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}
