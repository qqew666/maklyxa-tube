#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

#[cfg(any(windows, test))]
mod model;

#[cfg(any(windows, test))]
mod relay;

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod download;
#[cfg(windows)]
mod platform;
#[cfg(windows)]
mod tools;
#[cfg(windows)]
mod tor;
#[cfg(windows)]
mod window;

#[cfg(windows)]
fn main() {
    app::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("MaklyxaTube требует Windows и Microsoft Edge WebView2.");
    std::process::exit(1);
}
