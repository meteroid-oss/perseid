use perseid_test_sdk::api::{Example, ExampleOptions, FilesMultipartBody, Upload};
use std::{
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf};
use wiremock::{
    matchers::{body_bytes, header, method, path},
    Mock, MockServer, ResponseTemplate,
};

fn client(url: &str, retries: u32) -> Example {
    Example::new(
        "secret".into(),
        Some(ExampleOptions {
            server_url: Some(url.into()),
            num_retries: Some(retries),
            retry_schedule: Some(vec![Duration::ZERO; retries as usize]),
            timeout: Some(Duration::from_secs(3)),
            ..Default::default()
        }),
    )
}
fn success() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok":true}))
}

#[tokio::test]
async fn buffered_raw_upload_is_byte_exact_and_retryable() {
    let server = MockServer::start().await;
    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = attempts.clone();
    Mock::given(method("PUT"))
        .and(path("/upload"))
        .and(header("content-type", "application/octet-stream"))
        .and(header("authorization", "Bearer secret"))
        .and(body_bytes(vec![0, 255, 1, 128]))
        .respond_with(move |_: &wiremock::Request| {
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(503)
            } else {
                success()
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    assert!(
        client(&server.uri(), 1)
            .files()
            .upload(Upload::bytes(vec![0, 255, 1, 128]))
            .await
            .unwrap()
            .ok
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests[0].headers["example-req-id"],
        requests[1].headers["example-req-id"]
    );
    assert_eq!(requests[1].headers["example-retry-count"], "1");
}

struct GeneratedReader {
    remaining: usize,
    max_read: Arc<AtomicUsize>,
}
impl AsyncRead for GeneratedReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.max_read.fetch_max(buf.remaining(), Ordering::SeqCst);
        let size = self.remaining.min(buf.remaining());
        buf.put_slice(&vec![b'x'; size]);
        self.remaining -= size;
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn large_reader_uploads_in_bounded_chunks_and_is_not_retried() {
    let server = MockServer::start().await;
    let size = 512 * 1024;
    let maximum = Arc::new(AtomicUsize::new(0));
    Mock::given(path("/upload"))
        .and(header("content-length", size.to_string()))
        .and(body_bytes(vec![b'x'; size]))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let upload = Upload::reader(
        GeneratedReader {
            remaining: size,
            max_read: maximum.clone(),
        },
        Some(size as u64),
    );
    let error = client(&server.uri(), 3)
        .files()
        .upload(upload)
        .await
        .unwrap_err();
    assert_eq!(error.status().unwrap().as_u16(), 503);
    assert!(maximum.load(Ordering::SeqCst) <= 16 * 1024);
}

#[tokio::test]
async fn unknown_length_readers_use_streaming_transfer() {
    let server = MockServer::start().await;
    Mock::given(path("/upload"))
        .and(header("transfer-encoding", "chunked"))
        .and(body_bytes(b"streamed".to_vec()))
        .respond_with(success())
        .expect(1)
        .mount(&server)
        .await;
    let client = client(&server.uri(), 0);
    let sent = tokio::spawn(async move {
        client
            .files()
            .upload(Upload::reader(&b"streamed"[..], None))
            .await
    })
    .await
    .unwrap()
    .unwrap();
    assert!(sent.ok);
}

#[tokio::test]
async fn incorrect_reader_lengths_fail() {
    let server = MockServer::start().await;
    Mock::given(path("/upload"))
        .respond_with(success())
        .mount(&server)
        .await;
    for length in [2, 8] {
        let upload = Upload::reader(&b"four"[..], Some(length));
        assert!(client(&server.uri(), 0)
            .files()
            .upload(upload)
            .await
            .is_err());
    }
}

#[tokio::test]
async fn multipart_fields_filename_and_binary_content_are_encoded() {
    let server = MockServer::start().await;
    Mock::given(path("/multipart"))
        .respond_with(success())
        .expect(1)
        .mount(&server)
        .await;
    let body = FilesMultipartBody {
        file: Upload::reader(&b"\0\xffdata"[..], Some(6))
            .with_filename("data.csv")
            .with_content_type("text/csv"),
        title: "a title".into(),
        count: Some(5_000_000_000),
        kind: None,
    };
    assert!(
        client(&server.uri(), 0)
            .files()
            .multipart(body)
            .await
            .unwrap()
            .ok
    );
    let requests = server.received_requests().await.unwrap();
    let request = &requests[0];
    let content_type = request.headers["content-type"].to_str().unwrap();
    let boundary = content_type
        .strip_prefix("multipart/form-data; boundary=")
        .unwrap();
    let body = String::from_utf8_lossy(&request.body);
    assert!(body.contains("name=\"title\"\r\n\r\na title\r\n"));
    assert!(body.contains("name=\"count\"\r\n\r\n5000000000\r\n"));
    assert!(body.contains("filename=\"data.csv\"\r\nContent-Type: text/csv"));
    assert!(!body.contains("name=\"kind\""));
    assert!(request.body.windows(6).any(|w| w == b"\0\xffdata"));
    assert!(body.ends_with(&format!("--{boundary}--\r\n")));
    assert_eq!(
        request.headers["content-length"]
            .to_str()
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        request.body.len()
    );
}

#[tokio::test]
async fn multipart_retries_replay_the_same_body_and_idempotency_key() {
    let server = MockServer::start().await;
    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = attempts.clone();
    Mock::given(path("/multipart"))
        .respond_with(move |_: &wiremock::Request| {
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(503)
            } else {
                success()
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    let body = FilesMultipartBody {
        file: Upload::bytes("file"),
        title: "title".into(),
        count: None,
        kind: None,
    };
    client(&server.uri(), 1)
        .files()
        .multipart(body)
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].body, requests[1].body);
    assert_eq!(
        requests[0].headers["idempotency-key"],
        requests[1].headers["idempotency-key"]
    );
}

#[tokio::test]
async fn multipart_rejects_header_injection_before_sending() {
    let server = MockServer::start().await;
    let body = FilesMultipartBody {
        file: Upload::bytes("file").with_filename("bad\r\nheader"),
        title: "title".into(),
        count: None,
        kind: None,
    };
    assert!(client(&server.uri(), 0)
        .files()
        .multipart(body)
        .await
        .is_err());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn optional_binary_body_can_be_omitted() {
    let server = MockServer::start().await;
    Mock::given(path("/optional-upload"))
        .and(body_bytes(Vec::new()))
        .respond_with(success())
        .expect(1)
        .mount(&server)
        .await;
    assert!(
        client(&server.uri(), 0)
            .files()
            .optional_upload(None)
            .await
            .unwrap()
            .ok
    );
}

struct SseServer {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for SseServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn sse_server(chunks: Vec<(Duration, Vec<u8>)>, end: bool) -> SseServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            request.push(socket.read_u8().await.unwrap());
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let request = String::from_utf8(request).unwrap().to_lowercase();
        assert!(request.contains("accept: text/event-stream"));
        assert!(request.contains("authorization: bearer secret"));
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
        for (delay, bytes) in chunks {
            tokio::time::sleep(delay).await;
            let frame = format!("{:x}\r\n", bytes.len());
            if socket.write_all(frame.as_bytes()).await.is_err() {
                return;
            }
            if socket.write_all(&bytes).await.is_err() {
                return;
            }
            if socket.write_all(b"\r\n").await.is_err() {
                return;
            }
        }
        if end {
            let _ = socket.write_all(b"0\r\n\r\n").await;
        }
    });
    SseServer { url, task }
}

#[tokio::test]
async fn sse_parses_split_utf8_crlf_multiline_and_metadata() {
    let data = "\u{feff}: heartbeat\r\nid: 12\revent: log\rdata: café\rdata: next\rretry: 250\r\rid: bad\0id\nretry: -2\ndata:\n\nid:\n\ndata: third\n\ndata: unfinished";
    let chunks = data
        .as_bytes()
        .iter()
        .map(|b| (Duration::ZERO, vec![*b]))
        .collect();
    let server = sse_server(chunks, true).await;
    let mut stream = client(&server.url, 0).files().events().await.unwrap();
    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(first.event, "log");
    assert_eq!(first.data, "café\nnext");
    assert_eq!(first.id.as_deref(), Some("12"));
    assert_eq!(first.retry, Some(Duration::from_millis(250)));
    let second = stream.next().await.unwrap().unwrap();
    assert_eq!(second.event, "message");
    assert_eq!(second.data, "");
    assert_eq!(second.id.as_deref(), Some("12"));
    assert_eq!(
        stream.next().await.unwrap().unwrap().id.as_deref(),
        Some("")
    );
    assert!(stream.next().await.is_none());
    assert_eq!(stream.last_event_id(), Some(""));
}

#[tokio::test]
async fn sse_is_incremental_and_cancel_safe_without_a_whole_stream_timeout() {
    let server = sse_server(
        vec![
            (Duration::ZERO, b"data: first\n\ndata: par".to_vec()),
            (Duration::from_millis(250), b"tial\n\n".to_vec()),
        ],
        true,
    )
    .await;
    let client = Example::new(
        "secret".into(),
        Some(ExampleOptions {
            server_url: Some(server.url.clone()),
            timeout: Some(Duration::from_millis(100)),
            num_retries: Some(0),
            ..Default::default()
        }),
    );
    let mut stream = tokio::time::timeout(Duration::from_secs(1), client.files().events())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stream.next().await.unwrap().unwrap().data, "first");
    assert!(
        tokio::time::timeout(Duration::from_millis(30), stream.next())
            .await
            .is_err()
    );
    assert_eq!(stream.next().await.unwrap().unwrap().data, "partial");
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn sse_size_limits_and_transport_errors_terminate_the_stream() {
    let server = sse_server(
        vec![(Duration::ZERO, b"data: much too long\n\n".to_vec())],
        true,
    )
    .await;
    let mut stream = client(&server.url, 0)
        .files()
        .events()
        .await
        .unwrap()
        .with_max_event_bytes(8);
    assert!(stream.next().await.unwrap().is_err());
    assert!(stream.next().await.is_none());
    let server = sse_server(vec![(Duration::ZERO, b"data: okay\n\n".to_vec())], false).await;
    let mut stream = client(&server.url, 0).files().events().await.unwrap();
    assert!(stream.next().await.unwrap().is_ok());
    assert!(stream.next().await.unwrap().is_err());
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn sse_retries_only_while_opening_and_validates_content_type() {
    let server = MockServer::start().await;
    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = attempts.clone();
    Mock::given(path("/events"))
        .respond_with(move |_: &wiremock::Request| {
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(200).set_body_raw("data: ready\n\n", "text/event-stream")
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    let mut stream = client(&server.uri(), 1).files().events().await.unwrap();
    assert_eq!(stream.next().await.unwrap().unwrap().data, "ready");
    assert!(stream.next().await.is_none());
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    server.reset().await;
    Mock::given(path("/events"))
        .respond_with(success())
        .expect(1)
        .mount(&server)
        .await;
    assert!(client(&server.uri(), 0).files().events().await.is_err());
}

#[tokio::test]
async fn multipart_reader_bodies_are_not_retried() {
    let server = MockServer::start().await;
    Mock::given(path("/multipart"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let body = FilesMultipartBody {
        file: Upload::reader(&b"file"[..], Some(4)),
        title: "title".into(),
        count: None,
        kind: None,
    };
    let error = client(&server.uri(), 3)
        .files()
        .multipart(body)
        .await
        .unwrap_err();
    assert_eq!(error.status().unwrap().as_u16(), 503);
}

#[tokio::test]
async fn timeout_retries_require_a_replayable_body() {
    let server = MockServer::start().await;
    for (body, expected) in [
        (Upload::bytes("file"), 2u64),
        (Upload::reader(&b"file"[..], Some(4)), 1u64),
    ] {
        Mock::given(path("/upload"))
            .respond_with(success().set_delay(Duration::from_millis(150)))
            .expect(expected)
            .mount(&server)
            .await;
        let client = Example::new(
            "secret".into(),
            Some(ExampleOptions {
                server_url: Some(server.uri()),
                timeout: Some(Duration::from_millis(30)),
                retry_schedule: Some(vec![Duration::ZERO]),
                ..Default::default()
            }),
        );
        assert!(client.files().upload(body).await.is_err());
        server.verify().await;
        server.reset().await;
    }
}

#[tokio::test]
async fn sse_client_errors_retain_status_and_are_not_retried() {
    let server = MockServer::start().await;
    Mock::given(path("/events"))
        .respond_with(ResponseTemplate::new(403).set_body_string("denied"))
        .expect(1)
        .mount(&server)
        .await;
    match client(&server.uri(), 3).files().events().await {
        Ok(_) => panic!("403 must not open an event stream"),
        Err(error) => assert_eq!(error.status().unwrap().as_u16(), 403),
    }
}
