//! Terminal helpers: raw mode, hidden input, sizes, key polling.

use std::io::{self, Read, Write};

pub struct RawMode {
    fd: i32,
    saved: libc::termios,
}

impl RawMode {
    /// Disable line buffering, echo and signal keys on `fd`.
    pub fn enable(fd: i32) -> Option<RawMode> {
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut t) } != 0 {
            return None;
        }
        let saved = t;
        t.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
        t.c_iflag &= !(libc::IXON | libc::ICRNL);
        t.c_cc[libc::VMIN] = 1;
        t.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &t) } != 0 {
            return None;
        }
        Some(RawMode { fd, saved })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved);
        }
    }
}

pub fn is_tty(fd: i32) -> bool {
    unsafe { libc::isatty(fd) == 1 }
}

pub fn size() -> (u16, u16) {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_col > 0 {
        (ws.ws_col, ws.ws_row)
    } else {
        (80, 24)
    }
}

/// Wait up to `ms` for a key; returns the byte or None on timeout.
pub fn read_key(ms: i32) -> Option<u8> {
    let mut pfd = libc::pollfd {
        fd: 0,
        events: libc::POLLIN,
        revents: 0,
    };
    let r = unsafe { libc::poll(&mut pfd, 1, ms) };
    if r <= 0 {
        return None;
    }
    let mut b = [0u8; 1];
    match io::stdin().read(&mut b) {
        Ok(1) => Some(b[0]),
        // EOF (e.g. SSH session closed): behave like the exit key.
        _ => Some(b'0'),
    }
}

/// Read a line without echo (passwords). Restores the terminal afterwards.
pub fn read_hidden(prompt: &str) -> io::Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    let tty = unsafe { libc::tcgetattr(0, &mut t) } == 0;
    let saved = t;
    if tty {
        t.c_lflag &= !libc::ECHO;
        t.c_lflag |= libc::ECHONL;
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &t) };
    }
    let mut s = String::new();
    let r = io::stdin().read_line(&mut s);
    if tty {
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &saved) };
    }
    r?;
    Ok(s.trim_end_matches(['\n', '\r']).to_string())
}

pub const CLEAR: &str = "\x1b[H\x1b[2J\x1b[3J";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const RED: &str = "\x1b[31m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const RESET: &str = "\x1b[0m";
