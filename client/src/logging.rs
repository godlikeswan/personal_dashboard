use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;

use chrono::Local;

use crate::config;

static LOG_LOCK: Mutex<()> = Mutex::new(());

// The Windows build uses a GUI subsystem, so there is no console for println!.
// Keep a small append-only log next to dashboard-settings.txt instead.
pub fn log(message: &str) {
    let line = format!(
        "[{}] {message}\n",
        Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    let path = config::config_dir().join("dashboard.log");

    let _guard = LOG_LOCK.lock();
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}
