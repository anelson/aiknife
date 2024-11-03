use specta_typescript::Typescript;
use tauri::Manager;

pub mod chat;

pub fn run() {
    let builder = tauri_specta::Builder::<tauri::Wry>::new()
        .commands(tauri_specta::collect_commands![
            chat::new_session,
            chat::send_message,
            chat::check_api_key,
        ])
        .events(tauri_specta::collect_events![
            chat::events::MessagePending,
            chat::events::MessageResponse,
            chat::events::MessageError
        ])
        .error_handling(tauri_specta::ErrorHandlingMode::Throw);

    #[cfg(debug_assertions)] // <- Only export on non-release builds
    builder
        .export(Typescript::default(), "../src/bindings.ts")
        .expect("Failed to export typescript bindings");

    tauri::Builder::default()
        .invoke_handler(builder.invoke_handler())
        .manage(chat::AppState::new())
        .setup(move |app| {
            #[cfg(debug_assertions)]
            {
                let window = app.get_webview_window("main").unwrap();
                window.open_devtools();
            }

            // Wire up the tauri-specta strongly-typed event magic
            builder.mount_events(app);

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
