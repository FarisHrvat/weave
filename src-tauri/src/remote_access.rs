//! Terminal connections beyond SSH: Telnet, serial consoles, Mosh and X11
//! forwarding run inside Weave terminals, while VNC and RDP hand off to the
//! system's remote desktop apps.
//!
//! Telnet and serial sessions run this executable again in a helper mode
//! (`--weave-telnet` / `--weave-serial`) inside the terminal's pseudo-terminal,
//! so they reuse the normal tab, split, recording and broadcast features.

use crate::{profiles, ssh, terminal, AppState};
use portable_pty::CommandBuilder;
use serde::Deserialize;
use std::{
    collections::HashSet,
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    time::Duration,
};
use tauri::{AppHandle, State};

const TELNET_FLAG: &str = "--weave-telnet";
const SERIAL_FLAG: &str = "--weave-serial";
pub const SERIAL_BAUD_RATES: [u32; 12] = [
    300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600,
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionSpec {
    pub kind: String,
    pub profile_id: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub device: Option<String>,
    pub baud: Option<u32>,
    pub command: Option<String>,
}

pub(crate) fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with('-')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']'))
}

fn valid_username(user: &str) -> bool {
    user.len() <= 128
        && !user.starts_with('-')
        && user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@' | '\\'))
}

fn require_host(spec: &ConnectionSpec) -> Result<(String, u16), String> {
    let host = spec.host.as_deref().unwrap_or("").trim().to_string();
    if !valid_host(&host) {
        return Err("Enter a host name or IP address without spaces.".into());
    }
    let port = spec.port.unwrap_or(0);
    if port == 0 {
        return Err("Enter a port between 1 and 65535.".into());
    }
    Ok((host, port))
}

pub(crate) fn valid_serial_device(device: &str) -> bool {
    if device.len() > 128 {
        return false;
    }
    if let Some(number) = device
        .strip_prefix("COM")
        .or_else(|| device.strip_prefix(r"\\.\COM"))
    {
        return cfg!(windows)
            && !number.is_empty()
            && number.len() <= 3
            && number.chars().all(|c| c.is_ascii_digit());
    }
    let Some(name) = device.strip_prefix("/dev/") else {
        return false;
    };
    let allowed = if cfg!(target_os = "macos") {
        ["cu.", "tty."].as_slice()
    } else {
        [
            "ttyUSB",
            "ttyACM",
            "ttyS",
            "ttyAMA",
            "rfcomm",
            "serial/by-id/",
        ]
        .as_slice()
    };
    allowed.iter().any(|prefix| name.starts_with(prefix))
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '/'))
}

/// Finds a command-line tool even when Weave was opened from the Dock or
/// Start menu, where `PATH` is often short.
pub(crate) fn find_tool(name: &str) -> Option<PathBuf> {
    crate::platform::find_tool(name)
}

fn tool_path_env(tool: &Path) -> String {
    let mut dirs: Vec<PathBuf> = tool.parent().map(Path::to_path_buf).into_iter().collect();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.extend(crate::platform::extra_tool_dirs());
    std::env::join_paths(dirs)
        .map(|joined| joined.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn expand_key(key: &str) -> Result<String, String> {
    Ok(match key.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .ok_or("Home folder is unavailable; use an absolute SSH key path.")?
            .join(rest)
            .to_string_lossy()
            .into_owned(),
        None => key.to_string(),
    })
}

fn ssh_profile(state: &AppState, spec: &ConnectionSpec) -> Result<profiles::Profile, String> {
    let id = spec
        .profile_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or("Choose a saved SSH host for this connection.")?;
    let profile = profiles::get_profile(state, id)?;
    if profile.protocol != "ssh" {
        return Err("Mosh and X11 sessions need an SSH host profile.".into());
    }
    Ok(profile)
}

/// Builds the `mosh` command for an SSH profile. Mosh splits `--ssh` on
/// spaces, so key and jump host values that contain whitespace are refused.
pub(crate) fn mosh_args(profile: &profiles::Profile) -> Result<Vec<String>, String> {
    let mut ssh = format!("ssh -p {}", profile.port);
    if let Some(key) = profile.key_path.as_deref().filter(|key| !key.is_empty()) {
        let key = expand_key(key)?;
        if key
            .chars()
            .any(|c| c.is_whitespace() || c == '\'' || c == '"')
        {
            return Err("Mosh cannot use an SSH key path that contains spaces or quotes.".into());
        }
        ssh.push_str(&format!(" -i {key}"));
    }
    if let Some(jump) = profile.jump_host.as_deref().filter(|jump| !jump.is_empty()) {
        if jump
            .chars()
            .any(|c| c.is_whitespace() || c == '\'' || c == '"')
        {
            return Err("Mosh cannot use a jump host that contains spaces or quotes.".into());
        }
        ssh.push_str(&format!(" -J {jump}"));
    }
    if !valid_username(&profile.username) || !valid_host(&profile.host) {
        return Err("This SSH host has a user name or host that Mosh cannot use.".into());
    }
    Ok(vec![
        format!("--ssh={ssh}"),
        "--".into(),
        format!("{}@{}", profile.username, profile.host),
    ])
}

/// `xauth`, which `ssh -Y` needs to hand the remote program a display.
/// macOS gets it from XQuartz; Windows X servers work without it.
fn xauth() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        ["/opt/X11/bin/xauth", "/usr/X11/bin/xauth"]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.is_file())
    } else {
        find_tool("xauth")
    }
}

/// Whether an X server is ready to show forwarded windows.
fn x_server_ready() -> bool {
    if cfg!(windows) {
        std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], 6000)),
            Duration::from_millis(300),
        )
        .is_ok()
    } else {
        xauth().is_some()
    }
}

fn x_display() -> Option<String> {
    if let Ok(display) = std::env::var("DISPLAY") {
        return Some(display);
    }
    #[cfg(target_os = "macos")]
    {
        let output = crate::platform::command("/bin/launchctl")
            .args(["getenv", "DISPLAY"])
            .output()
            .ok()?;
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !value.is_empty() {
            return Some(value);
        }
    }
    cfg!(windows).then(|| "127.0.0.1:0.0".to_string())
}

pub(crate) fn x11_args(
    profile: &profiles::Profile,
    control_path: &Path,
    xauth: Option<&Path>,
    remote_command: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut args = vec!["-tt".to_string(), "-Y".into()];
    if let Some(xauth) = xauth {
        args.extend(["-o".into(), format!("XAuthLocation={}", xauth.display())]);
    }
    args.extend(ssh::sharing_args("auto", "300", control_path));
    args.extend(["-p".into(), profile.port.to_string()]);
    if let Some(key) = profile.key_path.as_deref().filter(|key| !key.is_empty()) {
        args.push("-i".into());
        args.push(expand_key(key)?);
    }
    if let Some(jump) = profile.jump_host.as_deref().filter(|jump| !jump.is_empty()) {
        args.push("-J".into());
        args.push(jump.into());
    }
    args.push("-l".into());
    args.push(profile.username.clone());
    args.push(profile.host.clone());
    if let Some(command) = remote_command.map(str::trim).filter(|c| !c.is_empty()) {
        if command.len() > 1024 || command.chars().any(char::is_control) {
            return Err("Keep the X11 command on one line and under 1024 characters.".into());
        }
        args.push(command.into());
    }
    Ok(args)
}

fn connection_command(state: &AppState, spec: &ConnectionSpec) -> Result<CommandBuilder, String> {
    match spec.kind.as_str() {
        "telnet" => {
            let (host, port) = require_host(spec)?;
            let exe = std::env::current_exe()
                .map_err(|error| format!("Could not locate Weave for Telnet: {error}"))?;
            let mut command = CommandBuilder::new(exe);
            command.args([TELNET_FLAG, &host, &port.to_string()]);
            Ok(command)
        }
        "serial" => {
            let device = spec.device.as_deref().unwrap_or("").trim();
            if !valid_serial_device(device) {
                return Err(format!(
                    "Choose a serial device such as {}.",
                    if cfg!(windows) {
                        "COM3"
                    } else if cfg!(target_os = "macos") {
                        "/dev/cu.usbserial-0001"
                    } else {
                        "/dev/ttyUSB0"
                    }
                ));
            }
            if !cfg!(windows) && !Path::new(device).exists() {
                return Err(format!(
                    "{device} is not connected. Plug in the adapter and refresh the device list."
                ));
            }
            let baud = spec.baud.unwrap_or(115200);
            if !SERIAL_BAUD_RATES.contains(&baud) {
                return Err("Choose a standard serial speed between 300 and 921600 baud.".into());
            }
            let exe = std::env::current_exe()
                .map_err(|error| format!("Could not locate Weave for serial: {error}"))?;
            let mut command = CommandBuilder::new(exe);
            command.args([SERIAL_FLAG, device, &baud.to_string()]);
            Ok(command)
        }
        "mosh" => {
            let profile = ssh_profile(state, spec)?;
            let mosh = find_tool("mosh").ok_or(if cfg!(windows) {
                "Mosh has no Windows version. Use SSH, or run Mosh inside WSL."
            } else if cfg!(target_os = "macos") {
                "Mosh is not installed. Install it with `brew install mosh`, and install mosh-server on the remote host."
            } else {
                "Mosh is not installed. Install the mosh package, and install mosh-server on the remote host."
            })?;
            let mut command = CommandBuilder::new(&mosh);
            command.args(mosh_args(&profile)?);
            command.env("PATH", tool_path_env(&mosh));
            command.env("LANG", "en_US.UTF-8");
            Ok(command)
        }
        "x11" => {
            let profile = ssh_profile(state, spec)?;
            if !x_server_ready() {
                return Err(if cfg!(windows) {
                    "X11 forwarding needs an X server. Start VcXsrv or Xming (with access control off), then try again."
                } else if cfg!(target_os = "macos") {
                    "X11 forwarding needs XQuartz. Install it from xquartz.org, sign out and back in, then try again."
                } else {
                    "X11 forwarding needs xauth. Install the xauth package, then try again."
                }
                .into());
            }
            let xauth = xauth();
            let mut command = CommandBuilder::new(
                crate::platform::system_tool("ssh").unwrap_or_else(|| "ssh".into()),
            );
            command.args(x11_args(
                &profile,
                &ssh::control_path(&profile)?,
                xauth.as_deref(),
                spec.command.as_deref(),
            )?);
            if let Some(display) = x_display() {
                command.env("DISPLAY", display);
            }
            Ok(command)
        }
        _ => Err("Choose Telnet, Serial, Mosh, or X11 for a terminal connection.".into()),
    }
}

#[tauri::command]
pub fn term_open_connection(
    app: AppHandle,
    state: State<'_, AppState>,
    connection: ConnectionSpec,
    cols: u16,
    rows: u16,
) -> Result<String, String> {
    let command = connection_command(&state, &connection)?;
    terminal::spawn_session(app, state, command, cols, rows)
}

#[tauri::command]
pub fn serial_devices() -> Result<Vec<String>, String> {
    let mut devices: Vec<String> = serialport::available_ports()
        .map_err(|error| format!("Could not list serial devices: {error}"))?
        .into_iter()
        .map(|port| port.port_name)
        // macOS lists each port twice; the call-out (cu) device is the one to use.
        .filter(|name| !(cfg!(target_os = "macos") && name.starts_with("/dev/tty.")))
        .filter(|name| valid_serial_device(name))
        .collect();
    devices.sort();
    devices.dedup();
    Ok(devices)
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteToolStatus {
    mosh: bool,
    xquartz: bool,
    rdp_app: Option<String>,
}

/// The app that opens RDP connections here, if one is installed.
fn rdp_app() -> Option<(PathBuf, &'static str)> {
    if cfg!(windows) {
        let system = std::env::var_os("SystemRoot")
            .map(|root| PathBuf::from(root).join("System32").join("mstsc.exe"))
            .filter(|path| path.is_file());
        return system.map(|path| (path, "Remote Desktop Connection"));
    }
    if cfg!(target_os = "macos") {
        return [
            ("/Applications/Windows App.app", "Windows App"),
            (
                "/Applications/Microsoft Remote Desktop.app",
                "Microsoft Remote Desktop",
            ),
        ]
        .into_iter()
        .find(|(app, _)| Path::new(app).is_dir())
        .map(|(app, name)| (PathBuf::from(app), name));
    }
    [
        ("xfreerdp3", "FreeRDP"),
        ("xfreerdp", "FreeRDP"),
        ("remmina", "Remmina"),
    ]
    .into_iter()
    .find_map(|(tool, name)| find_tool(tool).map(|path| (path, name)))
}

#[tauri::command]
pub fn remote_tools_status() -> RemoteToolStatus {
    RemoteToolStatus {
        mosh: find_tool("mosh").is_some(),
        xquartz: x_server_ready(),
        rdp_app: rdp_app().map(|(_, name)| name.to_string()),
    }
}

pub(crate) fn vnc_url(host: &str, port: u16, username: Option<&str>) -> Result<String, String> {
    if !valid_host(host) {
        return Err("Enter a host name or IP address without spaces.".into());
    }
    let user = match username.map(str::trim).filter(|user| !user.is_empty()) {
        Some(user) if valid_username(user) => format!(
            "{}@",
            percent_encoding::utf8_percent_encode(user, percent_encoding::NON_ALPHANUMERIC)
        ),
        Some(_) => return Err("The user name contains unsupported characters.".into()),
        None => String::new(),
    };
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    Ok(format!("vnc://{user}{host}:{port}"))
}

pub(crate) fn rdp_file(host: &str, port: u16, username: Option<&str>) -> Result<String, String> {
    if !valid_host(host) {
        return Err("Enter a host name or IP address without spaces.".into());
    }
    let mut content = format!("full address:s:{host}:{port}\nprompt for credentials:i:1\n");
    if let Some(user) = username.map(str::trim).filter(|user| !user.is_empty()) {
        if !valid_username(user) {
            return Err("The user name contains unsupported characters.".into());
        }
        content.push_str(&format!("username:s:{user}\n"));
    }
    Ok(content)
}

#[tauri::command]
pub fn remote_desktop_open(
    kind: String,
    host: String,
    port: u16,
    username: Option<String>,
) -> Result<String, String> {
    if port == 0 {
        return Err("Enter a port between 1 and 65535.".into());
    }
    let host = host.trim();
    match kind.as_str() {
        "vnc" => {
            let url = vnc_url(host, port, username.as_deref())?;
            crate::platform::open_url(&url).map_err(|_| {
                if cfg!(target_os = "macos") {
                    "macOS could not open Screen Sharing for this address.".to_string()
                } else {
                    "No VNC viewer is set up to open vnc:// links. Install one such as TigerVNC or RealVNC Viewer, or use Weave's built-in viewer.".to_string()
                }
            })?;
            Ok(if cfg!(target_os = "macos") {
                "Opened in Screen Sharing.".into()
            } else {
                "Opened in your VNC viewer.".into()
            })
        }
        "rdp" => {
            let (app, name) = rdp_app().ok_or(if cfg!(target_os = "macos") {
                "RDP needs Microsoft's Windows App (formerly Microsoft Remote Desktop). Install it from the Mac App Store, then try again."
            } else if cfg!(windows) {
                "Remote Desktop Connection (mstsc) is missing from this PC."
            } else {
                "RDP needs FreeRDP or Remmina. Install the freerdp or remmina package, then try again."
            })?;
            let content = rdp_file(host, port, username.as_deref())?;
            let path = std::env::temp_dir().join(format!("weave-{}.rdp", uuid::Uuid::new_v4()));
            std::fs::write(&path, content)
                .map_err(|error| format!("Could not prepare the RDP connection: {error}"))?;
            let mut command = if cfg!(target_os = "macos") {
                let mut command = crate::platform::command("/usr/bin/open");
                command.arg("-a").arg(&app).arg(&path);
                command
            } else if app
                .file_stem()
                .is_some_and(|stem| stem.to_string_lossy().starts_with("xfreerdp"))
            {
                let mut command = crate::platform::command(&app);
                let address = if host.contains(':') && !host.starts_with('[') {
                    format!("[{host}]:{port}")
                } else {
                    format!("{host}:{port}")
                };
                command.arg(format!("/v:{address}"));
                if let Some(user) = username
                    .as_deref()
                    .map(str::trim)
                    .filter(|user| !user.is_empty())
                {
                    command.arg(format!("/u:{user}"));
                }
                command
            } else {
                let mut command = crate::platform::command(&app);
                command.arg(&path);
                command
            };
            command
                .spawn()
                .map_err(|error| format!("Could not open {name}: {error}"))?;
            Ok(format!("Opened in {name}."))
        }
        _ => Err("Choose VNC or RDP.".into()),
    }
}

// ---------------------------------------------------------------------------
// Telnet protocol handling

const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;
const OPT_ECHO: u8 = 1;
const OPT_SGA: u8 = 3;
const OPT_TTYPE: u8 = 24;
const OPT_NAWS: u8 = 31;

#[derive(Clone, Copy, PartialEq)]
enum TelnetState {
    Data,
    Iac,
    Verb(u8),
    Sub,
    SubIac,
}

/// Minimal Telnet (RFC 854) state machine: strips protocol commands from the
/// server stream and answers option negotiation. Weave accepts server echo
/// and suppress-go-ahead, reports its terminal type and window size, and
/// refuses everything else.
pub(crate) struct TelnetParser {
    state: TelnetState,
    sub: Vec<u8>,
    answered: HashSet<(u8, u8)>,
    pub window: (u16, u16),
}

impl TelnetParser {
    pub fn new(window: (u16, u16)) -> Self {
        Self {
            state: TelnetState::Data,
            sub: Vec::new(),
            answered: HashSet::new(),
            window,
        }
    }

    pub fn naws_enabled(&self) -> bool {
        self.answered.contains(&(DO, OPT_NAWS))
    }

    pub fn naws(&self) -> Vec<u8> {
        let (cols, rows) = self.window;
        let mut out = vec![IAC, SB, OPT_NAWS];
        for byte in [cols.to_be_bytes(), rows.to_be_bytes()].concat() {
            out.push(byte);
            if byte == IAC {
                out.push(IAC);
            }
        }
        out.extend([IAC, SE]);
        out
    }

    fn reply(&mut self, verb: u8, option: u8, replies: &mut Vec<u8>) {
        let answer = match verb {
            DO if option == OPT_TTYPE || option == OPT_NAWS || option == OPT_SGA => WILL,
            DO => WONT,
            WILL if option == OPT_ECHO || option == OPT_SGA => DO,
            WILL => DONT,
            _ => return,
        };
        if self.answered.insert((verb, option)) {
            replies.extend([IAC, answer, option]);
            if verb == DO && option == OPT_NAWS {
                replies.extend(self.naws());
            }
        }
    }

    /// Returns (terminal data, bytes to send back to the server).
    pub fn feed(&mut self, input: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let mut data = Vec::with_capacity(input.len());
        let mut replies = Vec::new();
        for &byte in input {
            self.state = match self.state {
                TelnetState::Data if byte == IAC => TelnetState::Iac,
                TelnetState::Data => {
                    data.push(byte);
                    TelnetState::Data
                }
                TelnetState::Iac => match byte {
                    IAC => {
                        data.push(IAC);
                        TelnetState::Data
                    }
                    DO | DONT | WILL | WONT => TelnetState::Verb(byte),
                    SB => {
                        self.sub.clear();
                        TelnetState::Sub
                    }
                    _ => TelnetState::Data,
                },
                TelnetState::Verb(verb) => {
                    self.reply(verb, byte, &mut replies);
                    TelnetState::Data
                }
                TelnetState::Sub if byte == IAC => TelnetState::SubIac,
                TelnetState::Sub => {
                    if self.sub.len() < 512 {
                        self.sub.push(byte);
                    }
                    TelnetState::Sub
                }
                TelnetState::SubIac if byte == SE => {
                    // TTYPE SEND (1) → TTYPE IS (0) "XTERM-256COLOR"
                    if self.sub.first() == Some(&OPT_TTYPE) && self.sub.get(1) == Some(&1) {
                        replies.extend([IAC, SB, OPT_TTYPE, 0]);
                        replies.extend(b"XTERM-256COLOR");
                        replies.extend([IAC, SE]);
                    }
                    TelnetState::Data
                }
                TelnetState::SubIac => {
                    if byte == IAC && self.sub.len() < 512 {
                        self.sub.push(IAC);
                    }
                    TelnetState::Sub
                }
            };
        }
        (data, replies)
    }
}

/// Encodes keyboard input for the Telnet stream: IAC bytes are doubled and
/// Enter (a bare CR from the terminal) is sent as CR LF.
pub(crate) fn telnet_encode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() + 4);
    for &byte in input {
        match byte {
            IAC => out.extend([IAC, IAC]),
            b'\r' => out.extend(b"\r\n"),
            _ => out.push(byte),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Helper process mode

/// Runs Telnet or serial helper mode when Weave is started with one of the
/// helper flags. Returns `false` for a normal app launch.
pub fn serve_helper() -> bool {
    let args: Vec<String> = std::env::args().collect();
    let code = match args.get(1).map(String::as_str) {
        Some(TELNET_FLAG) if args.len() == 4 => run_telnet(&args[2], &args[3]),
        Some(SERIAL_FLAG) if args.len() == 4 => run_serial(&args[2], &args[3]),
        Some(TELNET_FLAG | SERIAL_FLAG) => Err("Invalid helper arguments.".into()),
        _ => return false,
    };
    if let Err(message) = code {
        let mut stdout = std::io::stdout();
        let _ = write!(stdout, "\r\n\x1b[31m{message}\x1b[0m\r\n");
        let _ = stdout.flush();
        std::thread::sleep(Duration::from_millis(200));
        std::process::exit(1);
    }
    std::process::exit(0);
}

fn set_raw() {
    let _ = crossterm::terminal::enable_raw_mode();
}

fn window_size() -> (u16, u16) {
    match crossterm::terminal::size() {
        Ok((cols, rows)) if cols > 0 => (cols, rows),
        _ => (80, 24),
    }
}

fn run_telnet(host: &str, port: &str) -> Result<(), String> {
    let port: u16 = port.parse().map_err(|_| "Invalid Telnet port.")?;
    if !valid_host(host) {
        return Err("Invalid Telnet host.".into());
    }
    set_raw();
    print!("Connecting to {host}:{port}…\r\n");
    let _ = std::io::stdout().flush();
    let address = (host.trim_matches(|c| c == '[' || c == ']'), port)
        .to_socket_addrs()
        .map_err(|error| format!("Could not resolve {host}: {error}"))?
        .next()
        .ok_or_else(|| format!("Could not resolve {host}."))?;
    let stream = TcpStream::connect_timeout(&address, Duration::from_secs(15))
        .map_err(|error| format!("Could not connect to {host}:{port}: {error}"))?;
    let _ = stream.set_nodelay(true);
    print!("Connected. Close the tab to disconnect.\r\n");
    let _ = std::io::stdout().flush();

    let socket = std::sync::Arc::new(std::sync::Mutex::new(
        stream
            .try_clone()
            .map_err(|error| format!("Could not start Telnet: {error}"))?,
    ));
    let parser = std::sync::Arc::new(std::sync::Mutex::new(TelnetParser::new(window_size())));
    let replies = socket.clone();
    let output_parser = parser.clone();
    let mut reader = stream;
    let output = std::thread::spawn(move || {
        let mut stdout = std::io::stdout();
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let Ok(mut parser) = output_parser.lock() else {
                        break;
                    };
                    let (data, answer) = parser.feed(&buffer[..count]);
                    drop(parser);
                    if !answer.is_empty() {
                        if let Ok(mut socket) = replies.lock() {
                            let _ = socket.write_all(&answer);
                        }
                    }
                    if stdout
                        .write_all(&data)
                        .and_then(|()| stdout.flush())
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    });
    // Report terminal resizes to servers that accepted window-size updates.
    let resize_socket = socket.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(300));
        let Ok(mut parser) = parser.lock() else { break };
        let size = window_size();
        if parser.naws_enabled() && parser.window != size {
            parser.window = size;
            let update = parser.naws();
            drop(parser);
            let Ok(mut socket) = resize_socket.lock() else {
                break;
            };
            if socket.write_all(&update).is_err() {
                break;
            }
        }
    });
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buffer = [0_u8; 4096];
        loop {
            let count = match stdin.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => count,
            };
            let encoded = telnet_encode(&buffer[..count]);
            let Ok(mut socket) = socket.lock() else { break };
            if socket.write_all(&encoded).is_err() {
                break;
            }
        }
        if let Ok(socket) = socket.lock() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
    });
    let _ = output.join();
    print!("\r\nConnection closed by {host}.\r\n");
    let _ = std::io::stdout().flush();
    Ok(())
}

/// An open serial port. Unix systems configure it with termios, which also
/// works with the pseudo-terminals the tests use; Windows uses COM ports.
#[cfg(unix)]
type SerialPort = std::fs::File;
#[cfg(windows)]
type SerialPort = Box<dyn serialport::SerialPort>;

#[cfg(unix)]
fn open_serial(device: &str, baud: u32) -> Result<SerialPort, String> {
    use std::os::fd::FromRawFd;
    let path = std::ffi::CString::new(device).map_err(|_| "Invalid serial device.")?;
    unsafe {
        let fd = libc::open(
            path.as_ptr(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK,
        );
        if fd < 0 {
            return Err(format!(
                "Could not open {device}: {}. Close other apps using the port and try again.",
                std::io::Error::last_os_error()
            ));
        }
        let file = std::fs::File::from_raw_fd(fd);
        let flags = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
        let mut attrs: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut attrs) != 0 {
            return Err(format!("{device} is not a serial port."));
        }
        libc::cfmakeraw(&mut attrs);
        attrs.c_cflag |= libc::CLOCAL | libc::CREAD;
        attrs.c_cflag &= !(libc::CSTOPB | libc::PARENB | libc::CRTSCTS);
        attrs.c_cflag = (attrs.c_cflag & !libc::CSIZE) | libc::CS8;
        attrs.c_cc[libc::VMIN] = 1;
        attrs.c_cc[libc::VTIME] = 0;
        let Some(speed) = termios_speed(baud) else {
            return Err(format!("{device} does not support {baud} baud."));
        };
        if libc::cfsetispeed(&mut attrs, speed) != 0
            || libc::cfsetospeed(&mut attrs, speed) != 0
            || libc::tcsetattr(fd, libc::TCSANOW, &attrs) != 0
        {
            return Err(format!("{device} does not support {baud} baud."));
        }
        Ok(file)
    }
}

/// macOS takes the speed as a number; Linux needs its B-constants.
#[cfg(unix)]
fn termios_speed(baud: u32) -> Option<libc::speed_t> {
    #[cfg(target_os = "macos")]
    {
        Some(baud as libc::speed_t)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Some(match baud {
            300 => libc::B300,
            1200 => libc::B1200,
            2400 => libc::B2400,
            4800 => libc::B4800,
            9600 => libc::B9600,
            19200 => libc::B19200,
            38400 => libc::B38400,
            57600 => libc::B57600,
            115200 => libc::B115200,
            230400 => libc::B230400,
            460800 => libc::B460800,
            921600 => libc::B921600,
            _ => return None,
        })
    }
}

#[cfg(windows)]
fn open_serial(device: &str, baud: u32) -> Result<SerialPort, String> {
    serialport::new(device, baud)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        .timeout(Duration::from_secs(3600))
        .open()
        .map_err(|error| match error.kind() {
            serialport::ErrorKind::NoDevice => format!("{device} is not connected."),
            serialport::ErrorKind::InvalidInput => {
                format!("{device} does not support {baud} baud.")
            }
            _ => format!(
                "Could not open {device}: {error}. Close other apps using the port and try again."
            ),
        })
}

fn run_serial(device: &str, baud: &str) -> Result<(), String> {
    let baud: u32 = baud.parse().map_err(|_| "Invalid serial speed.")?;
    if !valid_serial_device(device) || !SERIAL_BAUD_RATES.contains(&baud) {
        return Err("Invalid serial device or speed.".into());
    }
    let port = open_serial(device, baud)?;
    set_raw();
    print!("Connected to {device} at {baud} baud, 8N1. Close the tab to disconnect.\r\n");
    let _ = std::io::stdout().flush();
    let mut reader = port
        .try_clone()
        .map_err(|error| format!("Could not start serial session: {error}"))?;
    let mut writer = port;
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buffer = [0_u8; 4096];
        loop {
            match stdin.read(&mut buffer) {
                Ok(0) | Err(_) => std::process::exit(0),
                Ok(count) => {
                    if writer.write_all(&buffer[..count]).is_err() {
                        std::process::exit(0);
                    }
                }
            }
        }
    });
    let mut stdout = std::io::stdout();
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue
            }
            Err(error) => return Err(format!("Serial device disconnected: {error}")),
            Ok(count) => {
                if stdout
                    .write_all(&buffer[..count])
                    .and_then(|()| stdout.flush())
                    .is_err()
                {
                    break;
                }
            }
        }
    }
    print!("\r\nSerial device closed.\r\n");
    let _ = std::io::stdout().flush();
    Ok(())
}

#[cfg(test)]
#[cfg_attr(windows, allow(unused_imports, dead_code))]
mod tests {
    use super::*;

    fn profile() -> profiles::Profile {
        profiles::Profile {
            id: "host".into(),
            name: "Test".into(),
            host: "example.test".into(),
            protocol: "ssh".into(),
            port: 2222,
            username: "alice".into(),
            key_path: Some("/keys/id_ed25519".into()),
            jump_host: Some("bastion.example.test".into()),
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        }
    }

    #[test]
    fn telnet_strips_commands_and_answers_negotiation() {
        let mut parser = TelnetParser::new((100, 40));
        let input = [
            b'h', IAC, WILL, OPT_ECHO, IAC, DO, OPT_TTYPE, IAC, DO, 39, b'i', IAC, IAC, IAC, SB,
            OPT_TTYPE, 1, IAC, SE, b'!',
        ];
        let (data, replies) = parser.feed(&input);
        assert_eq!(data, vec![b'h', b'i', IAC, b'!']);
        let mut expected = vec![IAC, DO, OPT_ECHO, IAC, WILL, OPT_TTYPE, IAC, WONT, 39];
        expected.extend([IAC, SB, OPT_TTYPE, 0]);
        expected.extend(b"XTERM-256COLOR");
        expected.extend([IAC, SE]);
        assert_eq!(replies, expected);
        // Repeated requests are not answered again, which prevents loops.
        let (_, again) = parser.feed(&[IAC, WILL, OPT_ECHO]);
        assert!(again.is_empty());
    }

    #[test]
    fn telnet_reports_window_size_and_handles_split_commands() {
        let mut parser = TelnetParser::new((255, 24));
        let (data, first) = parser.feed(&[b'a', IAC]);
        assert_eq!((data, first), (vec![b'a'], vec![]));
        let (_, replies) = parser.feed(&[DO, OPT_NAWS]);
        assert_eq!(
            replies,
            vec![IAC, WILL, OPT_NAWS, IAC, SB, OPT_NAWS, 0, 255, 255, 0, 24, IAC, SE]
        );
        assert!(parser.naws_enabled());
        parser.window = (120, 40);
        assert_eq!(
            parser.naws(),
            vec![IAC, SB, OPT_NAWS, 0, 120, 0, 40, IAC, SE]
        );
    }

    #[test]
    fn telnet_encodes_enter_and_iac() {
        assert_eq!(telnet_encode(b"ls\r"), b"ls\r\n");
        assert_eq!(telnet_encode(&[1, IAC]), vec![1, IAC, IAC]);
    }

    #[test]
    fn telnet_parser_negotiates_with_a_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.write_all(&[IAC, DO, OPT_TTYPE]).unwrap();
            socket.write_all(b"login: ").unwrap();
            let mut reply = [0_u8; 3];
            socket.read_exact(&mut reply).unwrap();
            reply
        });
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut parser = TelnetParser::new((80, 24));
        let mut buffer = [0_u8; 64];
        let mut text = Vec::new();
        while !text.ends_with(b"login: ") {
            let count = stream.read(&mut buffer).unwrap();
            let (data, replies) = parser.feed(&buffer[..count]);
            stream.write_all(&replies).unwrap();
            text.extend(data);
        }
        assert_eq!(text, b"login: ");
        assert_eq!(server.join().unwrap(), [IAC, WILL, OPT_TTYPE]);
    }

    #[cfg(unix)]
    #[test]
    fn serial_port_configures_and_exchanges_bytes_through_a_pty() {
        use std::{fs::File, os::fd::FromRawFd};
        let (mut master, slave_name) = unsafe {
            let mut master_fd = 0;
            let mut slave_fd = 0;
            let mut name = [0 as libc::c_char; 128];
            assert_eq!(
                libc::openpty(
                    &mut master_fd,
                    &mut slave_fd,
                    name.as_mut_ptr(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                ),
                0
            );
            libc::close(slave_fd);
            (
                File::from_raw_fd(master_fd),
                std::ffi::CStr::from_ptr(name.as_ptr())
                    .to_string_lossy()
                    .into_owned(),
            )
        };
        let mut port = open_serial(&slave_name, 115200).unwrap();
        port.write_all(b"AT\r").unwrap();
        let mut reply = [0_u8; 3];
        master.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"AT\r");
        master.write_all(b"OK\r\n").unwrap();
        let mut answer = [0_u8; 4];
        port.read_exact(&mut answer).unwrap();
        assert_eq!(&answer, b"OK\r\n");
        assert!(open_serial("/dev/cu.weave-missing", 9600).is_err());
    }

    #[test]
    fn validates_hosts_and_serial_devices() {
        assert!(valid_host("router.local"));
        assert!(valid_host("[fe80::1]"));
        assert!(!valid_host("-oProxyCommand=x"));
        assert!(!valid_host("a b"));
        assert!(!valid_serial_device("/dev/disk0"));
        assert!(!valid_serial_device("/dev/cu.../../disk0"));
        assert!(!valid_serial_device("/dev/ttyUSB0/../../disk0"));
        assert!(!valid_serial_device("COM3; del"));
        if cfg!(target_os = "macos") {
            assert!(valid_serial_device("/dev/cu.usbserial-0001"));
        } else if cfg!(windows) {
            assert!(valid_serial_device("COM3"));
        } else {
            assert!(valid_serial_device("/dev/ttyUSB0"));
            assert!(valid_serial_device(
                "/dev/serial/by-id/usb-FTDI_FT232R-if00-port0"
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn builds_mosh_and_x11_commands_from_an_ssh_profile() {
        let args = mosh_args(&profile()).unwrap();
        assert_eq!(
            args,
            vec![
                "--ssh=ssh -p 2222 -i /keys/id_ed25519 -J bastion.example.test",
                "--",
                "alice@example.test"
            ]
        );
        let mut spaced = profile();
        spaced.key_path = Some("/my keys/id".into());
        assert!(mosh_args(&spaced).is_err());

        let args = x11_args(
            &profile(),
            Path::new("/tmp/cp"),
            Some(Path::new("/opt/X11/bin/xauth")),
            Some("xclock"),
        )
        .unwrap();
        assert!(args.contains(&"-Y".to_string()));
        assert!(args.contains(&"XAuthLocation=/opt/X11/bin/xauth".to_string()));
        assert_eq!(args.last().unwrap(), "xclock");
        assert!(x11_args(&profile(), Path::new("/tmp/cp"), None, Some("a\nb")).is_err());
    }

    #[test]
    fn builds_vnc_urls_and_rdp_files() {
        assert_eq!(
            vnc_url("mac.local", 5900, Some("bob")).unwrap(),
            "vnc://bob@mac.local:5900"
        );
        assert_eq!(
            vnc_url("fe80::1", 5901, None).unwrap(),
            "vnc://[fe80::1]:5901"
        );
        assert!(vnc_url("a b", 5900, None).is_err());
        assert_eq!(
            rdp_file("pc.example.test", 3389, Some("CORP\\bob")).unwrap(),
            "full address:s:pc.example.test:3389\nprompt for credentials:i:1\nusername:s:CORP\\bob\n"
        );
        assert!(rdp_file("pc", 3389, Some("bob\nfull address:s:evil")).is_err());
    }

    #[test]
    fn rejects_unknown_or_incomplete_connections() {
        let state = AppState::for_tests();
        let spec = |kind: &str| ConnectionSpec {
            kind: kind.into(),
            profile_id: None,
            host: None,
            port: None,
            device: None,
            baud: None,
            command: None,
        };
        assert!(connection_command(&state, &spec("rdp")).is_err());
        assert!(connection_command(&state, &spec("telnet"))
            .err()
            .unwrap()
            .contains("host"));
        assert!(connection_command(&state, &spec("serial"))
            .err()
            .unwrap()
            .contains("serial device"));
        assert!(connection_command(&state, &spec("mosh"))
            .err()
            .unwrap()
            .contains("SSH host"));
        let mut telnet = spec("telnet");
        telnet.host = Some("router.local".into());
        telnet.port = Some(23);
        assert!(connection_command(&state, &telnet).is_ok());
    }
}
