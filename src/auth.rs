use std::io::{self, IsTerminal, Write};
use std::time::Duration;

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::sync::Mutex;

use reqwest::header::AUTHORIZATION;
use reqwest::redirect::Policy;
use reqwest::StatusCode;

use crate::endpoint::normalize_endpoint;

const KEYRING_SERVICE: &str = "dev.xmadfox.madcommit";
const ENV_VAR: &str = "OPENAI_API_KEY";
const VALIDATE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSource {
    Env,
    Keyring,
}

pub struct ResolvedCredential {
    pub key: String,
    pub source: CredentialSource,
    pub endpoint: String,
}

impl std::fmt::Debug for ResolvedCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedCredential")
            .field("key", &"REDACTED")
            .field("source", &self.source)
            .field("endpoint", &self.endpoint)
            .finish()
    }
}

#[derive(Debug)]
pub enum AuthError {
    EmptyKey,
    MissingKey { endpoint: String },
    InvalidKey,
    EnvOverrideInvalid { endpoint: String },
    EnvOverrideEmpty,
    Keyring,
    KeyringUnavailable,
    RedirectRefused,
    ValidationFailed(String),
    Io(std::io::Error),
    NotInteractive,
    Declined,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::EmptyKey => write!(f, "API key cannot be empty."),
            AuthError::MissingKey { endpoint } => write!(
                f,
                "No API key found for {endpoint}.\n\
                 Set {ENV_VAR}, or run `madcommit auth login` in a terminal.\n\
                 On NixOS, a Secret Service provider such as GNOME Keyring, KWallet, or KeePassXC must be running with a D-Bus session."
            ),
            AuthError::InvalidKey => write!(
                f,
                "The API rejected this key. Check the key and endpoint, then try again."
            ),
            AuthError::EnvOverrideInvalid { endpoint } => write!(
                f,
                "{ENV_VAR} was rejected by {endpoint}.\n\
                 Change or unset {ENV_VAR}. Stored keyring credentials are not used while {ENV_VAR} is set."
            ),
            AuthError::EnvOverrideEmpty => write!(
                f,
                "{ENV_VAR} is set but empty. Change or unset it. Stored keyring credentials are not used while {ENV_VAR} is set."
            ),
            AuthError::Keyring => write!(
                f,
                "Could not read or write the system keyring credential."
            ),
            AuthError::KeyringUnavailable => write!(
                f,
                "Could not access the system keyring (Secret Service).\n\
                 On NixOS, start GNOME Keyring, KWallet, or KeePassXC, and ensure a D-Bus session is available.\n\
                 There is no plaintext file fallback."
            ),
            AuthError::RedirectRefused => write!(
                f,
                "The API endpoint responded with a redirect. Credentials were not sent to another host. Check --endpoint."
            ),
            AuthError::ValidationFailed(msg) => write!(f, "{msg}"),
            AuthError::Io(err) => write!(f, "{err}"),
            AuthError::NotInteractive => write!(
                f,
                "This action needs a terminal. Re-run `madcommit auth login` interactively, or set {ENV_VAR}."
            ),
            AuthError::Declined => write!(f, "API key replacement declined."),
        }
    }
}

impl std::error::Error for AuthError {}

impl From<std::io::Error> for AuthError {
    fn from(value: std::io::Error) -> Self {
        AuthError::Io(value)
    }
}

pub trait CredentialStore {
    fn get(&self, endpoint: &str) -> Result<Option<String>, AuthError>;
    fn set(&self, endpoint: &str, key: &str) -> Result<(), AuthError>;
    fn delete(&self, endpoint: &str) -> Result<bool, AuthError>;
}

pub struct KeyringStore;

impl CredentialStore for KeyringStore {
    fn get(&self, endpoint: &str) -> Result<Option<String>, AuthError> {
        let entry = keyring_entry(endpoint)?;
        match entry.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(map_keyring_error(err)),
        }
    }

    fn set(&self, endpoint: &str, key: &str) -> Result<(), AuthError> {
        keyring_entry(endpoint)?
            .set_password(key)
            .map_err(map_keyring_error)
    }

    fn delete(&self, endpoint: &str) -> Result<bool, AuthError> {
        let entry = keyring_entry(endpoint)?;
        match entry.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(err) => Err(map_keyring_error(err)),
        }
    }
}

fn keyring_entry(endpoint: &str) -> Result<keyring::Entry, AuthError> {
    keyring::Entry::new(KEYRING_SERVICE, endpoint).map_err(map_keyring_error)
}

fn map_keyring_error(err: keyring::Error) -> AuthError {
    match err {
        keyring::Error::NoStorageAccess(_) | keyring::Error::PlatformFailure(_) => {
            AuthError::KeyringUnavailable
        }
        _ => AuthError::Keyring,
    }
}

pub trait Prompter {
    fn is_interactive(&self) -> bool;
    fn read_secret(&self, prompt: &str) -> Result<String, AuthError>;
    fn confirm(&self, prompt: &str) -> Result<bool, AuthError>;
    fn message(&self, msg: &str);
}

pub struct StdPrompter;

impl Prompter for StdPrompter {
    fn is_interactive(&self) -> bool {
        io::stdin().is_terminal()
    }

    fn read_secret(&self, prompt: &str) -> Result<String, AuthError> {
        if !self.is_interactive() {
            return Err(AuthError::NotInteractive);
        }
        let value = rpassword::prompt_password(prompt)?;
        Ok(value)
    }

    fn confirm(&self, prompt: &str) -> Result<bool, AuthError> {
        if !self.is_interactive() {
            return Err(AuthError::NotInteractive);
        }
        eprint!("{prompt} [y/N] ");
        io::stderr().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        Ok(matches!(line.trim(), "y" | "Y" | "yes" | "Yes" | "YES"))
    }

    fn message(&self, msg: &str) {
        eprintln!("{msg}");
    }
}

pub trait KeyValidator {
    async fn validate(&self, endpoint: &str, api_key: &str) -> Result<(), AuthError>;
}

pub struct HttpKeyValidator {
    client: reqwest::Client,
}

fn no_redirect_http_client() -> Result<reqwest::Client, AuthError> {
    reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(VALIDATE_TIMEOUT)
        .build()
        .map_err(|err| AuthError::ValidationFailed(network_message(&err)))
}

impl HttpKeyValidator {
    pub fn new() -> Result<Self, AuthError> {
        Ok(Self {
            client: no_redirect_http_client()?,
        })
    }
}

pub fn build_chat_client(endpoint: String, api_key: String) -> Result<genai::Client, AuthError> {
    let http = no_redirect_http_client()?;
    let target_resolver = genai::resolver::ServiceTargetResolver::from_resolver_fn(
        move |service_target: genai::ServiceTarget| -> Result<genai::ServiceTarget, genai::resolver::Error> {
            let model = genai::ModelIden::new(
                genai::adapter::AdapterKind::OpenAI,
                service_target.model.model_name,
            );
            Ok(genai::ServiceTarget {
                model,
                endpoint: genai::resolver::Endpoint::from_owned(endpoint.clone()),
                auth: genai::resolver::AuthData::from_single(api_key.clone()),
            })
        },
    );
    Ok(genai::Client::builder()
        .with_reqwest(http)
        .with_service_target_resolver(target_resolver)
        .build())
}

impl KeyValidator for HttpKeyValidator {
    async fn validate(&self, endpoint: &str, api_key: &str) -> Result<(), AuthError> {
        validate_key_with_client(&self.client, endpoint, api_key).await
    }
}

async fn validate_key_with_client(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
) -> Result<(), AuthError> {
    let endpoint =
        normalize_endpoint(endpoint).map_err(|err| AuthError::ValidationFailed(err.to_string()))?;
    let url = format!("{endpoint}models");
    let response = client
        .get(&url)
        .header(AUTHORIZATION, format!("Bearer {api_key}"))
        .send()
        .await
        .map_err(|err| AuthError::ValidationFailed(network_message(&err)))?;

    let status = response.status();
    if status.is_redirection() {
        return Err(AuthError::RedirectRefused);
    }

    if status.is_success() {
        return Ok(());
    }

    if status == StatusCode::UNAUTHORIZED {
        return Err(AuthError::InvalidKey);
    }

    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(AuthError::ValidationFailed(
            "The API rate-limited key validation. Try again later.".to_string(),
        ));
    }

    Err(AuthError::ValidationFailed(format!(
        "Could not validate the API key (HTTP {}). Check the endpoint.",
        status.as_u16()
    )))
}

fn network_message(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        "Timed out contacting the API endpoint.".to_string()
    } else if err.is_connect() {
        "Could not connect to the API endpoint.".to_string()
    } else {
        "Network error contacting the API endpoint.".to_string()
    }
}

pub fn env_api_key() -> Option<String> {
    match std::env::var(ENV_VAR) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(_) => Some(String::new()),
    }
}

pub fn resolve_stored<S: CredentialStore>(
    store: &S,
    endpoint: &str,
    env_key: Option<String>,
) -> Result<Option<ResolvedCredential>, AuthError> {
    if let Some(value) = env_key {
        if value.is_empty() {
            return Err(AuthError::EnvOverrideEmpty);
        }
        return Ok(Some(ResolvedCredential {
            key: value,
            source: CredentialSource::Env,
            endpoint: endpoint.to_string(),
        }));
    }

    match store.get(endpoint)? {
        Some(key) if !key.is_empty() => Ok(Some(ResolvedCredential {
            key,
            source: CredentialSource::Keyring,
            endpoint: endpoint.to_string(),
        })),
        Some(_) | None => Ok(None),
    }
}

pub async fn obtain_api_key<S, P, V>(
    store: &S,
    prompter: &P,
    validator: &V,
    endpoint: &str,
) -> Result<ResolvedCredential, AuthError>
where
    S: CredentialStore,
    P: Prompter,
    V: KeyValidator,
{
    obtain_api_key_with_env(store, prompter, validator, endpoint, env_api_key()).await
}

async fn obtain_api_key_with_env<S, P, V>(
    store: &S,
    prompter: &P,
    validator: &V,
    endpoint: &str,
    env_key: Option<String>,
) -> Result<ResolvedCredential, AuthError>
where
    S: CredentialStore,
    P: Prompter,
    V: KeyValidator,
{
    let env_override_set = env_key.is_some();
    if let Some(creds) = resolve_stored(store, endpoint, env_key)? {
        return Ok(creds);
    }

    if !prompter.is_interactive() {
        return Err(AuthError::MissingKey {
            endpoint: endpoint.to_string(),
        });
    }

    prompter.message(&format!(
        "No API key found for {endpoint}.\nEnter a key to store in the system keyring after validation."
    ));
    prompt_validate_and_store(store, prompter, validator, endpoint, env_override_set).await
}

async fn prompt_validate_and_store<S, P, V>(
    store: &S,
    prompter: &P,
    validator: &V,
    endpoint: &str,
    env_override_set: bool,
) -> Result<ResolvedCredential, AuthError>
where
    S: CredentialStore,
    P: Prompter,
    V: KeyValidator,
{
    let key = prompter.read_secret("API key: ")?;
    if key.is_empty() {
        return Err(AuthError::EmptyKey);
    }
    validator.validate(endpoint, &key).await?;
    store.set(endpoint, &key)?;
    if env_override_set {
        prompter.message(&format!(
            "Stored a keyring credential for {endpoint}, but {ENV_VAR} is set and still takes precedence."
        ));
    } else {
        prompter.message(&format!(
            "Stored API key for {endpoint} in the system keyring."
        ));
    }
    Ok(ResolvedCredential {
        key,
        source: CredentialSource::Keyring,
        endpoint: endpoint.to_string(),
    })
}

pub async fn recover_invalid_key<S, P, V>(
    creds: &ResolvedCredential,
    store: &S,
    prompter: &P,
    validator: &V,
) -> Result<ResolvedCredential, AuthError>
where
    S: CredentialStore,
    P: Prompter,
    V: KeyValidator,
{
    if creds.source == CredentialSource::Env {
        return Err(AuthError::EnvOverrideInvalid {
            endpoint: creds.endpoint.clone(),
        });
    }
    if !prompter.is_interactive() {
        return Err(AuthError::ValidationFailed(format!(
            "The stored API key for {} was rejected. Run `madcommit auth login` or set {ENV_VAR}.",
            creds.endpoint
        )));
    }
    prompter.message(&format!(
        "The stored API key for {} was rejected.",
        creds.endpoint
    ));
    if !prompter.confirm("Replace the stored key and retry once?")? {
        return Err(AuthError::Declined);
    }
    prompt_validate_and_store(store, prompter, validator, &creds.endpoint, false).await
}

pub async fn cmd_login<S, P, V>(
    store: &S,
    prompter: &P,
    validator: &V,
    endpoint: &str,
) -> Result<(), AuthError>
where
    S: CredentialStore,
    P: Prompter,
    V: KeyValidator,
{
    if !prompter.is_interactive() {
        return Err(AuthError::NotInteractive);
    }
    prompt_validate_and_store(
        store,
        prompter,
        validator,
        endpoint,
        env_api_key().is_some(),
    )
    .await?;
    Ok(())
}

pub fn cmd_status<S: CredentialStore>(store: &S, endpoint: &str) -> Result<(), AuthError> {
    println!("Endpoint: {endpoint}");
    match env_api_key() {
        Some(value) if value.is_empty() => {
            println!("{ENV_VAR}: set but empty (override active; stored credentials are not used)");
        }
        Some(_) => {
            println!("{ENV_VAR}: set (override active; stored credentials are not used)");
        }
        None => println!("{ENV_VAR}: unset"),
    }
    match store.get(endpoint)? {
        Some(key) if !key.is_empty() => println!("Keyring: stored for this endpoint"),
        _ => println!("Keyring: no credential for this endpoint"),
    }
    Ok(())
}

pub fn cmd_logout<S: CredentialStore>(store: &S, endpoint: &str) -> Result<(), AuthError> {
    if store.delete(endpoint)? {
        eprintln!("Removed the keyring credential for {endpoint}.");
    } else {
        eprintln!("No keyring credential for {endpoint}.");
    }
    if env_api_key().is_some() {
        eprintln!("{ENV_VAR} is still set in this environment and is unchanged.");
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum ApiFailure {
    InvalidKey,
    RateLimited,
    Network,
    Other,
}

pub fn classify_genai_error(err: &genai::Error) -> ApiFailure {
    match err {
        genai::Error::WebAdapterCall { webc_error, .. }
        | genai::Error::WebModelCall { webc_error, .. } => classify_webc_error(webc_error),
        genai::Error::HttpError { status, body, .. } => classify_http(*status, body),
        genai::Error::Resolver { .. }
        | genai::Error::NoAuthData { .. }
        | genai::Error::RequiresApiKey { .. } => ApiFailure::InvalidKey,
        _ => ApiFailure::Other,
    }
}

fn classify_webc_error(err: &genai::webc::Error) -> ApiFailure {
    match err {
        genai::webc::Error::ResponseFailedStatus { status, body, .. } => {
            classify_http(*status, body)
        }
        genai::webc::Error::Reqwest(inner) => {
            if inner.is_timeout() || inner.is_connect() || inner.is_request() {
                ApiFailure::Network
            } else {
                ApiFailure::Other
            }
        }
        _ => ApiFailure::Other,
    }
}

fn classify_http(status: StatusCode, body: &str) -> ApiFailure {
    if status == StatusCode::TOO_MANY_REQUESTS {
        return ApiFailure::RateLimited;
    }
    if status == StatusCode::UNAUTHORIZED {
        return ApiFailure::InvalidKey;
    }
    if status == StatusCode::BAD_REQUEST && explicit_invalid_api_key_code(body) {
        return ApiFailure::InvalidKey;
    }
    ApiFailure::Other
}

fn explicit_invalid_api_key_code(body: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    value
        .get("error")
        .and_then(|err| err.get("code"))
        .and_then(|code| code.as_str())
        == Some("invalid_api_key")
}

pub fn user_message_for_api_failure(failure: &ApiFailure) -> String {
    match failure {
        ApiFailure::InvalidKey => "The API rejected the API key.".to_string(),
        ApiFailure::RateLimited => "The API rate-limited the request. Try again later.".to_string(),
        ApiFailure::Network => "Network error contacting the API endpoint.".to_string(),
        ApiFailure::Other => "The API request failed.".to_string(),
    }
}

#[cfg(test)]
#[derive(Default)]
struct MemoryStore {
    inner: Mutex<HashMap<String, String>>,
}

#[cfg(test)]
impl CredentialStore for MemoryStore {
    fn get(&self, endpoint: &str) -> Result<Option<String>, AuthError> {
        Ok(self
            .inner
            .lock()
            .expect("store lock")
            .get(endpoint)
            .cloned())
    }

    fn set(&self, endpoint: &str, key: &str) -> Result<(), AuthError> {
        self.inner
            .lock()
            .expect("store lock")
            .insert(endpoint.to_string(), key.to_string());
        Ok(())
    }

    fn delete(&self, endpoint: &str) -> Result<bool, AuthError> {
        Ok(self
            .inner
            .lock()
            .expect("store lock")
            .remove(endpoint)
            .is_some())
    }
}

#[cfg(test)]
pub struct MockPrompter {
    pub interactive: bool,
    secrets: Mutex<Vec<String>>,
    confirms: Mutex<Vec<bool>>,
    pub messages: Mutex<Vec<String>>,
}

#[cfg(test)]
impl MockPrompter {
    pub fn new(interactive: bool, secrets: Vec<String>, confirms: Vec<bool>) -> Self {
        Self {
            interactive,
            secrets: Mutex::new(secrets),
            confirms: Mutex::new(confirms),
            messages: Mutex::new(Vec::new()),
        }
    }
}

#[cfg(test)]
impl Prompter for MockPrompter {
    fn is_interactive(&self) -> bool {
        self.interactive
    }

    fn read_secret(&self, _prompt: &str) -> Result<String, AuthError> {
        if !self.interactive {
            return Err(AuthError::NotInteractive);
        }
        let mut secrets = self.secrets.lock().expect("secrets lock");
        if secrets.is_empty() {
            return Err(AuthError::NotInteractive);
        }
        Ok(secrets.remove(0))
    }

    fn confirm(&self, _prompt: &str) -> Result<bool, AuthError> {
        if !self.interactive {
            return Err(AuthError::NotInteractive);
        }
        let mut confirms = self.confirms.lock().expect("confirms lock");
        if confirms.is_empty() {
            return Ok(false);
        }
        Ok(confirms.remove(0))
    }

    fn message(&self, msg: &str) {
        self.messages
            .lock()
            .expect("messages lock")
            .push(msg.to_string());
    }
}

#[cfg(test)]
pub struct MockValidator {
    inner: Mutex<Vec<Result<(), AuthError>>>,
}

#[cfg(test)]
impl MockValidator {
    pub fn new(results: Vec<Result<(), AuthError>>) -> Self {
        Self {
            inner: Mutex::new(results),
        }
    }
}

#[cfg(test)]
impl KeyValidator for MockValidator {
    async fn validate(&self, _endpoint: &str, _api_key: &str) -> Result<(), AuthError> {
        let mut results = self.inner.lock().expect("validator lock");
        if results.is_empty() {
            return Ok(());
        }
        results.remove(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint::normalize_endpoint;

    const ENDPOINT: &str = "https://api.openai.com/v1/";

    #[test]
    fn env_overrides_keyring() {
        let store = MemoryStore::default();
        store.set(ENDPOINT, "stored-key").unwrap();
        let creds = resolve_stored(&store, ENDPOINT, Some("env-key".into()))
            .unwrap()
            .unwrap();
        assert_eq!(creds.source, CredentialSource::Env);
        assert_eq!(creds.key, "env-key");
    }

    #[test]
    fn empty_env_does_not_use_keyring() {
        let store = MemoryStore::default();
        store.set(ENDPOINT, "stored-key").unwrap();
        let err = resolve_stored(&store, ENDPOINT, Some(String::new())).unwrap_err();
        assert!(matches!(err, AuthError::EnvOverrideEmpty));
        assert!(!err.to_string().contains("stored-key"));
    }

    #[test]
    fn keyring_used_when_env_unset() {
        let store = MemoryStore::default();
        store.set(ENDPOINT, "stored-key").unwrap();
        let creds = resolve_stored(&store, ENDPOINT, None).unwrap().unwrap();
        assert_eq!(creds.source, CredentialSource::Keyring);
        assert_eq!(creds.key, "stored-key");
    }

    #[tokio::test]
    async fn missing_key_noninteractive_is_actionable() {
        let store = MemoryStore::default();
        let prompter = MockPrompter::new(false, vec![], vec![]);
        let validator = MockValidator::new(vec![]);
        let err = obtain_api_key_with_env(&store, &prompter, &validator, ENDPOINT, None)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("auth login"), "{msg}");
        assert!(msg.contains(ENV_VAR), "{msg}");
        assert!(msg.contains("NixOS"), "{msg}");
    }

    #[tokio::test]
    async fn interactive_setup_validates_before_save() {
        let store = MemoryStore::default();
        let prompter = MockPrompter::new(true, vec!["new-key".into()], vec![]);
        let validator = MockValidator::new(vec![Err(AuthError::InvalidKey)]);
        let err = obtain_api_key_with_env(&store, &prompter, &validator, ENDPOINT, None)
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::InvalidKey));
        assert!(store.get(ENDPOINT).unwrap().is_none());
    }

    #[tokio::test]
    async fn interactive_setup_saves_after_validation() {
        let store = MemoryStore::default();
        let prompter = MockPrompter::new(true, vec!["new-key".into()], vec![]);
        let validator = MockValidator::new(vec![Ok(())]);
        let creds = obtain_api_key_with_env(&store, &prompter, &validator, ENDPOINT, None)
            .await
            .unwrap();
        assert_eq!(creds.source, CredentialSource::Keyring);
        assert_eq!(store.get(ENDPOINT).unwrap().as_deref(), Some("new-key"));
    }

    #[tokio::test]
    async fn invalid_env_does_not_replace_stored() {
        let store = MemoryStore::default();
        store.set(ENDPOINT, "stored-key").unwrap();
        let creds = resolve_stored(&store, ENDPOINT, Some("bad-env".into()))
            .unwrap()
            .unwrap();
        let prompter = MockPrompter::new(true, vec!["replacement".into()], vec![true]);
        let validator = MockValidator::new(vec![Ok(())]);
        let err = recover_invalid_key(&creds, &store, &prompter, &validator)
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::EnvOverrideInvalid { .. }));
        assert!(err.to_string().contains("Change or unset"), "{err}");
        assert_eq!(store.get(ENDPOINT).unwrap().as_deref(), Some("stored-key"));
    }

    #[tokio::test]
    async fn invalid_keyring_replaces_with_consent() {
        let store = MemoryStore::default();
        store.set(ENDPOINT, "old-key").unwrap();
        let creds = resolve_stored(&store, ENDPOINT, None).unwrap().unwrap();
        let prompter = MockPrompter::new(true, vec!["new-key".into()], vec![true]);
        let validator = MockValidator::new(vec![Ok(())]);
        let updated = recover_invalid_key(&creds, &store, &prompter, &validator)
            .await
            .unwrap();
        assert_eq!(updated.key, "new-key");
        assert_eq!(store.get(ENDPOINT).unwrap().as_deref(), Some("new-key"));
    }

    #[test]
    fn logout_is_endpoint_scoped() {
        let store = MemoryStore::default();
        let a = normalize_endpoint("https://api.openai.com/v1").unwrap();
        let b = normalize_endpoint("https://example.com/v1").unwrap();
        store.set(&a, "key-a").unwrap();
        store.set(&b, "key-b").unwrap();
        assert!(store.delete(&a).unwrap());
        assert!(store.get(&a).unwrap().is_none());
        assert_eq!(store.get(&b).unwrap().as_deref(), Some("key-b"));
    }

    #[test]
    fn resolved_credential_debug_redacts_key() {
        let creds = ResolvedCredential {
            key: "dummy-secret".into(),
            source: CredentialSource::Keyring,
            endpoint: ENDPOINT.into(),
        };
        let rendered = format!("{creds:?}");
        assert!(!rendered.contains("dummy-secret"), "{rendered}");
        assert!(rendered.contains("REDACTED"), "{rendered}");
    }

    #[test]
    fn classify_401_as_invalid_key() {
        assert_eq!(
            classify_http(StatusCode::UNAUTHORIZED, ""),
            ApiFailure::InvalidKey
        );
        assert_eq!(
            classify_http(StatusCode::TOO_MANY_REQUESTS, ""),
            ApiFailure::RateLimited
        );
        assert_eq!(
            classify_http(
                StatusCode::BAD_REQUEST,
                r#"{"error":{"code":"invalid_api_key"}}"#
            ),
            ApiFailure::InvalidKey
        );
        assert_eq!(
            classify_http(
                StatusCode::TOO_MANY_REQUESTS,
                r#"{"error":{"code":"invalid_api_key","message":"invalid api key"}}"#
            ),
            ApiFailure::RateLimited
        );
        assert_eq!(
            classify_http(
                StatusCode::FORBIDDEN,
                r#"{"error":{"code":"invalid_api_key","message":"invalid api key"}}"#
            ),
            ApiFailure::Other
        );
        assert_eq!(
            classify_http(
                StatusCode::INTERNAL_SERVER_ERROR,
                "invalid api key dummy-secret"
            ),
            ApiFailure::Other
        );
        assert_eq!(
            classify_http(StatusCode::BAD_REQUEST, "invalid api key"),
            ApiFailure::Other
        );
        assert_eq!(
            user_message_for_api_failure(&ApiFailure::Other),
            "The API request failed."
        );
    }

    #[test]
    fn map_keyring_error_omits_backend_strings() {
        let secret = "dummy-secret-in-platform-error";
        let err = map_keyring_error(keyring::Error::PlatformFailure(Box::new(
            std::io::Error::other(secret),
        )));
        assert!(matches!(err, AuthError::KeyringUnavailable));
        assert!(!err.to_string().contains(secret));
        assert!(!format!("{err:?}").contains(secret));

        let err = map_keyring_error(keyring::Error::Invalid("password".into(), secret.into()));
        assert!(matches!(err, AuthError::Keyring));
        assert!(!err.to_string().contains(secret));
        assert!(!format!("{err:?}").contains(secret));
    }

    #[test]
    fn cli_display_keeps_auth_instructions() {
        let err: Box<dyn std::error::Error> = AuthError::MissingKey {
            endpoint: "https://api.openai.com/v1/".into(),
        }
        .into();
        let display = err.to_string();
        let debug = format!("{err:?}");
        assert!(display.contains("auth login"), "{display}");
        assert!(display.contains(ENV_VAR), "{display}");
        assert!(!display.contains("MissingKey"), "{display}");
        assert_ne!(display, debug);
    }

    #[tokio::test]
    async fn http_validator_accepts_200() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/models"))
            .and(wiremock::matchers::header(
                "authorization",
                "Bearer good-key",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "data": []
                })),
            )
            .mount(&server)
            .await;

        let endpoint = normalize_endpoint(&server.uri()).unwrap();
        let validator = HttpKeyValidator::new().unwrap();
        validator.validate(&endpoint, "good-key").await.unwrap();
    }

    #[tokio::test]
    async fn http_validator_maps_401() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/models"))
            .respond_with(wiremock::ResponseTemplate::new(401).set_body_string("invalid"))
            .mount(&server)
            .await;

        let endpoint = normalize_endpoint(&server.uri()).unwrap();
        let validator = HttpKeyValidator::new().unwrap();
        let err = validator.validate(&endpoint, "bad-key").await.unwrap_err();
        assert!(matches!(err, AuthError::InvalidKey));
        assert!(!err.to_string().contains("bad-key"));
    }

    #[tokio::test]
    async fn http_validator_refuses_redirect() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/models"))
            .respond_with(
                wiremock::ResponseTemplate::new(302)
                    .insert_header("Location", "https://evil.example/steal"),
            )
            .mount(&server)
            .await;

        let endpoint = normalize_endpoint(&server.uri()).unwrap();
        let validator = HttpKeyValidator::new().unwrap();
        let err = validator
            .validate(&endpoint, "secret-key")
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::RedirectRefused));
        assert!(!err.to_string().contains("secret-key"));
    }

    #[tokio::test]
    async fn generation_client_does_not_follow_redirects() {
        let server = wiremock::MockServer::start().await;
        let steal = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(302)
                    .insert_header("Location", format!("{}/steal", steal.uri())),
            )
            .mount(&server)
            .await;

        wiremock::Mock::given(wiremock::matchers::any())
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(0)
            .mount(&steal)
            .await;

        let endpoint = normalize_endpoint(&server.uri()).unwrap();
        let client = build_chat_client(endpoint, "secret-key".into()).unwrap();
        let req = genai::chat::ChatRequest::new(vec![genai::chat::ChatMessage::user("hi")]);
        let err = client
            .exec_chat("gpt-4.1-nano", req, None)
            .await
            .unwrap_err();
        let failure = classify_genai_error(&err);
        assert_ne!(failure, ApiFailure::InvalidKey);
        assert!(!format!("{err}").contains("secret-key"));
    }
}
