//! Runs against the SDK generated from tests/fixtures/torture.yaml (see tests/sdk/run.sh): binary
//! responses, read from a real connection as they are consumed.
use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};

use futures_core::Stream;
use tokio::io::{AsyncRead, ReadBuf};
use torture::{
    api::{BinaryResponse, Torture},
    error::{ApiErrorKind, Error},
};

/// Serves connection `n` (from 0) with `respond(n, stream)` from a thread, after its request head.
fn serve(respond: impl Fn(usize, &mut TcpStream) + Send + 'static) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let connections = Arc::new(AtomicUsize::new(0));
    let count = connections.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 2 {
                line.clear();
            }
            respond(count.fetch_add(1, Ordering::SeqCst), &mut stream);
        }
    });
    (url, connections)
}

fn head(stream: &mut TcpStream, status: &str, headers: &str) {
    let head = format!("HTTP/1.1 {status}\r\nconnection: close\r\n{headers}\r\n");
    stream.write_all(head.as_bytes()).unwrap();
    stream.flush().unwrap();
}

/// One chunk of a `transfer-encoding: chunked` body; an empty one ends it.
fn chunk(stream: &mut TcpStream, data: &str) {
    let _ = write!(stream, "{:x}\r\n{data}\r\n", data.len());
    let _ = stream.flush();
}

const CHUNKED: &str = "content-type: application/pdf\r\ntransfer-encoding: chunked\r\n";

fn client(url: &str, timeout: Duration, max_retries: u32) -> Torture {
    Torture::builder().token("t").base_url(url).timeout(timeout).max_retries(max_retries).build().unwrap()
}

async fn next(body: &mut BinaryResponse) -> Option<Result<bytes::Bytes, Error>> {
    std::future::poll_fn(|cx| Pin::new(&mut *body).poll_next(cx)).await
}

async fn read_to_end(body: &mut BinaryResponse) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0u8; 3];
    loop {
        let mut read = ReadBuf::new(&mut buf);
        std::future::poll_fn(|cx| Pin::new(&mut *body).poll_read(cx, &mut read)).await?;
        if read.filled().is_empty() {
            return Ok(out);
        }
        out.extend_from_slice(read.filled());
    }
}

#[tokio::test]
async fn chunks_are_read_as_the_server_sends_them() {
    let (go, wait) = mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    let (url, _) = serve(move |_, stream| {
        head(stream, "200 OK", CHUNKED);
        chunk(stream, "ab");
        wait.lock().unwrap().recv().unwrap();
        chunk(stream, "cd");
        chunk(stream, "ef");
        chunk(stream, "");
    });
    let mut body = client(&url, Duration::from_secs(5), 0).things().download("t").await.unwrap();
    assert_eq!(body.status(), 200);
    assert_eq!(body.headers()["content-type"], "application/pdf");
    assert_eq!(body.chunk().await.unwrap().unwrap(), "ab", "the first chunk, before the server sends more");
    go.send(()).unwrap();
    let mut rest = Vec::new();
    while let Some(chunk) = next(&mut body).await {
        rest.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(rest, b"cdef");
    assert!(body.chunk().await.unwrap().is_none());
}

#[tokio::test]
async fn the_whole_body_is_read_as_bytes_or_through_async_read() {
    let (url, _) = serve(|_, stream| {
        head(stream, "200 OK", "content-type: application/pdf\r\ncontent-length: 10\r\n");
        stream.write_all(b"%PDF-1.7\n.").unwrap();
    });
    let client = client(&url, Duration::from_secs(5), 0);
    let body = client.things().download("t").await.unwrap();
    assert_eq!(body.content_length(), Some(10));
    assert_eq!(body.bytes().await.unwrap(), "%PDF-1.7\n.");
    let mut body = client.things().download("t").await.unwrap();
    assert_eq!(read_to_end(&mut body).await.unwrap(), b"%PDF-1.7\n.");
}

#[tokio::test]
async fn an_error_status_fails_the_call_before_any_body() {
    let (url, _) = serve(|_, stream| {
        let body = r#"{"message":"gone"}"#;
        let headers = format!("content-type: application/json\r\ncontent-length: {}\r\n", body.len());
        head(stream, "404 Not Found", &headers);
        stream.write_all(body.as_bytes()).unwrap();
    });
    let error = client(&url, Duration::from_secs(5), 0).things().download("t").await.unwrap_err();
    assert_eq!(error.kind(), Some(ApiErrorKind::NotFound));
    assert_eq!(error.api().unwrap().text(), r#"{"message":"gone"}"#);
}

#[tokio::test]
async fn retries_end_once_the_headers_arrive() {
    let (url, connections) = serve(|n, stream| {
        if n == 0 {
            head(stream, "503 Service Unavailable", "retry-after-ms: 0\r\ncontent-length: 0\r\n");
        } else {
            head(stream, "200 OK", "content-length: 100\r\n");
            stream.write_all(b"partial").unwrap();
        }
    });
    let body = client(&url, Duration::from_secs(5), 2).things().download("t").await.unwrap();
    assert_eq!(connections.load(Ordering::SeqCst), 2, "the 503 was retried");
    let error = body.bytes().await.unwrap_err();
    assert!(!error.is_timeout(), "{error}");
    assert_eq!(connections.load(Ordering::SeqCst), 2, "a body cut short is not");
}

#[tokio::test]
async fn the_timeout_applies_to_each_read_not_the_whole_download() {
    let (url, _) = serve(|n, stream| {
        head(stream, "200 OK", CHUNKED);
        for _ in 0..5 {
            chunk(stream, "x");
            thread::sleep(Duration::from_millis(if n == 0 { 60 } else { 400 }));
        }
        chunk(stream, "");
    });
    let client = client(&url, Duration::from_millis(200), 0);
    let slow = client.things().download("t").await.unwrap();
    assert_eq!(slow.bytes().await.unwrap(), "xxxxx", "300ms in all, longer than the timeout");
    let mut stalled = client.things().download("t").await.unwrap();
    assert_eq!(stalled.chunk().await.unwrap().unwrap(), "x");
    assert!(stalled.chunk().await.unwrap_err().is_timeout());
    assert!(stalled.chunk().await.unwrap().is_none(), "the body ends after an error");
}
