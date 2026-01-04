use std::{io::ErrorKind, process::Stdio, time::Duration};

use regex::Regex;
use tokio::io::AsyncBufReadExt;

use crate::discovery::process::{ServerInfo, check_health};
use crate::error::spawn::SpawnError;

/// Spawn `opencode serve --port {port} --hostname 127.0.0.1` and parse the printed URL line.
/// If a port override is set, use that port; otherwise use port 0 (auto-select).
/// Then poll GET {base_url}/doc until success or timeout.
pub async fn spawn_and_wait() -> Result<ServerInfo, SpawnError> {
    let port_arg = crate::discovery::get_override_port()
        .map(|p| p.to_string())
        .unwrap_or_else(|| "0".to_string());

    let cmd = tokio::process::Command::new("opencode")
        .arg("serve")
        .arg("--port")
        .arg(&port_arg)
        .arg("--hostname")
        .arg("127.0.0.1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match cmd {
        Ok(child) => child,
        Err(err) => {
            if err.kind() != ErrorKind::NotFound {
                return Err(SpawnError::Spawn(err.to_string()));
            }

            let exe = std::env::current_exe().map_err(|e| SpawnError::Spawn(e.to_string()))?;
            let dir = exe
                .parent()
                .ok_or_else(|| SpawnError::Spawn("missing exe dir".to_string()))?;
            let path = dir.join("opencode");

            tokio::process::Command::new(path)
                .arg("serve")
                .arg("--port")
                .arg(&port_arg)
                .arg("--hostname")
                .arg("127.0.0.1")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| SpawnError::Spawn(e.to_string()))?
        }
    };

    let mut stdout = tokio::io::BufReader::new(child.stdout.take().expect("stdout")).lines();

    // Example line from server: "opencode server listening on http://127.0.0.1:4096"
    let re = Regex::new(r"http://([^\s:]+):(\d+)").unwrap();

    let mut found = None;
    // Read a few lines to find the URL
    for _ in 0..100 {
        if let Ok(Some(line)) = stdout.next_line().await {
            if let Some(cap) = re.captures(&line) {
                let host = cap.get(1).unwrap().as_str().to_string();
                let p: u16 = cap.get(2).unwrap().as_str().parse().unwrap_or(0);
                if p != 0 {
                    found = Some((host, p));
                    break;
                }
            }
        } else {
            break;
        }
    }

    let (host, p) = found.ok_or(SpawnError::Parse)?;
    let base_url = format!("http://{host}:{p}");

    // Wait for readiness
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if check_health(&base_url).await {
            let pid = child.id().unwrap_or_default();
            return Ok(ServerInfo {
                pid,
                port: p,
                base_url,
                name: "opencode".into(),
                command: "opencode serve".into(),
                owned: true,
            });
        }
        if tokio::time::Instant::now() > deadline {
            return Err(SpawnError::Timeout);
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}
