//! Ctrl-C for the commands that change Roblox state. The first SIGINT or SIGTERM asks the run to stop: nothing new is
//! sent, requests in flight finish and the lock is saved. The second exits at once with 130.

use std::sync::atomic::{AtomicBool, Ordering};

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    // only async-signal-safe calls here: an atomic swap and _exit
    if STOP.swap(true, Ordering::SeqCst) {
        unsafe { libc::_exit(130) };
    }
}

/// Installs the handler for SIGINT and SIGTERM, from a command holding the remote guard. System calls a signal lands
/// in are restarted, so a request in flight runs on to its own timeout instead of failing.
pub fn install() {
    // SAFETY: a zeroed sigaction is valid, and the handler only touches an atomic and calls _exit
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        action.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());
        libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut());
    }
}

/// True once a stop was asked for.
pub fn stopped() -> bool {
    STOP.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_first_signal_asks_for_a_stop_instead_of_ending_the_process() {
        super::install();
        assert!(!super::stopped());
        // SAFETY: raise only runs the handler just installed
        unsafe { libc::raise(libc::SIGTERM) };
        assert!(super::stopped());
    }
}
