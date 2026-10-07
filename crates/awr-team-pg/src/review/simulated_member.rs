//! Server-captured member provenance for the explicit simulated review policy.
//! Historical NULLs remain unknown; current bindings cannot reconstruct them.
use crate::workstream_auth::ReaderAuthority;
use crate::{PgError, PgResult};
use awr_core::{MemberIdentityKind, MemberIdentityMetadata};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_postgres::Transaction;

pub(crate) const POLICY: &str =
    awr_team::ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY;
pub(crate) const INDEPENDENCE: &str = "simulated_member_independent";
pub(crate) const APPROVAL_BASIS: &str = "simulated_member_independent_review";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Origin {
    codec: String,
    pub actor_id: String,
    pub client_id: String,
    actor_kind: String,
    actor_membership_version: String,
    pub member_id: String,
    member_actor_kind: String,
    member_membership_version: String,
    binding_id: Option<String>,
    member_identity: Option<MemberIdentityMetadata>,
}

impl Origin {
    fn validate(&self) -> PgResult<()> {
        let valid_id =
            |s: &str| !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control);
        let valid_version = |s: &str| s.parse::<i64>().is_ok_and(|v| v > 0 && v.to_string() == s);
        if self.codec != "awr-member-origin-v1"
            || ![&self.actor_id, &self.client_id, &self.member_id]
                .into_iter()
                .all(|s| valid_id(s))
            || !valid_version(&self.actor_membership_version)
            || !valid_version(&self.member_membership_version)
            || !matches!(self.actor_kind.as_str(), "human" | "agent")
            || !matches!(self.member_actor_kind.as_str(), "human" | "agent")
            || (self.actor_kind == "agent"
                && self.binding_id.as_deref().is_none_or(|s| !valid_id(s)))
            || (self.actor_kind == "human"
                && (self.binding_id.is_some() || self.actor_id != self.member_id))
        {
            return Err(PgError::EvidenceInvalid);
        }
        if let Some(metadata) = &self.member_identity {
            metadata.validate().map_err(|_| PgError::EvidenceInvalid)?;
            if !metadata.matches_member_actor_kind(&self.member_actor_kind) {
                return Err(PgError::EvidenceInvalid);
            }
        }
        Ok(())
    }

    pub(crate) fn require_simulated_agent(&self) -> PgResult<()> {
        self.validate()?;
        if self.actor_kind != "agent"
            || self.member_identity.as_ref().map(|m| m.kind)
                != Some(MemberIdentityKind::SimulatedMember)
        {
            return Err(PgError::Forbidden);
        }
        Ok(())
    }

    pub(crate) fn summary(&self) -> Value {
        json!({"member_id":self.member_id,
            "kind":self.member_identity.as_ref().map(|m| m.kind),
            "origin_digest":digest(self)})
    }
}

fn digest(value: &impl Serialize) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("closed provenance type"))
    )
}

pub(crate) fn decode_origin(value: Value) -> PgResult<Origin> {
    let origin: Origin = serde_json::from_value(value).map_err(|_| PgError::EvidenceInvalid)?;
    origin.validate()?;
    Ok(origin)
}

/// Called inside the already project-locked authenticated command transaction.
/// Bindings have no version column. Preserve their actual ID, never invent one.
pub(crate) async fn capture(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
) -> PgResult<Origin> {
    let binding = if auth.actor_kind == "agent" {
        let bindings = tx
            .query(
                "SELECT id,person_id FROM awr_team.person_agent_bindings
            WHERE tenant_id=$1 AND project_id=$2 AND agent_id=$3 AND status='active' FOR SHARE",
                &[&tenant, &project, &auth.actor_id],
            )
            .await?;
        if bindings.len() != 1 {
            return Err(PgError::Forbidden);
        }
        (
            Some(bindings[0].get::<_, String>(0)),
            bindings[0].get::<_, String>(1),
        )
    } else if auth.actor_kind == "human" {
        // Fresh human attribution is explicit. Missing metadata stays unspecified.
        let member = super::resolve_person_id(tx, tenant, project, &auth.actor_id).await?;
        if member != auth.actor_id {
            return Err(PgError::Forbidden);
        }
        (None, member)
    } else {
        return Err(PgError::Forbidden);
    };
    let row = tx.query_opt("SELECT p.member_identity,a.kind,m.membership_version
        FROM awr_team.persons p JOIN awr_team.actors a ON a.tenant_id=p.tenant_id AND a.id=p.id
        JOIN awr_team.project_memberships m ON m.tenant_id=p.tenant_id AND m.project_id=p.project_id AND m.actor_id=p.id
        WHERE p.tenant_id=$1 AND p.project_id=$2 AND p.id=$3
          AND p.status='active' AND a.status='active' FOR SHARE OF p,a,m",
        &[&tenant,&project,&binding.1]).await?.ok_or(PgError::Forbidden)?;
    let origin = Origin {
        codec: "awr-member-origin-v1".into(),
        actor_id: auth.actor_id.clone(),
        client_id: auth.client_id.clone(),
        actor_kind: auth.actor_kind.clone(),
        actor_membership_version: auth.membership_version.to_string(),
        member_id: binding.1,
        member_actor_kind: row.get(1),
        member_membership_version: row.get::<_, i64>(2).to_string(),
        binding_id: binding.0,
        member_identity: row
            .get::<_, Option<Value>>(0)
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| PgError::Forbidden)?,
    };
    origin.validate()?;
    Ok(origin)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceOrigins {
    codec: String,
    executor: Origin,
    submitter: Origin,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RoundOrigins {
    codec: String,
    executor: Origin,
    submitter: Origin,
    pub opener: Origin,
}

pub(crate) async fn evidence_origins(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
    contract_hash: &str,
    execution: Option<&str>,
) -> PgResult<Value> {
    let execution = execution.ok_or(PgError::EvidenceInvalid)?;
    let row = tx
        .query_opt(
            "SELECT executor_origin_json FROM awr_team.executions
        WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND work_id=$4 AND contract_hash=$5",
            &[&tenant, &project, &execution, &work, &contract_hash],
        )
        .await?
        .ok_or(PgError::EvidenceInvalid)?;
    let executor = decode_origin(
        row.get::<_, Option<Value>>(0)
            .ok_or(PgError::EvidenceInvalid)?,
    )?;
    executor.require_simulated_agent()?;
    Ok(json!(EvidenceOrigins {
        codec: "awr-member-evidence-origins-v1".into(),
        executor,
        submitter: capture(tx, tenant, project, auth).await?
    }))
}

pub(crate) async fn round_origins(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    evidence: Value,
) -> PgResult<RoundOrigins> {
    let evidence: EvidenceOrigins =
        serde_json::from_value(evidence).map_err(|_| PgError::EvidenceInvalid)?;
    if evidence.codec != "awr-member-evidence-origins-v1" {
        return Err(PgError::EvidenceInvalid);
    }
    evidence.executor.require_simulated_agent()?;
    evidence.submitter.validate()?;
    Ok(RoundOrigins {
        codec: "awr-member-review-origins-v1".into(),
        executor: evidence.executor,
        submitter: evidence.submitter,
        opener: capture(tx, tenant, project, auth).await?,
    })
}

pub(crate) async fn review_basis(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    origins: Value,
) -> PgResult<(Origin, Value)> {
    let origins: RoundOrigins =
        serde_json::from_value(origins).map_err(|_| PgError::EvidenceInvalid)?;
    if origins.codec != "awr-member-review-origins-v1" {
        return Err(PgError::EvidenceInvalid);
    }
    origins.executor.require_simulated_agent()?;
    origins.submitter.validate()?;
    origins.opener.validate()?;
    let reviewer = capture(tx, tenant, project, auth).await?;
    reviewer.require_simulated_agent()?;
    require_independent(&reviewer, &origins)?;
    let delegation_id = auth.delegation_id.as_ref().ok_or(PgError::Forbidden)?;
    if !auth.agent_review
        || !auth
            .delegated_actions
            .as_ref()
            .is_some_and(|a| a.contains(&awr_team::Action::ReviewDecide))
    {
        return Err(PgError::Forbidden);
    }
    let basis = json!({"codec":"awr-simulated-member-review-v1","policy":POLICY,
        "origins":origins,"reviewer":reviewer,
        "authority":{"delegation_id":delegation_id,"membership_version":auth.membership_version.to_string(),
            "workstream_grant_versions":auth.grant_versions,"snapshot_id":auth.snapshot},
        "approval_basis":APPROVAL_BASIS,"human_approval":false,"team_independent_acceptance":false});
    Ok((reviewer, basis))
}

fn require_independent(reviewer: &Origin, origins: &RoundOrigins) -> PgResult<()> {
    if [&origins.executor, &origins.submitter, &origins.opener]
        .into_iter()
        .any(|origin| {
            reviewer.member_id == origin.member_id
                || reviewer.actor_id == origin.actor_id
                || reviewer.client_id == origin.client_id
        })
    {
        return Err(PgError::AuthorCannotReview);
    }
    Ok(())
}

pub(crate) fn add_summary(data: &mut Value, origin: Option<&Origin>) {
    if let Some(origin) = origin {
        data["member_attribution"] = origin.summary();
    }
}

pub(crate) fn basis_summary(basis: &Value) -> Value {
    json!({"basis_digest":digest(basis), "reviewer_member_id":basis["reviewer"]["member_id"],
        "executor_member_id":basis["origins"]["executor"]["member_id"]})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn origin(name: &str) -> Origin {
        Origin {
            codec: "awr-member-origin-v1".into(),
            actor_id: format!("agent-{name}"),
            client_id: format!("client-{name}"),
            actor_kind: "agent".into(),
            actor_membership_version: "2".into(),
            member_id: format!("member-{name}"),
            member_actor_kind: "agent".into(),
            member_membership_version: "3".into(),
            binding_id: Some(format!("binding-{name}")),
            member_identity: Some(MemberIdentityMetadata {
                kind: MemberIdentityKind::SimulatedMember,
                controller_ref: Some("one-controller".into()),
            }),
        }
    }
    #[test]
    fn same_controller_is_allowed_but_each_original_identity_is_checked() {
        let a = origin("author");
        let reviewer = origin("reviewer");
        let origins = RoundOrigins {
            codec: "awr-member-review-origins-v1".into(),
            executor: a.clone(),
            submitter: origin("submitter"),
            opener: origin("supervisor"),
        };
        require_independent(&reviewer, &origins).unwrap();
        for original in [&origins.executor, &origins.submitter, &origins.opener] {
            for field in 0..3 {
                let mut changed = reviewer.clone();
                match field {
                    0 => changed.member_id = original.member_id.clone(),
                    1 => changed.actor_id = original.actor_id.clone(),
                    _ => changed.client_id = original.client_id.clone(),
                }
                assert!(matches!(
                    require_independent(&changed, &origins),
                    Err(PgError::AuthorCannotReview)
                ));
            }
        }
    }
    #[test]
    fn provenance_is_closed_and_unspecified_never_becomes_simulated() {
        let original = origin("author");
        original.require_simulated_agent().unwrap();
        let mut value = json!(original);
        value["caller_identity"] = json!("forged");
        assert!(decode_origin(value).is_err());
        let mut unspecified = original.clone();
        unspecified.member_identity = None;
        assert!(unspecified.require_simulated_agent().is_err());
        let mut no_binding = original.clone();
        no_binding.binding_id = None;
        assert!(no_binding.validate().is_err());
        let mut wrong_anchor = original;
        wrong_anchor.member_actor_kind = "human".into();
        assert!(wrong_anchor.validate().is_err());
    }
}
