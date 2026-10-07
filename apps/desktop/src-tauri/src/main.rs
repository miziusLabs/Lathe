// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[tokio::main]
async fn main() {
    if std::env::args().any(|arg| arg == "--agent") {
        if let Err(error) = lathe_agent::run().await {
            eprintln!("Lathe agent: {error}");
            std::process::exit(1);
        }
        return;
    }
    ade_lib::run()
}
