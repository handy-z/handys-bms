use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Duration;

use handy_sdk::app::updater_thread::Updater;
use handy_sdk::input::focus::roblox_ready;
use handy_sdk::input::key::{MOUSE_LEFT, key_down};
use handy_sdk::input::send::tap;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_XBUTTON2;

use crate::app::config::SettingsStore;

/// Hold-to-spam: while the mouse's X2 (forward) button is held, Roblox is the foreground window and the cursor is over
/// it, clicks the left button where the cursor is, 20 times a second. Sends input (the one part of bms that does); Settings::click_x2
/// turns it off. Its own thread; reads the button with GetAsyncKeyState, no hook.
pub struct Clicker {
    updater: Arc<Updater>,
    settings: Arc<SettingsStore>,
    clicking: AtomicBool,
}

impl Clicker {
    pub fn new(updater: Arc<Updater>, settings: Arc<SettingsStore>) -> Self {
        Self {
            updater,
            settings,
            clicking: AtomicBool::new(false),
        }
    }

    pub fn run(&self, stop: &AtomicBool) {
        while !stop.load(Relaxed) {
            let session = self.updater.session();
            let held = key_down(VK_XBUTTON2.0);
            let clicking = self.settings.get().click_x2 && held && session.is_some_and(|s| roblox_ready(s.proc.pid()));
            self.clicking.store(clicking, Relaxed);
            if clicking {
                tap(&MOUSE_LEFT, Duration::from_millis(10)); // Roblox drops a down+up delivered in the same frame
                std::thread::sleep(Duration::from_millis(40)); // 50 ms a click: 20 a second
            } else {
                std::thread::sleep(Duration::from_millis(10)); // starts within a frame of the press
            }
        }
    }

    /// Any thread.
    pub fn clicking(&self) -> bool {
        self.clicking.load(Relaxed)
    }
}
