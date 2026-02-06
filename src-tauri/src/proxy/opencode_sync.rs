use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;
use std::fs;
use std::collections::HashMap;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const OPENCODE_DIR: &str = ".config/opencode";
const OPENCODE_CONFIG_FILE: &str = "opencode.json";
const ANTIGRAVITY_CONFIG_FILE: &str = "antigravity.json";
const ANTIGRAVITY_ACCOUNTS_FILE: &str = "antigravity-accounts.json";
const BACKUP_SUFFIX: &str = ".antigravity.bak";

const ANTHROPIC_MODELS: &[&str] = &[
    "claude-sonnet-4-5",
    "claude-sonnet-4-5-thinking",
    "claude-opus-4-5-thinking",
];

const GOOGLE_MODELS: &[&str] = &[
    "gemini-3-pro-high",
    "gemini-3-pro-low",
    "gemini-3-flash",
    "gemini-3-pro-image",
    "gemini-2.5-flash",
    "gemini-2.5-flash-lite",
    "gemini-2.5-flash-thinking",
    "gemini-2.5-pro",
];

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OpencodeStatus {
    pub installed: bool,
    pub version: Option<String>,
    pub is_synced: bool,
    pub has_backup: bool,
    pub current_base_url: Option<String>,
    pub files: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct OpencodeAccount {
    email: String,
    #[serde(rename = "refreshToken")]
    refresh_token: String,
    #[serde(rename = "projectId", skip_serializing_if = "Option::is_none")]
    project_id: Option<String>,
    #[serde(rename = "rateLimitResetTimes", skip_serializing_if = "Option::is_none")]
    rate_limit_reset_times: Option<HashMap<String, i64>>,
}

fn get_opencode_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(OPENCODE_DIR))
}

fn get_config_paths() -> Option<(PathBuf, PathBuf, PathBuf)> {
    get_opencode_dir().map(|dir| {
        (
            dir.join(OPENCODE_CONFIG_FILE),
            dir.join(ANTIGRAVITY_CONFIG_FILE),
            dir.join(ANTIGRAVITY_ACCOUNTS_FILE),
        )
    })
}

pub fn check_opencode_installed() -> (bool, Option<String>) {
    let mut cmd = Command::new("opencode");
    cmd.arg("--version");
    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = match cmd.output() {
        Ok(o) => o,
        Err(_) => return (false, None),
    };

    if !output.status.success() {
        return (false, None);
    }

    let version = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_string();

    let cleaned = version
        .split(|c: char| !c.is_numeric() && c != '.')
        .filter(|p| !p.is_empty())
        .last()
        .map(|p| p.trim().to_string())
        .unwrap_or(version);

    (true, Some(cleaned))
}

fn get_provider_options<'a>(value: &'a Value, provider_name: &str) -> Option<&'a Value> {
    value.get("provider")
        .and_then(|p| p.get(provider_name))
        .and_then(|prov| prov.get("options"))
}

pub fn get_sync_status(proxy_url: &str) -> (bool, bool, Option<String>) {
    let Some((config_path, _, _)) = get_config_paths() else {
        return (false, false, None);
    };

    let mut is_synced = true;
    let mut has_backup = false;
    let mut current_base_url = None;

    let backup_path = config_path.with_file_name(
        format!("{}{}", OPENCODE_CONFIG_FILE, BACKUP_SUFFIX)
    );
    if backup_path.exists() {
        has_backup = true;
    }

    if !config_path.exists() {
        return (false, has_backup, None);
    }

    let content = match fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return (false, has_backup, None),
    };

    let json: Value = serde_json::from_str(&content).unwrap_or_default();

    let normalized_proxy = proxy_url.trim_end_matches('/');

    let anthropic_opts = get_provider_options(&json, "anthropic");
    let anthropic_url = anthropic_opts
        .and_then(|o| o.get("baseURL"))
        .and_then(|v| v.as_str());
    let anthropic_key = anthropic_opts
        .and_then(|o| o.get("apiKey"))
        .and_then(|v| v.as_str());

    let google_opts = get_provider_options(&json, "google");
    let google_url = google_opts
        .and_then(|o| o.get("baseURL"))
        .and_then(|v| v.as_str());
    let google_key = google_opts
        .and_then(|o| o.get("apiKey"))
        .and_then(|v| v.as_str());

    if let (Some(url), Some(_key)) = (anthropic_url, anthropic_key) {
        current_base_url = Some(url.to_string());
        if url.trim_end_matches('/') != normalized_proxy {
            is_synced = false;
        }
    } else {
        is_synced = false;
    }

    if let (Some(url), Some(_key)) = (google_url, google_key) {
        if url.trim_end_matches('/') != normalized_proxy {
            is_synced = false;
        }
    } else {
        is_synced = false;
    }

    (is_synced, has_backup, current_base_url)
}

fn create_backup(path: &PathBuf) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }

    let backup_path = path.with_file_name(format!(
        "{}{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        BACKUP_SUFFIX
    ));

    if backup_path.exists() {
        return Ok(());
    }

    fs::copy(path, &backup_path)
        .map_err(|e| format!("Failed to create backup: {}", e))?;

    Ok(())
}

fn ensure_object(value: &mut Value, key: &str) {
    let needs_reset = match value.get(key) {
        None => true,
        Some(v) if !v.is_object() => true,
        _ => false,
    };
    if needs_reset {
        value[key] = serde_json::json!({});
    }
}

fn ensure_provider_object(provider: &mut serde_json::Map<String, Value>, name: &str) {
    let needs_reset = match provider.get(name) {
        None => true,
        Some(v) if !v.is_object() => true,
        _ => false,
    };
    if needs_reset {
        provider.insert(name.to_string(), serde_json::json!({}));
    }
}

fn merge_provider_options(provider: &mut Value, base_url: &str, api_key: &str) {
    if provider.get("options").is_none() {
        provider["options"] = serde_json::json!({});
    }
    
    if let Some(options) = provider.get_mut("options").and_then(|o| o.as_object_mut()) {
        options.insert("baseURL".to_string(), Value::String(base_url.to_string()));
        options.insert("apiKey".to_string(), Value::String(api_key.to_string()));
    }
}

fn add_missing_models(provider: &mut Value, model_ids: &[&str]) {
    if provider.get("models").is_none() {
        provider["models"] = serde_json::json!({});
    }
    
    if let Some(models) = provider.get_mut("models").and_then(|m| m.as_object_mut()) {
        for &model_id in model_ids {
            if !models.contains_key(model_id) {
                models.insert(model_id.to_string(), serde_json::json!({ "name": model_id }));
            }
        }
    }
}

pub fn sync_opencode_config(
    proxy_url: &str,
    api_key: &str,
    sync_accounts: bool,
) -> Result<(), String> {
    let Some((config_path, _ag_config_path, ag_accounts_path)) = get_config_paths() else {
        return Err("Failed to get OpenCode config directory".to_string());
    };

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {}", e))?;
    }

    create_backup(&config_path)?;

    let mut config: Value = if config_path.exists() {
        fs::read_to_string(&config_path)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if !config.is_object() {
        config = serde_json::json!({});
    }

    if config.get("$schema").is_none() {
        config["$schema"] = Value::String("https://opencode.ai/config.json".to_string());
    }

    let normalized_url = proxy_url.trim_end_matches('/').to_string();

    ensure_object(&mut config, "provider");

    if let Some(provider) = config.get_mut("provider").and_then(|p| p.as_object_mut()) {
        ensure_provider_object(provider, "anthropic");
        if let Some(anthropic) = provider.get_mut("anthropic") {
            merge_provider_options(anthropic, &normalized_url, api_key);
            add_missing_models(anthropic, ANTHROPIC_MODELS);
        }

        ensure_provider_object(provider, "google");
        if let Some(google) = provider.get_mut("google") {
            merge_provider_options(google, &normalized_url, api_key);
            add_missing_models(google, GOOGLE_MODELS);
        }
    }

    let tmp_path = config_path.with_extension("tmp");
    fs::write(&tmp_path, serde_json::to_string_pretty(&config).unwrap())
        .map_err(|e| format!("Failed to write temp file: {}", e))?;
    fs::rename(&tmp_path, &config_path)
        .map_err(|e| format!("Failed to rename config file: {}", e))?;

    if sync_accounts {
        sync_accounts_file(&ag_accounts_path)?;
    }

    Ok(())
}

fn sync_accounts_file(accounts_path: &PathBuf) -> Result<(), String> {
    create_backup(accounts_path)?;

    let existing_content = if accounts_path.exists() {
        fs::read_to_string(accounts_path).ok()
    } else {
        None
    };

    let mut existing_rate_limits_by_email: HashMap<String, HashMap<String, i64>> = HashMap::new();
    
    if let Some(ref content) = existing_content {
        if let Ok(existing_json) = serde_json::from_str::<Value>(content) {
            if let Some(existing_accounts) = existing_json.get("accounts").and_then(|a| a.as_array()) {
                for acc in existing_accounts {
                    if let (Some(email), Some(rlt)) = (
                        acc.get("email").and_then(|e| e.as_str()),
                        acc.get("rateLimitResetTimes").and_then(|r| r.as_object())
                    ) {
                        let mut limits = HashMap::new();
                        for (key, val) in rlt.iter() {
                            if let Some(ts) = val.as_i64() {
                                limits.insert(key.clone(), ts);
                            }
                        }
                        if !limits.is_empty() {
                            existing_rate_limits_by_email.insert(email.to_string(), limits);
                        }
                    }
                }
            }
        }
    }

    let app_accounts = crate::modules::account::list_accounts()
        .map_err(|e| format!("Failed to list accounts: {}", e))?;

    let mut new_accounts: Vec<OpencodeAccount> = Vec::new();

    for acc in app_accounts {
        if acc.disabled || acc.proxy_disabled {
            continue;
        }

        let refresh_token = acc.token.refresh_token.clone();
        let project_id = acc.token.project_id.clone();
        
        let rate_limit_reset_times = existing_rate_limits_by_email
            .get(&acc.email)
            .cloned()
            .filter(|m| !m.is_empty());

        new_accounts.push(OpencodeAccount {
            email: acc.email,
            refresh_token,
            project_id,
            rate_limit_reset_times,
        });
    }

    let new_data = serde_json::json!({
        "accounts": new_accounts
    });

    let tmp_path = accounts_path.with_extension("tmp");
    fs::write(&tmp_path, serde_json::to_string_pretty(&new_data).unwrap())
        .map_err(|e| format!("Failed to write accounts temp file: {}", e))?;
    fs::rename(&tmp_path, accounts_path)
        .map_err(|e| format!("Failed to rename accounts file: {}", e))?;

    Ok(())
}

pub fn restore_opencode_config() -> Result<(), String> {
    let Some((config_path, _, accounts_path)) = get_config_paths() else {
        return Err("Failed to get OpenCode config directory".to_string());
    };

    let mut restored = false;

    let config_backup = config_path.with_file_name(format!(
        "{}{}", OPENCODE_CONFIG_FILE, BACKUP_SUFFIX
    ));
    if config_backup.exists() {
        fs::rename(&config_backup, &config_path)
            .map_err(|e| format!("Failed to restore config: {}", e))?;
        restored = true;
    }

    let accounts_backup = accounts_path.with_file_name(format!(
        "{}{}", ANTIGRAVITY_ACCOUNTS_FILE, BACKUP_SUFFIX
    ));
    if accounts_backup.exists() {
        fs::rename(&accounts_backup, &accounts_path)
            .map_err(|e| format!("Failed to restore accounts: {}", e))?;
        restored = true;
    }

    if restored {
        Ok(())
    } else {
        Err("No backup files found".to_string())
    }
}

pub fn read_opencode_config_content(file_name: Option<String>) -> Result<String, String> {
    let Some((opencode_path, ag_config_path, ag_accounts_path)) = get_config_paths() else {
        return Err("Failed to get OpenCode config directory".to_string());
    };

    // Allowlist of permitted file names
    let allowed_files = [
        OPENCODE_CONFIG_FILE,
        ANTIGRAVITY_CONFIG_FILE,
        ANTIGRAVITY_ACCOUNTS_FILE,
    ];

    // Determine which file to read
    let target_path = match file_name.as_deref() {
        Some(name) if name == ANTIGRAVITY_CONFIG_FILE => ag_config_path,
        Some(name) if name == ANTIGRAVITY_ACCOUNTS_FILE => ag_accounts_path,
        Some(name) if name == OPENCODE_CONFIG_FILE => opencode_path,
        Some(name) => {
            return Err(format!(
                "Invalid file name: {}. Allowed: {:?}",
                name, allowed_files
            ))
        }
        None => opencode_path, // Default to opencode.json
    };

    if !target_path.exists() {
        return Err(format!("Config file does not exist: {:?}", target_path));
    }

    fs::read_to_string(&target_path)
        .map_err(|e| format!("Failed to read config: {}", e))
}

#[tauri::command]
pub async fn get_opencode_sync_status(proxy_url: String) -> Result<OpencodeStatus, String> {
    let (installed, version) = check_opencode_installed();
    let (is_synced, has_backup, current_base_url) = if installed {
        get_sync_status(&proxy_url)
    } else {
        (false, false, None)
    };

    Ok(OpencodeStatus {
        installed,
        version,
        is_synced,
        has_backup,
        current_base_url,
        files: vec![
            OPENCODE_CONFIG_FILE.to_string(),
            ANTIGRAVITY_CONFIG_FILE.to_string(),
            ANTIGRAVITY_ACCOUNTS_FILE.to_string(),
        ],
    })
}

#[tauri::command]
pub async fn execute_opencode_sync(
    proxy_url: String,
    api_key: String,
    sync_accounts: Option<bool>,
) -> Result<(), String> {
    sync_opencode_config(&proxy_url, &api_key, sync_accounts.unwrap_or(false))
}

#[tauri::command]
pub async fn execute_opencode_restore() -> Result<(), String> {
    restore_opencode_config()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetOpencodeConfigRequest {
    pub file_name: Option<String>,
}

#[tauri::command]
pub async fn get_opencode_config_content(request: GetOpencodeConfigRequest) -> Result<String, String> {
    read_opencode_config_content(request.file_name)
}
