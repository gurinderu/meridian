use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// True if an HTTP response's status line is a 200.
pub fn is_healthy_response(raw: &str) -> bool {
    raw.lines()
        .next()
        .map(|line| {
            let mut parts = line.split_whitespace();
            parts.next(); // HTTP/x.y
            parts.next() == Some("200")
        })
        .unwrap_or(false)
}

/// Probe `GET /health` on `127.0.0.1:<port>`; true iff it answers 200.
pub async fn health_check(port: u16) -> bool {
    let Ok(mut stream) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await else {
        return false;
    };
    let req = "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    if stream.write_all(req.as_bytes()).await.is_err() {
        return false;
    }
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf).await;
    is_healthy_response(&String::from_utf8_lossy(&buf))
}
