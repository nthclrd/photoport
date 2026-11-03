//! What the user must install when usbmuxd is unreachable, per OS.

use serde::Serialize;

/// Apple Devices (Microsoft Store). Ships the Apple Mobile Device service;
/// we never redistribute Apple's driver ourselves.
pub const APPLE_DEVICES_STORE_URI: &str = "ms-windows-store://pdp/?productid=9NP83LWLPZ9K";
pub const APPLE_DEVICES_WEB_URL: &str = "https://apps.microsoft.com/detail/9NP83LWLPZ9K";

#[derive(Debug, Clone, Serialize)]
pub struct Help {
    pub os: &'static str,
    /// Shell commands to show (Linux), empty elsewhere.
    pub commands: Vec<&'static str>,
    pub store_uri: Option<&'static str>,
    pub web_url: Option<&'static str>,
}

pub fn help() -> Help {
    match std::env::consts::OS {
        "windows" => Help {
            os: "windows",
            commands: vec![],
            store_uri: Some(APPLE_DEVICES_STORE_URI),
            web_url: Some(APPLE_DEVICES_WEB_URL),
        },
        "linux" => Help {
            os: "linux",
            commands: vec![
                "sudo apt install usbmuxd        # Debian, Ubuntu, Mint",
                "sudo dnf install usbmuxd        # Fedora",
                "sudo pacman -S usbmuxd          # Arch",
                "sudo systemctl start usbmuxd",
            ],
            store_uri: None,
            web_url: None,
        },
        os => Help { os: if os == "macos" { "macos" } else { "other" }, commands: vec![], store_uri: None, web_url: None },
    }
}
