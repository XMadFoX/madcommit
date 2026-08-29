use std::fs;
use std::process::ExitCode;

use git2::Repository;

mod auth;
mod config;
mod endpoint;
mod git;

use clap::Parser;
use config::{AuthAction, Cli, Command, OutputFormat};

use auth::{
    build_chat_client, classify_genai_error, cmd_login, cmd_logout, cmd_status, obtain_api_key,
    recover_invalid_key, user_message_for_api_failure, ApiFailure, HttpKeyValidator, KeyringStore,
    StdPrompter,
};
use endpoint::normalize_endpoint;
use genai::chat::{ChatMessage, ChatRequest};

#[tokio::main]
async fn main() -> ExitCode {
    env_logger::init();
    log::debug!("Version: {}", env!("CARGO_PKG_VERSION"));
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let app_config = config::load_config(&cli)?;
    let endpoint = normalize_endpoint(&app_config.endpoint)?;
    let store = KeyringStore;
    let prompter = StdPrompter;
    let validator = HttpKeyValidator::new()?;

    if let Some(Command::Auth { action }) = cli.command {
        match action {
            AuthAction::Login => cmd_login(&store, &prompter, &validator, &endpoint).await?,
            AuthAction::Status => cmd_status(&store, &endpoint)?,
            AuthAction::Logout => cmd_logout(&store, &endpoint)?,
        }
        return Ok(());
    }

    let repo = Repository::open("./")?;

    let history = git::get_commit_history(&repo)?;
    log::info!("history: {history:?}");
    let diff_string = git::get_pretty_diff(&repo, 3)?;

    let template = fs::read_to_string(&app_config.template_path)?;

    let mut messages = vec![
        ChatMessage::system(template),
        ChatMessage::system(format!(
            "Here's summary of last commits for context:\n{}",
            history.join("\n")
        )),
        ChatMessage::user(&diff_string),
    ];

    if let Some(message) = cli.message {
        messages.push(ChatMessage::user(message));
    }

    log::debug!("Prepared {} chat messages", messages.len());
    let chat_req = ChatRequest::new(messages);

    log::debug!("Using model {} at {}", app_config.model, endpoint);

    let commit_msg = generate_commit_message(
        &store,
        &prompter,
        &validator,
        &endpoint,
        &app_config.model,
        chat_req,
    )
    .await?;
    log::debug!("Received commit message ({} bytes)", commit_msg.len());
    match app_config.output_format {
        OutputFormat::Plain => {
            // already streamed
        }
        OutputFormat::GitInteractiveCommit => {
            let commit_message = git::get_commit_message_interactively(&commit_msg)?;
            git::create_commit(&repo, &commit_message)?;
        }
    }

    Ok(())
}

async fn generate_commit_message(
    store: &KeyringStore,
    prompter: &StdPrompter,
    validator: &HttpKeyValidator,
    endpoint: &str,
    model: &str,
    chat_req: ChatRequest,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut creds = obtain_api_key(store, prompter, validator, endpoint).await?;
    let mut retried = false;

    loop {
        let client = build_chat_client(creds.endpoint.clone(), creds.key.clone())?;
        match client.exec_chat(model, chat_req.clone(), None).await {
            Ok(chat_res) => {
                return chat_res
                    .first_text()
                    .map(str::to_string)
                    .ok_or_else(|| "The API returned no text response".into());
            }
            Err(err) => {
                let failure = classify_genai_error(&err);
                log::debug!("API call failed: {failure:?}");
                if failure == ApiFailure::InvalidKey && !retried {
                    creds = recover_invalid_key(&creds, store, prompter, validator).await?;
                    retried = true;
                    continue;
                }
                return Err(user_message_for_api_failure(&failure).into());
            }
        }
    }
}
