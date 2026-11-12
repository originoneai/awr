use std::process::Command;

#[test]
fn version_identity_ac1_mcp_binary_reports_the_build_commit() {
    let output = Command::new(env!("CARGO_BIN_EXE_awr-mcp"))
        .env("AWR_LOCAL_WEB", "0")
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let line = String::from_utf8(output.stdout).unwrap();
    let injected = option_env!("AWR_BUILD_SHA")
        .filter(|s| s.len() >= 8 && s.bytes().all(|b| b.is_ascii_hexdigit()));
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if injected.is_none() && !root.join(".git").exists() {
        assert_eq!(line, format!("awr-mcp {}\n", env!("CARGO_PKG_VERSION")));
        return;
    }
    let prefix = format!("awr-mcp {} (", env!("CARGO_PKG_VERSION"));
    let identity = line
        .strip_prefix(&prefix)
        .and_then(|s| s.strip_suffix(")\n"))
        .expect("a Git build must report its source commit after the package version");
    let sha = identity.strip_suffix("-dirty").unwrap_or(identity);
    assert_eq!(sha.len(), 8, "{line}");
    assert!(sha.bytes().all(|b| b.is_ascii_hexdigit()), "{line}");
    if let Some(sha_override) = injected {
        assert_eq!(sha, sha_override[..8].to_ascii_lowercase());
        return;
    }
    let head = Command::new("git")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(head.status.success(), "{head:?}");
    assert_eq!(sha, &String::from_utf8(head.stdout).unwrap()[..8]);
}

#[test]
fn version_identity_ac5_mcp_help_keeps_its_existing_content() {
    let output = Command::new(env!("CARGO_BIN_EXE_awr-mcp"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let expected = concat!(
        "Serve AWR through single-project stdio, the local project catalog over stdio, or a shared multi-project HTTP endpoint\n\n",
        "Usage: awr-mcp [OPTIONS]\n\n",
        "Options:\n",
        "      --project <PROJECT>        With --catalog, the default project; otherwise the one project served\n",
        "      --catalog                  Serve the registered projects in the per-user local catalog over stdio\n",
        "      --allow-root <ALLOW_ROOT>  With --catalog, serve only projects under this directory; repeatable\n",
        "      --registry <REGISTRY>      Operator-owned multi-project registry; enables Streamable HTTP at /mcp\n",
        "      --listen <LISTEN>          [default: 127.0.0.1:8080]\n",
        "  -h, --help                     Print help\n",
        "  -V, --version                  Print version\n",
    )
    .replacen(
        "Usage: awr-mcp ",
        &format!("Usage: awr-mcp{} ", std::env::consts::EXE_SUFFIX),
        1,
    );
    assert_eq!(output.stdout, expected.as_bytes());
}
