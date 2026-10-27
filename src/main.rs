use std::io::stdout;
use std::path::PathBuf;
use std::sync::mpsc;

use anyhow::{Result, bail};
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::crossterm::{cursor, execute};

use tuidesk::app::{App, AppEvent};
use tuidesk::config;
#[cfg(unix)]
use tuidesk::ipc::ClientEnd;
use tuidesk::ipc::{self, Display, Hello};

const HELP: &str = "\
tuidesk - a KDE Plasma style desktop for the terminal

USAGE:
    tuidesk [OPTIONS]            Attach to the session, starting it if needed
    tuidesk ls                   List running sessions
    tuidesk kill [-S NAME]       Save and stop a session
    tuidesk msg COMMAND...       Control a running session (see: tuidesk msg help)

OPTIONS:
    -S, --session <NAME>  Session name (default: \"default\")
    -c, --config <DIR>    Config directory (default: $XDG_CONFIG_HOME/tuidesk or ~/.config/tuidesk)
        --standalone      Run in this terminal without a background server (no detach)
        --print-config    Print the default config.toml and exit
        --print-lua       Print the default init.lua and exit
        --print-apps      Print the built-in Kickoff app catalogue and exit
    -h, --help            Show this help
    -V, --version         Show version

Detach with the prefix key then d; run tuidesk again to reattach.
Inside a tuidesk window, `tuidesk msg` talks to that window's session.
";

// Windows rejects the server commands, so their arguments go unused there.
#[cfg_attr(windows, allow(dead_code))]
enum Command {
    Attach,
    Standalone,
    Server,
    List,
    Kill,
    Msg(Vec<String>),
}

fn main() -> Result<()> {
    let mut config_dir: Option<PathBuf> = None;
    // Inside a window, default to the session that window belongs to.
    let mut session =
        std::env::var("TUIDESK_SESSION").ok().filter(|s| valid_session_name(s)).unwrap_or_else(|| "default".into());
    let mut command = Command::Attach;
    // Initial screen size for a server, so restored windows keep their geometry before attach.
    #[cfg(unix)]
    let mut size: (u16, u16) = (80, 24);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-c" | "--config" => match args.next() {
                Some(d) => config_dir = Some(PathBuf::from(d)),
                None => bail!("--config needs a directory"),
            },
            "-S" | "--session" => match args.next() {
                Some(s) if valid_session_name(&s) => session = s,
                _ => {
                    bail!("--session needs a name of up to {} letters, digits, '-', '_' or '.'", ipc::MAX_SESSION_NAME)
                }
            },
            #[cfg(unix)]
            "--size" => {
                let parsed = args.next().and_then(|s| {
                    let (c, r) = s.split_once('x')?;
                    Some((c.parse().ok()?, r.parse().ok()?))
                });
                match parsed {
                    Some(s) => size = s,
                    None => bail!("--size needs COLSxROWS"),
                }
            }
            "--standalone" => command = Command::Standalone,
            "--server" => command = Command::Server,
            "ls" | "list" => command = Command::List,
            "kill" | "kill-server" => command = Command::Kill,
            "msg" => {
                command = Command::Msg(args.by_ref().collect());
            }
            "--print-config" => {
                print!("{}", config::DEFAULT_CONFIG);
                return Ok(());
            }
            "--print-lua" => {
                print!("{}", config::DEFAULT_INIT_LUA);
                return Ok(());
            }
            "--print-apps" => {
                print!("{}", config::BUILTIN_APPS);
                return Ok(());
            }
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("tuidesk {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            other => bail!("unknown argument '{other}' (see --help)"),
        }
    }
    let config_dir = config_dir.unwrap_or_else(config::config_dir);

    // Windows has no background server (it needs Unix sockets): the desktop always runs in the
    // terminal it was started in.
    #[cfg(windows)]
    let command = match command {
        Command::Attach => Command::Standalone,
        Command::Server | Command::List | Command::Kill | Command::Msg(_) => {
            bail!("background sessions are not available on Windows; run tuidesk without a command")
        }
        other => other,
    };

    match command {
        #[cfg(unix)]
        Command::List => {
            for name in ipc::list_sessions() {
                println!("{name}");
            }
            Ok(())
        }
        #[cfg(unix)]
        Command::Kill => ipc::kill(&session),
        #[cfg(unix)]
        Command::Msg(mut args) => {
            if args.first().is_some_and(|a| a == "screenshot") {
                absolutize_paths(&mut args[1..]);
            }
            let out = ipc::msg(&session, &args)?;
            if !out.is_empty() {
                println!("{out}");
            }
            Ok(())
        }
        #[cfg(unix)]
        Command::Server => run_server(session, config_dir, size),
        #[cfg(windows)]
        Command::Server | Command::List | Command::Kill | Command::Msg(_) => unreachable!("rejected above"),
        Command::Attach | Command::Standalone => {
            if std::env::var_os("TUIDESK").is_some() {
                bail!("already running inside tuidesk");
            }
            config::ensure_default_files(&config_dir);
            #[cfg(unix)]
            if matches!(command, Command::Attach) {
                return run_attach(session, config_dir);
            }
            run_standalone(session, config_dir)
        }
    }
}

/// Put the terminal into the mode tuidesk needs. Returns whether key release events are available.
fn setup_terminal() -> Result<bool> {
    terminal::enable_raw_mode()?;
    // Ask for unambiguous key reports (ctrl+space, alt+keys) where the terminal supports it.
    let enhanced = terminal::supports_keyboard_enhancement().unwrap_or(false);
    execute!(stdout(), EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
    if enhanced {
        execute!(stdout(), PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
    }
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal(enhanced);
        default_hook(info);
    }));
    Ok(enhanced)
}

fn restore_terminal(enhanced: bool) {
    if enhanced {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        stdout(),
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        cursor::SetCursorStyle::DefaultUserShape,
        cursor::Show
    );
    let _ = terminal::disable_raw_mode();
}

#[cfg(unix)]
fn run_attach(session: String, config_dir: PathBuf) -> Result<()> {
    let stream = ipc::connect(&session, &config_dir, true, terminal::size().unwrap_or((80, 24)))?;
    let enhanced = setup_terminal()?;
    let (cols, rows) = terminal::size()?;
    let images = ipc::detect_image_mode();
    let result = ipc::run_client(stream, Hello::local(cols, rows, enhanced, images));
    restore_terminal(enhanced);
    match result? {
        ClientEnd::Detached(msg) => println!("[{msg} from session {session}]"),
        ClientEnd::Exited => println!("[session {session} exited]"),
        ClientEnd::Lost => println!("[lost connection to session {session}]"),
    }
    Ok(())
}

fn valid_session_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= ipc::MAX_SESSION_NAME
        && !s.starts_with('.')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

#[cfg(unix)]
/// The server has its own working directory, so relative screenshot paths are made absolute
/// here, where the user typed them.
fn absolutize_paths(args: &mut [String]) {
    let Ok(cwd) = std::env::current_dir() else { return };
    for a in args {
        let keyword = matches!(a.as_str(), "screen" | "window" | "txt" | "text" | "ansi" | "html" | "htm" | "svg");
        let relative = !a.starts_with('/') && !a.starts_with('~');
        if !keyword && relative && a.parse::<u64>().is_err() {
            *a = cwd.join(&*a).display().to_string();
        }
    }
}

fn run_standalone(session: String, config_dir: PathBuf) -> Result<()> {
    // Both would restore and save the same session file and duplicate each other's windows.
    #[cfg(unix)]
    if ipc::list_sessions().contains(&session) {
        bail!(
            "session '{session}' is running in the background; attach with `tuidesk -S {session}` or pick another -S name"
        );
    }
    let enhanced = setup_terminal()?;
    let result = (|| -> Result<()> {
        let (cols, rows) = terminal::size()?;
        let images = ipc::detect_image_mode();
        let display = Display::local(Hello::local(cols, rows, enhanced, images))?;
        let (tx, rx) = mpsc::channel();
        let input_tx = tx.clone();
        std::thread::Builder::new().name("input".into()).spawn(move || {
            while let Ok(ev) = event::read() {
                if input_tx.send(AppEvent::Input(ev)).is_err() {
                    break;
                }
            }
        })?;
        let mut app = App::new(config_dir, session, tx, Some(display), false, (cols, rows));
        app.run(rx)
    })();
    restore_terminal(enhanced);
    result
}

#[cfg(unix)]
fn run_server(session: String, config_dir: PathBuf, size: (u16, u16)) -> Result<()> {
    let (listener, path) = ipc::bind(&session)?;
    let (tx, rx) = mpsc::channel();
    ipc::serve(listener, tx.clone());
    let mut app = App::new(config_dir, session, tx, None, true, size);
    let result = app.run(rx);
    drop(app);
    let _ = std::fs::remove_file(path);
    result
}
