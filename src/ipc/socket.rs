//! The Unix-socket transport between a server and its clients: the private runtime directory,
//! the session sockets, the server's accept loop and the attaching client. Unix only; on
//! Windows tuidesk runs standalone.

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};

use super::*;

// ------------------------------------------------------------------ paths

fn uid() -> u32 {
    // SAFETY: getuid has no preconditions.
    unsafe { libc::getuid() }
}

/// Create `dir` (mode 0700) if needed and make sure it is safe to put sockets in: a real
/// directory, owned by us, not accessible to others. Refuses directories another user made.
fn secure_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e).with_context(|| format!("creating {}", dir.display())),
    }
    let meta = std::fs::symlink_metadata(dir).with_context(|| format!("checking {}", dir.display()))?;
    if !meta.is_dir() {
        bail!("{} is not a directory (refusing to use it for tuidesk's sockets)", dir.display());
    }
    if meta.uid() != uid() {
        bail!("{} belongs to another user (refusing to use it for tuidesk's sockets)", dir.display());
    }
    if meta.mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("restricting {}", dir.display()))?;
    }
    Ok(())
}

pub fn runtime_dir() -> Result<PathBuf> {
    let dir = if let Some(d) = std::env::var_os("TUIDESK_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        PathBuf::from(d)
    } else if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        PathBuf::from(d).join("tuidesk")
    } else {
        PathBuf::from(format!("/tmp/tuidesk-{}", uid()))
    };
    secure_dir(&dir)?;
    Ok(dir)
}

/// Unix socket paths are limited to about 104 bytes (macOS) or 108 (Linux).
const SUN_PATH_MAX: usize = 100;

/// Directory holding the session sockets. Decided from the runtime directory alone, so every
/// session (and `tuidesk ls`) agrees on it.
fn socket_dir() -> Result<PathBuf> {
    let dir = runtime_dir()?;
    if dir.as_os_str().len() + 1 + MAX_SESSION_NAME + ".sock".len() < SUN_PATH_MAX {
        return Ok(dir);
    }
    // Too long for a socket: use a short directory derived from the long one.
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    dir.hash(&mut h);
    let short = PathBuf::from(format!("/tmp/tuidesk-{}-{:x}", uid(), h.finish()));
    secure_dir(&short)?;
    Ok(short)
}

pub fn socket_path(name: &str) -> Result<PathBuf> {
    Ok(socket_dir()?.join(format!("{name}.sock")))
}

/// User id of the process on the other end of a Unix socket.
fn peer_uid(s: &UnixStream) -> Option<u32> {
    use std::os::fd::AsRawFd;
    let fd = s.as_raw_fd();
    #[cfg(target_os = "linux")]
    {
        // SAFETY: getsockopt writes at most `len` bytes into `cred`.
        unsafe {
            let mut cred: libc::ucred = std::mem::zeroed();
            let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
            let r = libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut cred as *mut libc::ucred as *mut libc::c_void,
                &mut len,
            );
            (r == 0).then_some(cred.uid)
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let (mut euid, mut egid) = (0, 0);
        // SAFETY: getpeereid only writes the two ids.
        let r = unsafe { libc::getpeereid(fd, &mut euid, &mut egid) };
        (r == 0).then_some(euid)
    }
}

/// Connect to a socket served by our own user.
fn connect_own(path: &Path) -> io::Result<UnixStream> {
    let s = UnixStream::connect(path)?;
    if peer_uid(&s) != Some(uid()) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "socket is not served by this user"));
    }
    Ok(s)
}

/// Names of sessions with a live server.
pub fn list_sessions() -> Vec<String> {
    let Ok(dir) = socket_dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            let name = p.file_name()?.to_str()?.strip_suffix(".sock")?.to_string();
            connect_own(&p).ok().map(|_| name)
        })
        .collect();
    names.sort();
    names
}

// ------------------------------------------------------------------ server

/// Largest first message (hello, kill or msg) a connection may send.
const MAX_FIRST_FRAME: usize = 64 * 1024;

/// Accept clients and turn their traffic into app events. Runs in a background thread; each
/// connection is handled on its own thread, so a slow or hostile one cannot block others.
pub fn serve(listener: UnixListener, tx: Sender<AppEvent>) {
    std::thread::Builder::new()
        .name("accept".into())
        .spawn(move || {
            let next_conn = std::sync::atomic::AtomicU64::new(1);
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                if peer_uid(&stream) != Some(uid()) {
                    continue;
                }
                let conn = next_conn.fetch_add(1, Ordering::Relaxed);
                let tx = tx.clone();
                let _ =
                    std::thread::Builder::new().name(format!("conn-{conn}")).spawn(move || handshake(stream, conn, tx));
            }
        })
        .expect("spawning accept thread");
}

fn handshake(mut stream: UnixStream, conn: u64, tx: Sender<AppEvent>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let hello = match read_frame_max(&mut stream, MAX_FIRST_FRAME) {
        Ok((C_HELLO, payload)) => serde_json::from_slice::<Hello>(&payload).ok(),
        Ok((C_KILL, _)) => {
            let _ = tx.send(AppEvent::Kill);
            return;
        }
        Ok((C_MSG, payload)) => return answer_msg(stream, &payload, tx),
        _ => None,
    };
    let Some(hello) = hello.filter(|h| h.protocol == PROTOCOL) else {
        let _ = write_frame(&mut stream, S_DETACH, b"protocol mismatch: restart the server");
        return;
    };
    let _ = stream.set_read_timeout(None);
    let Ok(reader) = stream.try_clone() else { return };
    if tx.send(AppEvent::Attach { conn, stream, hello }).is_ok() {
        client_reader(reader, conn, tx);
    }
}

/// Hand a `tuidesk msg` command to the app and send its reply back.
fn answer_msg(mut stream: UnixStream, payload: &[u8], tx: Sender<AppEvent>) {
    let reply: Result<String, String> = match serde_json::from_slice::<Vec<String>>(payload) {
        Ok(args) => {
            let (rtx, rrx) = std::sync::mpsc::channel();
            if tx.send(AppEvent::Msg { args, reply: rtx }).is_err() {
                Err("server is shutting down".into())
            } else {
                rrx.recv_timeout(Duration::from_secs(10)).unwrap_or_else(|_| Err("no reply from server".into()))
            }
        }
        Err(e) => Err(format!("bad request: {e}")),
    };
    let _ = write_frame(&mut stream, S_REPLY, &serde_json::to_vec(&reply).unwrap_or_default());
}

fn client_reader(mut stream: UnixStream, conn: u64, tx: Sender<AppEvent>) {
    loop {
        match read_frame(&mut stream) {
            Ok((C_EVENT, payload)) => {
                if let Ok(ev) = serde_json::from_slice::<Event>(&payload)
                    && tx.send(AppEvent::ClientInput(conn, ev)).is_err()
                {
                    return;
                }
            }
            Ok((C_KILL, _)) => {
                let _ = tx.send(AppEvent::Kill);
            }
            Ok((C_CELL, payload)) => {
                if let Ok(cell) = serde_json::from_slice::<(u16, u16)>(&payload) {
                    let _ = tx.send(AppEvent::CellSize(conn, cell));
                }
            }
            Ok(_) => {}
            Err(_) => {
                let _ = tx.send(AppEvent::ClientGone(conn));
                return;
            }
        }
    }
}

/// Bind the session socket, refusing if a live server already owns it.
pub fn bind(name: &str) -> Result<(UnixListener, PathBuf)> {
    let path = socket_path(name)?;
    if UnixStream::connect(&path).is_ok() {
        bail!("session '{name}' is already running");
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))?;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    Ok((listener, path))
}

/// Start a detached server process for `name`.
fn start_server(name: &str, config_dir: &Path, size: (u16, u16)) -> Result<PathBuf> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::process::CommandExt;
    let log = runtime_dir()?.join(format!("{name}.log"));
    // Never follow a symlink planted in place of the log.
    let log_file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&log)
        .with_context(|| format!("creating {}", log.display()))?;
    let exe = std::env::current_exe().context("locating tuidesk binary")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["--server", "-S", name, "-c"])
        .arg(config_dir)
        .arg("--size")
        .arg(format!("{}x{}", size.0, size.1))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(log_file);
    // SAFETY: setsid is async-signal-safe; it detaches the server from this terminal.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().context("starting server")?;
    Ok(log)
}

// ------------------------------------------------------------------ client

/// Connect to a session, starting its server (sized `size`) if `create` is set.
pub fn connect(name: &str, config_dir: &Path, create: bool, size: (u16, u16)) -> Result<UnixStream> {
    let path = socket_path(name)?;
    if let Ok(s) = connect_own(&path) {
        return Ok(s);
    }
    if !create {
        bail!("no session named '{name}'");
    }
    let log = start_server(name, config_dir, size)?;
    for _ in 0..250 {
        std::thread::sleep(Duration::from_millis(20));
        if let Ok(s) = connect_own(&path) {
            return Ok(s);
        }
    }
    let detail = std::fs::read_to_string(&log).unwrap_or_default();
    bail!("server did not start (log: {}):\n{detail}", log.display())
}

/// Ask a session to save and exit, and wait (up to 5s) until it has.
pub fn kill(name: &str) -> Result<()> {
    let path = socket_path(name)?;
    let mut s = connect_own(&path).with_context(|| format!("no session named '{name}'"))?;
    write_frame(&mut s, C_KILL, b"")?;
    for _ in 0..250 {
        if UnixStream::connect(&path).is_err() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    bail!("session '{name}' did not stop")
}

/// Send a command to a running session and return its reply.
pub fn msg(name: &str, args: &[String]) -> Result<String> {
    let mut s = connect_own(&socket_path(name)?).with_context(|| format!("no session named '{name}'"))?;
    s.set_read_timeout(Some(Duration::from_secs(15)))?;
    write_frame(&mut s, C_MSG, &serde_json::to_vec(args)?)?;
    match read_frame(&mut s)? {
        (S_REPLY, payload) => match serde_json::from_slice::<Result<String, String>>(&payload)? {
            Ok(out) => Ok(out),
            Err(e) => bail!("{e}"),
        },
        (S_DETACH, msg) => bail!("{}", String::from_utf8_lossy(&msg)),
        _ => bail!("unexpected reply"),
    }
}

/// How a client session ended.
pub enum ClientEnd {
    Detached(String),
    Exited,
    Lost,
}

/// Attach the current terminal to a server. The caller sets up and restores the terminal.
pub fn run_client(stream: UnixStream, hello: Hello) -> Result<ClientEnd> {
    let mut writer = stream.try_clone()?;
    write_frame(&mut writer, C_HELLO, &serde_json::to_vec(&hello)?)?;

    let done = Arc::new(AtomicBool::new(false));
    let end = Arc::new(Mutex::new(ClientEnd::Lost));
    let reader = {
        let done = done.clone();
        let end = end.clone();
        let mut stream = stream;
        std::thread::spawn(move || {
            let mut out = io::stdout();
            loop {
                match read_frame(&mut stream) {
                    Ok((S_OUTPUT, bytes)) => {
                        if out.write_all(&bytes).and_then(|_| out.flush()).is_err() {
                            break;
                        }
                    }
                    Ok((S_DETACH, msg)) => {
                        *end.lock().unwrap() = ClientEnd::Detached(String::from_utf8_lossy(&msg).into_owned());
                        break;
                    }
                    Ok((S_EXIT, _)) => {
                        *end.lock().unwrap() = ClientEnd::Exited;
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            done.store(true, Ordering::SeqCst);
        })
    };

    while !done.load(Ordering::SeqCst) {
        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        let ev = event::read()?;
        if write_frame(&mut writer, C_EVENT, &serde_json::to_vec(&ev)?).is_err() {
            break;
        }
        // A resize can come with a new font size; images are sized in pixels.
        if matches!(ev, Event::Resize(..))
            && let Some(cell) = cell_size()
        {
            let _ = write_frame(&mut writer, C_CELL, &serde_json::to_vec(&cell)?);
        }
    }
    let _ = writer.shutdown(std::net::Shutdown::Both);
    let _ = reader.join();
    let end = std::mem::replace(&mut *end.lock().unwrap(), ClientEnd::Lost);
    Ok(end)
}
