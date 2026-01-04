use std::time::Duration;

use crate::error::discovery::DiscoveryError;
use netstat2::{AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo, TcpState, get_sockets_info};
use sysinfo::{Pid, Signal, System};

#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub pid: u32,
    pub port: u16,
    pub base_url: String,
    pub name: String,
    pub command: String,
    pub owned: bool, // true if spawned by this EGUI app
}

fn find_listening_port(pid: u32) -> Result<Option<u16>, DiscoveryError> {
    let sockets = get_sockets_info(
        AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6,
        ProtocolFlags::TCP,
    )
    .map_err(|e| DiscoveryError::NetworkQuery(e.to_string()))?;

    for s in sockets {
        if let ProtocolSocketInfo::Tcp(tcp) = s.protocol_socket_info {
            if tcp.state == TcpState::Listen {
                if s.associated_pids.iter().any(|p| *p as u32 == pid) {
                    return Ok(Some(tcp.local_port));
                }
            }
        }
    }
    Ok(None)
}

/// Try to discover a running OpenCode server process and its listening port.
/// Strategy:
/// - If a port override is set, try to connect directly to that port first
/// - Otherwise use sysinfo to enumerate processes, look for bun/node with command containing "opencode".
/// - Use netstat2 to resolve LISTENing port for that PID.
/// - Return first valid match with base_url = http://127.0.0.1:{port}.
pub fn discover() -> Result<Option<ServerInfo>, DiscoveryError> {
    // Check for port override first
    if let Some(override_port) = crate::discovery::get_override_port() {
        let base_url = format!("http://127.0.0.1:{override_port}");
        // Try to find the process listening on this port
        let sockets = get_sockets_info(
            AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6,
            ProtocolFlags::TCP,
        )
        .map_err(|e| DiscoveryError::NetworkQuery(e.to_string()))?;

        for s in sockets {
            if let ProtocolSocketInfo::Tcp(tcp) = s.protocol_socket_info {
                if tcp.state == TcpState::Listen && tcp.local_port == override_port {
                    if let Some(pid) = s.associated_pids.first() {
                        let mut sys = System::new_all();
                        sys.refresh_processes();
                        if let Some(p) = sys.process(Pid::from_u32(*pid as u32)) {
                            let name = p.name().to_string();
                            let cmd_vec = p.cmd();
                            let command = if cmd_vec.is_empty() {
                                String::new()
                            } else {
                                cmd_vec.join(" ")
                            };
                            
                            return Ok(Some(ServerInfo {
                                pid: *pid as u32,
                                port: override_port,
                                base_url,
                                name,
                                command,
                                owned: false,
                            }));
                        }
                    }
                }
            }
        }
        
        // If override port is set but no process found, return None
        // This allows the spawn logic to use the override port
        return Ok(None);
    }

    let mut sys = System::new_all();
    // Refresh processes list
    sys.refresh_processes();

    for (pid, p) in sys.processes() {
        let name = p.name().to_string();
        let cmd_vec = p.cmd();
        let command = if cmd_vec.is_empty() {
            String::new()
        } else {
            cmd_vec.join(" ")
        };

        // Heuristic: bun/node running opencode, or standalone opencode binary
        let is_candidate =
            (name.contains("bun") || name.contains("node") || name.contains("opencode"))
                && (command.contains("opencode") || name.contains("opencode"));

        if !is_candidate {
            continue;
        }

        let pid_u32 = pid.as_u32();
        if let Some(port) = find_listening_port(pid_u32)? {
            let base_url = format!("http://127.0.0.1:{port}");
            return Ok(Some(ServerInfo {
                pid: pid_u32,
                port,
                base_url,
                name,
                command,
                owned: false,
            }));
        }
    }

    Ok(None)
}

/// Attempt to gracefully stop a process by PID. Returns true if a signal was sent and the OS accepted it.
pub fn stop_pid(pid: u32) -> bool {
    let mut sys = System::new_all();
    sys.refresh_processes();
    if let Some(p) = sys.process(Pid::from_u32(pid)) {
        if let Some(sent) = p.kill_with(Signal::Term) {
            return sent;
        }
        return p.kill();
    }
    false
}

/// Lightweight readiness check against GET {base_url}/doc.
pub async fn check_health(base_url: &str) -> bool {
    let url = format!("{base_url}/doc");
    let client = reqwest::Client::new();
    match client
        .get(&url)
        .timeout(Duration::from_secs(3))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => true,
        _ => false,
    }
}
