use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    synchronizer::run().await
}
