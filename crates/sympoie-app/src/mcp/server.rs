use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{Value, json};

use super::{discovery_path, token};

pub struct Request {
    pub tool: String,
    pub arguments: Value,
    reply: Sender<Result<Value, String>>,
}

impl Request {
    pub fn answer(self, result: Result<Value, String>) {
        let _ = self.reply.send(result);
    }

    pub fn into_parts(self) -> (String, Value, Responder) {
        (self.tool, self.arguments, Responder(self.reply))
    }
}

pub struct Responder(Sender<Result<Value, String>>);

impl Responder {
    pub fn answer(self, result: Result<Value, String>) {
        let _ = self.0.send(result);
    }
}

pub struct McpServer {
    discovery: PathBuf,
    token: String,
    requests: Receiver<Request>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl McpServer {
    pub fn start(folder: &Path, wake: impl Fn() + Send + Sync + 'static) -> Result<Self, String> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|error| format!("cannot open the agent socket: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let port = listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        let token = token();
        let discovery = discovery_path(folder);
        let directory = discovery
            .parent()
            .expect("the discovery file is in a folder");
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        let _ = std::fs::write(directory.join(".gitignore"), "*\n");
        let advert = json!({"port": port, "token": token, "pid": std::process::id()});
        let temporary = discovery.with_extension("json.tmp");
        std::fs::write(&temporary, advert.to_string())
            .and_then(|()| restrict(&temporary))
            .and_then(|()| std::fs::rename(&temporary, &discovery))
            .map_err(|error| format!("cannot write {}: {error}", discovery.display()))?;

        let (sender, requests) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (stop, token) = (Arc::clone(&stop), token.clone());
            let wake = Arc::new(wake);
            std::thread::Builder::new()
                .name("sympoie-mcp".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        match listener.accept() {
                            Ok((stream, _)) => {
                                let (sender, token, wake) =
                                    (sender.clone(), token.clone(), Arc::clone(&wake));
                                std::thread::spawn(move || {
                                    serve(stream, &token, &sender, &*wake);
                                });
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                std::thread::sleep(Duration::from_millis(25));
                            }
                            Err(_) => std::thread::sleep(Duration::from_millis(100)),
                        }
                    }
                })
                .map_err(|error| error.to_string())?
        };
        Ok(Self {
            discovery,
            token,
            requests,
            stop,
            thread: Some(thread),
        })
    }

    pub fn try_request(&self) -> Option<Request> {
        self.requests.try_recv().ok()
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if std::fs::read_to_string(&self.discovery).is_ok_and(|text| text.contains(&self.token)) {
            let _ = std::fs::remove_file(&self.discovery);
        }
    }
}

#[cfg(unix)]
fn restrict(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn serve(stream: TcpStream, token: &str, sender: &Sender<Request>, wake: &dyn Fn()) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let mut line = String::new();
    if BufReader::new(stream).read_line(&mut line).is_err() {
        return;
    }
    let reply = match serde_json::from_str::<Value>(&line) {
        Ok(request) if request.get("token").and_then(Value::as_str) == Some(token) => {
            let tool = request
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let arguments = request.get("arguments").cloned().unwrap_or(json!({}));
            let (reply, answer) = mpsc::channel();
            if sender
                .send(Request {
                    tool,
                    arguments,
                    reply,
                })
                .is_err()
            {
                return;
            }
            wake();
            match answer.recv_timeout(Duration::from_secs(600)) {
                Ok(Ok(value)) => json!({"ok": value}),
                Ok(Err(error)) => json!({"error": error}),
                Err(_) => json!({"error": "Sympoie did not answer"}),
            }
        }
        Ok(_) => json!({"error": "the token does not match; Sympoie may have restarted"}),
        Err(error) => json!({"error": format!("invalid request: {error}")}),
    };
    let _ = writeln!(writer, "{reply}");
}

pub fn call(folder: &Path, tool: &str, arguments: &Value) -> Result<Value, String> {
    let closed = || {
        format!(
            "Sympoie is not open with agent mode on for {}. Ask the person to open this project in \
             Sympoie and turn on the AGENT switch in its status bar. Until then you can still edit \
             the project's files and validate them with check_project.",
            folder.display()
        )
    };
    let advert: Value = std::fs::read_to_string(discovery_path(folder))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .ok_or_else(closed)?;
    let port = advert
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .ok_or_else(closed)?;
    let token = advert
        .get("token")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut stream = TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_secs(2),
    )
    .map_err(|_| closed())?;
    let request = json!({"token": token, "tool": tool, "arguments": arguments});
    writeln!(stream, "{request}").map_err(|error| error.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|error| format!("lost the connection to Sympoie: {error}"))?;
    let reply: Value = serde_json::from_str(&line).map_err(|_| closed())?;
    match (reply.get("ok"), reply.get("error")) {
        (Some(value), _) => Ok(value.clone()),
        (_, Some(error)) => Err(error.as_str().unwrap_or("unknown error").to_owned()),
        _ => Err("Sympoie sent an unexpected reply".to_owned()),
    }
}
