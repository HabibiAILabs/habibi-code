//! Run with: cargo run -p libhabibi --example embed -- AUTH_DIRECTORY WORKSPACE PROMPT
//! AUTH_DIRECTORY is Habibi's credential directory, not a credential value.
use anyhow::{Context, Result, ensure};
use libhabibi::{CancellationToken, Event, Provider, Session, SessionConfig, auth::AuthStore};
use std::io::Write;
use tokio::sync::mpsc;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ensure!(
        args.len() == 3,
        "Usage: embed AUTH_DIRECTORY WORKSPACE PROMPT"
    );
    let config = SessionConfig::new(Provider::Codex, &args[1]);
    let mut session = Session::new(config, AuthStore::new(&args[0]))?;
    let (events, mut stream) = mpsc::channel(64);
    let consumer = tokio::spawn(async move {
        while let Some(event) = stream.recv().await {
            match event {
                Event::TextDelta(text) => {
                    print!("{text}");
                    std::io::stdout().flush()?;
                }
                Event::Permission { call, reply } => {
                    // Replace with your application's explicit approval policy or UI.
                    eprintln!(
                        "Denied {}: this example grants no tool permissions",
                        call.name
                    );
                    let _ = reply.send(false);
                }
                _ => {}
            }
        }
        Ok::<_, std::io::Error>(())
    });
    let cancel = CancellationToken::new();
    let result = session.prompt(args[2].clone(), events, cancel).await;
    consumer.await.context("Event consumer failed")??;
    println!();
    result
}
