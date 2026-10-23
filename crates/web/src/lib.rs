//! RecompHub web crate: the Dioxus UI, server functions, and server-only plumbing.

pub mod api;
pub mod app;
pub mod components;
pub mod pages;
pub mod routes;

#[cfg(feature = "desktop")]
pub mod desktop;

#[cfg(all(feature = "desktop", any(feature = "web", feature = "server")))]
compile_error!("the desktop renderer cannot be built with the web or server renderer features");

// Dioxus 0.7.10 silently loses SSR hydration data when these are unified (#5427).
#[cfg(all(feature = "web", feature = "server"))]
compile_error!(
    "build web and server as separate targets; do not pass --features web as shared dx fullstack arguments"
);

#[cfg(feature = "server")]
pub mod server;
