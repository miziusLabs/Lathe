#[tokio::main]
async fn main() -> anyhow::Result<()> {
    lathe_agent::run().await
}
