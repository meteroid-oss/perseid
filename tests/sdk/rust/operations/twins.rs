//! Runs against the SDK generated from tests/fixtures/edge-operations.yaml (see tests/sdk/run.sh):
//! the `_stream` twin of a multipart operation sends its `stream` part, the other leaves it out.
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
};

use edge_operations::api::{
    EdgeOperations, ThreadsCreateTranscriptionBody, ThreadsCreateTranscriptionStreamBody, Upload,
};

/// Serves every request over plain HTTP, as an event stream when its body asks for one, and
/// records the bodies.
fn serve(bodies: Arc<Mutex<Vec<String>>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 2 {
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        length = value.trim().parse().unwrap();
                    }
                }
                line.clear();
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let body = String::from_utf8_lossy(&body).into_owned();
            let (content_type, reply) = match body.contains("name=\"stream\"") {
                true => ("text/event-stream", "data: {\"id\":\"a\"}\n\ndata: {\"id\":\"b\"}\n\n"),
                false => ("application/json", "{\"id\":\"a\"}"),
            };
            bodies.lock().unwrap().push(body);
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                reply.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    url
}

#[tokio::test]
async fn only_the_stream_twin_sends_the_stream_part() {
    let bodies = Arc::default();
    let client = EdgeOperations::builder()
        .token("t")
        .base_url(serve(Arc::clone(&bodies)))
        .max_retries(0)
        .build()
        .unwrap();

    let body = ThreadsCreateTranscriptionBody {
        file: Upload::bytes("audio"),
        language: Some("en".into()),
    };
    let item = client.threads().create_transcription(body).await.unwrap();
    assert_eq!(item.id, "a");

    let body = ThreadsCreateTranscriptionStreamBody {
        file: Upload::bytes("audio"),
        language: None,
    };
    let mut events = client.threads().create_transcription_stream(body).await.unwrap();
    let mut ids = Vec::new();
    while let Some(item) = events.next().await {
        ids.push(item.unwrap().id);
    }
    assert_eq!(ids, ["a", "b"]);

    let bodies = bodies.lock().unwrap();
    assert!(!bodies[0].contains("name=\"stream\""), "{}", bodies[0]);
    assert!(bodies[0].contains("name=\"language\"\r\n\r\nen\r\n"), "{}", bodies[0]);
    assert!(bodies[1].contains("name=\"stream\"\r\n\r\ntrue\r\n"), "{}", bodies[1]);
}
