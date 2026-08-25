use std::fs;

use genai::adapter::AdapterKind;
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use git2::Repository;

mod config;
mod git;

use clap::Parser;
use config::{Cli, OutputFormat};

use genai::chat::{ChatMessage, ChatRequest};
use genai::{Client, ModelIden, ServiceTarget};

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
        ChatMessage::system(format!(
            "Here's summary of last commits for context:\n{}",
            history.join("\n")
        )),
        ChatMessage::user(&diff_string),
    ];

    if let Some(message) = cli.message {
        messages.push(ChatMessage::user(message));
    }

    log::debug!("Messages: {:?}", messages.clone());
    let chat_req = ChatRequest::new(messages);

    let endpoint = app_config.endpoint.clone();
    let target_resolver = ServiceTargetResolver::from_resolver_fn(
        move |service_target: ServiceTarget| -> Result<ServiceTarget, genai::resolver::Error> {
            let model = ModelIden::new(AdapterKind::OpenAI, service_target.model.model_name);
            Ok(ServiceTarget {
                model,
                endpoint: Endpoint::from_owned(endpoint.clone()),
                auth: AuthData::from_env("OPENAI_API_KEY"),
            })
        },
    );
    let client = Client::builder()
        .with_service_target_resolver(target_resolver)
        .build();

    log::debug!(
        "Using model {} at {}",
        app_config.model,
        app_config.endpoint
    );
    let chat_res = client.exec_chat(&app_config.model, chat_req, None).await?;
    let commit_msg = chat_res
        .first_text()
        .ok_or("The API returned no text response")?;
    log::debug!("Result:\n{commit_msg}");
    match app_config.output_format {
        OutputFormat::Plain => {
            // already streamed
        }
        OutputFormat::GitInteractiveCommit => {
            // Get the commit message interactively and create the commit
            let commit_message = git::get_commit_message_interactively(commit_msg)?;
            git::create_commit(&repo, &commit_message)?;
        }
    }

    Ok(())
}
