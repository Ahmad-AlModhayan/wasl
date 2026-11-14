//! The parts of the `wasl` CLI the tray app reuses: starting `wasld` at login and the message
//! catalog with its language choice. The commands themselves live in the `wasl` binary.

pub mod i18n;
pub mod service;
