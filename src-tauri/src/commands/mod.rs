//! Tauri commands. Thin wrappers over the services in `AppState`.

pub mod audio;
pub mod injection;
pub mod models;
pub mod settings;
pub mod targets;

use crate::app_state::AppState;

pub type State<'a> = tauri::State<'a, AppState>;
pub type CmdResult<T> = Result<T, crate::error::AppError>;
