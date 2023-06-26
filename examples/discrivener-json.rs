use clap::Parser;
use discrivener::Discrivener;

use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    select, signal,
};

async fn tokio_main(cli: Cli) {
    let mut discrivener = Discrivener::load(
        cli.model_path,
        Arc::new(|event| {
            let json_string = serde_json::to_string(&event).unwrap();
            // eprintln!("API Event: {:?}", json_string);
            println!("{}", json_string);
        }),
    )
    .await;

    let connection_result = discrivener
        .connect(
            cli.channel_id,
            cli.endpoint.as_str(),
            cli.guild_id,
            cli.session_id.as_str(),
            cli.user_id,
            cli.voice_token.as_str(),
        )
        .await;
    if let Err(e) = connection_result {
        eprintln!("Error joining voice channel: {}", e);
    }

    let mut stdin_reader = BufReader::new(tokio::io::stdin());
    loop {
        let mut line = String::with_capacity(120);
        select! {
            _ = stdin_reader.read_line(&mut line) => {
                line = line.trim().to_string();
                eprintln!("Speaking: '{}'", line);
                discrivener.speak(line);
            }
            _ = signal::ctrl_c() => {
                break;
            }
        }
    }
    eprintln!("Disconnecting...");
    discrivener.disconnect().await;
    eprintln!("Disconnected gracefully");
}

/// Connect to a discord voice channel
#[derive(clap::Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    model_path: String,

    /// Channel ID to connect to
    #[arg(short, long)]
    channel_id: u64,
    /// Discord voice endpoint, hostname
    #[arg(short, long)]
    endpoint: String,
    /// Guild ID to connect to
    #[arg(short, long)]
    guild_id: u64,
    /// Discord voice session ID
    #[arg(short, long)]
    session_id: String,
    /// Discord user ID
    #[arg(short, long)]
    user_id: u64,
    /// Discord voice token (NOT bot token)
    #[arg(short, long)]
    voice_token: String,

    #[arg(long, default_value = None)]
    save_everything_to_file: Option<String>,
}

fn main() {
    let args = Cli::parse();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(tokio_main(args));

    // see https://github.com/tokio-rs/tokio/issues/2318
    // for why this is necessary.
    runtime.shutdown_timeout(Duration::from_secs(0));
}
