use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};
use tokio::process::Command;

fn check_description(description: &str) -> Result<(), String> {
    if description.len() > 200 {
        return Err(format!(
            "description exceeds 200 bytes: {}",
            description.len()
        ));
    }
    Ok(())
}

fn check_catalog(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > 6144 {
        return Err(format!("catalog exceeds 6144 bytes: {}", bytes.len()));
    }
    Ok(())
}

fn check_limits(manifest: &Value) -> Result<(), String> {
    match manifest["limits"].as_array() {
        Some(limits)
            if !limits.is_empty()
                && limits
                    .iter()
                    .all(|v| v.as_str().is_some_and(|s| !s.is_empty())) =>
        {
            Ok(())
        }
        _ => Err("domain discovery needs a nonempty limits array of strings".into()),
    }
}

#[test]
fn description_guard_accepts_200_bytes_and_rejects_201_bytes() {
    assert!(check_description(&"a".repeat(200)).is_ok());
    assert!(check_description(&"a".repeat(201)).is_err());
    assert!(check_description(&"é".repeat(101)).is_err());
}

#[test]
fn schema_child_descriptions_fit_200_bytes() {
    for tool in awr_mcp::tools() {
        if awr_mcp::TOOL_NAMES.contains(&tool.name.as_ref()) {
            check_description(tool.description.as_deref().unwrap())
                .unwrap_or_else(|error| panic!("{}: {error}", tool.name));
        }
    }
}

#[test]
fn catalog_guard_accepts_6144_bytes_and_rejects_6145_bytes() {
    assert!(check_catalog(&vec![b'a'; 6144]).is_ok());
    assert!(check_catalog(&vec![b'a'; 6145]).is_err());
}

#[test]
fn limits_guard_rejects_missing_empty_and_nonstring_limits() {
    assert!(check_limits(&json!({"limits":["Use the current project revision."]})).is_ok());
    for value in [json!({}), json!({"limits":[]}), json!({"limits":[null]})] {
        assert!(check_limits(&value).is_err());
    }
}

struct Project(PathBuf);

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn first_level_catalog_is_short_and_empty_domain_calls_return_limits() {
    let project =
        Project(std::env::temp_dir().join(format!("awr-description-{}", awr_core::Id::new())));
    fs::create_dir_all(&project.0).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_awr-mcp"));
    command
        .arg("--project")
        .arg(&project.0)
        .env("AWR_LOCAL_WEB", "0")
        .env("AWR_TEAM_ENABLED", "0")
        .env_remove("AWR_MCP_TOOL_EXPOSURE_MODE")
        .kill_on_drop(true);
    let client = ().serve(TokioChildProcess::new(command).unwrap()).await.unwrap();
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), awr_mcp::domains::DOMAINS.len());
    check_catalog(&serde_json::to_vec(&tools).unwrap()).unwrap();
    for tool in &tools {
        check_description(tool.description.as_deref().unwrap()).unwrap();
        let reply = client
            .call_tool(
                CallToolRequestParams::new(tool.name.to_string())
                    .with_arguments(Default::default()),
            )
            .await
            .unwrap();
        assert_ne!(reply.is_error, Some(true));
        let manifest = reply.structured_content.unwrap();
        check_limits(&manifest).unwrap_or_else(|error| panic!("{}: {error}", tool.name));
        for child in manifest["children"].as_array().unwrap() {
            let name = child["name"].as_str().unwrap();
            let original = awr_mcp::tools()
                .into_iter()
                .find(|t| t.name == name)
                .unwrap();
            assert_eq!(
                child["input_schema"],
                serde_json::to_value(original.input_schema).unwrap()
            );
            assert_eq!(
                child["read_only"],
                original.annotations.unwrap().read_only_hint.unwrap()
            );
        }
    }
    client.cancel().await.unwrap();
}
