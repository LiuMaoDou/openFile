#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use filem_core::Engine;
use tauri::Manager;

#[tauri::command]
async fn command(
    engine: tauri::State<'_, Engine>,
    command: String,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .dispatch(&command, args)
            .map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| format!("{e:#}"))?
}
fn main() {
    if filem_core::run_helper_if_requested() {
        return;
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let engine = Engine::open_managed(app.path().app_local_data_dir()?)?;
            app.manage(engine);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![command])
        .run(tauri::generate_context!())
        .expect("FileM 无法启动");
}
