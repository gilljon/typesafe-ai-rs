use typesafe_ai_rs::{blocking::Client, Noul, SystemOneRequest};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let response = Client::new()?.system_one(SystemOneRequest::new(
        "I was charged twice.",
        [("billing", Noul::new("Is this about billing?"))],
    ))?;
    println!("billing: {}", response.nouls()["billing"].noul);
    Ok(())
}
