//! The window's views. Each panel owns its retained state (text fields,
//! scroll positions, drags) and reads the project from the store.

pub mod agent;
pub mod agent_panel;
pub mod dialogs;
pub mod editor;
pub mod generate;
pub mod generate_panel;
pub mod home;
pub mod inspector;
pub mod jobs;
pub mod left_panel;
pub mod media_panel;
pub mod motion_panel;
pub mod overlays;
pub mod preview;
pub mod screenshot;
pub mod text_panel;
pub mod timeline;
