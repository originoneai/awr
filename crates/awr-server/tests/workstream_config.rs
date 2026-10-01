use awr_server::service::{ProjectBinding, ServiceConfig};

#[test]
fn service_config_requires_explicit_and_unique_operator_bindings() {
    let p = ProjectBinding {
        key: "one".into(),
        tenant_id: "tenant".into(),
        project_id: "project".into(),
    };
    let mut c = ServiceConfig {
        version: 1,
        listen: "127.0.0.1:0".parse().unwrap(),
        allowed_hosts: vec![],
        allowed_web_origins: vec![],
        oauth: None,
        projects: vec![p.clone()],
    };
    c.validate().unwrap();
    c.projects.push(p);
    assert!(c.validate().is_err());
    c.projects.pop();
    c.listen = "0.0.0.0:8080".parse().unwrap();
    assert!(c.validate().is_err());
    c.allowed_hosts.push("team.example.org".into());
    c.validate().unwrap();
    c.allowed_hosts.push("*.example.org".into());
    assert!(c.validate().is_err());
}

#[test]
fn allowed_web_origins_must_be_exact_http_origins() {
    let p = ProjectBinding {
        key: "one".into(),
        tenant_id: "tenant".into(),
        project_id: "project".into(),
    };
    let mut c = ServiceConfig {
        version: 1,
        listen: "127.0.0.1:0".parse().unwrap(),
        allowed_hosts: vec![],
        allowed_web_origins: vec!["https://team.example.org".into()],
        oauth: None,
        projects: vec![p],
    };
    c.validate().unwrap();
    c.allowed_web_origins.push("*.example.org".into());
    assert!(c.validate().is_err());
    c.allowed_web_origins = vec!["ftp://x".into()];
    assert!(c.validate().is_err());
}
