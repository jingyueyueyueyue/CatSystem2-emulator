//! Physical host time, independent of game frames, task speed and app pause.
#[cfg(not(windows))]
use std::time::Instant;

use super::{HeadlessPlayer, LoggingHost};

#[derive(Debug)]
pub(super) struct HostClock {
    #[cfg(not(windows))]
    origin: Instant,
    trace_override: Option<u32>,
}

impl Default for HostClock {
    fn default() -> Self {
        Self {
            #[cfg(not(windows))]
            origin: Instant::now(),
            trace_override: None,
        }
    }
}

impl HostClock {
    fn millis(&self) -> u32 {
        if let Some(value) = self.trace_override {
            return value;
        }
        #[cfg(windows)]
        {
            // Low DWORD intentionally wraps every ~49.7 days, as the
            // original timeGetTime callers expect. This is system uptime,
            // not an artificial zero at the first sound request.
            unsafe { GetTickCount64() as u32 }
        }
        #[cfg(not(windows))]
        {
            // Origin is captured when the player host state is created.
            // Reading never resets it. Suspend does not alter this clock.
            self.origin.elapsed().as_millis() as u32
        }
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetTickCount64() -> u64;
}

impl LoggingHost<'_> {
    /// Physical monotonic milliseconds. Stamp owners still enforce their own
    /// u32::MAX "first call only" rule; reading this clock has no side effect.
    pub(super) fn host_millis(&self) -> u32 {
        self.data_services.host_clock.millis()
    }
}

impl HeadlessPlayer {
    /// Explicit deterministic trace time. None resumes the same physical
    /// clock, including the real time elapsed while the override was active.
    pub fn set_host_millis_override(&mut self, value: Option<u32>) {
        self.data_services.host_clock.trace_override = value;
    }
}
