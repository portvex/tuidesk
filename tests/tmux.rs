//! End-to-end tests: run the real binary inside a headless tmux and read the screen back.
//!
//! Each test gets its own tmux server, config, runtime and state directories, so tests can
//! run in parallel. They are skipped (with a message) when tmux is not installed.
// These tests drive the Unix client/server (and tmux); Windows runs tuidesk standalone only.
#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_tuidesk");
const COLS: u16 = 100;
const ROWS: u16 = 30;
const TIMEOUT: Duration = Duration::from_secs(10);

fn tmux_available() -> bool {
    Command::new("tmux").arg("-V").output().is_ok_and(|o| o.status.success())
}

struct Desk {
    name: String,
    dir: PathBuf,
}

impl Desk {
    /// Start a tuidesk client in tmux. `extra_config` is appended to a minimal test config.
    fn start(test: &str, extra_config: &str) -> Option<Self> {
        if !tmux_available() {
            eprintln!("skipping {test}: tmux not installed");
            return None;
        }
        let name = format!("tuidesk-it-{test}-{}", std::process::id());
        let dir = std::env::temp_dir().join(&name);
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["cfg", "run", "state", "work"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        // The screen is read back as text, so the title bar buttons stay text even when the
        // terminal running the tests shows images (Sixel through tmux, for one).
        let config = format!(
            "[general]\nshell = \"/bin/sh\"\nnotification_timeout = 30\npixel_buttons = false\n{extra_config}\n\n\
             [[apps]]\nname = \"Echo Test\"\ncommand = \"sh -c 'echo LAUNCHED-$((40+2)); sleep 600'\"\ncategory = \"Test\"\n"
        );
        std::fs::write(dir.join("cfg/config.toml"), config).unwrap();
        std::fs::write(dir.join("cfg/init.lua"), "-- empty for tests\n").unwrap();
        // A stand-in for $EDITOR that shows the file and stays open.
        let editor = dir.join("work/editor.sh");
        std::fs::write(&editor, "#!/bin/sh\necho EDITING \"$1\"\ncat \"$1\"\nsleep 600\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();
        let desk = Self { name, dir };
        desk.tmux(&[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "main",
            "-x",
            &COLS.to_string(),
            "-y",
            &ROWS.to_string(),
        ]);
        desk.tmux(&["set-option", "-g", "remain-on-exit", "on"]);
        desk.run_client();
        desk.wait_for("the panel", |s| s.contains(" tuidesk ") && s.contains("[floating]"));
        Some(desk)
    }

    fn env(&self) -> String {
        format!(
            "env TUIDESK_RUNTIME_DIR={run} XDG_STATE_HOME={state} SHELL=/bin/sh PS1='$ ' HOME={work} EDITOR={work}/editor.sh",
            run = self.dir.join("run").display(),
            state = self.dir.join("state").display(),
            work = self.dir.join("work").display(),
        )
    }

    /// (Re)start the client in the tmux pane; it prints CLIENT-DONE when it returns.
    fn run_client(&self) {
        let cmd = format!(
            "cd {work} && {env} {BIN} -c {cfg}; echo CLIENT-DONE; sleep 600",
            work = self.dir.join("work").display(),
            env = self.env(),
            cfg = self.dir.join("cfg").display(),
        );
        self.tmux(&["respawn-pane", "-k", "-t", "main", &cmd]);
    }

    fn tmux(&self, args: &[&str]) -> String {
        let out = Command::new("tmux").arg("-L").arg(&self.name).args(args).output().expect("running tmux");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn keys(&self, keys: &[&str]) {
        let mut args = vec!["send-keys", "-t", "main"];
        args.extend_from_slice(keys);
        self.tmux(&args);
        std::thread::sleep(Duration::from_millis(150));
    }

    fn type_line(&self, text: &str) {
        self.tmux(&["send-keys", "-t", "main", "-l", text]);
        self.keys(&["Enter"]);
    }

    /// Wait until the shell has run every line typed so far: it echoes `tag` with a computed
    /// suffix, which the typed line itself does not show.
    fn shell_done(&self, tag: &str) {
        self.type_line(&format!("echo {tag}-$((40+2))"));
        self.wait_for(tag, |s| s.contains(&format!("{tag}-42")));
    }

    fn prefix(&self, key: &str) {
        self.keys(&["C-Space"]);
        self.keys(&[key]);
    }

    /// Send a raw SGR mouse event at 0-based screen coordinates.
    fn mouse(&self, button: u16, x: usize, y: usize, release: bool) {
        let fin = if release { 'm' } else { 'M' };
        let seq = format!("\x1b[<{button};{};{}{fin}", x + 1, y + 1);
        self.tmux(&["send-keys", "-t", "main", "-l", &seq]);
        std::thread::sleep(Duration::from_millis(30));
    }

    fn screen(&self) -> String {
        self.tmux(&["capture-pane", "-p", "-t", "main"])
    }

    fn wait_for(&self, what: &str, pred: impl Fn(&str) -> bool) -> String {
        let start = Instant::now();
        loop {
            let s = self.screen();
            if pred(&s) {
                return s;
            }
            if start.elapsed() > TIMEOUT {
                let log = self.server_log();
                panic!("timed out waiting for {what}\n--- screen ---\n{s}\n--- server log ---\n{log}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn server_log(&self) -> String {
        std::fs::read_dir(self.dir.join("run"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
            .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
            .collect()
    }

    fn open_shell(&self) {
        let before = self.screen().matches("[x]").count();
        self.prefix("Enter");
        self.wait_for("a new window", |s| s.matches("[x]").count() > before);
        std::thread::sleep(Duration::from_millis(300));
    }

    /// Run `tuidesk msg ARGS` against this test's session: (success, output).
    fn msg(&self, args: &[&str]) -> (bool, String) {
        let out = Command::new(BIN)
            .arg("msg")
            .args(args)
            .env("TUIDESK_RUNTIME_DIR", self.dir.join("run"))
            .env_remove("TUIDESK_SESSION")
            .output()
            .expect("running tuidesk msg");
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        (out.status.success(), text)
    }

    /// Screen row and column of the first title bar containing `title`.
    fn title_pos(&self, title: &str) -> (usize, usize) {
        let s = self.screen();
        let row = s.lines().position(|l| l.contains(title) && l.contains("[x]")).expect("title bar");
        let line = s.lines().nth(row).unwrap();
        (line[..line.find(title).unwrap()].chars().count(), row)
    }

    fn work(&self) -> PathBuf {
        self.dir.join("work")
    }
}

impl Drop for Desk {
    fn drop(&mut self) {
        let _ = Command::new(BIN).args(["kill"]).env("TUIDESK_RUNTIME_DIR", self.dir.join("run")).output();
        self.tmux(&["kill-server"]);
        // Last resort: no server for this test's config may outlive it.
        let _ = Command::new("pkill").arg("-f").arg(self.dir.join("cfg")).output();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Lines whose window content (between the side borders) is exactly `text`.
fn content_lines(screen: &str, text: &str) -> usize {
    screen.lines().filter(|l| l.split('│').any(|cell| cell.trim() == text)).count()
}

/// (column, row) in characters of the first line whose window content is exactly `text`.
fn find_output(screen: &str, text: &str) -> Option<(usize, usize)> {
    screen.lines().enumerate().find_map(|(row, line)| {
        if !line.split('│').any(|c| c.trim() == text) {
            return None;
        }
        let byte = line.find(text)?;
        Some((line[..byte].chars().count(), row))
    })
}

#[test]
fn shell_window_runs_commands() {
    let Some(d) = Desk::start("shell", "") else { return };
    d.open_shell();
    d.type_line("echo hel''lo-$((6*7))");
    d.wait_for("command output", |s| content_lines(s, "hello-42") == 1);
}

#[test]
fn hvp_cursor_positioning_lands_in_place() {
    // Regression test for btop, which positions everything with CSI row;col f.
    let Some(d) = Desk::start("hvp", "") else { return };
    d.open_shell();
    d.prefix("m");
    d.type_line("clear; printf '\\033[5;10fHVP-OK'; sleep 600");
    let s = d.wait_for("HVP output", |s| s.contains("HVP-OK"));
    // Maximized: content starts at screen column 1, row 1, so CSI 5;10 f is row 5, column 10.
    let line: Vec<char> = s.lines().nth(5).unwrap().chars().collect();
    let found: String = line[10..16].iter().collect();
    assert_eq!(found, "HVP-OK", "screen:\n{s}");
}

#[test]
fn kickoff_searches_and_launches() {
    let Some(d) = Desk::start("kickoff", "") else { return };
    d.prefix("Space");
    d.wait_for("the launcher", |s| s.contains("Applications") && s.contains("Search:"));
    d.tmux(&["send-keys", "-t", "main", "-l", "echo te"]);
    d.wait_for("filtered results", |s| s.contains("Echo Test") && s.contains("Run: echo te"));
    d.keys(&["Enter"]);
    d.wait_for("the launched app", |s| s.contains("LAUNCHED-42"));
}

#[test]
fn kickoff_scrolls_a_long_list() {
    let apps: String = (1..=30)
        .map(|i| format!("[[apps]]\nname = \"Item {i:02}\"\ncommand = \"sh\"\ncategory = \"Long\"\n"))
        .collect();
    let Some(d) = Desk::start("kickoff-scroll", &format!("builtin_apps = false\n{apps}")) else { return };
    d.prefix("Space");
    let s = d.wait_for("the launcher", |s| s.contains("Item 01"));
    assert!(!s.contains("Item 30") && s.contains('┃'), "{s}");
    let (x, y) = find_text(&s, "Item 05").unwrap();
    for _ in 0..10 {
        d.mouse(65, x, y, false);
    }
    let s = d.wait_for("the end of the list", |s| s.contains("Item 30"));
    assert!(!s.contains("Item 01"), "{s}");
    // Clicking the top of the scrollbar goes back to the start.
    let bar_x = find_text(&s, "┃").unwrap().0;
    let top = find_text(&s, "Search:").unwrap().1 + 3;
    d.mouse(0, bar_x, top, false);
    d.mouse(0, bar_x, top, true);
    d.wait_for("the start of the list", |s| s.contains("Item 01") && !s.contains("Item 30"));
}

#[test]
fn kickoff_category_tabs_scroll() {
    let apps: String = ["Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot", "Golf", "Hotel", "India"]
        .iter()
        .map(|c| format!("[[apps]]\nname = \"{c} App\"\ncommand = \"sh\"\ncategory = \"Category {c}\"\n"))
        .collect();
    let Some(d) = Desk::start("kickoff-tabs", &format!("builtin_apps = false\n{apps}")) else { return };
    d.prefix("Space");
    let s = d.wait_for("the launcher", |s| s.contains("Search:") && s.contains(" All "));
    assert!(s.contains('›') && !s.contains('‹') && !s.contains(" Session "), "{s}");
    // Left from "All" wraps to the last tab, which scrolls into view.
    d.keys(&["Left"]);
    let s = d.wait_for("the last tab", |s| s.contains(" Session "));
    assert!(s.contains('‹') && !s.contains(" All "), "{s}");
    // The left arrow brings back the hidden tab before the first one shown.
    let (x, y) = find_text(&s, "‹").unwrap();
    for _ in 0..12 {
        d.mouse(0, x, y, false);
        d.mouse(0, x, y, true);
    }
    d.wait_for("the first tab again", |s| s.contains(" All ") && !s.contains('‹'));
}

#[test]
fn virtual_desktops_switch_and_move_windows() {
    let Some(d) = Desk::start("desktops", "") else { return };
    d.open_shell();
    d.type_line("echo DESK-$((1+0))");
    d.wait_for("output on desktop 1", |s| content_lines(s, "DESK-1") == 1);
    d.prefix("2");
    d.wait_for("empty desktop 2", |s| !s.contains("DESK-1"));
    d.prefix("1");
    d.wait_for("desktop 1 again", |s| content_lines(s, "DESK-1") == 1);
    // Move the window to desktop 2 (following it), then go back to the now empty desktop 1.
    d.prefix(">");
    d.wait_for("window followed to desktop 2", |s| content_lines(s, "DESK-1") == 1);
    d.prefix("1");
    d.wait_for("desktop 1 empty", |s| !s.contains("DESK-1"));
}

#[test]
fn tiling_places_windows_side_by_side() {
    let Some(d) = Desk::start("tiling", "") else { return };
    d.open_shell();
    d.open_shell();
    d.prefix("t");
    d.wait_for("two title bars on one row", |s| s.lines().any(|l| l.matches("[x]").count() == 2));
    assert!(d.screen().contains("[master]"));
}

#[test]
fn quick_tile_by_dragging_to_the_edge() {
    let Some(d) = Desk::start("drag", "") else { return };
    d.open_shell();
    let s = d.screen();
    let row = s.lines().position(|l| l.contains("[x]")).unwrap();
    let col = s.lines().nth(row).unwrap().chars().position(|c| c != ' ').unwrap() + 5;
    d.mouse(0, col, row, false);
    d.mouse(32, col - 5, row + 2, false);
    d.mouse(32, 0, row + 2, false);
    d.mouse(0, 0, row + 2, true);
    // Left half of a 100 column screen: the frame spans columns 0-49, so [x] starts at 46.
    d.wait_for("window tiled to the left half", |s| {
        s.lines().any(|l| l.find("[x]").is_some_and(|b| l[..b].chars().count() == 46))
    });
}

#[test]
fn selection_copies_and_middle_click_pastes() {
    let Some(d) = Desk::start("select", "") else { return };
    d.open_shell();
    d.type_line("echo SEL$((1))ECT");
    let s = d.wait_for("output", |s| content_lines(s, "SEL1ECT") == 1);
    let (x, y) = find_output(&s, "SEL1ECT").unwrap();
    // Double-click selects the word and copies it.
    for _ in 0..2 {
        d.mouse(0, x + 2, y, false);
        d.mouse(0, x + 2, y, true);
    }
    d.wait_for("selection drawn", |_| true);
    d.tmux(&["send-keys", "-t", "main", "-l", "echo "]);
    d.mouse(1, x + 2, y, false);
    d.mouse(1, x + 2, y, true);
    d.keys(&["Enter"]);
    d.wait_for("pasted word echoed back", |s| content_lines(s, "SEL1ECT") == 2);
}

#[test]
fn alt_tab_switches_to_previous_window() {
    let Some(d) = Desk::start("switcher", "") else { return };
    d.open_shell();
    d.type_line("echo WIN-$((0+1))");
    d.wait_for("first window", |s| content_lines(s, "WIN-1") == 1);
    d.open_shell();
    d.type_line("echo WIN-$((1+1))");
    d.wait_for("second window", |s| content_lines(s, "WIN-2") == 1);
    d.keys(&["M-Tab"]);
    d.wait_for("switcher overlay", |s| s.contains("Windows"));
    // Without key release events (tmux), the switcher commits after a short idle time.
    d.wait_for("switcher closed", |s| !s.contains(" Windows "));
    // The focused window is now the first one: closing it leaves only the second.
    d.prefix("x");
    d.wait_for("first window closed", |s| content_lines(s, "WIN-1") == 0 && content_lines(s, "WIN-2") == 1);
}

#[test]
fn detach_and_reattach_keeps_windows() {
    let Some(d) = Desk::start("detach", "") else { return };
    d.open_shell();
    d.type_line("echo KEEP-$((2*2))");
    d.wait_for("output", |s| content_lines(s, "KEEP-4") == 1);
    d.prefix("d");
    d.wait_for("detached message", |s| s.contains("[detached from session default]") && s.contains("CLIENT-DONE"));
    d.run_client();
    d.wait_for("same window after reattach", |s| content_lines(s, "KEEP-4") == 1);
}

#[test]
fn session_restores_windows_and_directories() {
    let Some(d) = Desk::start("restore", "") else { return };
    let sub = d.work().join("project-dir");
    std::fs::create_dir_all(&sub).unwrap();
    d.open_shell();
    d.type_line(&format!("cd {}", sub.display()));
    // The session saves the shell's directory, so the cd must have run.
    d.shell_done("cd-done");
    d.prefix("q");
    d.wait_for("exit message", |s| s.contains("[session default exited]"));
    d.run_client();
    d.wait_for("restored window", |s| s.contains("[x]"));
    std::thread::sleep(Duration::from_millis(300));
    d.type_line("pwd");
    d.wait_for("restored directory", |s| s.lines().any(|l| l.contains("/project-dir") && !l.contains("pwd")));
}

#[test]
fn config_changes_reload_live() {
    let Some(d) = Desk::start("reload", "") else { return };
    let cfg = d.dir.join("cfg/config.toml");
    let mut text = std::fs::read_to_string(&cfg).unwrap();
    text.push_str("\n[panel]\nkickoff_label = \" RELOADED \"\n");
    std::fs::write(&cfg, text).unwrap();
    d.wait_for("new panel label", |s| s.contains("RELOADED"));
}

#[test]
fn programs_can_post_notifications() {
    let Some(d) = Desk::start("notify", "") else { return };
    d.open_shell();
    d.type_line("printf '\\033]9;ping-%s\\007' pong");
    d.wait_for("notification popup", |s| s.contains("ping-pong") && s.contains("[1 new]"));
}

#[test]
fn copy_mode_searches_selects_and_pastes() {
    let Some(d) = Desk::start("copymode", "") else { return };
    d.open_shell();
    d.type_line("printf \"alpha\\nbravo needle-$((7))\\ncharlie\\n\"");
    d.wait_for("output", |s| s.contains("needle-7") && content_lines(s, "charlie") == 1);
    d.prefix("[");
    d.wait_for("copy mode status", |s| s.contains("COPY"));
    d.tmux(&["send-keys", "-t", "main", "-l", "?needle"]);
    d.keys(&["Enter"]);
    d.keys(&["v"]);
    d.keys(&["E"]);
    d.keys(&["y"]);
    d.wait_for("copied notification", |s| s.contains("Copied") && !s.contains("COPY "));
    d.tmux(&["send-keys", "-t", "main", "-l", "echo "]);
    d.prefix("]");
    d.keys(&["Enter"]);
    d.wait_for("pasted text echoed", |s| content_lines(s, "needle-7") == 1);
}

#[test]
fn clipboard_history_pastes_older_entries() {
    let Some(d) = Desk::start("clips", "") else { return };
    d.open_shell();
    // Programs copy with OSC 52; each copy lands in the history.
    d.type_line("printf '\\033]52;c;%s\\007' $(printf FIRST-CLIP | base64)");
    d.type_line("printf '\\033]52;c;%s\\007' $(printf SECOND-CLIP | base64)");
    d.shell_done("copied");
    d.prefix("v");
    let s = d.wait_for("clipboard picker", |s| s.contains("Clipboard") && s.contains("FIRST-CLIP"));
    let picker = &s[s.find("Clipboard").unwrap()..];
    let first = picker.find("FIRST-CLIP").unwrap();
    let second = picker.find("SECOND-CLIP").unwrap();
    assert!(second < first, "newest entry should be listed first:\n{s}");
    d.keys(&["Escape"]);
    d.tmux(&["send-keys", "-t", "main", "-l", "echo "]);
    d.prefix("v");
    d.wait_for("picker again", |s| s.contains("Clipboard"));
    d.keys(&["2"]);
    d.keys(&["Enter"]);
    d.wait_for("older entry pasted", |s| content_lines(s, "FIRST-CLIP") == 1);
}

#[test]
fn help_lists_and_filters_bindings() {
    let Some(d) = Desk::start("help", "") else { return };
    d.prefix("?");
    d.wait_for("cheat sheet", |s| s.contains(" Keys ") && s.contains("copy_mode"));
    d.tmux(&["send-keys", "-t", "main", "-l", "detach"]);
    d.wait_for("filtered to one binding", |s| s.contains("(1 key)") && s.contains("ctrl+space, d"));
    d.keys(&["Escape"]);
    d.wait_for("closed", |s| !s.contains(" Keys "));
}

#[test]
fn window_menu_moves_window_to_another_desktop() {
    let Some(d) = Desk::start("winmenu", "") else { return };
    d.open_shell();
    d.type_line("echo MENU-$((5*5))");
    d.wait_for("output", |s| content_lines(s, "MENU-25") == 1);
    let (x, y) = d.title_pos("[_]");
    d.mouse(2, x - 4, y, false);
    d.mouse(2, x - 4, y, true);
    let s = d.wait_for("window menu", |s| s.contains("Move to desktop 3") && s.contains("Keep above others"));
    let row = s.lines().position(|l| l.contains("Move to desktop 3")).unwrap();
    let col =
        s.lines().nth(row).unwrap().find("Move").map(|b| s.lines().nth(row).unwrap()[..b].chars().count()).unwrap();
    d.mouse(0, col + 2, row, false);
    d.mouse(0, col + 2, row, true);
    d.wait_for("window left desktop 1", |s| !s.contains("MENU-25"));
    d.prefix("3");
    d.wait_for("window on desktop 3", |s| content_lines(s, "MENU-25") == 1);
}

#[test]
fn rename_and_keep_above() {
    let Some(d) = Desk::start("rename", "") else { return };
    d.open_shell();
    d.prefix("R");
    d.wait_for("rename prompt", |s| s.contains("Rename window"));
    d.keys(&["C-u"]);
    d.tmux(&["send-keys", "-t", "main", "-l", "My Renamed Window"]);
    d.keys(&["Enter"]);
    d.wait_for("new title", |s| s.contains("My Renamed Window") && s.contains("[x]"));
    d.prefix("a");
    d.wait_for("on top marker", |s| s.contains("My Renamed Window  [on top]"));
}

#[test]
fn grid_and_bsp_layouts() {
    let Some(d) = Desk::start("layouts", "") else { return };
    for _ in 0..4 {
        d.open_shell();
    }
    let (ok, out) = d.msg(&["layout", "grid"]);
    assert!(ok, "{out}");
    d.wait_for("2x2 grid", |s| s.lines().filter(|l| l.matches("[x]").count() == 2).count() == 2);
    assert!(d.screen().contains("[grid]"));
    d.prefix("x");
    d.msg(&["layout", "bsp"]);
    // Three windows in bsp: left half, then the right half split top and bottom.
    d.wait_for("bsp split", |s| {
        let bars: Vec<usize> = s.lines().map(|l| l.matches("[x]").count()).filter(|&n| n > 0).collect();
        bars == vec![2, 1]
    });
}

#[test]
fn rules_send_windows_to_a_desktop() {
    let rule = "[[rules]]\ntitle = \"Echo Test\"\ndesktop = 3\n";
    let Some(d) = Desk::start("rules", rule) else { return };
    let (ok, out) = d.msg(&["launch", "Echo", "Test"]);
    assert!(ok, "{out}");
    std::thread::sleep(Duration::from_millis(500));
    assert!(!d.screen().contains("LAUNCHED-42"), "should not open on the current desktop");
    d.prefix("3");
    d.wait_for("window on desktop 3", |s| s.contains("LAUNCHED-42"));
}

#[test]
fn msg_controls_the_session() {
    let Some(d) = Desk::start("msg", "") else { return };
    let (ok, out) = d.msg(&["notify", "From", "a", "script"]);
    assert!(ok, "{out}");
    d.wait_for("notification", |s| s.contains("a script"));
    let (ok, id) = d.msg(&["spawn", "sh -c 'echo MSG-$((3*3)); sleep 600'"]);
    assert!(ok && id.trim().parse::<u64>().is_ok(), "{id}");
    d.wait_for("spawned window", |s| s.contains("MSG-9"));
    let (_, list) = d.msg(&["windows"]);
    assert!(list.lines().any(|l| l.starts_with(&format!("{}\t1\t*", id.trim()))), "{list}");
    d.msg(&["desktop", "2"]);
    assert_eq!(d.msg(&["desktops"]).1.trim(), "2/4");
    assert_eq!(d.msg(&["lua", "return 6 * 7"]).1.trim(), "42");
    let (ok, err) = d.msg(&["bogus-command"]);
    assert!(!ok && err.contains("unknown command"), "{err}");
    // From inside a window, `tuidesk msg` finds its own session through TUIDESK_SESSION.
    d.msg(&["desktop", "1"]);
    d.open_shell();
    d.type_line(&format!("{BIN} msg notify inside from-window"));
    d.wait_for("notification from a window", |s| s.contains("from-window"));
}

/// Row of the panel (the last non-empty screen line).
fn panel_row(screen: &str) -> usize {
    let lines: Vec<&str> = screen.lines().collect();
    lines.iter().rposition(|l| !l.trim().is_empty()).unwrap()
}

/// Column of `text` in the panel.
fn panel_col(screen: &str, text: &str) -> usize {
    let line = screen.lines().nth(panel_row(screen)).unwrap();
    let b = line.find(text).unwrap_or_else(|| panic!("{text:?} not in panel: {line}"));
    line[..b].chars().count()
}

const NAMED: &str = "desktop_names = [\"Main\", \"Code\"]\n";

#[test]
fn runner_calculates_and_copies() {
    let Some(d) = Desk::start("runcalc", "") else { return };
    d.open_shell();
    d.prefix("r");
    d.wait_for("runner", |s| s.contains(" Run ") && s.contains("> "));
    d.tmux(&["send-keys", "-t", "main", "-l", "2*(3+4)"]);
    d.wait_for("result", |s| s.contains("= 14"));
    d.keys(&["Enter"]);
    d.wait_for("copied", |s| s.contains("Copied 14"));
    d.tmux(&["send-keys", "-t", "main", "-l", "echo "]);
    d.prefix("]");
    d.keys(&["Enter"]);
    d.wait_for("pasted result", |s| content_lines(s, "14") == 1);
}

#[test]
fn runner_switches_to_a_window_by_title() {
    let Some(d) = Desk::start("runwin", "") else { return };
    for title in ["AlphaWin", "BetaWin"] {
        let (ok, out) = d.msg(&["spawn", &format!(r#"sh -c "printf '\033]2;{title}\007'; sleep 600""#)]);
        assert!(ok, "{out}");
        d.wait_for("titled window", |s| s.contains(title));
    }
    d.prefix("r");
    d.tmux(&["send-keys", "-t", "main", "-l", "alphaw"]);
    d.wait_for("window result", |s| s.contains("Window") && s.contains("AlphaWin  (desktop 1)"));
    d.keys(&["Enter"]);
    let start = Instant::now();
    loop {
        let list = d.msg(&["windows"]).1;
        if list.lines().any(|l| l.contains("\t*\tAlphaWin")) {
            break;
        }
        assert!(start.elapsed() < TIMEOUT, "AlphaWin not focused:\n{list}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn runner_opens_a_shell_in_a_directory() {
    let Some(d) = Desk::start("rundir", "") else { return };
    let target = d.work().join("deep").join("target-dir");
    std::fs::create_dir_all(&target).unwrap();
    d.prefix("r");
    let q = format!("{}/deep/targ", d.work().display());
    d.tmux(&["send-keys", "-t", "main", "-l", &q]);
    d.wait_for("folder result", |s| s.contains("Folder") && s.contains("target-dir"));
    d.keys(&["Enter"]);
    d.wait_for("new window", |s| s.contains("[x]"));
    std::thread::sleep(Duration::from_millis(300));
    d.type_line("pwd");
    d.wait_for("shell in the directory", |s| s.lines().any(|l| l.contains("/deep/target-dir") && !l.contains("pwd")));
}

#[test]
fn runner_runs_actions() {
    let Some(d) = Desk::start("runact", "") else { return };
    d.prefix("r");
    d.tmux(&["send-keys", "-t", "main", "-l", "layout gr"]);
    d.wait_for("action result", |s| s.contains("Action") && s.contains("layout grid"));
    d.keys(&["Enter"]);
    d.wait_for("grid layout", |s| s.contains("[grid]"));
}

#[test]
fn named_desktops_show_in_pager_and_on_switch() {
    let Some(d) = Desk::start("names", NAMED) else { return };
    d.wait_for("names in pager", |s| s.contains(" Main ") && s.contains(" Code ") && s.contains(" 3 "));
    d.prefix("2");
    d.wait_for("switch label", |s| s.contains("Desktop 2: Code"));
    d.wait_for("label fades", |s| !s.contains("Desktop 2: Code"));
}

#[test]
fn pager_right_click_renames_a_desktop() {
    let Some(d) = Desk::start("renamedesk", NAMED) else { return };
    let s = d.screen();
    let col = panel_col(&s, " 3 ") + 1;
    let row = panel_row(&s);
    d.mouse(2, col, row, false);
    d.mouse(2, col, row, true);
    d.wait_for("rename prompt", |s| s.contains("Rename desktop 3"));
    d.tmux(&["send-keys", "-t", "main", "-l", "Mail"]);
    d.keys(&["Enter"]);
    d.wait_for("new name in pager", |s| s.contains(" Mail "));
}

#[test]
fn drag_task_and_window_onto_pager() {
    let Some(d) = Desk::start("dragpager", NAMED) else { return };
    d.open_shell();
    d.type_line("echo DRAG-$((1))");
    d.wait_for("output", |s| content_lines(s, "DRAG-1") == 1);
    let s = d.screen();
    let row = panel_row(&s);
    let task = panel_col(&s, " sh ") + 1;
    let code = panel_col(&s, " Code ") + 2;
    d.mouse(0, task, row, false);
    d.mouse(32, (task + code) / 2, row, false);
    d.mouse(32, code, row, false);
    d.mouse(0, code, row, true);
    d.wait_for("window left desktop 1", |s| !s.contains("DRAG-1"));
    d.prefix("2");
    d.wait_for("window on Code", |s| content_lines(s, "DRAG-1") == 1);

    // Now drag the window by its title bar onto "Main".
    let (x, y) = d.title_pos("[_]");
    let s = d.screen();
    let main = panel_col(&s, " Main ") + 2;
    let grab = x.saturating_sub(10);
    d.mouse(0, grab, y, false);
    d.mouse(32, grab, y + 3, false);
    d.mouse(32, main, row, false);
    d.mouse(0, main, row, true);
    d.wait_for("window left Code", |s| !s.contains("DRAG-1"));
    d.prefix("1");
    d.wait_for("window back on Main", |s| content_lines(s, "DRAG-1") == 1);
}

/// Screen position (column, row) of the first occurrence of `text`.
fn find_text(screen: &str, text: &str) -> Option<(usize, usize)> {
    screen.lines().enumerate().find_map(|(row, line)| line.find(text).map(|b| (line[..b].chars().count(), row)))
}

fn spawn_on(d: &Desk, desktop: &str, marker: &str) -> String {
    d.msg(&["desktop", desktop]);
    let (ok, id) = d.msg(&["spawn", &format!("sh -c 'echo {marker}; sleep 600'")]);
    assert!(ok, "{id}");
    d.wait_for("spawned window", |s| s.contains(marker));
    id.trim().to_string()
}

#[test]
fn overview_shows_every_desktop_and_jumps() {
    let Some(d) = Desk::start("overview", NAMED) else { return };
    spawn_on(&d, "1", "OV-ONE");
    spawn_on(&d, "2", "OV-TWO");
    d.msg(&["desktop", "1"]);
    d.prefix("W");
    d.wait_for("both windows as cards", |s| {
        s.contains("OV-ONE") && s.contains("OV-TWO") && s.contains("1: Main (1)") && s.contains("2: Code (1)")
    });
    d.tmux(&["send-keys", "-t", "main", "-l", "ov-two"]);
    d.wait_for("filtered", |s| s.contains("Filter: ov-two") && !s.contains("OV-ONE"));
    d.keys(&["Enter"]);
    d.wait_for("overview closed", |s| !s.contains("Filter:"));
    assert_eq!(d.msg(&["desktops"]).1.trim(), "2/4");
}

#[test]
fn overview_drag_card_to_desktop_and_delete() {
    let Some(d) = Desk::start("ovdrag", NAMED) else { return };
    let one = spawn_on(&d, "1", "OV-DRAG");
    spawn_on(&d, "1", "OV-KEEP");
    d.prefix("W");
    let s = d.wait_for("cards", |s| s.contains("OV-DRAG") && s.contains("OV-KEEP"));
    let (cx, cy) = find_text(&s, "OV-DRAG").unwrap();
    let (bx, by) = find_text(&s, "3 (0)").unwrap();
    d.mouse(0, cx + 2, cy, false);
    d.mouse(32, (cx + bx) / 2, (cy + by) / 2, false);
    d.mouse(32, bx + 1, by, false);
    d.mouse(0, bx + 1, by, true);
    d.wait_for("desktop 3 has the window", |s| s.contains("3 (1)"));
    let list = d.msg(&["windows"]).1;
    assert!(list.lines().any(|l| l.starts_with(&format!("{one}\t3\t"))), "{list}");
    // Delete closes the selected card's window.
    d.tmux(&["send-keys", "-t", "main", "-l", "ov-keep"]);
    d.wait_for("filtered", |s| s.contains("Filter: ov-keep"));
    d.keys(&["Delete"]);
    d.wait_for("window closed", |s| s.contains("1: Main (0)"));
}

#[test]
fn wallpapers_differ_per_desktop() {
    let cfg = "[wallpaper.default]\nkind = \"pattern\"\ntext = \"x-\"\n\
               [wallpaper.2]\nkind = \"art\"\ntext = \"WALL-ART-2\"\nposition = \"top-left\"\n";
    let Some(d) = Desk::start("wallpaper", cfg) else { return };
    d.wait_for("pattern on desktop 1", |s| s.contains("x-x-x-x-") && !s.contains("WALL-ART-2"));
    d.prefix("2");
    d.wait_for("art on desktop 2", |s| s.contains("WALL-ART-2") && !s.contains("x-x-x-x-"));
}

#[test]
fn desktop_widgets_show_and_notes_open_in_editor() {
    let cfg = "[[desktop_widgets]]\nkind = \"notes\"\nfile = \"~/todo.txt\"\nposition = \"top-left\"\ntitle = \"Todo\"\n\
               [[desktop_widgets]]\nkind = \"sysinfo\"\nposition = \"bottom-right\"\ndesktops = [2]\n\
               [[desktop_widgets]]\nkind = \"clock\"\nformat = \"%Y\"\nposition = \"top-right\"\n";
    let Some(d) = Desk::start("widgets", cfg) else { return };
    std::fs::write(d.work().join("todo.txt"), "buy-milk\nwrite-tests\n").unwrap();
    let s = d.wait_for("notes widget", |s| s.contains("Todo") && s.contains("buy-milk") && s.contains("write-tests"));
    assert!(!s.contains("CPU ["), "sysinfo is only on desktop 2");
    // The clock shows the year in big digits: check the weekday line below it instead.
    let weekday = String::from_utf8(Command::new("date").arg("+%A").output().unwrap().stdout).unwrap();
    assert!(s.contains(weekday.trim()), "{s}");
    let (x, y) = find_text(&s, "buy-milk").unwrap();
    d.mouse(0, x, y, false);
    d.mouse(0, x, y, true);
    d.wait_for("editor window", |s| s.contains("EDITING") && s.contains("[x]"));
    d.prefix("2");
    d.wait_for("sysinfo on desktop 2", |s| s.contains("CPU [") && s.contains("RAM ["));
}

fn shots_config(d_work: &std::path::Path, format: &str) -> String {
    format!("[screenshot]\ndir = \"{}/shots\"\nformat = \"{format}\"\ncopy = \"path\"\n", d_work.display())
}

/// Files in the test's screenshot directory.
fn shots(d: &Desk) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> =
        std::fs::read_dir(d.work().join("shots")).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    v.sort();
    v
}

#[test]
fn msg_screenshots_of_screen_and_window() {
    let Some(d) = Desk::start("shotmsg", "") else { return };
    // Quoted for the inner shell, since < & > are shell syntax.
    let (ok, id) = d.msg(&["spawn", "sh -c \"echo 'SHOT-<&>-WIN'; sleep 600\""]);
    assert!(ok, "{id}");
    let id = id.trim().to_string();
    d.wait_for("spawned window", |s| s.contains("SHOT-<&>-WIN"));
    let txt = d.work().join("screen.txt");
    let (ok, out) = d.msg(&["screenshot", "txt", txt.to_str().unwrap()]);
    assert!(ok, "{out}");
    assert_eq!(out.trim(), txt.display().to_string());
    let text = std::fs::read_to_string(&txt).unwrap();
    assert_eq!(text.lines().count(), ROWS as usize, "{text}");
    assert!(text.contains("SHOT-<&>-WIN") && text.contains(" tuidesk "), "{text}");

    // A window shot has its title bar and border but not the panel.
    let svg = d.work().join("win.svg");
    let (ok, out) = d.msg(&["screenshot", id.as_str(), "svg", svg.to_str().unwrap()]);
    assert!(ok, "{out}");
    let svg = std::fs::read_to_string(&svg).unwrap();
    assert!(svg.starts_with("<svg") && svg.contains("SHOT-&lt;&amp;&gt;-WIN") && svg.contains("[x]"));
    assert!(!svg.contains("[floating]"), "panel must not be in a window shot");

    let ansi = d.work().join("win.ansi");
    d.msg(&["screenshot", "window", "ansi", ansi.to_str().unwrap()]);
    let ansi = std::fs::read_to_string(&ansi).unwrap();
    assert!(ansi.contains("\x1b[0;38;2;") && ansi.contains("SHOT-<&>-WIN"));

    let (ok, err) = d.msg(&["screenshot", "999"]);
    assert!(!ok && err.contains("no window 999"), "{err}");
}

#[test]
fn screenshot_keys_save_to_the_configured_directory() {
    let dir = std::env::temp_dir().join(format!("tuidesk-it-shotkeys-{}", std::process::id())).join("work");
    let Some(d) = Desk::start("shotkeys", &shots_config(&dir, "html")) else { return };
    d.open_shell();
    d.type_line("echo KEY-$((2+3))");
    d.wait_for("output", |s| content_lines(s, "KEY-5") == 1);
    d.prefix("P");
    d.wait_for("saved notification", |s| s.contains("Screenshot saved"));
    let files = shots(&d);
    assert_eq!(files.len(), 1, "{files:?}");
    let html = std::fs::read_to_string(&files[0]).unwrap();
    assert!(html.starts_with("<!DOCTYPE html>") && html.contains("KEY-5") && html.contains("[floating]"));
    // The notification itself is not part of the shot.
    assert!(!html.contains("Screenshot saved"));
    // copy = "path": the path is in the clipboard history.
    d.prefix("v");
    d.wait_for("path in clipboard history", |s| s.contains("Clipboard") && s.contains("tuidesk-it-sh"));
    d.keys(&["Escape"]);
    d.keys(&["C-Space"]);
    d.keys(&["C-p"]);
    d.wait_for("second shot", |_| shots(&d).len() == 2);
    let win = std::fs::read_to_string(&shots(&d)[1]).unwrap();
    assert!(win.contains("KEY-5") && !win.contains("[floating]"));
    // The first shot's popup is still on screen but is not captured.
    d.prefix("P");
    d.wait_for("third shot", |_| shots(&d).len() == 3);
    assert!(!std::fs::read_to_string(&shots(&d)[2]).unwrap().contains("Screenshot saved"));
}

#[test]
fn region_screenshot_by_dragging() {
    let dir = std::env::temp_dir().join(format!("tuidesk-it-shotregion-{}", std::process::id())).join("work");
    let Some(d) = Desk::start("shotregion", &shots_config(&dir, "txt")) else { return };
    d.open_shell();
    d.type_line("echo REGION-$((4*4)) OUTSIDE");
    let s = d.wait_for("output", |s| s.contains("REGION-16 OUTSIDE") && content_lines(s, "REGION-16 OUTSIDE") == 1);
    let (x, y) = s
        .lines()
        .enumerate()
        .find_map(|(row, l)| {
            (l.contains("REGION-16 OUTSIDE") && !l.contains("echo"))
                .then(|| (l[..l.find("REGION").unwrap()].chars().count(), row))
        })
        .unwrap();
    d.keys(&["C-Space"]);
    d.keys(&["M-p"]);
    d.wait_for("region hint", |s| s.contains("drag to select an area"));
    d.mouse(0, x, y, false);
    d.mouse(32, x + 9, y, false);
    d.mouse(0, x + 9, y, true);
    d.wait_for("saved", |s| s.contains("Screenshot saved"));
    assert_eq!(std::fs::read_to_string(&shots(&d)[0]).unwrap(), "REGION-16\n");
}

// ---------------------------------------------------------------- regressions from the review

#[test]
fn invalid_clock_format_does_not_crash() {
    let Some(d) = Desk::start("badclock", "[panel]\nclock_format = \"%Q\"\n") else { return };
    d.wait_for("placeholder clock", |s| s.lines().last().is_some_and(|l| l.trim_end().ends_with('?')));
    assert!(d.msg(&["windows"]).0, "server must still answer");
}

#[test]
fn commands_run_as_typed() {
    let Some(d) = Desk::start("cmdline", "") else { return };
    let (ok, out) = d.msg(&["spawn", "FOO=env-ok sh -c 'echo $FOO; sleep 600'"]);
    assert!(ok, "{out}");
    d.wait_for("env prefix", |s| s.contains("env-ok"));
    d.msg(&["spawn", "echo first && echo second-ran && sleep 600"]);
    d.wait_for("compound command", |s| s.contains("second-ran"));
}

#[test]
fn endless_lua_is_stopped() {
    let Some(d) = Desk::start("lualoop", "") else { return };
    let start = Instant::now();
    let (ok, err) = d.msg(&["lua", "while true do end"]);
    assert!(!ok && err.contains("stopped after"), "{err}");
    assert!(start.elapsed() < Duration::from_secs(8));
    assert!(d.msg(&["windows"]).0);
}

#[test]
fn window_menu_on_a_tiny_screen() {
    let Some(d) = Desk::start("tinymenu", "") else { return };
    d.open_shell();
    d.tmux(&["resize-window", "-t", "main", "-x", "30", "-y", "2"]);
    std::thread::sleep(Duration::from_millis(300));
    d.msg(&["window_menu"]);
    std::thread::sleep(Duration::from_millis(300));
    assert!(d.msg(&["windows"]).0, "server must survive the menu on a 2-row screen");
}

#[test]
fn paste_goes_to_the_runner_not_the_window() {
    let Some(d) = Desk::start("pasterunner", "") else { return };
    d.open_shell();
    d.prefix("r");
    d.wait_for("runner", |s| s.contains(" Run "));
    d.tmux(&["set-buffer", "2*21"]);
    d.tmux(&["paste-buffer", "-p", "-t", "main"]);
    d.wait_for("calculated", |s| s.contains("= 42"));
    d.keys(&["Escape"]);
    d.type_line("echo prompt-$((1))");
    let s = d.wait_for("shell output", |s| content_lines(s, "prompt-1") == 1);
    assert!(!s.contains("2*21echo"), "paste leaked into the shell:\n{s}");
}

#[test]
fn stalled_client_does_not_freeze_the_server() {
    let Some(d) = Desk::start("stall", "") else { return };
    d.msg(&["spawn", "sh -c 'while :; do date; done'"]);
    d.wait_for("busy window", |s| s.contains("[x]"));
    // The client process runs in the tmux pane; pause it as if its terminal hung.
    let client = d.tmux(&["display-message", "-p", "-t", "main", "#{pane_pid}"]);
    let pane: i32 = client.trim().parse().unwrap();
    let kids = Command::new("pgrep").arg("-P").arg(pane.to_string()).output().unwrap();
    let pid = String::from_utf8_lossy(&kids.stdout).lines().find_map(|l| l.trim().parse::<i32>().ok()).unwrap();
    Command::new("kill").args(["-STOP", &pid.to_string()]).status().unwrap();
    // The server keeps answering while the client's queue fills up, until it detaches the
    // client. How long that takes depends on how fast the machine draws frames.
    let start = Instant::now();
    let mut detached = false;
    while !detached && start.elapsed() < TIMEOUT * 2 {
        let asked = Instant::now();
        let ok = d.msg(&["windows"]).0;
        let took = asked.elapsed();
        assert!(ok && took < Duration::from_secs(1), "server answered in {took:?}");
        detached = d.server_log().contains("stopped reading");
        std::thread::sleep(Duration::from_millis(250));
    }
    Command::new("kill").args(["-CONT", &pid.to_string()]).status().unwrap();
    assert!(detached, "the stalled client was not detached");
    // The stalled client was detached; it notices and exits, and reattaching works.
    d.wait_for("client exited", |s| s.contains("CLIENT-DONE"));
    d.run_client();
    d.wait_for("reattached", |s| s.contains("[x]"));
}

#[test]
fn runtime_dir_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let Some(d) = Desk::start("perms", "") else { return };
    let mode = std::fs::metadata(d.dir.join("run")).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
}

#[test]
fn settings_dialog_previews_saves_and_discards() {
    let Some(d) = Desk::start("settings", "# keep this comment\n") else { return };
    let cfg = d.dir.join("cfg/config.toml");
    d.prefix("S");
    d.wait_for("settings dialog", |s| s.contains("System Settings") && s.contains("Virtual desktops"));
    // Panel page: moving the panel to the top previews at once.
    d.keys(&["4"]);
    d.wait_for("panel page", |s| s.contains("Kickoff label") && s.contains("[x] kickoff"));
    d.keys(&["Right"]);
    let top = |s: &str| s.lines().next().is_some_and(|l| l.contains("[floating]"));
    d.wait_for("panel at the top", |s| top(s) && s.contains("System Settings (unsaved)"));
    // A text field: the Kickoff label.
    d.keys(&["Down", "Enter", "C-u"]);
    d.tmux(&["send-keys", "-t", "main", "-l", " SETTINGS "]);
    d.keys(&["Enter"]);
    d.wait_for("new label in the panel", |s| s.lines().next().is_some_and(|l| l.contains(" SETTINGS ")));
    d.keys(&["C-s"]);
    d.wait_for("saved", |s| s.contains("Settings saved") && !s.contains("(unsaved)"));
    let text = std::fs::read_to_string(&cfg).unwrap();
    assert!(text.contains("# keep this comment\n"), "{text}");
    assert!(text.contains("notification_timeout = 30\n"), "{text}");
    assert!(text.contains("[panel]\nposition = \"top\"\nkickoff_label = \" SETTINGS \"\n"), "{text}");

    // Unsaved changes are reverted when discarded.
    d.keys(&["1"]);
    d.keys(&["Down", "Right"]);
    d.wait_for("five desktops", |s| s.contains("< 5 >"));
    d.keys(&["Escape"]);
    d.wait_for("asked to save", |s| s.contains("Unsaved changes:"));
    d.keys(&["d"]);
    d.wait_for("closed", |s| !s.contains("System Settings"));
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), text);
    let (ok, out) = d.msg(&["desktops"]);
    assert!(ok && out.trim() == "1/4", "{out}");
    // The saved settings stay in effect without a reload notification.
    let s = d.screen();
    assert!(top(&s) && !s.contains("Configuration reloaded"), "{s}");
}
