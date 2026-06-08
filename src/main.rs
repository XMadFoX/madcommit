use std::fs;

use git2::Repository;

mod config;
mod git;

use clap::Parser;
use config::{Cli, OutputFormat};

use genai::chat::printer::{print_chat_stream, PrintChatStreamOptions};
use genai::chat::{ChatMessage, ChatRequest};
use genai::Client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    log::debug!("Version: {}", env!("CARGO_PKG_VERSION"));

    let cli = Cli::parse();
    let app_config = config::load_config(&cli)?;

    let repo = Repository::open("./")?;

    let history = git::get_commit_history(&repo)?;
    log::info!("history: {history:?}");
    let diff_string = git::get_pretty_diff(&repo, 3)?;

    let template = fs::read_to_string(&app_config.template_path)?;

    let mut messages = vec![
        ChatMessage::system(template),
        ChatMessage::system(format!("Here's summary of last commits for context:\n{}", history.join("\n"))),
        ChatMessage::user(&diff_string),
    ];

    if let Some(message) = cli.message {
        messages.push(ChatMessage::user(message));
    }

    let chat_req = ChatRequest::new(messages);

    let client = Client::default();

    let print_options = PrintChatStreamOptions::from_print_events(false);

    let adapter_kind = client
        .resolve_service_target(&app_config.model).await?
        .model
        .adapter_kind;

    log::debug!("Using {} ({})", &app_config.model, adapter_kind);

    log::debug!("Answer: (streaming)");
    let chat_res = client
        .exec_chat_stream(&app_config.model, chat_req.clone(), None)
        .await?;
    let commit_msg = print_chat_stream(chat_res, Some(&print_options)).await?;
    log::debug!("Result:\n{commit_msg}");

    match app_config.output_format {
        OutputFormat::Plain => {
            // already streamed
        }
        OutputFormat::GitInteractiveCommit => {
            // Get the commit message interactively and create the commit
            let commit_message = git::get_commit_message_interactively(&commit_msg)?;
            git::create_commit(&repo, &commit_message)?;
        }
    }

    Ok(())
}
