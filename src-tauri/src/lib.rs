mod ai_sessions;
mod clipboard;
mod config;
mod editor;
mod fs;
mod git;
mod hook_registry;
mod hook_server;
mod path_access;
mod perf_log;
mod search;
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
        .manage(search::SearchManager::new())
        .manage(hook_server::HookState::new())
        .setup(|app| {
            clipboard::cleanup_old_clipboard_images();
            let handle = app.handle().clone();
            let config = crate::config::read_config(&handle);
            crate::path_access::sync_project_accesses(&handle, &config.projects);
            let pty_manager = app.state::<crate::pty::PtyManager>();
            let pty_clone = pty_manager.inner().clone();
            process_monitor::start_monitor(handle.clone(), pty_clone);

            // Hook server 按需启动：仅在用户开启 hookEnabled 时绑端口，
            // 避免 Windows 防火墙在首次启动时无条件弹出授权请求。
            if config.hook_enabled.unwrap_or(false) {
                let hook_state = app.state::<crate::hook_server::HookState>();
                if let Err(e) = crate::hook_server::start_hook_server(
                    handle.clone(),
                    hook_state.inner().clone(),
                ) {
                    eprintln!("[hook-server] 启动失败: {}", e);
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            config::load_config,
            config::save_config,
            path_access::prepare_project_access,
            path_access::get_full_disk_access_status,
            path_access::recheck_full_disk_access,
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
            search::start_search,
            search::cancel_search,
            hook_registry::register_ai_hooks,
            hook_registry::unregister_ai_hooks,
            hook_registry::get_hook_config_snippet,
            hook_registry::get_hook_status,
            hook_server::toggle_hook_server,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
