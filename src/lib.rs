//! `try` — fresh directories for every vibe.
//!
//! The binary prints a *shell script* on stdout; a small shell wrapper
//! (see [`init`]) `eval`s it so the parent shell can `cd`. The interactive UI
//! renders on stderr.

pub mod cli;
pub mod config;
pub mod doctor;
pub mod entries;
pub mod fuzzy;
pub mod git;
pub mod init;
pub mod naming;
pub mod paths;
pub mod script;
pub mod trash;
pub mod tui;
