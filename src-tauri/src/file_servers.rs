//! Built-in TFTP and HTTP servers for a chosen folder, for copying firmware
//! and configs to network devices or sharing files on the local network.
//! HTTP is read-only. TFTP accepts uploads only when allowed, never replaces
//! an existing file, and both refuse paths outside the folder.

use serde::Serialize;
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    net::{SocketAddr, TcpListener, TcpStream, UdpSocket},
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::Duration,
};
use tauri::{AppHandle, Emitter};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    id: String,
    kind: String,
    folder: String,
    port: u16,
    everyone: bool,
    uploads: bool,
    addresses: Vec<String>,
}

struct Running {
    info: ServerInfo,
    stop: Arc<AtomicBool>,
}

fn servers() -> &'static Mutex<HashMap<String, Running>> {
    static SERVERS: OnceLock<Mutex<HashMap<String, Running>>> = OnceLock::new();
    SERVERS.get_or_init(Default::default)
}

/// Resolves a requested name inside `root`, refusing anything that leaves it.
pub(crate) fn inside(root: &Path, requested: &str) -> Option<PathBuf> {
    let requested = requested.replace('\\', "/");
    let mut path = root.to_path_buf();
    for component in Path::new(requested.trim_start_matches('/')).components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    // Follow links, so a link inside the folder cannot point outside it.
    let resolved = match dunce::canonicalize(&path) {
        Ok(resolved) => resolved,
        Err(_) => dunce::canonicalize(path.parent()?)
            .ok()?
            .join(path.file_name()?),
    };
    resolved.starts_with(root).then_some(path)
}

fn log(app: &Option<AppHandle>, id: &str, line: String) {
    if let Some(app) = app {
        let _ = app.emit("file-server-log", json_line(id, &line));
    }
}

fn json_line(id: &str, line: &str) -> serde_json::Value {
    serde_json::json!({ "id": id, "line": line })
}

// TFTP (RFC 1350, with the blksize and tsize options of RFC 2348/2349).
const RRQ: u16 = 1;
const WRQ: u16 = 2;
const DATA: u16 = 3;
const ACK: u16 = 4;
const ERROR: u16 = 5;
const OACK: u16 = 6;

fn error_packet(code: u16, message: &str) -> Vec<u8> {
    let mut packet = vec![0, ERROR as u8, 0, code as u8];
    packet.extend_from_slice(message.as_bytes());
    packet.push(0);
    packet
}

struct Request {
    opcode: u16,
    name: String,
    options: HashMap<String, String>,
}

fn parse_request(packet: &[u8]) -> Option<Request> {
    let opcode = u16::from_be_bytes([*packet.first()?, *packet.get(1)?]);
    let mut parts = packet
        .get(2..)?
        .split(|byte| *byte == 0)
        .map(|part| String::from_utf8_lossy(part).to_string());
    let name = parts.next()?;
    let _mode = parts.next()?;
    let mut options = HashMap::new();
    while let (Some(key), Some(value)) = (parts.next(), parts.next()) {
        if !key.is_empty() {
            options.insert(key.to_ascii_lowercase(), value);
        }
    }
    Some(Request {
        opcode,
        name,
        options,
    })
}

/// Sends a packet and waits for the matching acknowledgement, retrying.
fn send_and_wait(socket: &UdpSocket, packet: &[u8], expect_block: u16) -> Result<(), String> {
    let mut buffer = [0_u8; 1024];
    for _ in 0..5 {
        socket.send(packet).map_err(|error| error.to_string())?;
        loop {
            match socket.recv(&mut buffer) {
                Ok(count) if count >= 4 && u16::from_be_bytes([buffer[0], buffer[1]]) == ACK => {
                    if u16::from_be_bytes([buffer[2], buffer[3]]) == expect_block {
                        return Ok(());
                    }
                }
                Ok(count) if count >= 2 && u16::from_be_bytes([buffer[0], buffer[1]]) == ERROR => {
                    return Err("The device cancelled the transfer.".into());
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }
    Err("The device stopped answering.".into())
}

fn serve_read(
    socket: &UdpSocket,
    path: &Path,
    options: &HashMap<String, String>,
) -> Result<u64, String> {
    let mut file = File::open(path).map_err(|_| "File not found")?;
    let size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let block_size = options
        .get("blksize")
        .and_then(|value| value.parse::<usize>().ok())
        .map(|size| size.clamp(8, 65464));
    let mut acknowledged = Vec::new();
    if let Some(size) = block_size {
        acknowledged.push(("blksize".to_string(), size.to_string()));
    }
    if options.contains_key("tsize") {
        acknowledged.push(("tsize".to_string(), size.to_string()));
    }
    if !acknowledged.is_empty() {
        let mut packet = vec![0, OACK as u8];
        for (key, value) in &acknowledged {
            packet.extend_from_slice(key.as_bytes());
            packet.push(0);
            packet.extend_from_slice(value.as_bytes());
            packet.push(0);
        }
        send_and_wait(socket, &packet, 0)?;
    }
    let block_size = block_size.unwrap_or(512);
    let mut block: u16 = 1;
    let mut sent = 0_u64;
    let mut data = vec![0_u8; block_size];
    loop {
        let mut filled = 0;
        while filled < block_size {
            let count = file
                .read(&mut data[filled..])
                .map_err(|error| error.to_string())?;
            if count == 0 {
                break;
            }
            filled += count;
        }
        let mut packet = vec![0, DATA as u8];
        packet.extend_from_slice(&block.to_be_bytes());
        packet.extend_from_slice(&data[..filled]);
        send_and_wait(socket, &packet, block)?;
        sent += filled as u64;
        if filled < block_size {
            return Ok(sent);
        }
        block = block.wrapping_add(1);
    }
}

fn serve_write(
    socket: &UdpSocket,
    path: &Path,
    options: &HashMap<String, String>,
) -> Result<u64, String> {
    let block_size = options
        .get("blksize")
        .and_then(|value| value.parse::<usize>().ok())
        .map(|size| size.clamp(8, 65464));
    let partial = path.with_extension(format!(
        "{}weave-part",
        path.extension()
            .map(|ext| format!("{}.", ext.to_string_lossy()))
            .unwrap_or_default()
    ));
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(&partial)
        .map_err(|_| "Could not create the file")?;
    let result = (|| {
        let first = match block_size {
            Some(size) => {
                let mut packet = vec![0, OACK as u8];
                packet.extend_from_slice(b"blksize\0");
                packet.extend_from_slice(size.to_string().as_bytes());
                packet.push(0);
                packet
            }
            None => vec![0, ACK as u8, 0, 0],
        };
        let block_size = block_size.unwrap_or(512);
        let mut buffer = vec![0_u8; block_size + 4];
        let mut expected: u16 = 1;
        let mut last_ack = first;
        let mut received = 0_u64;
        loop {
            let mut tries = 0;
            let count = loop {
                socket.send(&last_ack).map_err(|error| error.to_string())?;
                match socket.recv(&mut buffer) {
                    Ok(count)
                        if count >= 4 && u16::from_be_bytes([buffer[0], buffer[1]]) == DATA =>
                    {
                        let block = u16::from_be_bytes([buffer[2], buffer[3]]);
                        if block == expected {
                            break count;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => {
                        tries += 1;
                        if tries >= 5 {
                            return Err("The device stopped sending.".to_string());
                        }
                    }
                }
            };
            file.write_all(&buffer[4..count])
                .map_err(|error| error.to_string())?;
            received += (count - 4) as u64;
            last_ack = vec![0, ACK as u8];
            last_ack.extend_from_slice(&expected.to_be_bytes());
            if count - 4 < block_size {
                socket.send(&last_ack).map_err(|error| error.to_string())?;
                return Ok(received);
            }
            expected = expected.wrapping_add(1);
        }
    })();
    match result {
        Ok(received) => {
            file.sync_all().map_err(|error| error.to_string())?;
            if path.exists() {
                let _ = std::fs::remove_file(&partial);
                return Err("A file with that name appeared meanwhile".into());
            }
            std::fs::rename(&partial, path).map_err(|error| error.to_string())?;
            Ok(received)
        }
        Err(error) => {
            let _ = std::fs::remove_file(&partial);
            Err(error)
        }
    }
}

fn handle_tftp(
    root: PathBuf,
    uploads: bool,
    packet: Vec<u8>,
    peer: SocketAddr,
    local_ip: std::net::IpAddr,
    app: Option<AppHandle>,
    id: String,
) {
    let Ok(socket) = UdpSocket::bind((local_ip, 0)) else {
        return;
    };
    if socket.connect(peer).is_err() {
        return;
    }
    socket.set_read_timeout(Some(Duration::from_secs(2))).ok();
    let Some(request) = parse_request(&packet) else {
        let _ = socket.send(&error_packet(4, "Illegal TFTP operation"));
        return;
    };
    let Some(path) = inside(&root, &request.name) else {
        let _ = socket.send(&error_packet(2, "Access violation"));
        log(
            &app,
            &id,
            format!(
                "{peer} asked for {} outside the folder; refused.",
                request.name
            ),
        );
        return;
    };
    let outcome = match request.opcode {
        RRQ if path.is_file() => serve_read(&socket, &path, &request.options)
            .map(|bytes| format!("{peer} downloaded {} ({bytes} bytes).", request.name)),
        RRQ => {
            let _ = socket.send(&error_packet(1, "File not found"));
            Err(format!(
                "{peer} asked for {}, which does not exist.",
                request.name
            ))
        }
        WRQ if !uploads => {
            let _ = socket.send(&error_packet(2, "Uploads are turned off"));
            Err(format!(
                "{peer} tried to upload {}; uploads are off.",
                request.name
            ))
        }
        WRQ if path.exists() => {
            let _ = socket.send(&error_packet(6, "File already exists"));
            Err(format!(
                "{peer} tried to replace {}; refused.",
                request.name
            ))
        }
        WRQ => serve_write(&socket, &path, &request.options)
            .map(|bytes| format!("{peer} uploaded {} ({bytes} bytes).", request.name)),
        _ => {
            let _ = socket.send(&error_packet(4, "Illegal TFTP operation"));
            return;
        }
    };
    match outcome {
        Ok(line) => log(&app, &id, line),
        Err(error) => {
            let _ = socket.send(&error_packet(0, &error));
            log(&app, &id, format!("Transfer with {peer} failed: {error}"));
        }
    }
}

fn run_tftp(
    socket: UdpSocket,
    root: PathBuf,
    uploads: bool,
    stop: Arc<AtomicBool>,
    app: Option<AppHandle>,
    id: String,
) {
    socket
        .set_read_timeout(Some(Duration::from_millis(300)))
        .ok();
    let local_ip = socket
        .local_addr()
        .map(|address| address.ip())
        .unwrap_or([0, 0, 0, 0].into());
    let local_ip = if local_ip.is_unspecified() {
        [0, 0, 0, 0].into()
    } else {
        local_ip
    };
    let mut buffer = [0_u8; 1024];
    while !stop.load(Ordering::Relaxed) {
        if let Ok((count, peer)) = socket.recv_from(&mut buffer) {
            let (root, app, id, packet) = (
                root.clone(),
                app.clone(),
                id.clone(),
                buffer[..count].to_vec(),
            );
            thread::spawn(move || handle_tftp(root, uploads, packet, peer, local_ip, app, id));
        }
    }
}

// HTTP: GET and HEAD, folder listings, byte ranges.

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn url_path(path: &str) -> String {
    path.split('/')
        .map(|part| {
            percent_encoding::utf8_percent_encode(part, percent_encoding::NON_ALPHANUMERIC)
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn respond(
    stream: &mut TcpStream,
    status: &str,
    headers: &[(&str, String)],
    body: &[u8],
    head_only: bool,
) {
    let mut response = format!(
        "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    let _ = stream.write_all(response.as_bytes());
    if !head_only {
        let _ = stream.write_all(body);
    }
}

fn listing(root: &Path, folder: &Path, request_path: &str) -> String {
    let mut entries: Vec<(String, bool, u64)> = std::fs::read_dir(folder)
        .map(|items| {
            items
                .flatten()
                .filter(|item| !item.file_name().to_string_lossy().starts_with('.'))
                .map(|item| {
                    let meta = item.metadata().ok();
                    (
                        item.file_name().to_string_lossy().into_owned(),
                        meta.as_ref().is_some_and(|meta| meta.is_dir()),
                        meta.map_or(0, |meta| meta.len()),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    entries.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then(a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    let base = request_path.trim_end_matches('/');
    let mut rows = String::new();
    if folder != root {
        rows.push_str("<li><a href=\"../\">../</a></li>");
    }
    for (name, is_dir, size) in entries {
        let href = format!(
            "{}/{}{}",
            html_escape(base),
            url_path(&name),
            if is_dir { "/" } else { "" }
        );
        let label = if is_dir {
            format!("{}/", html_escape(&name))
        } else {
            format!("{} <small>{size} bytes</small>", html_escape(&name))
        };
        rows.push_str(&format!("<li><a href=\"{href}\">{label}</a></li>"));
    }
    format!("<!doctype html><meta charset=utf-8><title>Weave · {0}</title><body style=\"font:14px -apple-system,sans-serif;padding:24px\"><h1 style=\"font-size:18px\">{0}</h1><ul>{rows}</ul>", html_escape(if request_path.is_empty() { "/" } else { request_path }))
}

fn mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("txt" | "cfg" | "conf" | "log" | "md") => "text/plain; charset=utf-8",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn handle_http(mut stream: TcpStream, root: PathBuf, app: Option<AppHandle>, id: String) {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let peer = stream
        .peer_addr()
        .map(|address| address.to_string())
        .unwrap_or_default();
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(clone) => clone,
        Err(_) => return,
    });
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    let mut range = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(value) = line
            .to_ascii_lowercase()
            .strip_prefix("range:")
            .map(|value| value.trim().to_string())
        {
            range = value
                .strip_prefix("bytes=")
                .and_then(|value| value.split_once('-'))
                .and_then(|(start, end)| {
                    Some((start.parse::<u64>().ok()?, end.parse::<u64>().ok()))
                });
        }
    }
    let mut parts = first.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
    let head_only = method == "HEAD";
    if method != "GET" && !head_only {
        respond(
            &mut stream,
            "405 Method Not Allowed",
            &[("Allow", "GET, HEAD".into())],
            b"This server is read-only.",
            false,
        );
        return;
    }
    let decoded = percent_encoding::percent_decode_str(target.split('?').next().unwrap_or("/"))
        .decode_utf8_lossy()
        .to_string();
    let Some(path) = inside(&root, &decoded) else {
        respond(
            &mut stream,
            "403 Forbidden",
            &[],
            b"Outside the shared folder.",
            head_only,
        );
        return;
    };
    if path.is_dir() {
        if !decoded.ends_with('/') {
            respond(
                &mut stream,
                "301 Moved Permanently",
                &[("Location", format!("{}/", url_path(&decoded)))],
                b"",
                head_only,
            );
            return;
        }
        let body = listing(&root, &path, &decoded);
        respond(
            &mut stream,
            "200 OK",
            &[("Content-Type", "text/html; charset=utf-8".into())],
            body.as_bytes(),
            head_only,
        );
        return;
    }
    let Ok(mut file) = File::open(&path) else {
        respond(&mut stream, "404 Not Found", &[], b"Not found.", head_only);
        return;
    };
    let size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let (status, start, end) = match range {
        Some((start, end)) if start < size => (
            "206 Partial Content",
            start,
            end.unwrap_or(size - 1).min(size - 1),
        ),
        Some(_) => {
            respond(
                &mut stream,
                "416 Range Not Satisfiable",
                &[("Content-Range", format!("bytes */{size}"))],
                b"",
                head_only,
            );
            return;
        }
        None => ("200 OK", 0, size.saturating_sub(1)),
    };
    let length = if size == 0 { 0 } else { end - start + 1 };
    let mut headers = format!("HTTP/1.1 {status}\r\nConnection: close\r\nContent-Type: {}\r\nAccept-Ranges: bytes\r\nContent-Length: {length}\r\n", mime(&path));
    if status.starts_with("206") {
        headers.push_str(&format!("Content-Range: bytes {start}-{end}/{size}\r\n"));
    }
    headers.push_str("\r\n");
    if stream.write_all(headers.as_bytes()).is_err() || head_only {
        return;
    }
    if file.seek(SeekFrom::Start(start)).is_ok()
        && std::io::copy(&mut file.take(length), &mut stream).is_ok()
    {
        log(&app, &id, format!("{peer} downloaded {decoded}."));
    }
}

fn run_http(
    listener: TcpListener,
    root: PathBuf,
    stop: Arc<AtomicBool>,
    app: Option<AppHandle>,
    id: String,
) {
    listener.set_nonblocking(true).ok();
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).ok();
                let (root, app, id) = (root.clone(), app.clone(), id.clone());
                thread::spawn(move || handle_http(stream, root, app, id));
            }
            Err(_) => thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// This computer's IPv4 addresses on the network, for telling devices where
/// to connect.
fn local_addresses() -> Vec<String> {
    let mut addresses = Vec::new();
    if !cfg!(target_os = "macos") {
        // Connecting a UDP socket sends nothing; it only picks the address
        // the system would use to reach other machines.
        if let Some(address) = std::net::UdpSocket::bind(("0.0.0.0", 0))
            .and_then(|socket| socket.connect(("192.0.2.1", 9)).map(|()| socket))
            .and_then(|socket| socket.local_addr())
            .ok()
            .map(|address| address.ip().to_string())
            .filter(|address| !address.starts_with("127.") && address != "0.0.0.0")
        {
            addresses.push(address);
        }
        return addresses;
    }
    if let Ok(output) = crate::platform::command("/sbin/ifconfig").output() {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("inet ") {
                if let Some(address) = rest
                    .split_whitespace()
                    .next()
                    .filter(|address| !address.starts_with("127."))
                {
                    addresses.push(address.to_string());
                }
            }
        }
    }
    addresses
}

pub(crate) fn start(
    kind: &str,
    folder: &str,
    port: u16,
    everyone: bool,
    uploads: bool,
    app: Option<AppHandle>,
) -> Result<ServerInfo, String> {
    let root = dunce::canonicalize(folder).map_err(|_| "Choose a folder that exists.")?;
    if !root.is_dir() {
        return Err("Choose a folder to share.".into());
    }
    let ip = if everyone { "0.0.0.0" } else { "127.0.0.1" };
    let id = uuid::Uuid::new_v4().simple().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let in_use = |error: std::io::Error| match error.kind() {
        std::io::ErrorKind::AddrInUse => {
            format!("Port {port} is already in use by another app. Choose another port.")
        }
        std::io::ErrorKind::PermissionDenied => {
            format!("The system did not allow port {port}. Choose a port above 1024.")
        }
        _ => format!("Could not start the server: {error}"),
    };
    let actual_port = match kind {
        "tftp" => {
            let socket = UdpSocket::bind((ip, port)).map_err(in_use)?;
            let actual = socket.local_addr().map_or(port, |address| address.port());
            let (root, stop, app, id) = (root.clone(), stop.clone(), app.clone(), id.clone());
            thread::spawn(move || run_tftp(socket, root, uploads, stop, app, id));
            actual
        }
        "http" => {
            let listener = TcpListener::bind((ip, port)).map_err(in_use)?;
            let actual = listener.local_addr().map_or(port, |address| address.port());
            let (root, stop, app, id) = (root.clone(), stop.clone(), app.clone(), id.clone());
            thread::spawn(move || run_http(listener, root, stop, app, id));
            actual
        }
        _ => return Err("Choose TFTP or HTTP.".into()),
    };
    let info = ServerInfo {
        id: id.clone(),
        kind: kind.into(),
        folder: root.to_string_lossy().into_owned(),
        port: actual_port,
        everyone,
        uploads: uploads && kind == "tftp",
        addresses: if everyone {
            local_addresses()
        } else {
            vec!["127.0.0.1".into()]
        },
    };
    servers()
        .lock()
        .map_err(|_| "Servers are busy. Try again.")?
        .insert(
            id,
            Running {
                info: info.clone(),
                stop,
            },
        );
    Ok(info)
}

#[tauri::command]
pub fn file_server_start(
    app: AppHandle,
    kind: String,
    folder: String,
    port: u16,
    everyone: bool,
    uploads: bool,
) -> Result<ServerInfo, String> {
    start(&kind, &folder, port, everyone, uploads, Some(app))
}

#[tauri::command]
pub fn file_server_stop(id: String) -> Result<(), String> {
    if let Some(server) = servers()
        .lock()
        .map_err(|_| "Servers are busy. Try again.")?
        .remove(&id)
    {
        server.stop.store(true, Ordering::Relaxed);
    }
    Ok(())
}

#[tauri::command]
pub fn file_servers_list() -> Result<Vec<ServerInfo>, String> {
    Ok(servers()
        .lock()
        .map_err(|_| "Servers are busy. Try again.")?
        .values()
        .map(|server| server.info.clone())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tftp_request(opcode: u16, name: &str, options: &[(&str, &str)]) -> Vec<u8> {
        let mut packet = opcode.to_be_bytes().to_vec();
        for part in [name, "octet"]
            .into_iter()
            .chain(options.iter().flat_map(|(key, value)| [*key, *value]))
        {
            packet.extend_from_slice(part.as_bytes());
            packet.push(0);
        }
        packet
    }

    fn download(
        server: SocketAddr,
        name: &str,
        options: &[(&str, &str)],
    ) -> Result<Vec<u8>, String> {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket
            .send_to(&tftp_request(RRQ, name, options), server)
            .unwrap();
        let mut data = Vec::new();
        let mut buffer = [0_u8; 2048];
        loop {
            let (count, from) = socket.recv_from(&mut buffer).unwrap();
            match u16::from_be_bytes([buffer[0], buffer[1]]) {
                OACK => {
                    socket.send_to(&[0, ACK as u8, 0, 0], from).unwrap();
                }
                DATA => {
                    data.extend_from_slice(&buffer[4..count]);
                    socket
                        .send_to(&[0, ACK as u8, buffer[2], buffer[3]], from)
                        .unwrap();
                    let block_size = options
                        .iter()
                        .find(|(key, _)| *key == "blksize")
                        .map_or(512, |(_, value)| value.parse().unwrap());
                    if count - 4 < block_size {
                        return Ok(data);
                    }
                }
                _ => return Err(String::from_utf8_lossy(&buffer[4..count - 1]).to_string()),
            }
        }
    }

    #[test]
    fn serves_and_receives_files_over_tftp() {
        let folder = tempfile::tempdir().unwrap();
        let firmware: Vec<u8> = (0..1500).map(|index| (index % 251) as u8).collect();
        std::fs::write(folder.path().join("fw.bin"), &firmware).unwrap();
        std::fs::write(folder.path().join("exact.bin"), vec![7_u8; 1024]).unwrap();
        let info = start(
            "tftp",
            folder.path().to_str().unwrap(),
            0,
            false,
            true,
            None,
        )
        .unwrap();
        let server: SocketAddr = format!("127.0.0.1:{}", info.port).parse().unwrap();
        assert_eq!(download(server, "fw.bin", &[]).unwrap(), firmware);
        assert_eq!(
            download(server, "/fw.bin", &[("blksize", "1428"), ("tsize", "0")]).unwrap(),
            firmware
        );
        assert_eq!(download(server, "exact.bin", &[]).unwrap().len(), 1024);
        assert!(download(server, "../etc/passwd", &[])
            .unwrap_err()
            .contains("Access violation"));
        assert!(download(server, "missing.bin", &[])
            .unwrap_err()
            .contains("not found"));

        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket
            .send_to(&tftp_request(WRQ, "backup.cfg", &[]), server)
            .unwrap();
        let mut buffer = [0_u8; 600];
        let (_, from) = socket.recv_from(&mut buffer).unwrap();
        assert_eq!(&buffer[..4], &[0, ACK as u8, 0, 0]);
        socket
            .send_to(
                &[&[0, DATA as u8, 0, 1][..], b"hostname switch1\n"].concat(),
                from,
            )
            .unwrap();
        socket.recv_from(&mut buffer).unwrap();
        let saved = folder.path().join("backup.cfg");
        for _ in 0..50 {
            if std::fs::read_to_string(&saved).is_ok_and(|text| !text.is_empty()) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(
            std::fs::read_to_string(&saved).unwrap(),
            "hostname switch1\n"
        );
        socket
            .send_to(&tftp_request(WRQ, "backup.cfg", &[]), server)
            .unwrap();
        let (count, _) = socket.recv_from(&mut buffer).unwrap();
        assert!(String::from_utf8_lossy(&buffer[..count]).contains("already exists"));
        file_server_stop(info.id).unwrap();
    }

    #[test]
    fn serves_a_read_only_folder_over_http() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::create_dir(folder.path().join("sub dir")).unwrap();
        std::fs::write(folder.path().join("sub dir/notes.txt"), "0123456789").unwrap();
        // Windows does not allow these characters in file names.
        let hostile_name = cfg!(unix);
        if hostile_name {
            std::fs::create_dir(folder.path().join("a\"><b>x")).unwrap();
        }
        let info = start(
            "http",
            folder.path().to_str().unwrap(),
            0,
            false,
            false,
            None,
        )
        .unwrap();
        let get = |request: &str| {
            let mut stream = TcpStream::connect(("127.0.0.1", info.port)).unwrap();
            stream.write_all(request.as_bytes()).unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        };
        let index = get("GET / HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(index.starts_with("HTTP/1.1 200") && index.contains("href=\"/sub%20dir/\""));
        assert!(get("GET /sub%20dir HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 301"));
        let part = get("GET /sub%20dir/notes.txt HTTP/1.1\r\nRange: bytes=4-\r\n\r\n");
        assert!(part.starts_with("HTTP/1.1 206") && part.ends_with("456789"));
        assert!(get("GET /../../etc/passwd HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 403"));
        if hostile_name {
            let hostile = get("GET /a%22%3E%3Cb%3Ex/ HTTP/1.1\r\n\r\n");
            assert!(hostile.starts_with("HTTP/1.1 200"), "{hostile}");
            assert!(!hostile.contains("<b>"), "{hostile}");
        }
        assert!(get("PUT /x HTTP/1.1\r\nContent-Length: 0\r\n\r\n").starts_with("HTTP/1.1 405"));
        file_server_stop(info.id).unwrap();
        assert!(inside(Path::new("/srv/files"), "a/../../b").is_none());
    }
}
