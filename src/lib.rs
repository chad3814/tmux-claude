//! nrun — run a long-lived command (dev server, docker container) inside a
//! dedicated tmux window, stream its output back for transparent capture, and
//! own the process lifetime so it is cleaned up on exit or signal.

pub mod cli;
pub mod docker;
pub mod supervisor;
pub mod tail;
pub mod tmux;

pub use supervisor::run;
