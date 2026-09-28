use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GuestRequest {
    Python {
        code: String,
    },
    Javascript {
        code: String,
    },
    Shell {
        command: String,
        workspace_hex: String,
        env: Vec<(String, String)>,
    },
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GuestResponse {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    #[serde(default)]
    pub workspace_hex: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}
