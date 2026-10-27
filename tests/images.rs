//! End-to-end tests for images in windows. A fake client that claims a Ghostty-like terminal
//! attaches to a real server; the tests run programs that send images and check what the host
//! terminal would receive: the image data, virtual placements and placeholder cells.
// These tests drive the Unix client/server (and tmux); Windows runs tuidesk standalone only.
#![cfg(unix)]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;

const BIN: &str = env!("CARGO_BIN_EXE_tuidesk");
const PLACEHOLDER: &str = "\u{10EEEE}";
const TIMEOUT: Duration = Duration::from_secs(10);

struct Host {
    dir: PathBuf,
    run: PathBuf,
    server: Child,
    output: Arc<Mutex<Vec<u8>>>,
    _stream: UnixStream,
}

fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = vec![kind];
    f.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    f.extend_from_slice(payload);
    f
}

impl Host {
    fn start(name: &str) -> Self {
        Self::start_with(name, "placeholders")
    }

    /// Start a server and attach a client whose terminal shows images with `mode`.
    fn start_with(name: &str, mode: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("tuidesk-img-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["cfg", "state", "work"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        // A short runtime dir keeps the socket path predictable.
        let run = PathBuf::from(format!("/tmp/ti-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&run);
        std::fs::write(dir.join("cfg/config.toml"), "[general]\nshell = \"/bin/sh\"\nrestore_session = false\n")
            .unwrap();
        std::fs::write(dir.join("cfg/init.lua"), "").unwrap();
        let server = Command::new(BIN)
            .args(["--server", "-S", "img", "--size", "100x30", "-c"])
            .arg(dir.join("cfg"))
            .env("TUIDESK_RUNTIME_DIR", &run)
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("HOME", dir.join("work"))
            .env("PS1", "$ ")
            .current_dir(dir.join("work"))
            .spawn()
            .unwrap();
        let sock = run.join("img.sock");
        let start = Instant::now();
        let mut stream = loop {
            if let Ok(s) = UnixStream::connect(&sock) {
                break s;
            }
            assert!(start.elapsed() < TIMEOUT, "server did not start");
            std::thread::sleep(Duration::from_millis(20));
        };
        let hello = format!(
            r#"{{"protocol":3,"cols":100,"rows":30,"term":"xterm-256color","colorterm":"truecolor",
                 "enhanced":false,"image_mode":"{mode}","cell_w":10,"cell_h":20}}"#
        );
        stream.write_all(&frame(1, hello.as_bytes())).unwrap();
        let output = Arc::new(Mutex::new(Vec::new()));
        let (sink, mut reader) = (output.clone(), stream.try_clone().unwrap());
        std::thread::spawn(move || {
            let mut head = [0u8; 5];
            while reader.read_exact(&mut head).is_ok() {
                let len = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
                let mut payload = vec![0u8; len];
                if reader.read_exact(&mut payload).is_err() {
                    break;
                }
                if head[0] == 1 {
                    sink.lock().unwrap().extend_from_slice(&payload);
                }
            }
        });
        Self { dir, run, server, output, _stream: stream }
    }

    fn msg(&self, args: &[&str]) -> String {
        let out = Command::new(BIN)
            .args(["-S", "img", "msg"])
            .args(args)
            .env("TUIDESK_RUNTIME_DIR", &self.run)
            .output()
            .unwrap();
        assert!(out.status.success(), "msg {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn wait_output(&self, what: &str, pred: impl Fn(&str) -> bool) -> String {
        let start = Instant::now();
        loop {
            let text = String::from_utf8_lossy(&self.output.lock().unwrap()).into_owned();
            if pred(&text) {
                return text;
            }
            assert!(start.elapsed() < TIMEOUT, "timed out waiting for {what}; got {} bytes", text.len());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Screen text as tuidesk renders it (placeholders become blanks in screenshots).
    fn screen(&self) -> String {
        let path = self.dir.join("screen.txt");
        self.msg(&["screenshot", "txt", path.to_str().unwrap()]);
        std::fs::read_to_string(path).unwrap()
    }

    fn work(&self) -> PathBuf {
        self.dir.join("work")
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = Command::new(BIN).args(["-S", "img", "kill"]).env("TUIDESK_RUNTIME_DIR", &self.run).output();
        let _ = self.server.kill();
        let _ = self.server.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = std::fs::remove_dir_all(&self.run);
    }
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, 160]));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

/// Count placeholder cells drawn with image id `hid` as their foreground color, following
/// the SGR sequences in the output.
fn placeholders_for(text: &str, hid: u32) -> usize {
    let want = format!("38;2;{};{};{}", (hid >> 16) & 255, (hid >> 8) & 255, hid & 255);
    let mut fg_matches = false;
    let mut count = 0;
    let mut rest = text;
    while let Some(i) = rest.find(['\x1b', '\u{10EEEE}']) {
        rest = &rest[i..];
        if let Some(sgr) = rest.strip_prefix("\x1b[") {
            let end = sgr.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(sgr.len());
            if sgr[end..].starts_with('m') {
                let params = &sgr[..end];
                if params.contains("38;") || params == "0" || params.is_empty() || params.contains("39") {
                    fg_matches = params.split("38;").nth(1).is_some_and(|p| format!("38;{p}").starts_with(&want));
                }
            }
            rest = &sgr[end.min(sgr.len())..];
        } else if rest.starts_with('\u{10EEEE}') {
            count += usize::from(fg_matches);
            rest = &rest['\u{10EEEE}'.len_utf8()..];
        } else {
            rest = &rest[1..];
        }
    }
    count
}

/// Host image id from the first "a=t,...,i=N" transmission.
fn transmitted_id(text: &str) -> Option<u32> {
    const HEAD: &str = "\x1b_Ga=t,q=2,i=";
    let at = text.find(HEAD)?;
    text[at + HEAD.len()..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

#[test]
fn kitty_image_from_a_shell_command() {
    let h = Host::start("kitty");
    let b64 = base64::engine::general_purpose::STANDARD.encode(png(40, 40));
    std::fs::write(h.work().join("img.b64"), &b64).unwrap();
    // Display a 4x2-cell image at the cursor, like `kitty +kitten icat` does.
    h.msg(&[
        "spawn",
        r#"sh -c "printf '\033_Ga=T,f=100,i=7,c=4,r=2;'; cat img.b64; printf '\033\\\\'; echo AFTER-IMAGE; sleep 600""#,
    ]);
    let out = h.wait_output("image and placeholders", |t| t.contains("a=p,U=1") && t.contains(PLACEHOLDER));
    let hid = transmitted_id(&out).expect("image transmitted to the host");
    assert!(out.contains(&format!("i={hid},f=100")), "PNG forwarded as PNG");
    assert!(out.contains(&format!("a=p,U=1,q=2,i={hid},p={},c=4,r=2", (4 << 9) | 2)));
    assert_eq!(placeholders_for(&out, hid), 8, "4x2 placeholder cells");
    // The cursor moved past the image like text, so the next output follows it.
    assert!(h.screen().contains("AFTER-IMAGE"));
}

#[test]
fn programs_are_told_images_work() {
    let h = Host::start("query");
    // A tiny program that sends the queries yazi sends and prints the replies it gets.
    let script = r#"
import os, sys, termios, tty, select
fd = sys.stdin.fileno(); old = termios.tcgetattr(fd); tty.setraw(fd)
os.write(1, b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[16t\x1b[14t\x1b[0c")
buf = b""
while select.select([fd], [], [], 2)[0]:
    buf += os.read(fd, 1024)
    if buf.count(b"c") and b"\x1b[?" in buf and buf.rstrip().endswith(b"c"): break
termios.tcsetattr(fd, termios.TCSADRAIN, old)
for part in buf.split(b"\x1b"):
    if part: print("REPLY:" + repr(b"\x1b" + part))
import time; time.sleep(600)
"#;
    std::fs::write(h.work().join("q.py"), script).unwrap();
    h.msg(&["spawn", "python3 q.py"]);
    let start = Instant::now();
    let screen = loop {
        let s = h.screen();
        if s.contains("REPLY:") {
            break s;
        }
        assert!(start.elapsed() < TIMEOUT, "no reply:\n{s}");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(screen.contains(r"\x1b_Gi=31;OK"), "{screen}");
    assert!(screen.contains(r"\x1b[6;20;10t"), "cell size reported: {screen}");
    assert!(screen.contains(r"\x1b[?62;4;22c"), "sixel advertised: {screen}");
}

#[test]
fn sixel_and_iterm_images() {
    let h = Host::start("sixel");
    // A 20x12 pixel red sixel image: two bands of 6 rows.
    h.msg(&["spawn", r#"sh -c "printf '\033Pq#1;2;100;0;0#1!20~-#1!20~\033\\\\'; echo SIXEL-DONE; sleep 600""#]);
    let out = h.wait_output("sixel image", |t| t.contains(",f=32,s=20,v=12"));
    let hid = transmitted_id(&out).unwrap();
    // 20x12 px at 10x20 px cells is 2x1 cells.
    h.wait_output("sixel placeholders", |t| placeholders_for(t, hid) == 2);
    assert!(h.screen().contains("SIXEL-DONE"));

    let b64 = base64::engine::general_purpose::STANDARD.encode(png(30, 20));
    std::fs::write(h.work().join("it.b64"), &b64).unwrap();
    h.msg(&["spawn", r#"sh -c "printf '\033]1337;File=inline=1;width=3:'; cat it.b64; printf '\007'; sleep 600""#]);
    h.wait_output("iTerm2 image as PNG with a 3-column placement", |t| t.contains(",c=3,r=1"));
}

#[test]
fn yazi_previews_images() {
    if Command::new("yazi").arg("--version").output().is_err() {
        eprintln!("skipping: yazi not installed");
        return;
    }
    let h = Host::start("yazi");
    let pics = h.work().join("pics");
    std::fs::create_dir_all(&pics).unwrap();
    std::fs::write(pics.join("a.png"), png(80, 60)).unwrap();
    h.msg(&["spawn", &format!("yazi {}", pics.display())]);
    // yazi sends the image (RGB through shared memory or directly) and draws placeholders.
    let out = h.wait_output("yazi image preview", |t| t.contains("\x1b_Ga=t,q=2") && t.contains(PLACEHOLDER));
    let hid = transmitted_id(&out).unwrap();
    assert!(h.wait_output("placeholders for yazi's image", |t| placeholders_for(t, hid) > 0).contains("a=p,U=1"));
}

fn icat(h: &Host, cols: u16, rows: u16) {
    let b64 = base64::engine::general_purpose::STANDARD.encode(png(40, 40));
    std::fs::write(h.work().join("img.b64"), &b64).unwrap();
    let cmd = format!(
        r#"sh -c "printf '\033_Ga=T,f=100,q=2,c={cols},r={rows};'; cat img.b64; printf '\033\\\\'; sleep 600""#
    );
    h.msg(&["spawn", &cmd]);
}

#[test]
fn iterm_terminals_get_cropped_inline_images() {
    let h = Host::start_with("iterm", "iterm");
    icat(&h, 4, 2);
    let out = h.wait_output("iTerm2 image", |t| t.contains("\x1b]1337;File=inline=1;"));
    assert!(out.contains("width=4;height=2;preserveAspectRatio=0;doNotMoveCursor=1:"), "whole 4x2 image");
    assert!(!out.contains(PLACEHOLDER), "no placeholder characters reach an iTerm2 terminal");
    assert!(!out.contains("\x1b_G"), "no kitty graphics either");
    // Nothing changes, nothing is sent again.
    std::thread::sleep(Duration::from_millis(600));
    let count = |t: &str| t.matches("\x1b]1337;File=").count();
    let before = count(&String::from_utf8_lossy(&h.output.lock().unwrap()));
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(count(&String::from_utf8_lossy(&h.output.lock().unwrap())), before);
    // A window opened on top covers the image's right side: no full-screen repaint, just the
    // visible 2x2 part redrawn once (the covered cells now show the new window).
    let mark = h.output.lock().unwrap().len();
    h.msg(&["spawn", "sh -c 'echo COVERING; sleep 600'"]);
    let after = h.wait_output("cropped image", |t| t[mark.min(t.len())..].contains("width=2;height=2;"));
    std::thread::sleep(Duration::from_millis(500));
    let after = String::from_utf8_lossy(&h.output.lock().unwrap()[mark..]).into_owned() + &after[..0];
    assert!(!after.contains("\x1b[2J"), "no full-screen repaint");
    assert_eq!(count(&after), 1, "only the changed piece is redrawn");
}

#[test]
fn sixel_terminals_get_sixel_images() {
    let h = Host::start_with("sixelhost", "sixel");
    icat(&h, 4, 2);
    // 4x2 cells of 10x20 pixels: a 40x40 pixel sixel image.
    let out = h.wait_output("sixel image", |t| t.contains("\x1bP0;1;0q\"1;1;40;40"));
    assert!(!out.contains(PLACEHOLDER) && !out.contains("\x1b_G"));
    // Drawn at the image's cell, then the cursor is restored.
    let at = out.find("\x1bP0;1;0q").unwrap();
    assert!(out[..at].ends_with('H') && out[at..].contains("\x1b\\\x1b8"));
}

fn gif(frames: &[[u8; 4]], delay_ms: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
        for color in frames {
            let buf = image::RgbaImage::from_pixel(40, 40, image::Rgba(*color));
            enc.encode_frame(image::Frame::from_parts(buf, 0, 0, image::Delay::from_numer_denom_ms(delay_ms, 1)))
                .unwrap();
        }
    }
    out
}

/// Spawn a program that shows an animated GIF with the iTerm2 protocol (like `imgcat`).
fn imgcat_gif(h: &Host) {
    let b64 = base64::engine::general_purpose::STANDARD.encode(gif(&[[255, 0, 0, 255], [0, 0, 255, 255]], 60));
    std::fs::write(h.work().join("anim.b64"), &b64).unwrap();
    h.msg(&[
        "spawn",
        r#"sh -c "printf '\033]1337;File=inline=1;width=4;height=2:'; cat anim.b64; printf '\007'; sleep 600""#,
    ]);
}

#[test]
fn kitty_animations_play_on_the_host() {
    let h = Host::start("anim");
    let b64 = |px: &[u8]| base64::engine::general_purpose::STANDARD.encode(px);
    let red = b64(&[255, 0, 0, 255].repeat(4));
    let blue = b64(&[0, 0, 255, 255].repeat(4));
    // A 2x2 image, a second frame, then play forever: what `kitten icat` sends for a GIF.
    let script = format!(
        "printf '\\033_Ga=T,f=32,s=2,v=2,i=5,q=2,c=4,r=2;{red}\\033\\\\'; \
         printf '\\033_Ga=f,f=32,s=2,v=2,i=5,q=2,z=50;{blue}\\033\\\\'; \
         printf '\\033_Ga=a,i=5,q=2,s=3,v=1\\033\\\\'; sleep 600"
    );
    std::fs::write(h.work().join("anim.sh"), script).unwrap();
    h.msg(&["spawn", "sh anim.sh"]);
    let out = h.wait_output("image", |t| t.contains("\x1b_Ga=t,q=2,i="));
    let hid = transmitted_id(&out).unwrap();
    // Each frame change re-sends the image under the same host id, compressed.
    let head = format!("\x1b_Ga=t,q=2,i={hid},f=32,s=2,v=2,o=z");
    h.wait_output("several frames", |t| t.matches(&head).count() >= 6);
}

#[test]
fn gif_animations_on_iterm_terminals() {
    let h = Host::start_with("animiterm", "iterm");
    imgcat_gif(&h);
    let count = |t: &str| t.matches("\x1b]1337;File=").count();
    h.wait_output("first frame", |t| count(t) >= 1);
    let mark = h.output.lock().unwrap().len();
    // 60 ms frames: about 16 redraws a second, without full-screen repaints.
    let after = h.wait_output("frames keep coming", |t| count(&t[mark.min(t.len())..]) >= 6);
    assert!(!after[mark..].contains("\x1b[2J"));
}

#[test]
fn gif_animations_on_sixel_terminals() {
    let h = Host::start_with("animsixel", "sixel");
    imgcat_gif(&h);
    let count = |t: &str| t.matches("\x1bP0;1;0q").count();
    h.wait_output("several sixel frames", |t| count(t) >= 6);
}
