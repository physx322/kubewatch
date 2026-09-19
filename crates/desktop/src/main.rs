//! KubeWatch — application de bureau native pour piloter des clusters Kubernetes.
#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod backend;
mod format;
mod icons;
mod state;
mod theme;
mod views;
mod widgets;

fn main() -> eframe::Result<()> {
    app::run()
}
