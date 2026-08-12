#[tokio::main]
async fn main() {
    if let Err(error) = cider_bot::run().await {
        eprintln!("cider-bot: {error}");
        std::process::exit(1);
    }
}
