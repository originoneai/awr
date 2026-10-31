//! The shape in which a work and its readiness are shown is the contract between the entries: the CLI and the
//! MCP server both build their replies from these functions.
use awr_core::*;
use serde_json::{Value, json};

fn projected(title: &str, summary: &str) -> Projected<WorkItem> {
    Projected {
        item: WorkItem {
            archived: false,
            ordinary_completion: None,
            meta: ProjectionMeta {
                id: Id::new(),
                external_key: "W".into(),
                revision: 3,
                source_ref: SourceRef {
                    source_id: Id::new(),
                    locator: "file:///work.yaml".into(),
                    source_revision: 7,
                    source_fingerprint: "sha256:aa".into(),
                    pointer: None,
                    start_line: None,
                    end_line: None,
                    section_fingerprint: None,
                },
            },
            title: title.into(),
            kind: None,
            owner: Some("coordinator".into()),
            required: true,
            raw_status: "ready".into(),
            status: WorkStatus::Ready,
            priority: Some("P1".into()),
            milestone: None,
            score: None,
            evidence_level: None,
            summary: summary.into(),
            next_action: "Draft the analysis".into(),
            blocker: None,
            acceptance: vec![],
            tags: vec![],
            paths: vec![],
            spec_refs: vec![],
            definition: None,
        },
        source: Source {
            id: Id::new(),
            project_id: Id::new(),
            domain: "ledger".into(),
            role: "primary".into(),
            locator: "file:///work.yaml".into(),
            format: "yaml".into(),
            adapter: "yaml-ledger-v1".into(),
            revision: 7,
            fingerprint: "sha256:aa".into(),
            freshness: Freshness::Fresh,
            config: json!({}),
        },
        project_revision: 9,
    }
}

fn readiness(ready: bool) -> WorkReadiness {
    let work = projected("Work", "");
    let claim = Claim {
        id: Id::new(),
        project_id: Id::new(),
        work_item_id: work.item.meta.id,
        session_id: Id::new(),
        agent_id: "worker-a".into(),
        branch_id: None,
        status: "active".into(),
        acquired_at: 1,
        expires_at: Some(2),
        released_at: None,
        revision: 1,
    };
    WorkReadiness {
        dependencies: DependencyGraph {
            work_item_key: "W".into(),
            required_only: true,
            dependencies: vec![],
            edges: vec![],
            missing_keys: vec![],
            cycle_keys: vec![],
            project_revision: 9,
        },
        work,
        ready,
        active_claims: vec![claim],
        diagnostics: vec![ReadinessDiagnostic {
            code: "active_claim".into(),
            work_item_key: "W".into(),
            detail: "claimed by worker-a".into(),
        }],
        branch_id: None,
        evaluated_at: 5,
    }
}

fn keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    keys
}

#[test]
fn a_work_is_shown_with_exactly_these_fields() {
    let value = work_brief(&projected("Work", "A summary"));
    assert_eq!(
        keys(&value),
        [
            "archived",
            "blocker",
            "external_key",
            "freshness",
            "id",
            "milestone",
            "next_action",
            "ordinary_completion",
            "ordinary_work_policy",
            "owner",
            "priority",
            "raw_status",
            "revision",
            "source_ref",
            "source_revision",
            "status",
            "summary",
            "title"
        ]
    );
    assert_eq!(value["external_key"], "W");
    assert_eq!(value["revision"], 3);
    assert_eq!(value["source_revision"], 7);
    assert_eq!(value["status"], "ready");
}

#[test]
fn the_summary_is_the_title_when_there_is_none_and_is_always_short() {
    assert_eq!(
        work_brief(&projected("The title", ""))["summary"],
        "The title"
    );
    assert_eq!(
        work_brief(&projected("The title", "The summary"))["summary"],
        "The summary"
    );
    let long = "word ".repeat(200);
    assert!(short_text(&long).chars().count() <= 240);
    assert_eq!(
        work_brief(&projected("T", &long))["summary"],
        short_text(&long)
    );
}

#[test]
fn readiness_adds_whether_it_can_be_selected_why_not_and_who_holds_a_claim_and_nothing_else() {
    let ready = readiness(false);
    let value = ready_brief(&ready);
    let mut expected = keys(&work_brief(&ready.work));
    expected.extend(["active_claims", "diagnostics", "ready"].map(String::from));
    expected.sort();
    assert_eq!(keys(&value), expected);
    assert_eq!(value["ready"], false);
    assert_eq!(value["diagnostics"][0]["code"], "active_claim");
    assert_eq!(value["diagnostics"][0]["work_item_key"], "W");
    assert_eq!(value["diagnostics"][0]["detail"], "claimed by worker-a");
    // A claim is shown by who holds it and until when, never by its internal bookkeeping.
    assert_eq!(
        keys(&value["active_claims"][0]),
        ["agent_id", "expires_at", "id", "session_id"]
    );
    assert_eq!(ready_brief(&readiness(true))["ready"], true);
}
