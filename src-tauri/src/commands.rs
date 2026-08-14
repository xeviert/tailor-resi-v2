use crate::error::AppError;

#[tauri::command]
pub fn ping() -> Result<String, AppError> {
    Ok("pong".to_string())
}
