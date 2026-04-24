mod ai_sessions;
mod clipboard;
mod config;
mod editor;
mod fs;
mod git;
mod path_access;
mod perf_log;
mod process_monitor;
mod pty;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(tauri_plugin_window_state::StateFlags::MAXIMIZED)
                .build(),
        )
        .manage(pty::PtyManager::new())
        .manage(fs::FsWatcherManager::new())
        .manage(ai_sessions::SessionCache::new())
        .manage(git::GitRepoCache::new())
        .manage(path_access::PathAccessManager::new())
        .setup(|app| {
            clipboard::cleanup_old_clipboard_images();
            let handle = app.handle().clone();
            let config = crate::config::read_config(&handle);
            crate::path_access::sync_project_accesses(&handle, &config.projects);
            let pty_manager = app.state::<crate::pty::PtyManager>();
            let pty_clone = pty_manager.inner().clone();
            process_monitor::start_monitor(handle, pty_clone);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            config::load_config,
            config::save_config,
            path_access::prepare_project_access,
            pty::create_pty,
            pty::write_pty,
            pty::resize_pty,
            pty::kill_pty,
            fs::list_directory,
            fs::watch_directory,
            fs::unwatch_directory,
            fs::create_file,
            fs::create_directory,
            fs::read_file_content,
            fs::rename_entry,
            ai_sessions::get_ai_sessions,
            git::get_git_status,
            git::get_git_diff,
            git::discover_git_repos,
            git::get_git_log,
            git::get_repo_branches,
            git::get_commit_files,
            git::get_commit_file_diff,
            git::git_pull,
            git::git_push,
            editor::open_in_vscode,
            perf_log::clear_perf_log,
            perf_log::read_perf_log,
            perf_log::get_perf_log_path,
            perf_log::log_perf_from_frontend,
            clipboard::save_clipboard_rgba_image,
            clipboard::read_clipboard_image_macos,
            clipboard::read_clipboard_image,
            clipboard::read_clipboard_file_paths,
            clipboard::read_clipboard_file_paths_macos,
            clipboard::load_image_to_clipboard,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
