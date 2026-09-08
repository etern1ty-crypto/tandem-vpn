//! One typed IPC surface. Blocking OS/network work never runs on the UI thread.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{Emitter, Manager};

#[derive(Default)]
struct Runtime {
    busy: AtomicBool,
    closing: AtomicBool,
}
struct AppState {
    runtime: Arc<Runtime>,
}
struct BusyGuard(Arc<Runtime>);
impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::Release);
    }
}

#[tauri::command]
async fn request(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    action: tandem_core::Action,
) -> Result<serde_json::Value, String> {
    let runtime = Arc::clone(&state.runtime);
    if runtime.closing.load(Ordering::Acquire) {
        return Err("Application is closing; no new operations are accepted".into());
    }
    if runtime
        .busy
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("Another request is active; wait and retry".into());
    }
    let guard = BusyGuard(Arc::clone(&runtime));
    if runtime.closing.load(Ordering::Acquire) {
        drop(guard);
        app.exit(0);
        return Err("Application is closing".into());
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        tandem_core::Workbench::new()
            .and_then(|workbench| workbench.execute(action))
            .map_err(|error| error.to_string())
    })
    .await;
    if runtime.closing.load(Ordering::Acquire) {
        app.exit(0);
    }
    match result {
        Ok(result) => result,
        Err(error) => Err(format!(
            "Operation worker failed: {error}; inspect recovery status before retrying"
        )),
    }
}

pub fn run() {
    let result = tauri::Builder::default()
        .manage(AppState {
            runtime: Arc::new(Runtime::default()),
        })
        .invoke_handler(tauri::generate_handler![request])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<AppState>();
                let runtime = &state.runtime;
                runtime.closing.store(true, Ordering::Release);
                if runtime.busy.load(Ordering::Acquire) {
                    api.prevent_close();
                    let _ = window.emit(
                        "shutdown-wait",
                        "Завершаем текущую операцию перед закрытием. Не выключайте компьютер.",
                    );
                }
            }
        })
        .run(tauri::generate_context!());
    if let Err(error) = result {
        eprintln!("Tandem Workbench startup failed: {error}");
        std::process::exit(1);
    }
}
