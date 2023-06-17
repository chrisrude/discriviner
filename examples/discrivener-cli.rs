use clap::Parser;
use colored::Colorize;
use discrivener::model::types::{TranscribedMessage, VoiceChannelEvent};
use discrivener::Discrivener;
use std::sync::Arc;
use tokio::signal;

fn on_text(message: TranscribedMessage, log_performance: bool) {
    if message.segments.is_empty() {
        println!();
        return;
    }
    if log_performance {
        println!(
            "Processing time: +{}ms",
            message.processing_time.as_millis()
        );
        println!("Audio duration: {}ms", message.audio_duration.as_millis());
        println!(
            "total time / Audio duration: {}",
            message.processing_time.as_millis() as f32 / message.audio_duration.as_millis() as f32
        );
    }

    let mut first: bool = true;
    for segment in message.segments {
        // todo: color code based on probability
        if first {
            println!(
                "{} says: {}",
                message.user_id.to_string().bright_green(),
                segment.text().bold(),
            );
            first = false;
        } else {
            println!("\t\t\t {}", segment.text().bold());
        }
    }
}

#[tokio::main]
async fn tokio_main(cli: Cli) {
    let log_performance = cli.log_performance;
    let mut discrivener = Discrivener::load(
        cli.model_path,
        Arc::new(move |event| match event {
            VoiceChannelEvent::TranscribedMessage(message) => on_text(message, log_performance),
            VoiceChannelEvent::Connect(status) => {
                println!(
                    "Connection status: {} to channel #{}",
                    "connected".bright_green(),
                    if let Some(channel_id) = status.channel_id {
                        channel_id.to_string().bright_green()
                    } else {
                        "unknown".bright_red()
                    }
                )
            }
            VoiceChannelEvent::UserJoin(user_data) => {
                println!(
                    "User {} {}",
                    user_data.user_id,
                    if user_data.joined {
                        "joined".bright_green()
                    } else {
                        "left".bright_purple()
                    }
                )
            }
            VoiceChannelEvent::Reconnect(status) => {
                println!(
                    "Connection status: {} to channel #{}",
                    "reconnected".bright_green(),
                    if let Some(channel_id) = status.channel_id {
                        channel_id.to_string().bright_green()
                    } else {
                        "unknown".bright_red()
                    }
                )
            }
            VoiceChannelEvent::Disconnect(_) => {
                println!("Connection status: {}", "disconnected".bright_red());
            }
            VoiceChannelEvent::SilentChannel(silent) => {
                if silent {
                    println!("Channel is silent");
                } else {
                    println!("Someone is talking");
                }
            }
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
    if connection_result.is_ok() {
        println!("Joined voice channel");
    } else {
        println!("Error joining voice channel");
    }

    signal::ctrl_c().await.unwrap();
    discrivener.disconnect().await;
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

    #[arg(short, long, default_value = "false")]
    log_performance: bool,

    #[arg(long, default_value = None)]
    save_everything_to_file: Option<String>,
}

fn main() {
    let args = Cli::parse();
    tokio_main(args);
}
