//! Codex app-server wiring for ChatGPT plan usage.
//!
//! Source: <https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server>
//!
//! The app passes the OAuth access token to the child process as
//! `ACCESS_TOKEN`, starts app-server over stdio with a Responses provider,
//! and drives newline-delimited JSON (`initialize` -> `initialized` ->
//! `thread/start` -> `turn/start`, `thread/resume` after token renewal).

use serde_json::{json, Value};

use crate::siwc::AGENT_NAME;

/// Environment variable carrying the OAuth access token for app-server.
pub const ENV_ACCESS_TOKEN: &str = "ACCESS_TOKEN";

pub const MODEL_PROVIDER: &str = "openai_chatgpt_plan";

/// `codex app-server` arguments that select the ChatGPT plan provider.
pub fn app_server_args() -> Vec<String> {
    vec![
        "app-server".to_string(),
        "--listen".to_string(),
        "stdio://".to_string(),
        "-c".to_string(),
        format!("model_provider=\"{MODEL_PROVIDER}\""),
        "-c".to_string(),
        format!("model_providers.{MODEL_PROVIDER}.name=\"ChatGPT plan\""),
        "-c".to_string(),
        format!("model_providers.{MODEL_PROVIDER}.base_url=\"https://api.openai.com/v1\""),
        "-c".to_string(),
        format!("model_providers.{MODEL_PROVIDER}.env_key=\"{ENV_ACCESS_TOKEN}\""),
        "-c".to_string(),
        format!("model_providers.{MODEL_PROVIDER}.wire_api=\"responses\""),
        "-c".to_string(),
        format!(
            "model_providers.{MODEL_PROVIDER}.requires_openai_auth=false"
        ),
        "-c".to_string(),
        format!("model_providers.{MODEL_PROVIDER}.supports_websockets=false"),
    ]
}

/// `initialize` params. `name` must match the SIWC `agent_name_hint`.
pub fn initialize_params(app_title: &str, app_version: &str) -> Value {
    json!({
        "clientInfo": {
            "name": AGENT_NAME,
            "title": app_title,
            "version": app_version,
        }
    })
}

pub fn thread_start_params(model: &str) -> Value {
    json!({ "model": model })
}

pub fn turn_start_params(thread_id: &str, message: &str) -> Value {
    json!({ "threadId": thread_id, "input": message })
}

pub fn thread_resume_params(thread_id: &str) -> Value {
    json!({ "threadId": thread_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_select_plan_provider_over_stdio() {
        let args = app_server_args();
        assert_eq!(&args[0..3], &["app-server", "--listen", "stdio://"]);
        let joined = args.join(" ");
        assert!(joined.contains("model_provider=\"openai_chatgpt_plan\""));
        assert!(joined.contains("env_key=\"ACCESS_TOKEN\""));
        assert!(joined.contains("wire_api=\"responses\""));
        assert!(joined.contains("supports_websockets=false"));
    }

    #[test]
    fn initialize_names_app_consistently() {
        let p = initialize_params("Don-a-Token", "0.1.0");
        assert_eq!(p["clientInfo"]["name"], AGENT_NAME);
        assert_eq!(p["clientInfo"]["title"], "Don-a-Token");
        assert_eq!(p["clientInfo"]["version"], "0.1.0");
    }

    #[test]
    fn turn_params_carry_thread_and_input() {
        let p = turn_start_params("thread-1", "hello");
        assert_eq!(p["threadId"], "thread-1");
        assert_eq!(p["input"], "hello");
    }
}
