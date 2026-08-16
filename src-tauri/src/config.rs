#[cfg(debug_assertions)]
use std::path::PathBuf;

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
