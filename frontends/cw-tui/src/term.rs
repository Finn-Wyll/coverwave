//! Keeping the terminal ours while the UI is up.
//!
//! ratatui only redraws cells it believes have changed, so anything else
//! that writes to the terminal leaves debris on screen until those cells
//! happen to change again (seen as "the menu gets corrupted"). Two guards:
//!
//! - [`StderrToLog`] points file descriptor 2 at a log file while the UI
//!   runs, so messages that native code (CoreAudio, AppKit and others)
//!   prints straight to stderr land in the log instead of on screen. It is
//!   restored on exit and before any panic message prints.
//! - [`draw_synced`] wraps each frame in a synchronized update (DEC mode
//!   2026), so terminals that support it show whole frames instead of
//!   half-drawn ones. Terminals without it ignore the sequence.

use ratatui::crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Draws one frame inside a synchronized update.
pub fn draw_synced(
    terminal: &mut ratatui::DefaultTerminal,
    draw: impl FnOnce(&mut ratatui::Frame),
) -> std::io::Result<()> {
    let mut out = std::io::stdout();
    ratatui::crossterm::queue!(out, BeginSynchronizedUpdate)?;
    out.flush()?;
    let result = terminal.draw(draw).map(|_| ());
    // End the update even if drawing failed, or the terminal stays frozen.
    ratatui::crossterm::execute!(out, EndSynchronizedUpdate)?;
    result
}

/// While alive, stderr goes to a log file. Dropping it puts stderr back.
pub struct StderrToLog {
    #[cfg(unix)]
    saved: libc::c_int,
    pub path: PathBuf,
}

impl StderrToLog {
    /// Redirects stderr to `path` (truncated). Returns `None` if the file
    /// cannot be opened or redirection is unsupported; the UI still runs.
    #[cfg(unix)]
    pub fn start(path: &Path) -> Option<StderrToLog> {
        use std::os::fd::IntoRawFd;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok()?;
        }
        let file = std::fs::File::create(path).ok()?;
        // SAFETY: plain descriptor calls on fds we own or on fd 2; every
        // result is checked, and the saved descriptor is restored on drop.
        unsafe {
            let saved = libc::dup(libc::STDERR_FILENO);
            if saved < 0 {
                return None;
            }
            let fd = file.into_raw_fd();
            let ok = libc::dup2(fd, libc::STDERR_FILENO) >= 0;
            libc::close(fd);
            if !ok {
                libc::close(saved);
                return None;
            }
            SAVED_STDERR.store(saved, std::sync::atomic::Ordering::SeqCst);
            Some(StderrToLog {
                saved,
                path: path.to_path_buf(),
            })
        }
    }

    #[cfg(not(unix))]
    pub fn start(_path: &Path) -> Option<StderrToLog> {
        None
    }
}

/// The original stderr, for the panic hook (-1 when not redirected).
#[cfg(unix)]
static SAVED_STDERR: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

/// Points stderr back at the terminal, if it was redirected. Safe to call
/// more than once.
pub fn restore_stderr() {
    #[cfg(unix)]
    {
        let saved = SAVED_STDERR.swap(-1, std::sync::atomic::Ordering::SeqCst);
        if saved >= 0 {
            // SAFETY: `saved` is a descriptor we duplicated and still own.
            unsafe {
                libc::dup2(saved, libc::STDERR_FILENO);
                libc::close(saved);
            }
        }
    }
}

impl Drop for StderrToLog {
    fn drop(&mut self) {
        #[cfg(unix)]
        let _ = self.saved;
        restore_stderr();
    }
}

/// Installs a panic hook that restores stderr and then runs the existing
/// hook (ratatui's, which restores the terminal and prints the message).
pub fn hook_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_stderr();
        previous(info);
    }));
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn stderr_goes_to_the_log_and_comes_back() {
        let dir = std::env::temp_dir().join(format!("cw-term-{}", std::process::id()));
        let log = dir.join("stderr.log");
        {
            let guard = StderrToLog::start(&log).expect("redirect");
            assert_eq!(guard.path, log);
            // Write through the raw descriptor, as native code would.
            let msg = b"native noise\n";
            // SAFETY: writing a byte buffer to fd 2.
            let n = unsafe { libc::write(libc::STDERR_FILENO, msg.as_ptr().cast(), msg.len()) };
            assert_eq!(n, msg.len() as isize);
        }
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "native noise\n");
        // Restored: the saved descriptor is gone and a second restore is a no-op.
        assert_eq!(SAVED_STDERR.load(std::sync::atomic::Ordering::SeqCst), -1);
        restore_stderr();
        std::fs::remove_dir_all(&dir).ok();
    }
}
