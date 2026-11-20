//! VNC inside Weave: the viewer (noVNC) talks WebSocket to a one-time local
//! bridge, which forwards bytes to the VNC server directly or through a saved
//! SSH host with `ssh -W`, so the screen is encrypted on the way.

use crate::{profiles, AppState};
use serde::Serialize;
use std::{
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream, ToSocketAddrs},
    process::Stdio,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tauri::State;
use tungstenite::{
    handshake::server::{ErrorResponse, Request, Response},
    Message,
};

type Target = (
    Box<dyn Write + Send>,
    mpsc::Receiver<Vec<u8>>,
    Option<std::process::Child>,
);

fn reader_thread(mut source: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            match source.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if sender.send(buffer[..count].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    receiver
}

fn open_target(
    host: &str,
    port: u16,
    through: Option<&profiles::Profile>,
) -> Result<Target, String> {
    if let Some(profile) = through {
        let mut command = crate::platform::command("ssh");
        command
            .args(crate::direct_transfer::ssh_options(profile)?)
            .args([
                "-p",
                &profile.port.to_string(),
                "-W",
                &target_address(host, port),
            ])
            .arg(format!("{}@{}", profile.username, profile.host))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|error| format!("Could not start SSH for the VNC connection: {error}"))?;
        let stdin = child.stdin.take().ok_or("Could not talk to SSH.")?;
        let stdout = child.stdout.take().ok_or("Could not talk to SSH.")?;
        return Ok((Box::new(stdin), reader_thread(stdout), Some(child)));
    }
    let address = (host, port)
        .to_socket_addrs()
        .map_err(|error| format!("Could not find {host}: {error}"))?
        .find_map(|address| TcpStream::connect_timeout(&address, Duration::from_secs(10)).ok())
        .ok_or_else(|| format!("Could not connect to {host} on port {port}. Check that screen sharing is on and the port is right."))?;
    let reader = address
        .try_clone()
        .map_err(|error| format!("Could not prepare the VNC connection: {error}"))?;
    Ok((Box::new(address), reader_thread(reader), None))
}

fn target_address(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Accepts one viewer on `listener` whose request path is `/token`, then
/// relays until either side closes.
#[allow(clippy::result_large_err)] // tungstenite fixes the callback signature.
fn bridge(
    listener: TcpListener,
    token: String,
    mut connect: impl FnMut() -> Result<Target, String>,
) {
    listener.set_nonblocking(true).ok();
    let started = Instant::now();
    let stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error)
                if error.kind() == ErrorKind::WouldBlock
                    && started.elapsed() < Duration::from_secs(60) =>
            {
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return,
        }
    };
    drop(listener);
    stream.set_nonblocking(false).ok();
    let expected = format!("/{token}");
    let accepted = tungstenite::accept_hdr(
        stream,
        |request: &Request, mut response: Response| -> Result<Response, ErrorResponse> {
            if request.uri().path() != expected {
                let mut refusal = ErrorResponse::new(Some("Not allowed".into()));
                *refusal.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
                return Err(refusal);
            }
            let wants_binary = request
                .headers()
                .get("Sec-WebSocket-Protocol")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.split(',').any(|part| part.trim() == "binary"));
            if wants_binary {
                response.headers_mut().insert(
                    "Sec-WebSocket-Protocol",
                    "binary".parse().expect("valid header"),
                );
            }
            Ok(response)
        },
    );
    let Ok(mut socket) = accepted else { return };
    let Ok((mut writer, receiver, mut child)) = connect() else {
        let _ = socket.close(None);
        return;
    };
    socket
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(15)))
        .ok();
    'relay: loop {
        match socket.read() {
            Ok(Message::Binary(data)) => {
                if writer
                    .write_all(&data)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
        loop {
            match receiver.try_recv() {
                Ok(chunk) => {
                    if socket.write(Message::Binary(chunk.into())).is_err() {
                        break 'relay;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break 'relay,
            }
        }
        if socket.flush().is_err() {
            break;
        }
    }
    let _ = socket.close(None);
    let _ = socket.flush();
    if let Some(child) = child.as_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VncBridge {
    url: String,
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('-')
        && host.len() <= 253
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-_:".contains(c))
}

/// Starts a bridge for one viewer. It closes when the viewer disconnects, or
/// after a minute if nothing connects.
#[tauri::command]
pub fn vnc_bridge_start(
    state: State<'_, AppState>,
    host: String,
    port: u16,
    profile_id: Option<String>,
) -> Result<VncBridge, String> {
    let host = host
        .trim()
        .trim_matches(|c| c == '[' || c == ']')
        .to_string();
    if !valid_host(&host) || port == 0 {
        return Err("Enter the VNC host name or address and port.".into());
    }
    let through = match profile_id.filter(|id| !id.is_empty()) {
        Some(id) => {
            let profile = profiles::get_profile(&state, &id)?;
            if profile.protocol != "ssh" {
                return Err("Choose an SSH host for the VNC tunnel.".into());
            }
            Some(profile)
        }
        None => None,
    };
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("Could not start the viewer: {error}"))?;
    let address = listener.local_addr().map_err(|error| error.to_string())?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let url = format!("ws://{address}/{token}");
    thread::spawn(move || {
        bridge(listener, token, || {
            open_target(&host, port, through.as_ref())
        })
    });
    Ok(VncBridge { url })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target_to(address: std::net::SocketAddr) -> Target {
        let stream = TcpStream::connect(address).unwrap();
        let reader = stream.try_clone().unwrap();
        (Box::new(stream), reader_thread(reader), None)
    }

    #[test]
    fn relays_bytes_between_the_viewer_and_the_server() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let server_address = server.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            stream.write_all(b"RFB 003.008\n").unwrap();
            let mut reply = [0_u8; 12];
            stream.read_exact(&mut reply).unwrap();
            stream.write_all(&reply).unwrap();
        });
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            bridge(listener, "secret".into(), move || {
                Ok(target_to(server_address))
            })
        });
        let request = tungstenite::client::IntoClientRequest::into_client_request(format!(
            "ws://{address}/secret"
        ))
        .unwrap();
        let (mut socket, _) =
            tungstenite::client(request, TcpStream::connect(address).unwrap()).unwrap();
        let greeting = loop {
            if let Message::Binary(data) = socket.read().unwrap() {
                break data;
            }
        };
        assert_eq!(&greeting[..], b"RFB 003.008\n");
        socket
            .send(Message::Binary(b"RFB 003.008\n".to_vec().into()))
            .unwrap();
        let echoed = loop {
            if let Message::Binary(data) = socket.read().unwrap() {
                break data;
            }
        };
        assert_eq!(&echoed[..], b"RFB 003.008\n");
    }

    #[test]
    fn refuses_viewers_without_the_token() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || bridge(listener, "secret".into(), || Err("unused".into())));
        let request = tungstenite::client::IntoClientRequest::into_client_request(format!(
            "ws://{address}/guess"
        ))
        .unwrap();
        assert!(tungstenite::client(request, TcpStream::connect(address).unwrap()).is_err());
        assert!(valid_host("mac.local") && !valid_host("-oProxy") && !valid_host("a b"));
    }

    #[test]
    fn formats_ipv6_targets_for_ssh_direct_tcpip() {
        assert_eq!(target_address("nas.example", 5900), "nas.example:5900");
        assert_eq!(target_address("2001:db8::1", 5901), "[2001:db8::1]:5901");
    }
}
