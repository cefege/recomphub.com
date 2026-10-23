//! Launches RecompHub. The client bundle renders `app::App` in the browser. On the
//! server, the Dioxus fullstack router is wrapped with RecompHub's own HTTP routes
//! and the background tasks start once before the first request.

#[cfg(feature = "server")]
fn main() {
    dioxus::serve(|| async {
        dotenvy::dotenv().ok();
        let config = recomphub::server::config::Config::from_env()?;
        let database_url = config.database_url.clone();
        recomphub::server::config::install(config);
        recomphub::server::bootstrap::start(&database_url).await?;
        let app = dioxus::server::router(recomphub::app::App);
        Ok(recomphub::server::http::with_site_routes(app))
    });
}

#[cfg(all(not(feature = "server"), feature = "desktop"))]
fn main() {
    match recomphub_desktop::run_shell_launch_helper_if_requested() {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            eprintln!("RecompHub native launch worker failed: {error}");
            std::process::exit(1);
        }
    }
    if let Err(error) = recomphub::desktop::initialize() {
        eprintln!("RecompHub desktop could not start: {error}");
        std::process::exit(1);
    }
    let config = dioxus::desktop::Config::new()
        .with_window(
            dioxus::desktop::WindowBuilder::new()
                .with_title("RecompHub")
                .with_inner_size(dioxus::desktop::LogicalSize::new(1100.0, 720.0))
                .with_min_inner_size(dioxus::desktop::LogicalSize::new(800.0, 560.0)),
        )
        .with_navigation_handler(|_| false);
    dioxus::LaunchBuilder::desktop()
        .with_cfg(config)
        .launch(recomphub::app::App);
}

#[cfg(all(not(feature = "server"), not(feature = "desktop")))]
fn main() {
    dioxus::launch(recomphub::app::App);
}
