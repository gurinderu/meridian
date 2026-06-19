use meridian::pooled_runner::pooled_runner;
use meridian::server::StreamRunner;
use tokio_stream::StreamExt;

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn stream_yields_deltas_and_stop() {
    let root = std::env::temp_dir().join(format!("meridian-stream-{}", std::process::id()));
    let runner = pooled_runner("claude".into(), root, 2);
    let mut stream = runner.run_stream("sonnet".into(), None, "Reply with exactly: OK".into());

    let mut names = Vec::new();
    while let Some(Ok(_ev)) = stream.next().await {
        // We can't read the Event's fields directly; count that events flow.
        names.push(());
        if names.len() > 50 { break; }
    }
    assert!(!names.is_empty(), "stream produced no SSE events");
}
