use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket},
    process::Stdio,
    thread,
    time::{Duration, Instant},
};

fn validate_host(host: &str) -> Result<&str, String> {
    let host = host.trim();
    if host.parse::<IpAddr>().is_ok() {
        return Ok(host);
    }
    if host.is_empty()
        || host.len() > 253
        || !host.is_ascii()
        || host.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(
            "Enter a valid hostname or IP address without spaces or command options.".into(),
        );
    }
    Ok(host)
}

/// The system's ping or traceroute for `host`, with its arguments and how
/// long to let it run.
fn diagnostic_command(
    host: &str,
    ipv6: bool,
    trace: bool,
) -> Result<(String, Vec<String>, Duration), String> {
    let owned = |args: &[&str]| args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
    let timeout = Duration::from_secs(if trace { 40 } else { 15 });
    if cfg!(windows) {
        let family = if ipv6 { "-6" } else { "-4" };
        return Ok(if trace {
            (
                "tracert".into(),
                owned(&[family, "-d", "-h", "16", "-w", "1000", host]),
                timeout,
            )
        } else {
            (
                "ping".into(),
                owned(&[family, "-n", "4", "-w", "3000", host]),
                timeout,
            )
        });
    }
    if cfg!(target_os = "macos") {
        return Ok(if trace {
            (
                if ipv6 {
                    "/usr/sbin/traceroute6"
                } else {
                    "/usr/sbin/traceroute"
                }
                .into(),
                owned(&["-n", "-m", "16", "-w", "1", "-q", "1", host]),
                timeout,
            )
        } else {
            (
                if ipv6 { "/sbin/ping6" } else { "/sbin/ping" }.into(),
                if ipv6 {
                    owned(&["-n", "-c", "4", host])
                } else {
                    owned(&["-n", "-c", "4", "-W", "3000", host])
                },
                timeout,
            )
        });
    }
    let family = if ipv6 { "-6" } else { "-4" };
    let tool = |name: &str| {
        crate::platform::find_tool(name).map(|path| path.to_string_lossy().into_owned())
    };
    if trace {
        if let Some(traceroute) = tool("traceroute") {
            return Ok((
                traceroute,
                owned(&[family, "-n", "-m", "16", "-w", "1", "-q", "1", host]),
                timeout,
            ));
        }
        if let Some(tracepath) = tool("tracepath") {
            return Ok((tracepath, owned(&[family, "-n", "-m", "16", host]), timeout));
        }
        return Err("Install traceroute (or tracepath) to trace the route.".into());
    }
    let ping = tool("ping").ok_or("Install ping (iputils-ping) to check the host.")?;
    Ok((
        ping,
        owned(&[family, "-n", "-c", "4", "-W", "3", host]),
        timeout,
    ))
}

fn run_diagnostic(host: &str, trace: bool) -> Result<String, String> {
    let host = validate_host(host)?;
    let ipv6 = matches!(host.parse::<IpAddr>(), Ok(IpAddr::V6(_)));
    let (program, args, timeout) = diagnostic_command(host, ipv6, trace)?;
    let mut child = crate::platform::command(program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            format!(
                "Could not start {}: {error}",
                if trace { "traceroute" } else { "ping" }
            )
        })?;
    let start = Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|error| format!("Could not check network tool status: {error}"))?
            .is_some()
        {
            break;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "{} took too long. Check the address and network connection.",
                if trace { "Traceroute" } else { "Ping" }
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("Could not read network tool output: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = format!(
        "{}{}{}",
        stdout,
        if stdout.is_empty() || stderr.is_empty() {
            ""
        } else {
            "\n"
        },
        stderr
    );
    let text = text.trim();
    if text.is_empty() {
        return Err(format!(
            "{} returned no output for {host}.",
            if trace { "Traceroute" } else { "Ping" }
        ));
    }
    if !output.status.success() && !trace {
        return Err(format!("Ping did not receive a reply from {host}.\n{text}"));
    }
    Ok(text.chars().take(64 * 1024).collect())
}

fn check_port(host: &str, port: u16) -> Result<String, String> {
    let host = validate_host(host)?;
    if port == 0 {
        return Err("Enter a port between 1 and 65535.".into());
    }
    let addresses: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|error| format!("Could not resolve {host}: {error}"))?
        .collect();
    if addresses.is_empty() {
        return Err(format!("No network address was found for {host}."));
    }
    let mut last_error = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, Duration::from_secs(3)) {
            Ok(_) => return Ok(format!("{host}:{port} is reachable over TCP ({address}).")),
            Err(error) => last_error = Some(error),
        }
    }
    Ok(format!(
        "{host}:{port} is not accepting TCP connections: {}",
        last_error.unwrap()
    ))
}

fn parse_mac(mac: &str) -> Result<[u8; 6], String> {
    let parts: Vec<_> = mac.split([':', '-']).collect();
    if parts.len() != 6 || parts.iter().any(|part| part.len() != 2) {
        return Err("Enter a MAC address as six pairs, such as 00:11:22:33:44:55.".into());
    }
    let mut bytes = [0; 6];
    for (index, part) in parts.iter().enumerate() {
        bytes[index] = u8::from_str_radix(part, 16)
            .map_err(|_| "The MAC address contains a non-hexadecimal character.".to_string())?;
    }
    Ok(bytes)
}

fn send_wake_packet(mac: &str, address: Ipv4Addr, port: u16) -> Result<String, String> {
    let mac_bytes = parse_mac(mac)?;
    if port == 0 {
        return Err("Enter a Wake-on-LAN UDP port between 1 and 65535.".into());
    }
    let mut packet = [0xff; 102];
    for chunk in packet[6..].as_chunks_mut::<6>().0 {
        chunk.copy_from_slice(&mac_bytes);
    }
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|error| format!("Could not open a UDP socket: {error}"))?;
    socket
        .set_broadcast(true)
        .map_err(|error| format!("Could not enable network broadcast: {error}"))?;
    socket.send_to(&packet, (address, port)).map_err(|error| {
        format!("Could not send the Wake-on-LAN packet to {address}:{port}: {error}")
    })?;
    Ok(format!(
        "Wake-on-LAN packet sent to {address}:{port}. The device may take a moment to start."
    ))
}

#[tauri::command]
pub async fn network_ping(host: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || run_diagnostic(&host, false))
        .await
        .map_err(|error| format!("Ping stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn network_traceroute(host: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || run_diagnostic(&host, true))
        .await
        .map_err(|error| format!("Traceroute stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn network_check_port(host: String, port: u16) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || check_port(&host, port))
        .await
        .map_err(|error| format!("Port check stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn network_wake(mac: String, address: String, port: u16) -> Result<String, String> {
    let address = address
        .parse::<Ipv4Addr>()
        .map_err(|_| "Enter an IPv4 broadcast address, such as 192.168.1.255.".to_string())?;
    tauri::async_runtime::spawn_blocking(move || send_wake_packet(&mac, address, port))
        .await
        .map_err(|error| format!("Wake-on-LAN stopped unexpectedly: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn validates_hosts_and_mac_addresses() {
        assert!(validate_host("nas.example.com").is_ok());
        assert!(validate_host("::1").is_ok());
        assert!(validate_host("-c 100").is_err());
        assert!(validate_host("nas;touch /tmp/file").is_err());
        assert_eq!(
            parse_mac("00:11:22:aa:BB:ff").unwrap(),
            [0, 17, 34, 170, 187, 255]
        );
        assert!(parse_mac("00:11:22:33:44:GG").is_err());
    }

    #[test]
    fn checks_a_local_tcp_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(check_port("127.0.0.1", port).unwrap().contains("reachable"));
        assert!(check_port("127.0.0.2", port)
            .unwrap()
            .contains("not accepting"));
    }

    #[test]
    fn sends_a_complete_wake_packet() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let port = receiver.local_addr().unwrap().port();
        send_wake_packet("00-11-22-33-44-55", Ipv4Addr::LOCALHOST, port).unwrap();
        let mut packet = [0; 102];
        let (length, _) = receiver.recv_from(&mut packet).unwrap();
        assert_eq!(length, 102);
        assert_eq!(&packet[..6], &[0xff; 6]);
        for chunk in packet[6..].as_chunks::<6>().0 {
            assert_eq!(chunk, &[0, 17, 34, 51, 68, 85]);
        }
    }

    #[test]
    fn runs_ipv4_and_ipv6_loopback_diagnostics() {
        for host in ["127.0.0.1", "::1"] {
            for trace in [false, true] {
                match run_diagnostic(host, trace) {
                    Ok(output) => assert!(output.contains(host), "{output}"),
                    // Minimal Linux images ship without ping or traceroute.
                    Err(error) if error.starts_with("Install ") => {}
                    Err(error) => panic!("{host}: {error}"),
                }
            }
        }
    }
}
