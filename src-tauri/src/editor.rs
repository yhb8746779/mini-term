use std::path::Path;
use std::process::Command;
use tauri::AppHandle;

/// 使用用户在设置中配置的 VS Code 可执行文件打开指定路径。
///
/// VS Code 可执行文件路径由后端直接从 config.json 读取(`AppConfig.vscode_path`),
/// 前端仅传入要打开的目录/文件路径。这样避免了 executable 参数被前端或任何
/// 拿到 invoke 能力的代码替换为任意可执行文件的风险。
#[tauri::command]
pub fn open_in_vscode(app: AppHandle, path: String) -> Result<(), String> {
    let cfg = crate::config::read_config(&app);
    let exe = cfg.vscode_path.as_deref().unwrap_or("").trim();
    if exe.is_empty() {
        return Err(
            "尚未配置 VS Code 可执行文件路径,请在『设置 → 系统设置 → 外部编辑器』中指定。"
                .to_string(),
        );
    }

    let exe_path = Path::new(exe);
    if !exe_path.exists() {
        return Err(format!("配置的 VS Code 路径不存在:{}", exe));
    }

    let mut cmd = Command::new(exe_path);
    cmd.arg(&path);

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 VS Code 失败:{}", e))
}
