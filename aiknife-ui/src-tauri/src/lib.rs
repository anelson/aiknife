use anyhow::Context;
use tauri::Manager;

pub mod chat;
mod error;

pub use error::{Result, UiError};

fn generate_specta_bindings() -> Result<tauri_specta::Builder> {
    let builder = tauri_specta::Builder::<tauri::Wry>::new()
        .commands(tauri_specta::collect_commands![
            chat::new_session,
            chat::send_message,
            chat::check_api_key,
            chat::abort_message,
            chat::retry_message,
        ])
        .events(tauri_specta::collect_events![
            chat::events::NewMessage,
            chat::events::MessageStatusChanged,
            chat::events::MessageStream,
        ])
        .error_handling(tauri_specta::ErrorHandlingMode::Throw);

    #[cfg(debug_assertions)]
    builder
        .export(
            specta_typescript::Typescript::default(),
            "../src/bindings.ts",
        )
        .expect("Failed to export typescript bindings");

    Ok(builder)
}

pub fn run() -> Result<()> {
    // NOTE: `tracing` is not initialized at all.  In Cargo.toml the `log` and `log-always`
    // features are enabled.  See <https://docs.rs/tracing/latest/tracing/#crate-feature-flags>
    // for details; tl;dr is that this forces tracing to emit `log` crate records for every event.
    //
    // It appears that the tauri log plugin internally used `log`, so we don't explicitly
    // initialize `log` either.

    let builder = generate_specta_bindings()?;

    #[cfg(debug_assertions)]
    let level = { log::LevelFilter::Debug };
    #[cfg(not(debug_assertions))]
    let level = { log::LevelFilter::Info };

    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(level)
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Webview,
                ))
                .build(),
        )
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
        .context("error while running tauri application")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regenerate the specta bindings in a test.  This is also done on every run of the
    /// application in debug builds, but sometimes being able to run that in a test without running
    /// the whole app is useful.
    #[test]
    fn test_specta_bindings() {
        let builder = generate_specta_bindings().unwrap();

        let ts = builder
            .export_str(specta_typescript::Typescript::default())
            .unwrap();
        println!("Generated Specta typescript:\n{}", ts);
        assert!(!ts.is_empty());
    }
}
