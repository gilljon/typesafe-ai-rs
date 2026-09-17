use serde_json::json;
use typesafe_ai_rs::{Choice, Client, Noul, Question, Score, SystemOneRequest};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let response = Client::new()?
        .system_one(SystemOneRequest::new(
            json!({"document": "I was charged twice. Please fix this ASAP."}),
            [
                (
                    "billing",
                    Question::from(Noul::new("Is this about billing?")),
                ),
                (
                    "tone",
                    Choice::new([
                        ("calm", json!(null)),
                        ("frustrated", json!(null)),
                        ("angry", json!(null)),
                    ])
                    .instructions("What is the customer's tone?")
                    .into(),
                ),
                (
                    "urgency",
                    Score::new(["can wait", "this week", "today"])
                        .instructions("How urgent is this?")
                        .into(),
                ),
            ],
        ))
        .await?;
    println!("billing: {}", response.nouls()["billing"].noul);
    println!("tone: {}", response.choices()["tone"].choice);
    println!("urgency: {}", response.scores()["urgency"].score);
    println!("request ID: {:?}", response.request_id());
    Ok(())
}
