use meridian::service::{health_check, is_healthy_response};

#[test]
fn parses_200_status_line() {
    assert!(is_healthy_response("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"));
    assert!(is_healthy_response("HTTP/1.0 200 OK\r\n\r\n"));
    assert!(!is_healthy_response("HTTP/1.1 503 Service Unavailable\r\n\r\n"));
    assert!(!is_healthy_response(""));
}

#[tokio::test]
async fn health_check_false_when_nothing_listening() {
    // Port 1 is privileged/unused for a user app -> connect fails -> false.
    assert!(!health_check(1).await);
}

#[tokio::test]
async fn health_check_true_against_a_200_listener() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut s, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 256];
        let _ = s.read(&mut buf).await;
        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await;
    });
    assert!(health_check(port).await);
}
