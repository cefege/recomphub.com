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

#[cfg(not(feature = "server"))]
fn main() {
    dioxus::launch(recomphub::app::App);
}
