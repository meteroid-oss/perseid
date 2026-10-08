//! The default client goes through the proxies of the environment, which is process-wide: one
//! test sets it, and this file is its own test binary.
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
};

use hyper_util::client::legacy::connect::HttpConnector;
use petstore::{api::Petstore, error::Error};

const PET: &str = r#"{"id":"1","name":"Rex","created_at":"2024-01-01T00:00:00Z"}"#;
const VARIABLES: [&str; 8] = [
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
];
/// `Basic` and the base64 of `ada:s@cret`, lowercased like the recorded heads.
const AUTHORIZATION: &str = "\r\nproxy-authorization: basic ywrhonnay3jlda==\r\n";

/// What a server received on each connection: the request head, then what followed it.
type Seen = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

/// Serves on a local port, answering each request with a pet. A CONNECT is acknowledged, then
/// the first bytes of the tunnel are recorded.
fn serve() -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let seen = Seen::default();
    let recorded = seen.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let head = read_head(&mut stream);
            if head.starts_with("connect ") {
                let established = b"HTTP/1.1 200 Connection Established\r\n\r\n";
                stream.write_all(established).unwrap();
                let mut tunneled = [0; 512];
                let read = stream.read(&mut tunneled).unwrap_or(0);
                recorded
                    .lock()
                    .unwrap()
                    .push((head, tunneled[..read].to_vec()));
            } else {
                recorded.lock().unwrap().push((head, Vec::new()));
                let answer = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{PET}",
                    PET.len()
                );
                stream.write_all(answer.as_bytes()).unwrap();
            }
        }
    });
    (address, seen)
}

fn read_head(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut byte = [0];
    while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
        head.push(byte[0]);
    }
    String::from_utf8_lossy(&head).to_lowercase()
}

fn client(base_url: &str) -> Petstore {
    Petstore::builder()
        .base_url(base_url)
        .max_retries(0)
        .build()
        .unwrap()
}

#[tokio::test]
async fn the_default_client_honours_the_proxy_variables() {
    for name in VARIABLES {
        std::env::remove_var(name);
    }
    let (proxy, proxied) = serve();
    let (origin, served) = serve();

    // Plain HTTP goes to the proxy with the absolute URL and the proxy's credentials.
    std::env::set_var("http_proxy", format!("http://ada:s%40cret@{proxy}"));
    client("http://pets.example.com")
        .pets()
        .retrieve("1")
        .await
        .unwrap();
    let (head, _) = proxied.lock().unwrap().remove(0);
    assert!(
        head.starts_with("get http://pets.example.com/pets/1 http/1.1\r\n"),
        "{head}"
    );
    assert!(head.contains("\r\nhost: pets.example.com\r\n"), "{head}");
    assert!(head.contains(AUTHORIZATION), "{head}");

    // HTTPS is tunnelled: TLS starts once the proxy accepted the CONNECT.
    std::env::set_var("HTTPS_PROXY", format!("http://ada:s%40cret@{proxy}"));
    let error = client("https://pets.example.com")
        .pets()
        .retrieve("1")
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Connection(_)), "{error:?}");
    let (head, tunneled) = proxied.lock().unwrap().remove(0);
    assert!(
        head.starts_with("connect pets.example.com:443 http/1.1\r\n"),
        "{head}"
    );
    assert!(head.contains(AUTHORIZATION), "{head}");
    assert_eq!(tunneled.first(), Some(&0x16), "a TLS handshake record");
    assert!(
        tunneled.windows(16).any(|w| w == b"pets.example.com"),
        "the SNI"
    );

    // NO_PROXY hosts, and clients over a connector of their own, connect directly.
    std::env::set_var("NO_PROXY", "localhost, 127.0.0.0/8");
    client(&format!("http://{origin}"))
        .pets()
        .retrieve("1")
        .await
        .unwrap();
    std::env::remove_var("NO_PROXY");
    let direct = Petstore::builder()
        .base_url(format!("http://{origin}"))
        .connector(HttpConnector::new())
        .build()
        .unwrap();
    direct.pets().retrieve("1").await.unwrap();
    assert_eq!(served.lock().unwrap().len(), 2);
    assert!(proxied.lock().unwrap().is_empty());

    // ALL_PROXY covers the schemes without a variable of their own.
    std::env::remove_var("http_proxy");
    std::env::set_var("ALL_PROXY", &proxy);
    client("http://pets.example.com")
        .pets()
        .retrieve("1")
        .await
        .unwrap();
    let (head, _) = proxied.lock().unwrap().remove(0);
    assert!(
        head.starts_with("get http://pets.example.com/pets/1 "),
        "{head}"
    );
    assert!(!head.contains("proxy-authorization"), "{head}");
}
