//! Response limits apply before native clients buffer or deserialize a body.
#![cfg(not(target_arch = "wasm32"))]

use cdk_http_client::{Async, HttpClient, HttpError, Transport, MAX_RESPONSE_BYTES};

fn assert_limit<T>(result: Result<T, HttpError>) {
    let error = result.err().expect("oversized response must fail");
    assert!(matches!(
        error,
        HttpError::ResponseTooLarge { limit } if limit == MAX_RESPONSE_BYTES
    ));
    assert!(
        !error.is_replay_safe(),
        "size errors must not replay a payment"
    );
}

#[tokio::test]
async fn native_response_limit_covers_raw_json_and_error_paths() {
    let mut server = mockito::Server::new_async().await;
    let oversized = format!("\"{}\"", "x".repeat(MAX_RESPONSE_BYTES - 1));
    let mut mocks = Vec::new();
    for method in ["GET", "POST", "PATCH", "PUT"] {
        mocks.push(
            server
                .mock(method, "/large")
                .with_body(&oversized)
                .create_async()
                .await,
        );
    }
    let client = HttpClient::new();
    let url = format!("{}/large", server.url());
    let payload = serde_json::json!({"request": "unchanged"});
    assert_limit(client.get_raw(&url).await);
    assert_limit(client.fetch::<String>(&url).await);
    assert_limit(client.post_json::<_, String>(&url, &payload).await);
    assert_limit(client.post_form::<_, String>(&url, &payload).await);
    assert_limit(client.patch_json::<_, String>(&url, &payload).await);
    assert_limit(client.get(&url).send().await);
    assert_limit(client.post(&url).json(&payload).send_json::<String>().await);
    assert_limit(client.patch(&url).send().await);
    assert_limit(client.put(&url).send().await);
    let transport = Async::default();
    let parsed: url::Url = url.parse().expect("local URL");
    assert_limit(transport.http_get(parsed.clone(), None).await);
    assert_limit(transport.http_post(parsed.clone(), None, &payload).await);
    assert_limit(transport.http_post_form(parsed, None, &payload).await);

    let error_response = server
        .mock("POST", "/error")
        .with_status(503)
        .with_body(oversized)
        .create_async()
        .await;
    assert_limit(client.post(&format!("{}/error", server.url())).send().await);
    error_response.assert_async().await;
    drop(mocks);
}

#[tokio::test]
async fn native_response_limit_counts_chunks_and_discards_failed_connections() {
    let mut server = mockito::Server::new_async().await;
    let client = HttpClient::new();
    for extra in [0, 1] {
        let route = format!("/chunks/{extra}");
        let response = server
            .mock("GET", route.as_str())
            .with_chunked_body(move |writer| {
                for _ in 0..MAX_RESPONSE_BYTES / 4096 {
                    writer.write_all(&[b'x'; 4096])?;
                }
                if extra != 0 {
                    writer.write_all(b"x")?;
                }
                Ok(())
            })
            .create_async()
            .await;
        let result = client.get_raw(&format!("{}{route}", server.url())).await;
        if extra == 0 {
            assert_eq!(
                result
                    .expect("exact limit")
                    .bytes()
                    .await
                    .expect("bytes")
                    .len(),
                MAX_RESPONSE_BYTES
            );
        } else {
            assert_limit(result);
        }
        response.assert_async().await;
    }
    let small = server
        .mock("GET", "/small")
        .with_body("\"ok\"")
        .create_async()
        .await;
    let value: String = client
        .fetch(&format!("{}/small", server.url()))
        .await
        .expect("clean next response");
    assert_eq!(value, "ok");
    small.assert_async().await;
}

// bitreq exposes headers only after reading its body; its streaming cap is
// covered above. Immediate header rejection is available in reqwest.
#[cfg(feature = "reqwest")]
#[tokio::test]
async fn native_response_limit_rejects_headers_without_waiting_for_body() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Duration;
    let listener = TcpListener::bind("127.0.0.1:0").expect("listen");
    let address = listener.local_addr().expect("address");
    let (finished, receiver) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).expect("request");
            request.push(byte[0]);
            assert!(request.len() < 4096);
        }
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            MAX_RESPONSE_BYTES + 1
        )
        .expect("headers");
        // Keep the body unavailable until the client has accepted or rejected the headers.
        let _ = receiver.recv_timeout(Duration::from_secs(5));
    });
    let client = HttpClient::new();
    let url = format!("http://{address}/headers");
    let result = tokio::time::timeout(Duration::from_secs(2), client.get_raw(&url)).await;
    let _ = finished.send(());
    server.join().expect("server finished");
    assert_limit(result.expect("reject without waiting for body"));
}
