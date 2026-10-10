use awr_mcp::domains::DOMAINS;
use std::collections::BTreeSet;
use toml::Value;

fn server(source: &str) -> toml::value::Table {
    let config: Value = toml::from_str(source).unwrap();
    config
        .get("mcp_servers")
        .and_then(Value::as_table)
        .and_then(|servers| servers.get("awr"))
        .and_then(Value::as_table)
        .unwrap()
        .clone()
}

fn strings(table: &toml::value::Table, key: &str) -> Vec<String> {
    table[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn codex_templates_and_layer_note_track_the_mcp_catalog() {
    let domains: BTreeSet<_> = DOMAINS
        .iter()
        .map(|domain| domain.name.to_owned())
        .collect();
    let children: BTreeSet<_> = DOMAINS
        .iter()
        .flat_map(|domain| domain.children.iter().map(|child| (*child).to_owned()))
        .collect();

    let grouped = server(include_str!("../../../examples/codex/config.toml.example"));
    assert_eq!(
        grouped["command"].as_str(),
        Some("/absolute/path/to/awr-mcp")
    );
    assert_eq!(
        strings(&grouped, "args"),
        ["--project", "/absolute/path/to/initialized/project"]
    );
    assert_eq!(
        grouped["cwd"].as_str(),
        Some("/absolute/path/to/initialized/project")
    );
    let grouped_tools: BTreeSet<_> = strings(&grouped, "enabled_tools").into_iter().collect();
    assert!(grouped_tools.is_subset(&domains), "{grouped_tools:?}");
    assert!(grouped_tools.is_disjoint(&children), "{grouped_tools:?}");
    assert_eq!(
        grouped_tools,
        BTreeSet::from([
            "awr_query".to_owned(),
            "awr_context".to_owned(),
            "awr_work".to_owned(),
            "awr_evidence".to_owned(),
        ])
    );

    let flat = server(include_str!(
        "../../../examples/codex/config.flat.toml.example"
    ));
    assert_eq!(
        flat["env"]
            .as_table()
            .and_then(|env| env.get("AWR_MCP_TOOL_EXPOSURE_MODE"))
            .and_then(Value::as_str),
        Some("flat")
    );
    let flat_tools: BTreeSet<_> = strings(&flat, "enabled_tools").into_iter().collect();
    assert!(flat_tools.is_subset(&children), "{flat_tools:?}");
    assert!(flat_tools.is_disjoint(&domains), "{flat_tools:?}");
    assert_eq!(
        flat_tools,
        BTreeSet::from([
            "awr_project_status".to_owned(),
            "awr_work_ready".to_owned(),
            "awr_work_get".to_owned(),
            "awr_context_compile".to_owned(),
            "awr_work_transition".to_owned(),
            "awr_event_append".to_owned(),
            "awr_evidence_record".to_owned(),
            "awr_search".to_owned(),
        ])
    );

    let note = include_str!("../../../docs/dev/integrations/codex.md");
    assert!(note.contains("L1 host note + L2 optional adapter"));
    assert!(note.contains("0.5.1"));
    assert!(note.contains("`awr_query`"));
    assert!(note.contains("awr_project_status"));
    assert!(note.contains("config.flat.toml.example"));
    assert!(!note.contains("0.162.0"));
    assert!(!note.contains("Packaged **0.4.0**"));
    assert!(!note.contains("The expected tools are:"));
    for domain in &domains {
        assert!(note.contains(&format!("`{domain}`")), "{domain}");
    }
    let index = include_str!("../../../docs/dev/integrations/README.md");
    assert!(
        index
            .lines()
            .any(|line| line.starts_with("| Codex | L1 + L2 |"))
    );
}
