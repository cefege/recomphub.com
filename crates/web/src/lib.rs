//! RecompHub web crate: the Dioxus UI, server functions, and server-only plumbing.

pub mod api;
pub mod app;
pub mod components;
pub mod pages;
pub mod routes;

#[cfg(feature = "server")]
pub mod server;
