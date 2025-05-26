use ::config::{Config, File};
use clap::Parser;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppConfig {
    pub model: String,
    pub template_path: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            model: "gpt-4.1-nano".to_string(),
            template_path: "template.md".to_string(),
        }
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[arg(short, long, value_name = "FILE")]
    pub template: Option<String>,

    #[arg(short, long)]
    pub model: Option<String>,
}

pub fn load_config() -> Result<AppConfig, Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Determine config file path
    let project_dirs = ProjectDirs::from("dev", "xmadfox", "madcommit")
        .ok_or("Could not determine project directories")?;
    let config_dir = project_dirs.config_dir();
    let config_file_path = config_dir.join("config.toml");

    // Create default config file if it doesn't exist
    if !config_file_path.exists() {
        std::fs::create_dir_all(config_dir)?; // Ensure directory exists
        let default_config = AppConfig::default();
        let toml_string = toml::to_string_pretty(&default_config)?;
        std::fs::write(&config_file_path, toml_string)?;
        log::info!("Created default config file at: {:?}", config_file_path);
    }

    // Build configuration
    let settings = Config::builder()
        .add_source(Config::try_from(&AppConfig::default())?) // 1. Defaults
        .add_source(File::from(config_file_path.clone()).required(false)) // 2. Config file (optional)
        .build()?;

    let mut app_config: AppConfig = settings.try_deserialize()?;

    // 4. Override with CLI arguments
    if let Some(template_path) = cli.template {
        app_config.template_path = template_path;
    }
    if let Some(model) = cli.model {
        app_config.model = model;
    }

    Ok(app_config)
}

