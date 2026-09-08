//! Architectural guard plus an actual blocked HTTP request. Transport routing
//! and failure tests use real loopback sockets in teleark-telegram/network.
use gpui_kit::http_client::{AsyncBody, Request};
use std::{
    io::ErrorKind,
    path::Path,
    task::{Context, Poll, Waker},
};

#[test]
fn framework_http_client_cannot_open_an_independent_socket() {
    let destination = std::net::TcpListener::bind("127.0.0.1:0").expect("controlled proxy fixture");
    destination
        .set_nonblocking(true)
        .expect("controlled proxy fixture");
    let request = Request::get(format!(
        "http://{}/avatar",
        destination.local_addr().expect("controlled proxy fixture")
    ))
    .body(AsyncBody::default())
    .expect("controlled proxy fixture");
    let mut response = crate::application_http_client().send(request);
    assert!(matches!(
        response
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Err(_))
    ));
    assert_eq!(
        destination
            .accept()
            .expect_err("operation must fail closed")
            .kind(),
        ErrorKind::WouldBlock
    );
}

fn visit(path: &Path, sources: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(path).expect("controlled proxy fixture") {
        let entry = entry.expect("controlled proxy fixture");
        let path = entry.path();
        if path.is_dir() {
            visit(&path, sources);
        } else if path.extension().is_some_and(|e| e == "rs")
            && !path
                .file_name()
                .expect("controlled proxy fixture")
                .to_string_lossy()
                .ends_with("tests.rs")
        {
            let text = std::fs::read_to_string(&path).expect("controlled proxy fixture");
            let production = text
                .split("\n#[cfg(test)]\nmod ")
                .next()
                .expect("controlled proxy fixture")
                .to_owned();
            sources.push((path.to_string_lossy().replace('\\', "/"), production));
        }
    }
}

#[test]
fn network_constructors_and_external_links_have_one_enforced_owner() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("controlled proxy fixture");
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(root).expect("controlled proxy fixture") {
        let src = entry.expect("controlled proxy fixture").path().join("src");
        if src.is_dir() {
            visit(&src, &mut sources);
        }
    }
    let mut outbound = 0;
    let mut pools = 0;
    let mut external_links = 0;
    for (path, source) in &sources {
        for needle in [
            "TcpStream::connect(",
            "TcpSocket::",
            "UdpSocket::",
            "lookup_host(",
            "lookup_ip(",
            "to_socket_addrs(",
        ] {
            let count = source.matches(needle).count();
            if path.ends_with("teleark-telegram/src/network.rs") && needle == "TcpStream::connect("
            {
                outbound += count;
            } else {
                assert_eq!(count, 0, "unreviewed network exit {needle} in {path}");
            }
        }
        assert!(
            !source.contains("SenderPool::new("),
            "default sender pool bypass in {path}"
        );
        let count = source.matches("SenderPool::with_configuration(").count();
        if count > 0 {
            assert!(path.ends_with("teleark-telegram/src/lib.rs"));
            assert!(source.contains("proxy_url: Some(gateway_url)"));
            pools += count;
        }
        let count = source.matches(".open_url(").count();
        if count > 0 {
            assert!(
                path.ends_with("teleark-gui/src/app/proxy.rs"),
                "external browser bypass in {path}"
            );
            assert!(source.contains("if self.restrict_external_links()"));
            external_links += count;
        }
        assert!(
            !source.contains(".set_http_client("),
            "framework HTTP override in {path}"
        );
    }
    assert_eq!((outbound, pools, external_links), (1, 1, 1));
    let main = &sources
        .iter()
        .find(|(path, _)| path.ends_with("teleark-gui/src/main.rs"))
        .expect("controlled proxy fixture")
        .1;
    assert!(main.contains(".with_http_client(application_http_client())"));
    let startup = &sources
        .iter()
        .find(|(path, _)| path.ends_with("teleark-gui/src/startup.rs"))
        .expect("controlled proxy fixture")
        .1;
    assert!(startup.contains("DesktopTelegram::open_default_configured"));
    assert!(!startup.contains("DesktopTelegram::open_default()"));
}
