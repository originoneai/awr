//! Local MCP surface for Team review/completion validation (WS-018).
//! Authenticated evidence/review/rework/complete run on Team HTTP/MCP
//! (`awr_team_command` / `awr_team_query` via awr-server). This tool only
//! explains independence and receipt rules.
use rmcp::model::{CallToolResult, Tool, ToolAnnotations};
use serde_json::{Value, json};

pub(crate) fn tool() -> Tool {
    let schema = json!({
        "type":"object","additionalProperties":false,
        "required":["action"],
        "properties":{
            "action":{"type":"string","enum":["capabilities","explain_independence","explain_receipt"]},
            "author_person_id":{"type":"string"},
            "reviewer_person_id":{"type":"string"},
            "completion_policy":{"type":"string"},
            "independence_kind":{"type":"string"}
        }
    });
    let mut tool = Tool::new(
        "awr_team_review",
        "Explain Team review independence and completion receipt rules. Does not mutate Team state; use authenticated awr_team_command delivery.evidence.submit for typed candidate-bound evidence, evidence.submit for generic evidence, then review.open/review.accept/review.return/work.rework/work.complete. Same person's second agent is not team-independent.",
        schema.as_object().unwrap().clone(),
    );
    tool.annotations = Some(
        ToolAnnotations::new()
            .read_only(true)
            .destructive(false)
            .idempotent(true)
            .open_world(false),
    );
    tool
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_keep_generic_and_typed_evidence_entries() {
        let result = handle(json!({"action":"capabilities"})).unwrap();
        let content = result.structured_content.unwrap();
        let commands = content["team_commands"].as_array().unwrap();
        assert!(commands.contains(&json!("evidence.submit")));
        assert!(commands.contains(&json!("delivery.evidence.submit")));
        assert_eq!(
            content["rules"]["same_person_second_agent_not_team_independent"],
            true
        );
    }
}

pub(crate) fn handle(args: Value) -> Result<CallToolResult, awr_core::Error> {
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or_else(|| awr_core::Error::InvalidInput("action required".into()))?;
    match action {
        "capabilities" => Ok(CallToolResult::structured(json!({
            "protocol":"awr-team-review-local-v1",
            "team_commands":[
                "evidence.submit","delivery.evidence.submit","review.open","review.accept",
                "review.return","work.rework","work.complete"
            ],
            "team_queries":["evidence.inspect","review.inspect","completion.inspect"],
            "rules":{
                "independence_unit":"person",
                "same_person_second_agent_not_team_independent":true,
                "personal_self_review_requires_explicit_policy":"trusted_execution_and_author_self_review",
                "personal_self_review_never_labeled_team_independent":true,
                "receipt_omits_provider_private_session":true
            }
        }))),
        "explain_independence" => {
            let author = args
                .get("author_person_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let reviewer = args
                .get("reviewer_person_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let policy = args
                .get("completion_policy")
                .and_then(|v| v.as_str())
                .unwrap_or("trusted_execution_and_review");
            let same = !author.is_empty() && author == reviewer;
            let personal_allowed = policy == "trusted_execution_and_author_self_review";
            let kind = if same {
                if personal_allowed {
                    "personal_self_review"
                } else {
                    "forbidden"
                }
            } else {
                "team_independent"
            };
            Ok(CallToolResult::structured(json!({
                "same_person": same,
                "independence_kind": kind,
                "team_independent_acceptance": kind == "team_independent",
                "personal_self_review_allowed_by_policy": personal_allowed
            })))
        }
        "explain_receipt" => {
            let kind = args
                .get("independence_kind")
                .and_then(|v| v.as_str())
                .unwrap_or("unspecified");
            Ok(CallToolResult::structured(json!({
                "independence_kind": kind,
                "team_independent_acceptance": kind == "team_independent",
                "usable_for_ws030_adoption_auth": kind == "team_independent",
                "provider_private_session": null
            })))
        }
        _ => Err(awr_core::Error::InvalidInput(
            "unsupported awr_team_review action".into(),
        )),
    }
}
