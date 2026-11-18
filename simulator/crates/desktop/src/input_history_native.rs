//! Actual Windows message producer for input160's 256-bit device history.
//! This observes native messages before winit translation, never InputState.
//! Evidence: Yukikoi26a590,26a69d/708,26a971..26ac02 -> device+628.
#[cfg(windows)]
mod windows {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use anyhow::{Context, Result};
    use winit::event_loop::EventLoop;
    use winit::platform::windows::EventLoopBuilderExtWindows;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    use crate::HeadlessPlayer;

    #[derive(Default)]
    struct DeviceHistory {
        bytes: [u8; 32],
        unsupported_touch: bool,
    }

    impl DeviceHistory {
        fn key(&mut self, key: u8, pressed: bool) {
            let byte = &mut self.bytes[usize::from(key) / 8];
            let mask = 1u8 << (key & 7);
            if pressed { *byte |= mask; } else { *byte &= !mask; }
        }

        fn message(&mut self, message: u32, wparam: usize) {
            match message {
                // Only WM_KEY*, not WM_SYSKEY*. Original uses CL (low8 VK).
                0x0100 => self.key(wparam as u8, true),
                0x0101 => self.key(wparam as u8, false),
                0x0201 => self.key(1, true),
                0x0202 => self.key(1, false),
                // The actual selector maps WM_MBUTTON to bit2, WM_RBUTTON
                // to bit3. Do not replace this with the standard VK names.
                0x0207 => self.key(2, true),
                0x0208 => self.key(2, false),
                0x0204 => self.key(3, true),
                0x0205 => self.key(3, false),
                0x020b | 0x020c => {
                    // Preserve the actual PE's 0x20/0x40 comparisons. Native
                    // XBUTTON1/2 values1/2 must not be silently substituted.
                    let key = match (wparam >> 16) as u16 { 0x20 => 4, 0x40 => 5, _ => return };
                    self.key(key, message != 0x020c);
                }
                // Original WM_TOUCH can suppress the left-button producer
                // through device+618. Until that exact raw touch lifetime is
                // bound, stop publishing instead of claiming complete bytes.
                0x0240 => self.unsupported_touch = true,
                _ => {}
            }
            // Focus changes, wheels, cursor movement and OS key repeats do
            // not clear history. Repeats simply write an already-set bit.
        }
    }

    thread_local! {
        static DEVICES: RefCell<BTreeMap<isize, DeviceHistory>> = RefCell::new(BTreeMap::new());
    }

    /// MSG's initial fields have the same ABI on both supported Windows word
    /// sizes. Only this prefix is read; the pointer is owned by winit/Win32.
    #[repr(C)]
    struct MessagePrefix { window: isize, message: u32, wparam: usize, lparam: isize }

    pub(super) fn event_loop() -> Result<EventLoop<()>> {
        let mut builder = EventLoop::builder();
        builder.with_msg_hook(|pointer| {
            if !pointer.is_null() {
                // winit documents this callback pointer as a live const MSG.
                let message = unsafe { &*pointer.cast::<MessagePrefix>() };
                if crate::settings_menu_windows::observe_menu_command(
                    message.window, message.message, message.wparam, message.lparam,
                ) { return true; }
                if message.window != 0 {
                    DEVICES.with_borrow_mut(|devices| {
                        if message.message == 0x0082 { devices.remove(&message.window); }
                        else { devices.entry(message.window).or_default().message(message.message, message.wparam); }
                    });
                }
            }
            false // Every message still follows normal winit dispatch.
        });
        builder.build().context("create Windows event loop with raw device history")
    }

    pub(super) fn publish(player: &mut HeadlessPlayer, window: &Window) -> Result<()> {
        let handle = window.window_handle().context("read real window handle for raw device history")?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            anyhow::bail!("Windows device history received a non-Win32 window");
        };
        let snapshot = DEVICES.with_borrow_mut(|devices| {
            let device = devices.entry(handle.hwnd.get()).or_default();
            (!device.unsupported_touch).then_some(device.bytes)
        });
        if let Some(snapshot) = snapshot { player.observe_host_device_history(snapshot); }
        else { player.data_services.messages.pending_device_history = None; }
        Ok(())
    }
}

#[cfg(windows)]
pub(super) fn event_loop() -> anyhow::Result<winit::event_loop::EventLoop<()>> { windows::event_loop() }

#[cfg(windows)]
pub(super) fn publish(player: &mut crate::HeadlessPlayer, window: &winit::window::Window) -> anyhow::Result<()> {
    windows::publish(player, window)
}
