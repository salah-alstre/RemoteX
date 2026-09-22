//! RemoteX Windows service: privileged input and secure-desktop access for approved elevated sessions.
//! The binary (`remotex-service.exe`) is a thin wrapper; the pieces live here so they can be tested.

#![cfg(windows)]

pub mod agent;
pub mod broker;
mod desktop;
pub mod install;
mod launch;
pub mod server;
pub mod service;
