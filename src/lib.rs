//! tuidesk: a KDE Plasma style desktop for the terminal.
//!
//! The binary (`src/main.rs`) is the command line; everything else lives here so it can also be
//! exercised by the fuzz targets in `fuzz/`.

pub mod app;
pub mod config;
pub mod graphics;
pub mod input;
pub mod ipc;
pub mod keys;
pub mod lua;
pub mod notify;
pub mod runner;
pub mod screenshot;
pub mod session;
pub mod settings;
pub mod term;
pub mod theme;
pub mod tray;
pub mod ui;
pub mod wm;
