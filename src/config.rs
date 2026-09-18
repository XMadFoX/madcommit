use std::path::{Path, PathBuf};

use ::config::{Config, File};
use clap::{Parser, Subcommand, ValueEnum};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone, ValueEnum)]
pub enum OutputFormat {
    Plain,
    GitInteractiveCommit,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppConfig {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template_path: Option<String>,
    pub output_format: OutputFormat,
    pub endpoint: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            model: "gpt-4.1-nano".to_string(),
            template_path: None,
            output_format: OutputFormat::GitInteractiveCommit,
            endpoint: "https://api.openai.com/v1/".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateSource {
    Cli(PathBuf),
    Config { path: PathBuf, config_file: PathBuf },
    Auto,
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub app_config: AppConfig,
    pub template_source: TemplateSource,
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[arg(
        short,
        long,
        value_name = "FILE",
        help = "Use a custom template. Relative paths are resolved from the current directory."
    )]
    pub template: Option<String>,

    #[arg(long)]
    pub model: Option<String>,

    #[arg(long, help = "OpenAI-compatible API endpoint", global = true)]
    pub endpoint: Option<String>,

    #[arg(short, long, value_enum)]
    pub output: Option<OutputFormat>,

    #[arg(
        short,
        long,
        help = "Custom context to help AI generate a better commit message"
    )]
    pub message: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Manage stored API credentials
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum AuthAction {
    /// Validate an API key and store it in the system keyring
    Login,
    /// Show whether a key is stored for the current endpoint (never prints the key)
    Status,
    /// Delete the stored key for the current endpoint
    Logout,
}

pub fn load_config(cli: &Cli) -> Result<LoadedConfig, Box<dyn std::error::Error>> {
    let project_dirs = ProjectDirs::from("dev", "xmadfox", "madcommit")
        .ok_or("Could not determine project directories")?;
    let config_dir = project_dirs.config_dir();
    let config_file_path = config_dir.join("config.toml");

    if !config_file_path.exists() {
        std::fs::create_dir_all(config_dir)?;
        let toml_string = toml::to_string_pretty(&AppConfig::default())?;
        std::fs::write(&config_file_path, toml_string)?;
        log::info!("Created default config file at: {:?}", config_file_path);
    }

    let settings = Config::builder()
        .add_source(Config::try_from(&AppConfig::default())?)
        .add_source(File::from(config_file_path.clone()).required(false))
        .build()?;

    let mut app_config: AppConfig = settings.try_deserialize()?;
    let template_source = template_source(
        cli.template.as_deref(),
        app_config.template_path.as_deref(),
        &config_file_path,
        &std::env::current_dir()?,
    );

    if let Some(model) = &cli.model {
        app_config.model = model.clone();
    }
    if let Some(endpoint) = &cli.endpoint {
        app_config.endpoint = endpoint.clone();
    }
    if let Some(output_format) = &cli.output {
        app_config.output_format = output_format.clone();
    }

    Ok(LoadedConfig {
        app_config,
        template_source,
    })
}

fn template_source(
    cli_path: Option<&str>,
    config_path: Option<&str>,
    config_file_path: &Path,
    current_dir: &Path,
) -> TemplateSource {
    if let Some(path) = cli_path {
        return TemplateSource::Cli(resolve_path(path, current_dir));
    }

    if let Some(path) = config_path {
        let config_dir = config_file_path.parent().unwrap_or_else(|| Path::new("."));
        return TemplateSource::Config {
            path: resolve_path(path, config_dir),
            config_file: config_file_path.to_path_buf(),
        };
    }

    TemplateSource::Auto
}

fn resolve_path(path: &str, base_dir: &Path) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn default_config_has_no_template_path() {
        let serialized = toml::to_string_pretty(&AppConfig::default()).unwrap();
        assert!(!serialized.contains("template_path"));
    }

    #[test]
    fn template_paths_keep_their_source_base_directory() {
        let temp = tempdir().unwrap();
        let cwd = temp.path().join("cwd");
        let config_dir = temp.path().join("config");
        let config_file = config_dir.join("config.toml");

        assert_eq!(
            template_source(Some("cli.md"), Some("config.md"), &config_file, &cwd),
            TemplateSource::Cli(cwd.join("cli.md"))
        );
        assert_eq!(
            template_source(None, Some("config.md"), &config_file, &cwd),
            TemplateSource::Config {
                path: config_dir.join("config.md"),
                config_file: config_file.clone(),
            }
        );
        assert_eq!(
            template_source(None, None, &config_file, &cwd),
            TemplateSource::Auto
        );
    }

    #[test]
    fn absolute_template_paths_are_unchanged() {
        let temp = tempdir().unwrap();
        let absolute = temp.path().join("template.md");
        let config_file = temp.path().join("config/config.toml");

        assert_eq!(
            template_source(
                Some(absolute.to_str().unwrap()),
                None,
                &config_file,
                temp.path()
            ),
            TemplateSource::Cli(absolute)
        );
    }
}
