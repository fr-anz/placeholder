//! Fork identity. Rename the app here: data directory and window/app IDs.
//! Keeps this build fully separate from an installed MonoCode or Placeholder.

/// The name shown in window titles and the desktop launcher.
pub(crate) const APP_NAME: &str = "Placeholder";
/// Directory under the platform data dir (XDG_DATA_HOME, LOCALAPPDATA, Application Support).
pub(crate) const DATA_DIR_NAME: &str = "Placeholder";
/// Wayland app_id / X11 WM_CLASS base for the main window.
pub(crate) const APP_ID: &str = "io.github.fr_anz.placeholder";
pub(crate) const MINION_CHAT_APP_ID: &str = "io.github.fr_anz.placeholder.minion-chat";
pub(crate) const QUICK_COMPOSER_APP_ID: &str = "io.github.fr_anz.placeholder.quick-composer";
