use std::io::{IsTerminal, Write};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const FRAME: Duration = Duration::from_millis(80);
// a finished step that took at least this long gets its own line in the scrollback
const SLOW: Duration = Duration::from_secs(1);
const MAX_ACTIVE_LINES: usize = 4;
const BAR_WIDTH: usize = 24;
const CAT: &str = include_str!("cat.txt");
// the cat is dropped on terminals too short to hold it next to the output above
const CAT_MIN_ROWS: usize = 20;

const PINK: &str = "38;5;213";
const GREEN: &str = "38;5;114";
const RED: &str = "38;5;203";
const GRAY: &str = "38;5;245";
const YELLOW: &str = "38;5;221";
const CYAN: &str = "38;5;117";

struct Active {
    id: usize,
    label: String,
    since: Instant,
}

struct Inner {
    active: Vec<Active>,
    total: usize,
    done: usize,
    started_count: usize,
    // lines of the status block currently on screen, 0 when nothing is drawn
    drawn: usize,
    frame: usize,
    stopped: bool,
}

// progress output on stderr: an animated status block on a terminal, plain
// numbered lines everywhere else so logs stay greppable
pub struct Ui {
    inner: Mutex<Inner>,
    wake: Condvar,
    tty: bool,
    color: bool,
    began: Instant,
}

impl Ui {
    pub fn new(total: usize) -> Ui {
        let tty = std::io::stderr().is_terminal() && std::env::var("TERM").map_or(true, |t| t != "dumb") && enable_ansi();
        let color = tty && std::env::var_os("NO_COLOR").is_none();
        Ui {
            inner: Mutex::new(Inner {
                active: Vec::new(),
                total,
                done: 0,
                started_count: 0,
                drawn: 0,
                frame: 0,
                stopped: false,
            }),
            wake: Condvar::new(),
            tty,
            color,
            began: Instant::now(),
        }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{}m{}\x1b[0m", code, text)
        } else {
            text.to_string()
        }
    }

    pub fn banner(&self, names: &[String], jobs: usize, total: usize) {
        if !self.tty {
            return;
        }
        let cat = self.paint(PINK, "=^.^=");
        let title = self.paint(&format!("1;{}", PINK), "meow");
        let what = format!("{} · {} steps · {} jobs", names.join(" "), total, jobs);
        eprintln!("{} {}  {}", cat, title, self.paint(GRAY, &what));
    }

    // permanent output above the status block
    pub fn line(&self, text: &str) {
        let mut s = self.inner.lock().unwrap();
        self.erase(&mut s);
        let err = std::io::stderr();
        let mut w = err.lock();
        let _ = writeln!(w, "{}", text);
        self.draw(&mut s, &mut w);
        let _ = w.flush();
    }

    // output of a command that succeeded, usually compiler warnings
    pub fn block(&self, label: &str, text: &str) {
        if !self.color {
            self.line(&format!("--- {}\n{}", label, text));
            return;
        }
        let (body, warnings, errors) = self.style_body(text);
        let accent = if errors > 0 { RED } else if warnings > 0 { YELLOW } else { PINK };
        let icon = if errors > 0 { "✗" } else if warnings > 0 { "⚠" } else { "›" };
        let head = format!("{} {}", self.paint(&format!("1;{}", accent), &format!("╭─ {}", icon)), self.paint("1", label));
        let foot = self.paint(accent, &format!("╰─{}", count_text(warnings, errors)));
        self.line(&format!("{}\n{}\n{}", head, self.fence(accent, &body), foot));
    }

    pub fn fail_block(&self, label: &str, command: &str, text: &str) {
        if !self.color {
            self.line(&format!("--- FAILED: {}\n{}", command, text));
            return;
        }
        let (body, warnings, errors) = self.style_body(text);
        let head = format!(
            "{} {}",
            self.paint(&format!("1;{}", RED), "╭─ ✗"),
            self.paint(&format!("1;{}", RED), &format!("{} failed", label))
        );
        let cmd = format!("{} {}", self.paint(RED, "│"), self.paint(GRAY, &format!("$ {}", command)));
        let foot = self.paint(RED, &format!("╰─{}", count_text(warnings, errors)));
        self.line(&format!("{}\n{}\n{}\n{}", head, cmd, self.fence(RED, &body), foot));
    }

    // a command line printed by -v
    pub fn command(&self, command: &str) {
        self.line(&self.paint(GRAY, &format!("$ {}", command)));
    }

    fn fence(&self, accent: &str, body: &str) -> String {
        let bar = self.paint(accent, "│");
        body.lines().map(|l| format!("{} {}", bar, l)).collect::<Vec<_>>().join("\n")
    }

    // colors compiler diagnostics, returns the text with the warning and error counts
    fn style_body(&self, text: &str) -> (String, usize, usize) {
        let mut warnings = 0;
        let mut errors = 0;
        let lines: Vec<String> = text
            .lines()
            .map(|l| {
                if l.contains(": warning: ") {
                    warnings += 1;
                } else if l.contains(": error: ") || l.contains(": fatal error: ") {
                    errors += 1;
                }
                self.style_line(l)
            })
            .collect();
        (lines.join("\n"), warnings, errors)
    }

    fn style_line(&self, line: &str) -> String {
        const KINDS: [(&str, &str); 4] = [
            ("fatal error", "1;38;5;203"),
            ("error", "1;38;5;203"),
            ("warning", "1;38;5;221"),
            ("note", "1;38;5;117"),
        ];
        for (kind, code) in KINDS {
            let pat = format!(": {}: ", kind);
            if let Some(i) = line.find(&pat) {
                let loc = &line[..i];
                let rest = &line[i + pat.len()..];
                // clang puts the warning flag last in brackets
                let (msg, flag) = match rest.rfind(" [-") {
                    Some(j) if rest.ends_with(']') => (&rest[..j], &rest[j..]),
                    _ => (rest, ""),
                };
                return format!(
                    "{}{} {}{}",
                    self.paint(CYAN, loc),
                    self.paint(code, &format!(": {}:", kind)),
                    self.paint("1", msg),
                    self.paint(GRAY, flag)
                );
            }
        }
        if let Some(bar) = line.find('|') {
            let (gutter, code) = line.split_at(bar);
            if gutter.chars().all(|c| c.is_ascii_digit() || c == ' ') {
                let code = &code[1..];
                let marks = code.trim();
                let styled = if !marks.is_empty() && marks.contains('^') && marks.chars().all(|c| "^~ ".contains(c)) {
                    self.paint(PINK, code)
                } else {
                    code.to_string()
                };
                return format!("{}{}{}", self.paint(GRAY, gutter), self.paint(GRAY, "|"), styled);
            }
        }
        if line.starts_with("In file included from") || line.starts_with("In function") || line.contains(" generated.") {
            return self.paint(GRAY, line);
        }
        line.to_string()
    }

    pub fn start(&self, id: usize, label: &str) {
        let mut s = self.inner.lock().unwrap();
        s.started_count += 1;
        if !self.tty {
            eprintln!("[{}/{}] {}", s.started_count, s.total, label);
            return;
        }
        s.active.push(Active { id, label: label.to_string(), since: Instant::now() });
    }

    // a step that was already fresh or that just ended counts as done
    pub fn finish(&self, id: usize, ran: bool) {
        let mut s = self.inner.lock().unwrap();
        s.done += 1;
        let Some(pos) = s.active.iter().position(|a| a.id == id) else {
            return;
        };
        let a = s.active.remove(pos);
        if ran && a.since.elapsed() >= SLOW {
            let mark = self.paint(GREEN, "✓");
            let took = self.paint(GRAY, &format!("{:.1}s", a.since.elapsed().as_secs_f64()));
            self.erase(&mut s);
            let err = std::io::stderr();
            let mut w = err.lock();
            let _ = writeln!(w, "{} {} {}", mark, a.label, took);
            self.draw(&mut s, &mut w);
            let _ = w.flush();
        }
    }

    pub fn abort_step(&self, id: usize) {
        let mut s = self.inner.lock().unwrap();
        s.active.retain(|a| a.id != id);
    }

    // redraws the status block until stop, meant to run on its own thread
    pub fn animate(&self) {
        if !self.tty {
            return;
        }
        let mut s = self.inner.lock().unwrap();
        while !s.stopped {
            let (guard, _) = self.wake.wait_timeout(s, FRAME).unwrap();
            s = guard;
            if s.stopped {
                break;
            }
            s.frame += 1;
            self.erase(&mut s);
            let err = std::io::stderr();
            let mut w = err.lock();
            self.draw(&mut s, &mut w);
            let _ = w.flush();
        }
    }

    pub fn stop(&self) {
        let mut s = self.inner.lock().unwrap();
        s.stopped = true;
        self.erase(&mut s);
        self.wake.notify_all();
    }

    pub fn summary(&self, built: usize, fresh: usize, cached: usize) {
        let secs = self.began.elapsed().as_secs_f64();
        let from_cache = if cached > 0 { format!(", cached {}", cached) } else { String::new() };
        if !self.tty {
            eprintln!("built {}{}, fresh {}, {:.1}s", built, from_cache, fresh, secs);
            return;
        }
        let mark = self.paint(&format!("1;{}", GREEN), "✓ done");
        let from_cache = if cached > 0 { format!(" ({} from cache)", cached) } else { String::new() };
        let rest = format!("built {}{} · fresh {} · {:.1}s", built, from_cache, fresh, secs);
        eprintln!("{}  {}", mark, self.paint(GRAY, &rest));
    }

    fn erase(&self, s: &mut Inner) {
        if s.drawn == 0 {
            return;
        }
        let mut out = String::from("\r\x1b[2K");
        for _ in 1..s.drawn {
            out.push_str("\x1b[1A\x1b[2K");
        }
        s.drawn = 0;
        let err = std::io::stderr();
        let mut w = err.lock();
        let _ = w.write_all(out.as_bytes());
    }

    fn draw(&self, s: &mut Inner, w: &mut impl Write) {
        if !self.tty || s.active.is_empty() {
            return;
        }
        let (rows, cols) = terminal_size();
        let width = cols.saturating_sub(1).max(20);
        let spin = SPINNER[s.frame % SPINNER.len()];
        let filled = if s.total == 0 { 0 } else { (s.done * BAR_WIDTH / s.total).min(BAR_WIDTH) };
        let bar = format!(
            "{}{}",
            self.paint(PINK, &"█".repeat(filled)),
            self.paint(GRAY, &"░".repeat(BAR_WIDTH - filled))
        );
        let counts = format!("{}/{}", s.done, s.total);
        let elapsed = format!("{:.1}s", self.began.elapsed().as_secs_f64());
        let head = format!(
            "{} {} {}  {}",
            self.paint(PINK, spin),
            bar,
            counts,
            self.paint(GRAY, &elapsed)
        );
        let mut lines: Vec<String> = Vec::new();
        if rows >= CAT_MIN_ROWS {
            for l in CAT.lines() {
                let l: String = l.trim_end().chars().take(width).collect();
                lines.push(self.paint(PINK, &l));
            }
        }
        lines.push(head);
        let shown = s.active.len().min(MAX_ACTIVE_LINES);
        for a in s.active.iter().take(shown) {
            let took = format!("{:.1}s", a.since.elapsed().as_secs_f64());
            let room = width.saturating_sub(4 + took.len() + 2);
            let label: String = a.label.chars().take(room).collect();
            lines.push(format!("  {} {}  {}", self.paint(GRAY, "›"), label, self.paint(GRAY, &took)));
        }
        if s.active.len() > shown {
            lines.push(self.paint(GRAY, &format!("  + {} more", s.active.len() - shown)));
        }
        let _ = w.write_all(lines.join("\n").as_bytes());
        s.drawn = lines.len();
    }
}

#[cfg(unix)]
#[repr(C)]
#[derive(Default)]
struct WinSize {
    rows: u16,
    cols: u16,
    xpixel: u16,
    ypixel: u16,
}

#[cfg(unix)]
extern "C" {
    fn ioctl(fd: i32, request: std::ffi::c_ulong, ...) -> i32;
}

// android reports itself as its own target os but uses the linux request numbers
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
const TIOCGWINSZ: std::ffi::c_ulong = 0x5413;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
const TIOCGWINSZ: std::ffi::c_ulong = 0x4008_7468;

// a status line wider than the terminal wraps and the redraw would leave debris,
// so the size has to be real
#[cfg(unix)]
fn terminal_size() -> (usize, usize) {
    let mut ws = WinSize::default();
    // SAFETY: fd 2 is valid for the call and ws is a writable winsize
    let rc = unsafe { ioctl(2, TIOCGWINSZ, &mut ws as *mut WinSize) };
    if rc == 0 && ws.cols > 0 {
        return (ws.rows as usize, ws.cols as usize);
    }
    let cols = std::env::var("COLUMNS").ok().and_then(|c| c.parse().ok()).unwrap_or(80);
    (0, cols)
}

#[cfg(windows)]
mod console {
    #[repr(C)]
    #[derive(Default)]
    pub struct Coord {
        pub x: i16,
        pub y: i16,
    }
    #[repr(C)]
    #[derive(Default)]
    pub struct Rect {
        pub left: i16,
        pub top: i16,
        pub right: i16,
        pub bottom: i16,
    }
    #[repr(C)]
    #[derive(Default)]
    pub struct ScreenBufferInfo {
        pub size: Coord,
        pub cursor: Coord,
        pub attributes: u16,
        pub window: Rect,
        pub maximum: Coord,
    }
    extern "system" {
        pub fn GetStdHandle(which: u32) -> isize;
        pub fn GetConsoleMode(handle: isize, mode: *mut u32) -> i32;
        pub fn SetConsoleMode(handle: isize, mode: u32) -> i32;
        pub fn GetConsoleScreenBufferInfo(handle: isize, info: *mut ScreenBufferInfo) -> i32;
    }
    pub const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    pub const VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
}

// the windows console reads escape sequences only after this mode bit is set
// a console that refuses it gets plain line output
#[cfg(windows)]
pub fn enable_ansi() -> bool {
    use console::*;
    // SAFETY: the handle comes from GetStdHandle and mode is a writable u32
    unsafe {
        let h = GetStdHandle(STD_ERROR_HANDLE);
        let mut mode = 0u32;
        if h == 0 || h == -1 || GetConsoleMode(h, &mut mode) == 0 {
            // mintty and other pty front ends are not consoles but set TERM and read escapes
            return std::env::var_os("TERM").is_some();
        }
        mode & VIRTUAL_TERMINAL_PROCESSING != 0 || SetConsoleMode(h, mode | VIRTUAL_TERMINAL_PROCESSING) != 0
    }
}

#[cfg(not(windows))]
pub fn enable_ansi() -> bool {
    true
}

#[cfg(windows)]
fn terminal_size() -> (usize, usize) {
    use console::*;
    let mut info = ScreenBufferInfo::default();
    // SAFETY: the handle comes from GetStdHandle and info is a writable screen buffer record
    let rc = unsafe { GetConsoleScreenBufferInfo(GetStdHandle(STD_ERROR_HANDLE), &mut info) };
    if rc != 0 && info.window.right >= info.window.left {
        let cols = (info.window.right - info.window.left + 1) as usize;
        let rows = (info.window.bottom - info.window.top + 1).max(0) as usize;
        return (rows, cols);
    }
    let cols = std::env::var("COLUMNS").ok().and_then(|c| c.parse().ok()).unwrap_or(80);
    (0, cols)
}

fn count_text(warnings: usize, errors: usize) -> String {
    let mut parts = Vec::new();
    if warnings > 0 {
        parts.push(format!("{} warning{}", warnings, if warnings == 1 { "" } else { "s" }));
    }
    if errors > 0 {
        parts.push(format!("{} error{}", errors, if errors == 1 { "" } else { "s" }));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" {}", parts.join(", "))
    }
}
