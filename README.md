# tuidesk

A KDE Plasma style desktop that runs entirely inside a terminal. Windows are real
pseudo-terminals, so anything you already use (yazi, nvim, ncdu, btop, htop, lazygit,
your shell) runs inside them.

- Floating windows with title bars, `[_] [^] [x]` buttons, mouse move/resize, double-click
  maximize, drag-to-edge quick tiling, Alt+drag move and Alt+right-drag resize
- Per-desktop layouts: floating, or tiled as master/stack, monocle, grid, columns or bsp
  (drag the master's border to resize it)
- Window menu (right-click a title bar): minimize, maximize, float, keep above, move to
  desktop, rename, close
- Virtual desktops with names, a pager (drag a task or window onto it to move it there,
  right-click to rename) and an Alt+Tab window switcher
- KRunner-style runner (Alt+F2): calculator, open windows, apps, folders, man pages, actions
- Overview (Ctrl+F9): every window on every desktop as a live preview card; filter, jump,
  close, or drag a card onto another desktop
- Screenshots like Spectacle: whole screen, active window or a dragged region, saved as
  text, colored ANSI, HTML or SVG
- Per-desktop wallpapers (color, ASCII art, pattern, Lua text) and desktop widgets (big
  clock, notes file you can click to edit, system monitor, Lua)
- Detach and reattach like tmux: the desktop keeps running when the terminal closes
- Sessions: open windows, their directories, geometry and desktops are restored on next start
- Mouse text selection with copy (to the host clipboard via OSC 52) and middle-click paste
- Keyboard copy mode with vi motions and regex search through history
- Clipboard history (like Klipper) and a searchable key binding cheat sheet
- `tuidesk msg` to control a session from scripts, cron jobs or other windows
- Images inside windows: yazi previews, kitty icat, chafa, timg, img2sixel, imgcat, ...
  (kitty graphics, Sixel and iTerm2 protocols; shown in kitty, Ghostty, iTerm2, WezTerm,
  VS Code and Sixel terminals such as foot, xterm and Konsole)
- Full terminal emulation (alacritty_terminal): scroll regions, synchronized output, OSC 8
  hyperlinks (Ctrl+click opens them), cursor shapes
- Bottom (or top) panel: Kickoff launcher, pager, task manager, CPU, RAM, battery, notifications, clock
- Kickoff menu with search, categories and "Run: <anything>"
- Notifications from Lua, from programs (`printf '\e]9;hello\a'`, OSC 777) and on errors
- TOML config and Lua plugins, both hot-reloaded on save
- System Settings dialog for the theme, colors, panel widgets, key bindings and more, with live
  preview; it saves to config.toml and keeps your comments
- Truecolor, 256 and 16 color output (works on the Linux console), several border styles

## Build and run

```sh
cargo build --release
./target/release/tuidesk            # attach to the "default" session, starting it if needed
./target/release/tuidesk -S work    # a separate, named session
./target/release/tuidesk ls         # list running sessions
./target/release/tuidesk kill       # save and stop a session
./target/release/tuidesk --standalone   # run in this terminal only, no background server
```

Requires a Rust toolchain only (1.95 or newer; Lua 5.4 is vendored). Works on Linux and macOS,
and on Windows without the background server (see below).

### Install

| Platform | Package | Install |
|---|---|---|
| Any Linux | static executable (musl, x86_64 / aarch64) | unpack `tuidesk-<v>-<arch>-unknown-linux-musl.tar.gz`, put `tuidesk` on `PATH` |
| Debian, Ubuntu (x86_64) | `.deb` | `sudo apt install ./tuidesk_<v>-1_amd64.deb` |
| Debian, Ubuntu (ARM64) | `.deb` | `sudo apt install ./tuidesk_<v>-1_arm64.deb` |
| Fedora, RHEL, openSUSE (x86_64) | `.rpm` | `sudo dnf install ./tuidesk-<v>-1.x86_64.rpm` |
| Fedora, RHEL, openSUSE (ARM64) | `.rpm` | `sudo dnf install ./tuidesk-<v>-1.aarch64.rpm` |
| Arch Linux | `PKGBUILD` | `makepkg -si` |
| Alpine | `APKBUILD` | `abuild -r` |
| NixOS, Nix | `flake.nix` | `nix run github:portvex/tuidesk`, or `nix profile install github:portvex/tuidesk` |
| macOS, Linux | Homebrew formula (prebuilt executable, no Rust needed) | `brew install portvex/tap/tuidesk` |
| Windows (x86_64) | NSIS installer | run `tuidesk-<v>-windows-x86_64-setup.exe` (per user, adds to PATH) |
| Windows (ARM64) | NSIS installer | run `tuidesk-<v>-windows-arm64-setup.exe` (per user, adds to PATH) |
| Windows (x86_64 / ARM64) | portable `.zip` | unpack `tuidesk-<v>-windows-<arch>-portable.zip`, run `tuidesk.exe` from it |

Every package installs the manual pages:

| Page | Contents |
|---|---|
| `man tuidesk` | tuidesk(1): options, `ls`/`kill`/`msg`, keys, mouse, runner, copy mode, environment, files |
| `man tuidesk-config` | tuidesk-config(5): every config.toml setting, actions, apps, rules |
| `man tuidesk-lua` | tuidesk-lua(7): the Lua plugin API, events and window tables |

Without installing, read one with `man ./man/tuidesk.1`. The runner (Alt+F2) opens them too:
`man tuidesk`.

**Windows** runs tuidesk in the terminal it is started in (Windows Terminal is recommended),
like `--standalone` elsewhere: there is no background server, so `ls`, `kill`, `msg` and
detaching are not available. Windows needs no WSL; for the full feature set, WSL runs the Linux
build. Configuration is in `%APPDATA%\tuidesk`, saved sessions in `%LOCALAPPDATA%\tuidesk`.

### Building the release packages

`cargo xtask dist` (the `xtask/` crate) builds everything into `dist/` with a `SHA256SUMS`
file, using each format's own tool:

```sh
cargo xtask dist                # every kind this host can build
cargo xtask dist linux deb rpm  # only these
cargo xtask help                # the kinds and the tools each needs
```

| Kind | Output | Built with |
|---|---|---|
| `linux` | static musl executables, x86_64 and aarch64 | cargo-zigbuild (`cargo install cargo-zigbuild`, plus zig) |
| `deb` | `.deb` of those executables | cargo-deb (`[package.metadata.deb]` in Cargo.toml) |
| `rpm` | `.rpm` of those executables | cargo-generate-rpm (`[package.metadata.generate-rpm]`) |
| `macos` | universal (arm64 + x86_64) executable | cargo and lipo, on macOS |
| `windows` | portable `.zip` and NSIS installer, x86_64 and arm64 | cargo-zigbuild and makensis (`packaging/windows/tuidesk.nsi`) |
| `source` | `tuidesk-<v>.tar.gz` from git, what the Arch and Alpine recipes build from | git |
| `recipes` | `PKGBUILD`, `APKBUILD` and `Formula/tuidesk.rb` for this version | templates in `packaging/`; the formula installs the `macos` and `linux` executables, so run it after those |

For a release, bump `version` in Cargo.toml and the man pages, merge, and push the tag
`v<version>`. The release workflow (`.github/workflows/release.yml`) runs the CI checks,
builds everything on Linux and macOS, publishes the GitHub release (the recipes download from
there), and pushes the new formula to the `portvex/homebrew-tap` repository with a deploy key
(the `HOMEBREW_TAP_DEPLOY_KEY` secret). The Nix flake builds from the repository itself.
tuidesk needs Rust 1.95 or newer to build (`rust-version` in Cargo.toml).

`tuidesk` runs a background server per session that owns the windows; the terminal you start
it in is a client. Detach with the prefix key then `d` (or "Detach" in Kickoff), close the
terminal, and run `tuidesk` again later to get the same desktop back. Sockets live in
`$TUIDESK_RUNTIME_DIR`, `$XDG_RUNTIME_DIR/tuidesk` or `/tmp/tuidesk-<uid>`; the server log is next
to the socket. Sessions are saved to `~/.local/state/tuidesk/` on quit, on detach and every
30 seconds, and restored when a server starts (`general.restore_session`). A session file
that cannot be read is kept as `session-<name>.json.bak` rather than overwritten.

The socket directory must belong to you and is kept at mode 0700; tuidesk refuses a directory
owned by someone else, and only accepts connections from your own user. A client that stops
reading (for example a frozen terminal) is detached instead of slowing the desktop down.
`--standalone` refuses to start while a background server owns the same session name.

On first start, `~/.config/tuidesk/config.toml` and `~/.config/tuidesk/init.lua` are created
(`$XDG_CONFIG_HOME` is respected; `-c DIR` uses another directory). Both files are documented
inline. `tuidesk --print-config` and `tuidesk --print-lua` print the built-in defaults.

Kickoff comes with a catalogue of about 40 well-known terminal programs (editors, file
managers, git and system tools, mail, music, ...) and shows the ones that are installed;
`tuidesk --print-apps` lists them. `[[apps]]` entries in config.toml are added to the
catalogue, and an entry with the same name as a built-in one replaces it. `hidden_apps`
under `[general]` leaves entries out, and `builtin_apps = false` turns the catalogue off.

Kickoff comes with a catalogue of about 40 well-known terminal programs (editors, file
managers, git and system tools, mail, music, ...) and shows the ones that are installed;
`tuidesk --print-apps` lists them. `[[apps]]` entries in config.toml are added to the
catalogue, and an entry with the same name as a built-in one replaces it. `hidden_apps`
under `[general]` leaves entries out, and `builtin_apps = false` turns the catalogue off.

## Keys

Terminals cannot see the Super key, so window management uses a **prefix key**
(default `ctrl+space`, then the key below) plus a few direct Alt shortcuts.
Press the prefix twice to send it to the program.

| Prefix, then | Action | Direct |
|---|---|---|
| `space` | Kickoff launcher | `alt+f1` |
| `enter` / `c` | New terminal | `alt+enter` |
| `x` | Close window | `alt+f4` |
| `m` / `up` | Maximize / restore | `alt+f10` |
| `n` | Minimize | `alt+f9` |
| `tab` / `shift+tab` | Next / previous window | |
| | Window switcher (hold Alt, press Tab to step, release to pick) | `alt+tab` / `alt+shift+tab` |
| `1`..`9` / `.` `,` | Go to desktop N / next / previous | `ctrl+f1`..`ctrl+f4` |
| `>` / `<` | Move window to next / previous desktop (and follow it) | |
| `d` | Detach | |
| `h` `l` `k` `j` / arrows | Quick tile left, right, top, bottom (in tiling: reorder) | |
| `t` / `T` | Toggle floating / tiled layout, cycle through all layouts | |
| `f` | Float or unfloat the window in tiling layout | |
| `s` | Swap with master | |
| `w` | Keyboard move/resize mode | |
| `[` | Copy mode (see below) | |
| `]` / `p` | Paste the last copy | |
| `v` | Clipboard history | |
| `?` | Key binding cheat sheet (type to filter) | |
| `o` | Window menu | |
| `a` | Keep window above others | |
| `R` | Rename window | |
| `W` | Overview | `ctrl+f9` |
| `P` / `ctrl+p` / `alt+p` | Screenshot of the screen / focused window / a dragged region | |
| | Scroll history | `shift+pageup` / `shift+pagedown` |
| `b` | Notification history | |
| `S` | System Settings (also in Kickoff) | |
| `e` / `y` / `g` | Launch Editor / Files / lazygit (`g` comes from init.lua) | |
| `r` | Runner (see below) | `alt+f2` |
| `ctrl+r` | Reload config | |
| `q` | Quit | |

All bindings are configurable under `[keys.global]` and `[keys.prefix]`, or in System
Settings; `"none"` turns off a default binding.

Mouse: click a task to focus or minimize it, middle-click a task to close it, middle-click
a title bar to send the window to the back, and scroll over the panel to cycle windows.
Click a desktop in the pager to switch, or scroll over it.

Selecting text: drag inside a window (double-click selects a word, triple-click a line).
The selection is copied to tuidesk's clipboard and, through OSC 52, to your terminal's
clipboard. Middle-click pastes. Hold Shift to select in programs that use the mouse
themselves (htop, nvim with mouse enabled). Dragging past the window edge scrolls history.

The runner (Alt+F2, or prefix then `r`) searches as you type and mixes results:
`2*(3+4)` or `sqrt(2)` calculates (Enter copies the result), a window title switches to that
window on any desktop, app names launch, `~/src/pro` lists folders (Enter opens a shell
there, Tab completes), `man tar` opens a manual page, action names such as `layout grid` or
`detach` run, and anything else can be run as a command.

Copy mode (prefix then `[`) moves a cursor through the window and its history:
`h j k l`, `w b e`, `0 ^ $`, `H M L`, `g G`, `ctrl+u/d/b/f`; `/` and `?` search (regex,
case-insensitive unless the pattern has capitals), `n`/`N` repeat; `v`, `V`, `ctrl+v` select
characters, lines or a block; `y` or Enter copies and leaves; `q` leaves.

The Alt+Tab switcher commits when you release Alt on terminals with the kitty keyboard
protocol (kitty, Ghostty, WezTerm, foot, recent iTerm2 and Alacritty). Elsewhere (Terminal.app,
inside tmux) it picks the highlighted window after a short pause, or on Enter.

**macOS:** for Alt shortcuts, set Option to send Meta/Esc+ (Terminal.app: Settings >
Profiles > Keyboard > "Use Option as Meta key"; iTerm2: Profiles > Keys > Left Option = Esc+).
The prefix key works without this.

## Configuration

System Settings (prefix then `S`, or "System Settings" in Kickoff) edits the common options:
general behaviour, windows and layouts, the theme and its colors, the wallpaper, the panel
(position, label, clock, and which widgets show in what order), the prefix key and all key
bindings, and screenshots. Changes show at once; `ctrl+s` saves them to config.toml with your
comments and formatting kept, and closing with unsaved changes asks whether to save or
discard them. Tab or a click switches pages, arrows select and change values, Enter edits text
or toggles, Delete resets a value to its default or unbinds a key, and shift+up/down reorders
panel widgets. Everything else, such as rules, apps and per-desktop wallpapers, is edited in
the file.

User values are merged over the built-in defaults, so a config file only needs the keys you
change:

```toml
[general]
layout = "tiling"
desktops = 6
desktop_names = ["Main", "Code", "Web", "Chat"]
autostart = ["$SHELL", "btop"]   # used when there is no saved session
restore_session = true
inherit_cwd = true               # new windows open in the focused window's directory

[theme]
preset = "breeze-light"   # breeze-dark, breeze-light, tty16
accent = "#ff8800"
border = "rounded"

[panel]
position = "top"
widgets = ["kickoff", "pager", "tasks", "lua:load", "cpu", "mem", "clock"]

[keys.prefix]
"v" = "spawn nvim ~/notes.md"

[[apps]]                  # added to the built-in catalogue
name = "Notes"
command = "nvim ~/notes.md"
category = "Office"

[[rules]]
command = "htop"
maximized = true

[[rules]]
title = "System Monitor"   # name of the launcher entry, or part of the window title
desktop = 4                # opens there without switching to it
above = true
```

Rule settings: `width`, `height` (percent), `x`, `y` (cells), `maximized`, `minimized`,
`floating`, `above`, `desktop`.

Wallpapers and desktop widgets:

```toml
[wallpaper.default]          # all desktops; [wallpaper.2] etc. override per desktop
kind = "pattern"             # logo (default), none, art, pattern, lua
text = ". "

[wallpaper.2]
kind = "art"
file = "~/.config/tuidesk/art/mountains.txt"
position = "bottom-right"    # top-left, top, top-right, left, center, right, bottom-*
color = "#102030"

[[desktop_widgets]]
kind = "clock"               # clock, notes, sysinfo, lua
position = "top-right"

[[desktop_widgets]]
kind = "notes"               # click to edit in $EDITOR; reloads when the file changes
file = "~/notes.md"
position = "bottom-left"
desktops = [1]
```

## Images

Programs in windows can show images with the kitty graphics protocol (what yazi uses), Sixel
or iTerm2 inline images. tuidesk keeps them per window and shows them on your terminal with the
best method it supports:

| Your terminal | How images are shown |
|---|---|
| kitty, Ghostty | kitty Unicode placeholders: the terminal draws the images |
| iTerm2, WezTerm, VS Code | iTerm2 inline images drawn by tuidesk |
| foot, xterm, Konsole, mlterm, ... (Sixel) | Sixel images drawn by tuidesk (255 colors) |

Either way images move with their window, are hidden by windows on top, clip at window edges
and scroll with the text. tuidesk detects the terminal when you attach (`TERM`, `TERM_PROGRAM`,
a kitty graphics query and the device attributes) and tells programs that images work only
when they do, so elsewhere programs fall back to their text previews. Force a method with
`[general] image_protocol = "kitty" | "iterm" | "sixel" | "none"`, or turn images off with
`images = false`. With iTerm2 and Sixel, only the image pieces that change are redrawn, so
windows move over images without flicker. Images are re-sent when you reattach and limited to
about 192 MB per window (animation frames included).

Animations play: kitty animation frames (what `kitten icat` sends for GIFs), animated GIFs sent
with the iTerm2 protocol (`imgcat`), and programs that redraw Sixel images in place. They pause
while their window is hidden (another desktop, minimized).

Supported: kitty transmission by direct data, file, temporary file and shared memory, PNG and
raw RGB(A), zlib compression, chunking, placements at the cursor and virtual placements (`U=1`),
deletion, animation frames and control (`a=f`, `a=a`); Sixel; iTerm2 `File=` with `inline=1`
(PNG, JPEG, GIF including animated, WebP, BMP). Not supported: kitty frame composition between
existing frames (`a=c`) and drawing images below text (`z < 0`).

On those terminals the title bar buttons `[_] [^] [x]` are drawn as icons too (minimize, maximize
or restore, and a red close button on the focused window), in the theme's title bar colors.
Turn this off with `[general] pixel_buttons = false`; screenshots always show the text buttons.

## Screenshots

Shots go to `~/Pictures/tuidesk/tuidesk-YYYYmmdd-HHMMSS-mmm.<format>`; the path (or the
shot's text) is copied to the clipboard and a notification shows where it went:

```toml
[screenshot]
dir = "~/Pictures/tuidesk"
format = "html"   # txt, ansi (view with cat or less -R), html, svg
copy = "path"     # path, text or none
```

Shots are drawn offscreen without menus or tuidesk's own screenshot popups, so they also work
with no terminal attached: `tuidesk msg screenshot window svg ~/shot.svg`.

## Scripting with `tuidesk msg`

```sh
tuidesk msg notify "Build" "finished in 42s"
tuidesk msg spawn "btop"            # prints the new window id
tuidesk msg windows                 # id, desktop, focused (*), title, command
tuidesk msg focus 3 ; tuidesk msg close 3
tuidesk msg desktop 2 ; tuidesk msg layout grid ; tuidesk msg detach
tuidesk msg lua 'return #tuidesk.windows()'
tuidesk msg screenshot 3 html ~/win.html   # window 3; or screen / window, txt/ansi/html/svg
```

Any action from `[keys]` works as a command. Inside a tuidesk window, `tuidesk msg` talks to that
window's own session (through `$TUIDESK_SESSION`), so `make && tuidesk msg notify done` works.

On the Linux console use `preset = "tty16"` (`color_mode = "16"` is detected automatically
when `TERM=linux`).

## Lua plugins

`init.lua` gets a `tuidesk` table:

```lua
tuidesk.notify(title, body, timeout)
tuidesk.spawn("btop")
tuidesk.action("tile_left")
tuidesk.bind("alt+g", fn)             -- direct key
tuidesk.bind_prefix("g", fn)          -- after the prefix
tuidesk.on("window_open", fn)         -- startup, tick, window_open, window_close, focus
tuidesk.widget("name", fn, seconds)   -- show as "lua:name" in panel.widgets
tuidesk.rule{ command = "lazygit", maximized = true }
tuidesk.windows()                     -- {id, title, command, focused, minimized, exit_code, desktop}
tuidesk.action("desktop 2")           -- any action from [keys], e.g. "move_to_desktop 3", "detach"
tuidesk.focus(id); tuidesk.close(id)
```

Modules in `~/.config/tuidesk/lua/` can be loaded with `require`. Errors appear as notifications.
Each call into Lua may run for 2 seconds before it is stopped, and actions triggered from Lua
may nest 4 levels deep, so a buggy script cannot freeze or crash the desktop.

## Layout of the code

| File | Purpose |
|---|---|
| `src/main.rs` | Command line, terminal setup, client / server / standalone entry points |
| `src/app.rs` | State, actions, windows, sessions, switcher, display, event loop |
| `src/input.rs` | Keyboard and mouse routing, selection, dragging, Kickoff menu |
| `src/ipc.rs` | Client/server socket protocol and the output backend |
| `src/session.rs` | Session save and restore |
| `src/settings.rs` | System Settings dialog: pages, fields and comment-preserving config edits |
| `src/wm.rs` | Windows, desktops, focus history, floating/tiling geometry, hit testing |
| `src/term.rs` | PTY + alacritty_terminal emulation, selection, OSC notifications |
| `src/ui.rs` | Rendering: wallpaper, windows, panel, Kickoff, notifications |
| `src/keys.rs` | Key parsing and key/mouse encoding for programs |
| `src/config.rs`, `config/` | Config loading/merging, default config.toml, init.lua and app catalogue |
| `man/` | Manual pages: tuidesk(1), tuidesk-config(5), tuidesk-lua(7) |
| `packaging/`, `flake.nix` | Arch, Alpine, Homebrew and NSIS recipes; the Nix flake |
| `xtask/` | `cargo xtask dist`: release builds and packages |
| `src/theme.rs` | Presets, color parsing and downgrading |
| `src/lua.rs` | Lua plugin API |
| `src/tray.rs`, `src/notify.rs` | System stats and notifications |

## Development

```sh
cargo fmt-check     # rustfmt (rustfmt.toml)
cargo lint          # clippy on all targets, warnings are errors ([lints] in Cargo.toml)
cargo test          # unit, tmux, image and man page tests
cargo deny check    # licenses, bans, sources, advisories (deny.toml)
cargo audit         # RustSec vulnerability database
scripts/fuzz.sh     # libFuzzer targets in fuzz/ (needs a nightly toolchain via rustup)
scripts/miri.sh     # unit tests under Miri, for undefined behaviour (nightly; installs Miri)
scripts/coverage.sh # code coverage of all tests: summary plus HTML in target/llvm-cov/html
scripts/coverage.sh --check   # fail below the line/function floors set in the script
scripts/coverage.sh --lcov    # target/lcov.info for editors and CI services
```

Coverage uses cargo-llvm-cov (`cargo install cargo-llvm-cov`) with rustup's `llvm-tools`
component, and counts the unit, tmux and image tests, including the tuidesk servers the
integration tests start. The floors in `scripts/coverage.sh` sit just under the current numbers
(about 79% of lines); raise them as tests are added, never lower them to get a push through.

Miri checks the code the unit tests run, including the `unsafe` inside dependencies such as
the image decoders, zlib, base64, the terminal emulator and ratatui. It cannot call C or the
OS, so tests that run Lua are skipped there (`#[cfg_attr(miri, ignore)]`), and tuidesk's own
`unsafe` blocks, which are all OS calls (process groups, `poll`/`read`, `getuid`,
`getpeereid`, `setsid`, shared memory), are covered by the integration tests instead.

Git hooks come from cargo-husky and are installed into `.git/hooks` whenever the tests are
built (sources in `.cargo-husky/hooks`): **pre-commit** runs gitleaks, `cargo fmt --check` and
clippy; **pre-push** runs the tests with the coverage check (plain `cargo test` when cargo-llvm-cov is
not installed), `cargo deny` and `cargo audit`. If you use a global
`core.hooksPath`, point this repository back at its own hooks once:
`git config core.hooksPath .git/hooks`.

Fuzz targets (`fuzz/fuzz_targets/`): program output splitting and image commands
(`output_splitter`, `kitty_commands`), `sixel`, `iterm_image`, client/server frames
(`ipc_frames`), key specs, actions and the calculator (`text_inputs`), `config`, and tiling
geometry with its invariants (`tile_layout`).

## Tests

```sh
cargo test
```

`tests/man.rs` keeps the manual pages honest: every command line option, every
`[general]`, `[panel]` and `[screenshot]` setting and every action used in the default key
bindings has to appear in them, their version has to match `Cargo.toml`, and they have to pass
`mandoc -Tlint` (skipped when mandoc is missing).

`tests/images.rs` attaches fake clients (kitty placeholders, iTerm2 and Sixel terminals) to a
real server and checks what the terminal would receive when programs (a shell, yazi, Sixel and
iTerm2 output) show images, including clipping under overlapping windows.

Unit tests cover parsing, encoding, layout and the emulator. `tests/tmux.rs` runs the real
binary inside a headless tmux (skipped when tmux is missing) and checks shells, the launcher,
cursor positioning, desktops and their names, dragging onto the pager, the runner, the
overview, wallpapers and desktop widgets, screenshots, layouts, drag-to-tile, selection and paste, copy mode, the
clipboard history, the cheat sheet, the window menu, rename and keep-above, rules, the
switcher, detach/reattach, session restore, `tuidesk msg`, live config reload, System Settings and notifications.
