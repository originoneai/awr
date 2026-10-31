use crate::{Error, Result, WorkItem, WorkStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Source state actions. Claim, release and handoff remain separate runtime operations.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkAction {
    Progress,
    Block,
    Unblock,
    Cancel,
    Reopen,
    Complete,
}
impl WorkAction {
    pub fn mutation_type(self) -> &'static str {
        match self {
            Self::Progress => "work.progress",
            Self::Block => "work.block",
            Self::Unblock => "work.unblock",
            Self::Cancel => "work.cancel",
            Self::Reopen => "work.reopen",
            Self::Complete => "work.complete",
        }
    }
    pub fn event_type(self) -> &'static str {
        match self {
            Self::Progress => "work.progressed",
            Self::Block => "work.blocked",
            Self::Unblock => "work.unblocked",
            Self::Cancel => "work.cancelled",
            Self::Reopen => "work.reopened",
            Self::Complete => "work.completed",
        }
    }
    pub fn next_status(self, from: WorkStatus) -> Result<WorkStatus> {
        use WorkStatus::*;
        match (self, from) {
            (Self::Progress, Planned | Ready | Claimed | InProgress) => Ok(InProgress),
            (Self::Block, InProgress) => Ok(Blocked),
            (Self::Unblock, Blocked) => Ok(InProgress),
            (Self::Cancel, Planned | Ready | Claimed | InProgress | Blocked) => Ok(Cancelled),
            (Self::Reopen, Completed | Cancelled) => Ok(Planned),
            (Self::Complete, InProgress) => Ok(Completed),
            _ => Err(Error::InvalidTransition(format!(
                "cannot {self:?} work with source state {from:?}"
            ))),
        }
    }
    pub fn needs_dependencies(self) -> bool {
        matches!(self, Self::Progress | Self::Unblock | Self::Complete)
    }
    pub fn needs_claim(self) -> bool {
        matches!(self, Self::Progress | Self::Block | Self::Complete)
    }
    /// Work done under a claim, as against an act on the work: what a policy that renews on activity renews on.
    pub fn is_activity(self) -> bool {
        matches!(self, Self::Progress | Self::Block | Self::Unblock)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkActionBinding {
    pub action: WorkAction,
    pub from: WorkStatus,
    pub to: WorkStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<crate::CompletionBinding>,
}
impl WorkActionBinding {
    /// Whitelist the exact action patch; a domain label cannot authorize arbitrary fields.
    pub fn validate(&self, changes: &Value) -> Result<()> {
        match (self.action, &self.completion) {
            (WorkAction::Complete, Some(binding)) => binding.validate()?,
            (WorkAction::Complete, None) => {
                return Err(Error::EvidenceMissing(
                    "completion action requires immutable acceptance and evidence bindings".into(),
                ));
            }
            (_, Some(_)) => {
                return Err(Error::InvalidInput(
                    "only completion may carry a completion binding".into(),
                ));
            }
            _ => (),
        }
        if self.action.next_status(self.from)? != self.to {
            return Err(Error::InvalidTransition(
                "work action target state disagrees with its transition".into(),
            ));
        }
        let fields = changes
            .as_object()
            .ok_or_else(|| Error::InvalidInput("work action patch must be an object".into()))?;
        if changes["status"] != serde_json::to_value(self.to)? {
            return Err(Error::InvalidInput(
                "work action patch must contain its exact target status".into(),
            ));
        }
        for (field, value) in fields {
            match field.as_str() {
                "status" => (),
                "ordinary_completion" if self.action == WorkAction::Reopen && value.is_null() => (),
                "evidence" | "verification" | "evidence_level"
                    if self.action == WorkAction::Complete =>
                {
                    ()
                }
                "next_action" if self.action != WorkAction::Complete => {
                    bounded(value.as_str(), 4096, "next_action")?
                }
                "summary" if self.action == WorkAction::Progress => {
                    bounded(value.as_str(), 16384, "summary")?
                }
                "blocker" if self.action == WorkAction::Block => {
                    bounded(value.as_str(), 4096, "blocker")?
                }
                "blocker"
                    if matches!(
                        self.action,
                        WorkAction::Unblock | WorkAction::Cancel | WorkAction::Reopen
                    ) && value.is_null() =>
                {
                    ()
                }
                _ => {
                    return Err(Error::InvalidInput(format!(
                        "field {field} is not authorized by this work action"
                    )));
                }
            }
        }
        if matches!(
            self.action,
            WorkAction::Progress | WorkAction::Unblock | WorkAction::Reopen
        ) {
            if changes["next_action"].as_str().is_none() {
                return Err(Error::InvalidInput(format!(
                    "{:?} requires --next-action (1..4096 bytes) describing the next source-backed step; the source next_action is only rewritten through this explicit input",
                    self.action
                )));
            }
            bounded(changes["next_action"].as_str(), 4096, "next_action")?;
        }
        if self.action == WorkAction::Block {
            bounded(changes["blocker"].as_str(), 4096, "blocker")?;
        }
        if matches!(
            self.action,
            WorkAction::Unblock | WorkAction::Cancel | WorkAction::Reopen
        ) && fields.get("blocker") != Some(&Value::Null)
        {
            return Err(Error::InvalidInput(
                "this action must explicitly clear the source blocker".into(),
            ));
        }
        Ok(())
    }
}
fn bounded(value: Option<&str>, limit: usize, name: &str) -> Result<()> {
    if value.is_none_or(|s| s.trim().is_empty() || s.len() > limit) {
        return Err(Error::InvalidInput(format!(
            "{name} must contain 1..{limit} bytes"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct WorkActionInput {
    pub action: WorkAction,
    pub reason: String,
    pub next_action: Option<String>,
    pub summary: Option<String>,
    pub blocker: Option<String>,
}
impl WorkActionInput {
    pub fn plan(&self, work: &WorkItem) -> Result<(WorkActionBinding, Value)> {
        bounded(Some(&self.reason), 4096, "reason")?;
        let binding = WorkActionBinding {
            action: self.action,
            from: work.status,
            to: self.action.next_status(work.status)?,
            completion: None,
        };
        let mut changes = json!({"status":binding.to});
        if let Some(value) = &self.next_action {
            changes["next_action"] = json!(value);
        }
        if let Some(value) = &self.summary {
            changes["summary"] = json!(value);
        }
        if let Some(value) = &self.blocker {
            changes["blocker"] = json!(value);
        } else if matches!(
            self.action,
            WorkAction::Unblock | WorkAction::Cancel | WorkAction::Reopen
        ) {
            changes["blocker"] = Value::Null;
        }
        if self.action == WorkAction::Reopen && work.ordinary_completion.is_some() {
            changes["ordinary_completion"] = Value::Null;
        }
        binding.validate(&changes)?;
        Ok((binding, changes))
    }
}
