//! Lets `ssh` ask for passwords, verification codes (Duo, OTP) and new host
//! keys when it runs without a terminal, as it does for Files, tunnels and
//! copies. `ssh` starts Weave's own binary as SSH_ASKPASS; that helper asks the
//! running app over a private channel (a Unix socket in a private folder, or a
//! loopback port on Windows) with a one-time token, and the app answers from
//! the credential store or shows the question in the window.

use crate::{profiles::Profile, vault, AppState};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::Command,
    sync::{mpsc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use zeroize::Zeroizing;

const HELPER_FLAG: &str = "WEAVE_SSH_ASKPASS";
const SOCKET_VAR: &str = "WEAVE_SSH_ASKPASS_SOCKET";
const TOKEN_VAR: &str = "WEAVE_SSH_ASKPASS_TOKEN";
const PROFILE_VAR: &str = "WEAVE_SSH_ASKPASS_PROFILE";

#[cfg(windows)]
use std::net::{TcpListener as Listener, TcpStream as Stream};
#[cfg(unix)]
use std::os::unix::net::{UnixListener as Listener, UnixStream as Stream};

struct Server {
    /// Where the helper connects: a socket path, or 127.0.0.1:port on Windows.
    address: String,
    token: String,
}

/// Opens the channel the helper connects to.
fn listen() -> std::io::Result<(Listener, String)> {
    #[cfg(unix)]
    {
        let folder =
            std::env::temp_dir().join(format!("weave-askpass-{}", uuid::Uuid::new_v4().simple()));
        crate::platform::create_private_dir(&folder)?;
        let socket = folder.join("s");
        let listener = Listener::bind(&socket)?;
        Ok((listener, socket.to_string_lossy().into_owned()))
    }
    #[cfg(windows)]
    {
        let listener = Listener::bind(("127.0.0.1", 0))?;
        let address = listener.local_addr()?.to_string();
        Ok((listener, address))
    }
}

static SERVER: OnceLock<Server> = OnceLock::new();
type Answer = Option<Zeroizing<String>>;
/// Questions shown in the window, waiting for an answer and whether to remember it.
type Waiting = Mutex<HashMap<String, mpsc::Sender<(Answer, bool)>>>;
static PENDING: OnceLock<Waiting> = OnceLock::new();
/// When a saved password was last offered for a profile, so a wrong one is
/// not offered again in a loop.
static OFFERED: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

#[derive(Serialize, Deserialize)]
struct Request {
    token: String,
    profile: String,
    prompt: String,
}

#[derive(Serialize, Deserialize)]
struct Reply {
    answer: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptEvent {
    id: String,
    connection: String,
    prompt: String,
    /// Hide what is typed (passwords and codes); host-key questions are visible.
    secret: bool,
    /// Offer to remember the answer in the Keychain.
    can_remember: bool,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Kind {
    Password,
    Passphrase,
    HostKey,
    Other,
}

pub(crate) fn classify(prompt: &str) -> Kind {
    let lower = prompt.to_lowercase();
    if lower.contains("continue connecting") || lower.contains("(yes/no") {
        Kind::HostKey
    } else if lower.contains("passphrase") {
        Kind::Passphrase
    } else if lower.contains("password")
        && ![
            "verification",
            "code",
            "otp",
            "passcode",
            "token",
            "duo",
            "new password",
            "pin",
            "one-time",
            "oath",
            "factor",
            "2fa",
        ]
        .iter()
        .any(|word| lower.contains(word))
    {
        Kind::Password
    } else {
        Kind::Other
    }
}

/// Starts listening for helper questions. Called once when Weave starts.
pub fn start<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let (listener, address) =
        listen().map_err(|error| format!("Could not prepare SSH sign-in prompts: {error}"))?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let _ = SERVER.set(Server {
        address,
        token: token.clone(),
    });
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let app = app.clone();
            let token = token.clone();
            std::thread::spawn(move || handle(&app, &token, stream));
        }
    });
    Ok(())
}

fn handle<R: Runtime>(app: &AppHandle<R>, token: &str, stream: Stream) {
    serve_one(token, stream, |request| answer(app, request));
}

/// Reads one question, checks its token, and writes back the answer.
fn serve_one(token: &str, stream: Stream, answer: impl Fn(&Request) -> Answer) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(stream) => stream,
        Err(_) => return,
    });
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let Ok(request) = serde_json::from_str::<Request>(&line) else {
        return;
    };
    if request.token != token {
        return;
    }
    let answer = answer(&request);
    let reply = Reply {
        answer: answer.as_ref().map(|value| value.to_string()),
    };
    let mut stream = stream;
    let _ = writeln!(
        stream,
        "{}",
        serde_json::to_string(&reply).unwrap_or_default()
    );
}

fn answer<R: Runtime>(app: &AppHandle<R>, request: &Request) -> Answer {
    let state = app.state::<AppState>();
    let profile = crate::profiles::get_profile(&state, &request.profile).ok()?;
    let kind = classify(&request.prompt);
    if kind == Kind::Password {
        let offered = OFFERED.get_or_init(Default::default);
        let recently = offered
            .lock()
            .ok()?
            .get(&profile.id)
            .is_some_and(|time| time.elapsed() < Duration::from_secs(20));
        if !recently {
            if let Ok(password) = vault::password_for(&state, &profile) {
                if !password.is_empty() {
                    offered
                        .lock()
                        .ok()?
                        .insert(profile.id.clone(), Instant::now());
                    return Some(password);
                }
            }
        }
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    let (sender, receiver) = mpsc::channel();
    PENDING
        .get_or_init(Default::default)
        .lock()
        .ok()?
        .insert(id.clone(), sender);
    let event = PromptEvent {
        id: id.clone(),
        connection: format!("{} ({}@{})", profile.name, profile.username, profile.host),
        prompt: request.prompt.trim_end().to_string(),
        secret: kind != Kind::HostKey,
        can_remember: kind == Kind::Password,
    };
    if app.emit("ssh-prompt", event).is_err() {
        return None;
    }
    let _ = app.get_webview_window("main").map(|window| {
        let _ = window.show();
        let _ = window.set_focus();
    });
    let result = receiver.recv_timeout(Duration::from_secs(300));
    PENDING
        .get_or_init(Default::default)
        .lock()
        .ok()?
        .remove(&id);
    let (answer, remember) = result.ok()?;
    if remember {
        if let Some(password) = answer.as_ref() {
            let _ = vault::save_profile_password(&state, &profile.id, password);
        }
    }
    answer
}

#[tauri::command]
pub fn ssh_prompt_answer(id: String, answer: Option<String>, remember: bool) -> Result<(), String> {
    let sender = PENDING
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Sign-in prompts are busy.")?
        .remove(&id)
        .ok_or("This sign-in question has expired. Try connecting again.")?;
    let _ = sender.send((answer.map(Zeroizing::new), remember));
    Ok(())
}

/// Lets `command` (an `ssh`, `scp` or `rsync` run) ask Weave for answers.
pub fn apply(command: &mut Command, profile: &Profile) {
    // Tests run the real helper from the built app, not the test binary.
    let helper = std::env::var_os("WEAVE_ASKPASS_HELPER")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok());
    let (Some(server), Some(helper)) = (SERVER.get(), helper) else {
        return;
    };
    command
        .env("SSH_ASKPASS", helper)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env(
            "DISPLAY",
            std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into()),
        )
        .env(HELPER_FLAG, "1")
        .env(SOCKET_VAR, &server.address)
        .env(TOKEN_VAR, &server.token)
        .env(PROFILE_VAR, &profile.id);
}

fn ask(socket: &str, request: &Request) -> Option<String> {
    let mut stream = Stream::connect(socket).ok()?;
    writeln!(stream, "{}", serde_json::to_string(request).ok()?).ok()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    serde_json::from_str::<Reply>(&line).ok()?.answer
}

/// Runs as the SSH_ASKPASS helper: forwards ssh's question to the app and
/// prints the answer. Returns false when this process is not the helper.
pub fn serve_askpass() -> bool {
    if std::env::var_os(HELPER_FLAG).is_none() {
        return false;
    }
    let request = || -> Option<Request> {
        Some(Request {
            token: std::env::var(TOKEN_VAR).ok()?,
            profile: std::env::var(PROFILE_VAR).ok()?,
            prompt: std::env::args().nth(1).unwrap_or_default(),
        })
    };
    let answer = request()
        .zip(std::env::var(SOCKET_VAR).ok())
        .and_then(|(request, socket)| ask(&socket, &request));
    match answer {
        Some(answer) => {
            println!("{answer}");
            std::process::exit(0)
        }
        None => std::process::exit(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real sshd and ssh; the app side runs in a mock app and the answer is
    /// given the way the dialog gives it. Needs the built app binary.
    #[cfg(unix)]
    #[test]
    #[ignore = "starts a local sshd and needs the built weave binary"]
    fn signs_in_through_the_app_prompt_with_a_real_ssh_server() {
        use std::{collections::HashMap, sync::Mutex};
        let helper = std::env::var("WEAVE_ASKPASS_HELPER").expect("WEAVE_ASKPASS_HELPER");
        assert!(std::path::Path::new(&helper).exists());
        let dir = tempfile::tempdir().unwrap();
        let path = |name: &str| dir.path().join(name);
        let keygen = |file: &str, phrase: &str| {
            assert!(crate::platform::command("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", phrase, "-f"])
                .arg(path(file))
                .status()
                .unwrap()
                .success());
        };
        keygen("host", "");
        keygen("client", "correct horse");
        std::fs::copy(path("client.pub"), path("authorized_keys")).unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let user = String::from_utf8(
            crate::platform::command("id")
                .arg("-un")
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        std::fs::write(
            path("sshd_config"),
            format!(
                "Port {port}\nListenAddress 127.0.0.1\nHostKey {}\nAuthorizedKeysFile {}\nPidFile {}\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\nStrictModes no\nAllowUsers {user}\n",
                path("host").display(),
                path("authorized_keys").display(),
                path("sshd.pid").display()
            ),
        )
        .unwrap();
        let mut sshd = crate::platform::command("/usr/sbin/sshd")
            .args(["-D", "-e", "-f"])
            .arg(path("sshd_config"))
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(500));

        let app = tauri::test::mock_app();
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE profiles (id TEXT PRIMARY KEY, name TEXT, host TEXT, protocol TEXT, port INTEGER, username TEXT, key_path TEXT, jump_host TEXT, color TEXT, favorite INTEGER, tls_pin_sha256 TEXT, s3_region TEXT); CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        db.execute("INSERT INTO profiles VALUES ('hpc-test', 'HPC', '127.0.0.1', 'ssh', ?1, ?2, NULL, NULL, '', 0, NULL, NULL)", rusqlite::params![port, user]).unwrap();
        app.manage(AppState {
            db: Mutex::new(db),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            session_passwords: Mutex::new(HashMap::new()),
        });
        start(app.handle().clone()).unwrap();
        // Plays the dialog: answers each question as a person would.
        let asked = std::sync::Arc::new(Mutex::new(Vec::new()));
        let seen = asked.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(50));
            let waiting: Vec<String> = PENDING
                .get_or_init(Default::default)
                .lock()
                .unwrap()
                .keys()
                .cloned()
                .collect();
            for id in waiting {
                let count = {
                    let mut list = seen.lock().unwrap();
                    list.push(id.clone());
                    list.len()
                };
                let answer = if count == 1 { "yes" } else { "correct horse" };
                let _ = ssh_prompt_answer(id, Some(answer.into()), false);
            }
        });
        let state = app.state::<AppState>();
        let profile = crate::profiles::get_profile(&state, "hpc-test").unwrap();
        let mut command = crate::platform::command("ssh");
        command
            .args(["-o", "ConnectTimeout=5", "-o"])
            .arg(format!(
                "UserKnownHostsFile={}",
                path("known_hosts").display()
            ))
            .arg("-i")
            .arg(path("client"))
            .args([
                "-p",
                &port.to_string(),
                "-l",
                &user,
                "127.0.0.1",
                "echo signed-in",
            ])
            .stdin(std::process::Stdio::null());
        apply(&mut command, &profile);
        let output = command.output().unwrap();
        let _ = sshd.kill();
        let _ = sshd.wait();
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("signed-in"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            asked.lock().unwrap().len(),
            2,
            "host key and passphrase questions"
        );
    }

    #[test]
    fn answers_the_helper_over_the_socket_only_with_the_token() {
        let (listener, socket) = listen().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(2).flatten() {
                serve_one("right", stream, |request| {
                    assert_eq!(request.profile, "hpc");
                    Some(Zeroizing::new(format!(
                        "answer to {}",
                        request.prompt.trim()
                    )))
                });
            }
        });
        let request = |token: &str| Request {
            token: token.into(),
            profile: "hpc".into(),
            prompt: "Verification code: ".into(),
        };
        let socket = socket.as_str();
        assert_eq!(
            ask(socket, &request("right")).as_deref(),
            Some("answer to Verification code:")
        );
        assert_eq!(ask(socket, &request("wrong")), None);
    }

    #[test]
    fn tells_passwords_from_codes_and_host_keys() {
        for prompt in ["ada@login.hpc.example.edu's password: ", "Password: "] {
            assert_eq!(classify(prompt), Kind::Password, "{prompt}");
        }
        for prompt in [
            "Verification code: ",
            "Duo two-factor login for ada\n\nEnter a passcode or select one of the following options:\n\n 1. Duo Push to iOS\n\nPasscode or option (1-1): ",
            "One-time password (OATH) for `ada': ",
            "TOTP code: ",
        ] {
            assert_eq!(classify(prompt), Kind::Other, "{prompt}");
        }
        assert_eq!(
            classify("Enter passphrase for key '/Users/ada/.ssh/id_ed25519': "),
            Kind::Passphrase
        );
        assert_eq!(
            classify("The authenticity of host 'login (10.0.0.5)' can't be established.\nED25519 key fingerprint is SHA256:abc.\nAre you sure you want to continue connecting (yes/no/[fingerprint])? "),
            Kind::HostKey
        );
    }
}
