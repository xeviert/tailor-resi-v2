#[cfg(debug_assertions)]
use std::path::PathBuf;
#[cfg(not(debug_assertions))]
use std::fs;
use std::{path::Path, sync::OnceLock};
use tauri::App;
#[cfg(not(debug_assertions))]
use tauri::Manager;

const CREDENTIAL_SERVICE: &str = "com.resitailor.app";
const CREDENTIAL_ACCOUNT: &str = "openai-api-key";
static WORKSPACE_ROOT: OnceLock<std::path::PathBuf> = OnceLock::new();
static EXTENSION_DIRECTORY: OnceLock<std::path::PathBuf> = OnceLock::new();

pub fn initialize_runtime(_app: &App) -> Result<(), String> {
    #[cfg(debug_assertions)]
    let (workspace, extension) = {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or_else(|| "src-tauri must have a repository-root parent".to_string())?
            .to_path_buf();
        (repository.clone(), repository.join("browser-extension"))
    };

    #[cfg(not(debug_assertions))]
    let (workspace, extension) = {
        let workspace = _app
            .path()
            .app_local_data_dir()
            .map_err(|error| error.to_string())?
            .join("workspace");
        let resources = _app
            .path()
            .resource_dir()
            .map_err(|error| error.to_string())?;
        seed_release_workspace(&resources, &workspace)?;
        (workspace, resources.join("browser-extension"))
    };

    WORKSPACE_ROOT
        .set(workspace)
        .map_err(|_| "Runtime workspace was initialized twice".to_string())?;
    EXTENSION_DIRECTORY
        .set(extension)
        .map_err(|_| "Extension directory was initialized twice".to_string())?;
    Ok(())
}

#[cfg(not(debug_assertions))]
fn seed_release_workspace(resources: &Path, workspace: &Path) -> Result<(), String> {
    fs::create_dir_all(workspace).map_err(|error| error.to_string())?;
    for relative in ["resume/content", "resume/templates", "resume/scripts"] {
        copy_directory(&resources.join(relative), &workspace.join(relative), true)?;
    }
    let evidence_source = resources.join("resume/evidence-bank.json");
    let evidence_target = workspace.join("resume/evidence-bank.json");
    if !evidence_target.is_file() {
        copy_file(&evidence_source, &evidence_target)?;
    }
    for relative in [
        "resume/variants",
        "resume/generated",
        "resume/qa",
        "data/job-captures",
        "data/tailoring-results",
        "data/api-usage",
    ] {
        fs::create_dir_all(workspace.join(relative)).map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(not(debug_assertions))]
fn copy_directory(source: &Path, target: &Path, overwrite: bool) -> Result<(), String> {
    if !source.is_dir() {
        return Err(format!("Packaged resource is missing: {}", source.display()));
    }
    fs::create_dir_all(target).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let destination = target.join(entry.file_name());
        if entry.path().is_dir() {
            copy_directory(&entry.path(), &destination, overwrite)?;
        } else if overwrite || !destination.exists() {
            copy_file(&entry.path(), &destination)?;
        }
    }
    Ok(())
}

#[cfg(not(debug_assertions))]
fn copy_file(source: &Path, target: &Path) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::copy(source, target).map_err(|error| error.to_string())?;
    Ok(())
}

pub fn workspace_root() -> Option<std::path::PathBuf> {
    WORKSPACE_ROOT.get().cloned()
}

pub fn extension_directory() -> Option<std::path::PathBuf> {
    EXTENSION_DIRECTORY.get().cloned()
}

fn credential_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_ACCOUNT).map_err(|error| error.to_string())
}

pub fn stored_openai_api_key() -> Option<String> {
    credential_entry()
        .ok()?
        .get_password()
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn resolved_openai_api_key() -> Option<String> {
    std::env::var("OPENAI_API_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(stored_openai_api_key)
}

pub fn save_openai_api_key(value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("Enter a non-empty OpenAI API key.".to_string());
    }
    credential_entry()?
        .set_password(trimmed)
        .map_err(|error| error.to_string())
}

pub fn delete_openai_api_key() -> Result<(), String> {
    match credential_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

pub fn libreoffice_path() -> Option<std::path::PathBuf> {
    let known = Path::new(r"C:\Program Files\LibreOffice\program\soffice.com");
    if known.is_file() {
        return Some(known.to_path_buf());
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .flat_map(|directory| [directory.join("soffice.com"), directory.join("soffice.exe")])
            .find(|candidate| candidate.is_file())
    })
}

/// Loads developer-only configuration from the repository root.
///
/// `dotenvy` preserves variables already supplied by the process, so CI and
/// explicit shell configuration take precedence over local development values.
#[cfg(debug_assertions)]
pub fn load_development_env() {
    let path = development_env_path();
    if !path.is_file() {
        return;
    }

    if let Err(error) = load_env_file(&path) {
        eprintln!("[config] Could not load {}: {error}", path.display());
    }
}

#[cfg(not(debug_assertions))]
pub fn load_development_env() {}

#[cfg(debug_assertions)]
fn development_env_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri must have a repository-root parent")
        .join(".env")
}

#[cfg(debug_assertions)]
fn load_env_file(path: &std::path::Path) -> Result<(), dotenvy::Error> {
    dotenvy::from_path(path)
}

#[cfg(test)]
mod tests {
    #[cfg(debug_assertions)]
    use super::{development_env_path, load_env_file};
    #[cfg(debug_assertions)]
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[cfg(debug_assertions)]
    #[test]
    fn development_env_is_at_the_repository_root() {
        let path = development_env_path();
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(".env")
        );
        assert!(path.parent().unwrap().join("src-tauri").is_dir());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn env_file_fills_missing_values_without_overriding_process_values() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let key = format!("RESI_TAILOR_ENV_TEST_{suffix}");
        let path = std::env::temp_dir().join(format!("resi-tailor-{suffix}.env"));

        fs::write(&path, format!("{key}=from-file\n")).unwrap();
        load_env_file(&path).unwrap();
        assert_eq!(std::env::var(&key).as_deref(), Ok("from-file"));

        std::env::set_var(&key, "from-process");
        load_env_file(&path).unwrap();
        assert_eq!(std::env::var(&key).as_deref(), Ok("from-process"));

        std::env::remove_var(&key);
        fs::remove_file(path).unwrap();
    }
}
