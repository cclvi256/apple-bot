use cider_bot::routes::app_routes;
use tracing::info;

#[tokio::main]
async fn main() {
    let app = app_routes();
    let listener = tokio::net::TcpListener::bind("[::]:8888")
        .await
        .expect("Failed to bind TCP listener");
    info!("Listening on: http://[::]:8888");
    axum::serve(listener, app)
        .await
        .expect("Failed to serve HTTP service");
}
