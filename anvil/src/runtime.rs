//! A Tokio runtime beside the GTK main loop. LSP, DAP, AI, git and dotnet run there; the
//! UI awaits the result inside `glib::spawn_future_local`, so nothing blocks
//! a frame.

use std::future::Future;
use std::sync::OnceLock;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("anvil-io")
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

/// Runs `future` on the I/O runtime and waits for it from the UI.
pub async fn run<F>(future: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match runtime().spawn(future).await {
        Ok(value) => value,
        Err(e) => std::panic::resume_unwind(e.into_panic()),
    }
}

/// The runtime itself, for spawning work that reports through channels
/// instead of being awaited.
pub fn handle() -> tokio::runtime::Handle {
    runtime().handle().clone()
}
