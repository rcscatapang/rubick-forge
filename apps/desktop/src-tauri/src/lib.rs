//! The desktop shell: a window, and later notifications and keychain access.
//!
//! **D3**: this process is strictly an API client. It never opens SQLite, never
//! spawns a process, never talks to tmux or git. Closing it changes nothing
//! about running agents.

/// Version of the app shell, surfaced to the frontend so the UI can show what
/// it is running next to the daemon version it talks to.
#[tauri::command]
fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![app_version])
        .run(tauri::generate_context!())
        .expect("error while running rubick-forge");
}

#[cfg(test)]
mod tests {
    #[test]
    fn app_version_is_the_crate_version() {
        assert_eq!(super::app_version(), env!("CARGO_PKG_VERSION"));
    }
}
