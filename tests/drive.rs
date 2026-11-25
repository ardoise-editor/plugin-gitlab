//! The plugin as the editor runs it, against a server that answers as GitLab
//! does: the list of merge requests, one opened with its file and comment,
//! and its merge, asked twice.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ardoise_plugin::protocol::{methods, read_message, write_message, Message};
use serde_json::{json, Value};

const MR: &str = r#"{"iid": 5, "title": "Better things", "state": "opened", "sha": "feed01",
 "source_branch": "topic", "target_branch": "main", "author": {"username": "ada"},
 "web_url": "https://x/mr/5", "has_conflicts": false, "draft": false}"#;

/// Every request received: its line and its body.
type Asked = Arc<Mutex<Vec<(String, String)>>>;

/// Answers by what is asked, and keeps what was.
fn gitlab() -> (String, Asked) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = asked.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let seen = seen.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut length = 0;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = header.to_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                }
                let mut sent = vec![0; length];
                reader.read_exact(&mut sent).unwrap();
                let line = line.trim_end().trim_end_matches(" HTTP/1.1").to_string();
                seen.lock()
                    .unwrap()
                    .push((line.clone(), String::from_utf8(sent).unwrap()));
                let path = line.split(['?', ' ']).nth(1).unwrap_or_default();
                let body = if path.ends_with("/user") {
                    r#"{"username": "me"}"#.to_string()
                } else if path.ends_with("/projects/team%2Fapp") {
                    r#"{"default_branch": "main"}"#.to_string()
                } else if path.ends_with("/repository/branches") {
                    r#"[{"name": "main"}, {"name": "topic"}]"#.to_string()
                } else if path.ends_with("/merge_requests") {
                    format!("[{MR}]")
                } else if path.ends_with("/merge_requests/5") {
                    MR.to_string()
                } else if path.ends_with("/diffs") {
                    r#"[{"old_path": "a.rs", "new_path": "a.rs", "diff": "@@ -1 +1 @@\n-x\n+y\n"}]"#
                        .to_string()
                } else if path.ends_with("/discussions") {
                    r#"[{"id": "t1", "notes": [{"id": 1, "body": "looks good to me",
                        "author": {"username": "bob"}, "system": false}]}]"#
                        .to_string()
                } else if path.ends_with("/pipelines") {
                    r#"[{"id": 77, "status": "success"}]"#.to_string()
                } else {
                    "{}".to_string()
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            });
        }
    });
    (address, asked)
}

struct Editor {
    child: Child,
    input: ChildStdin,
    from: Receiver<Message>,
    panel: String,
}

impl Editor {
    fn send(&mut self, message: &Message) {
        write_message(&mut self.input, message).unwrap();
        self.input.flush().unwrap();
    }

    fn event(&mut self, action: &str, value: Value) {
        let mut params = json!({"panel": "main", "action": action});
        if !value.is_null() {
            params["value"] = value;
        }
        self.send(&Message::notification(methods::UI_EVENT, params));
    }

    /// Reads what the plugin says until the panel shows `wanted`.
    fn until(&mut self, wanted: &str) {
        let end = Instant::now() + Duration::from_secs(20);
        while !self.panel.contains(wanted) {
            let left = end.saturating_duration_since(Instant::now());
            let message = self
                .from
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("never showed {wanted:?}: {}", self.panel));
            if let Message::Notification { method, params } = message {
                if method == methods::RENDER && params["panel"] == "main" {
                    self.panel = params["root"].to_string();
                }
            }
        }
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_merge_request_is_listed_read_and_merged() {
    let (address, asked) = gitlab();
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "-q", "-b", "topic"],
        vec!["remote", "add", "origin", "git@gitlab.test:team/app.git"],
    ] {
        assert!(Command::new("git")
            .args(args)
            .current_dir(project.path())
            .status()
            .unwrap()
            .success());
    }
    // The server already known, its token in the file (no keychain here).
    std::fs::write(
        data.path().join("servers.json"),
        json!({"servers": {"gitlab.test": {"address": address, "token": "tok"}}}).to_string(),
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_gitlab"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    let (to, from) = channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(output);
        while let Ok(Some(message)) = read_message(&mut reader) {
            if to.send(message).is_err() {
                break;
            }
        }
    });
    let mut editor = Editor {
        child,
        input,
        from,
        panel: String::new(),
    };
    editor.send(&Message::request(
        1,
        methods::INITIALIZE,
        json!({
            "api": 1, "app_version": "2.8.0",
            "settings": {"locale": "en", "dark": false},
            "workspace": [project.path()],
            "plugin_dir": data.path(), "data_dir": data.path(),
        }),
    ));
    editor.send(&Message::notification(
        methods::PANEL_OPENED,
        json!({"panel": "main"}),
    ));
    editor.until("!5  Better things");
    editor.event("mrs", json!("5"));
    editor.until("looks good to me");
    editor.until("a.rs");
    editor.until("confirm:mr:merge");

    let merges = || {
        asked
            .lock()
            .unwrap()
            .iter()
            .filter(|(line, _)| line.ends_with("/merge_requests/5/merge"))
            .map(|(_, body)| body.clone())
            .collect::<Vec<_>>()
    };
    editor.event("confirm:mr:merge", Value::Null);
    editor.until("Confirm");
    assert!(merges().is_empty(), "one click only arms the button");
    editor.event("confirm:mr:merge", Value::Null);
    editor.until("Merge asked");
    let sent: Value = serde_json::from_str(&merges()[0]).unwrap();
    assert_eq!(sent["sha"], "feed01");
    // No branch is deleted that was not asked for.
    assert_eq!(sent["should_remove_source_branch"], false);
}
