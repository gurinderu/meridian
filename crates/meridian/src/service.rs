use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::path::PathBuf;

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
    // Bound the whole probe: a peer that accepts but never replies must not hang
    // `meridian status` forever (read_to_end has no implicit timeout).
    let dur = std::time::Duration::from_secs(5);
    let Ok(Ok(mut stream)) =
        tokio::time::timeout(dur, tokio::net::TcpStream::connect(("127.0.0.1", port))).await
    else {
        return false;
    };
    let req = "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    if stream.write_all(req.as_bytes()).await.is_err() {
        return false;
    }
    let mut buf = Vec::new();
    if tokio::time::timeout(dur, stream.read_to_end(&mut buf)).await.is_err() {
        return false;
    }
    is_healthy_response(&String::from_utf8_lossy(&buf))
}

const LABEL: &str = "dev.meridian.proxy";

/// macOS LaunchAgent plist running `<exe> serve --port <port>`.
pub fn launchd_plist(exe: &str, port: u16) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>serve</string>
    <string>--port</string>
    <string>{port}</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
</dict>
</plist>
"#
    )
}

/// systemd user service running `<exe> serve --port <port>`.
pub fn systemd_unit(exe: &str, port: u16) -> String {
    format!(
        "[Unit]\nDescription=Meridian local proxy\nAfter=network.target\n\n\
         [Service]\nExecStart={exe} serve --port {port}\nRestart=on-failure\n\n\
         [Install]\nWantedBy=default.target\n"
    )
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

/// Write + load the platform service unit for `<current_exe> serve --port <port>`.
pub fn install(port: u16) -> std::io::Result<String> {
    let exe = std::env::current_exe()?.to_string_lossy().into_owned();
    #[cfg(target_os = "macos")]
    {
        let path = home().join("Library/LaunchAgents").join(format!("{LABEL}.plist"));
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, launchd_plist(&exe, port))?;
        let _ = std::process::Command::new("launchctl").args(["unload", &path.to_string_lossy()]).status();
        std::process::Command::new("launchctl").args(["load", &path.to_string_lossy()]).status()?;
        Ok(path.to_string_lossy().into_owned())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let path = home().join(".config/systemd/user/meridian.service");
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, systemd_unit(&exe, port))?;
        let _ = std::process::Command::new("systemctl").args(["--user", "daemon-reload"]).status();
        std::process::Command::new("systemctl").args(["--user", "enable", "--now", "meridian.service"]).status()?;
        Ok(path.to_string_lossy().into_owned())
    }
}

/// Stop + remove the platform service unit.
pub fn uninstall() -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let path = home().join("Library/LaunchAgents").join(format!("{LABEL}.plist"));
        let _ = std::process::Command::new("launchctl").args(["unload", &path.to_string_lossy()]).status();
        let _ = std::fs::remove_file(&path);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = std::process::Command::new("systemctl").args(["--user", "disable", "--now", "meridian.service"]).status();
        let _ = std::fs::remove_file(home().join(".config/systemd/user/meridian.service"));
    }
    Ok(())
}
