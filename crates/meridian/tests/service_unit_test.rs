use meridian::service::{launchd_plist, systemd_unit};

#[test]
fn launchd_plist_has_label_and_serve_args() {
    let p = launchd_plist("/usr/local/bin/meridian", 8787);
    assert!(p.contains("dev.meridian.proxy"), "label");
    assert!(p.contains("<string>/usr/local/bin/meridian</string>"), "exe");
    assert!(p.contains("<string>serve</string>"));
    assert!(p.contains("<string>--port</string>"));
    assert!(p.contains("<string>8787</string>"));
    assert!(p.contains("<key>RunAtLoad</key>") && p.contains("<key>KeepAlive</key>"));
}

#[test]
fn systemd_unit_has_execstart_and_restart() {
    let u = systemd_unit("/usr/bin/meridian", 9000);
    assert!(u.contains("ExecStart=/usr/bin/meridian serve --port 9000"), "execstart: {u}");
    assert!(u.contains("Restart=on-failure"));
    assert!(u.contains("WantedBy=default.target"));
}
