//! A scan that finds credentials in several files names all of them in one stop (issue #627).
//!
//! The intake scan used to end at the first file the secret boundary refused, so a project with findings in several
//! documents was fixed one `init` at a time. It now goes on through the other documents and reports every file with
//! its category and location, never its content. One finding is reported exactly as before.

use awr_core::Id;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Project(PathBuf);

impl Project {
    fn new(name: &str) -> Self {
        let p = Self(std::env::temp_dir().join(format!("awr-findings-{name}-{}", Id::new())));
        fs::create_dir_all(p.0.join("docs")).unwrap();
        p
    }
    fn write(&self, relative: &str, text: &str) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .arg("--project")
            .arg(&self.0)
            .arg("--json")
            .args(args)
            .output()
            .unwrap()
    }
    /// The error report of a failed command, from stderr.
    fn fails(&self, args: &[&str]) -> (Value, String) {
        let out = self.run(args);
        assert!(!out.status.success(), "{args:?} should fail");
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let report = serde_json::from_str(
            stderr
                .lines()
                .rev()
                .find(|l| l.trim_start().starts_with('{'))
                .unwrap_or("{}"),
        )
        .unwrap();
        (report, all)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn secret(label: &str) -> String {
    format!("# Notes\n\nsetup\npassword: hunter2-{label}-hunter2hunter2\n")
}

fn file_name(finding: &Value) -> String {
    finding["location"]["locator"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string()
}

#[test]
fn every_file_with_a_finding_is_named_in_one_stop() {
    let p = Project::new("several");
    p.write("README.md", &secret("a"));
    p.write("docs/guide.md", &secret("b"));
    p.write("docs/setup.md", &format!("{}\nmore\n", secret("c")));
    p.write("docs/clean.md", "# Clean\n\nnothing here\n");

    let (report, all) = p.fails(&["init"]);
    assert_eq!(report["code"], "RuleViolation");
    let details = &report["details"];
    assert_eq!(details["finding_total"], 3, "{report}");
    let findings = details["findings"].as_array().unwrap();
    let mut names: Vec<String> = findings.iter().map(file_name).collect();
    names.sort();
    assert_eq!(names, ["README.md", "guide.md", "setup.md"]);
    for finding in findings {
        assert!(
            finding["category"].is_string() && finding["location"]["line"].as_u64().is_some(),
            "{finding}"
        );
    }
    // The first finding keeps the fields a caller already reads, and is the first of the list.
    assert_eq!(details["category"], findings[0]["category"]);
    assert_eq!(details["location"], findings[0]["location"]);
    assert!(!all.contains("hunter2"), "no content in any output");
    assert!(!p.0.join(".awr").exists(), "nothing was written");
}

#[test]
fn one_finding_is_reported_exactly_as_before() {
    let p = Project::new("single");
    p.write("README.md", &secret("a"));
    p.write("docs/clean.md", "# Clean\n");
    let (report, all) = p.fails(&["init"]);
    assert_eq!(report["code"], "RuleViolation");
    assert!(
        report["details"].get("findings").is_none()
            && report["details"].get("finding_total").is_none(),
        "{report}"
    );
    assert_eq!(report["details"]["location"]["line"], 4);
    assert!(
        report["details"]["location"]["locator"]
            .as_str()
            .unwrap()
            .ends_with("README.md")
    );
    assert!(!all.contains("hunter2"));
}

#[test]
fn the_list_is_bounded_and_the_total_is_exact() {
    let p = Project::new("bounded");
    for n in 0..55 {
        p.write(&format!("docs/note-{n:02}.md"), &secret(&n.to_string()));
    }
    let (report, all) = p.fails(&["init"]);
    assert_eq!(report["details"]["finding_total"], 55);
    assert_eq!(report["details"]["findings"].as_array().unwrap().len(), 50);
    assert!(!all.contains("hunter2"));
}

#[test]
fn a_blocked_draft_leaves_a_diagnostic_that_lists_every_file_and_no_content() {
    let p = Project::new("draft");
    p.write("README.md", &secret("a"));
    p.write("docs/guide.md", &secret("b"));
    let draft = std::env::temp_dir().join(format!("awr-findings-draft-{}.json", Id::new()));
    let (report, _) = p.fails(&["init", "--write-draft", &draft.to_string_lossy()]);
    assert_eq!(report["details"]["finding_total"], 2);
    let diagnostic: Value = serde_json::from_slice(&fs::read(&draft).unwrap()).unwrap();
    assert_eq!(diagnostic["status"], "content_review_required");
    assert_eq!(diagnostic["diagnostic"]["details"]["finding_total"], 2);
    assert!(!fs::read_to_string(&draft).unwrap().contains("hunter2"));
    let _ = fs::remove_file(draft);
}
