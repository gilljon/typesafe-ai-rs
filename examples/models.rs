use typesafe_ai_rs::Client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    for model in Client::new()?.models().list().await?.models {
        println!("{}: {}", model.name, model.description);
    }
    Ok(())
}
