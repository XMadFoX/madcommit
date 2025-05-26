use std::fs;

use git2::Repository;

mod config;
mod git;

use config::OutputFormat;

use genai::chat::printer::{print_chat_stream, PrintChatStreamOptions};
use genai::chat::{ChatMessage, ChatRequest};
use genai::Client;
use simple_logger::SimpleLogger;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    SimpleLogger::new().env().init().unwrap();

    let app_config = config::load_config()?;

    let repo = Repository::open("./")?;

    let diff_string = git::get_pretty_diff(&repo, 3)?;

    let template = fs::read_to_string(&app_config.template_path)?;

    let chat_req = ChatRequest::new(vec![
        ChatMessage::system(template),
        ChatMessage::user(&diff_string),
    ]);

    let client = Client::default();

    let print_options = PrintChatStreamOptions::from_print_events(false);

    let adapter_kind = client
        .resolve_service_target(&app_config.model)?
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
