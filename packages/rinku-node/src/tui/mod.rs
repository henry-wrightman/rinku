#[cfg(feature = "tui")]
pub mod app;
#[cfg(feature = "tui")]
pub mod ui;
#[cfg(feature = "tui")]
pub mod event;
#[cfg(feature = "tui")]
pub mod runner;

#[cfg(feature = "tui")]
pub use app::App;
#[cfg(feature = "tui")]
pub use event::EventHandler;
#[cfg(feature = "tui")]
pub use runner::run_tui;
