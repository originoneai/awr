//! Connection management reuses live project identity and policy locks. The
//! synchronous adapter runs under these locks, so permission revocation cannot
//! race between a successful authorization check and the local cutoff.
use crate::{
    PgError, PgResult,
    pool::PgPool,
    workstream_auth::{authenticate, authenticate_writer, workstream_credential_hash},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionSubject {
    pub actor_id: String,
    pub client_id: String,
}

struct Credential {
    id: String,
    reference: String,
    hash: String,
    expires_at_ms: Option<i64>,
    revoked_at_ms: Option<i64>,
    project_scoped: bool,
}

/// Private hashes only serve the trusted adapter's predicate. They have no
/// serialization or Debug implementation and never enter a public response.
pub struct ConnectionScope {
    credentials: Vec<Credential>,
    live_hashes: BTreeSet<String>,
    pub is_self: bool,
    pub member_name: String,
}

impl ConnectionScope {
    pub fn allows(&self, bearer: &str) -> bool {
        workstream_credential_hash(bearer).is_ok_and(|hash| self.live_hashes.contains(&hash))
    }

    pub fn credentials(&self) -> Vec<Value> {
        self.credentials.iter().map(|c| json!({
            "connection_id":c.reference,"kind":"shared_access","member_name":self.member_name,
            "expires_at_ms":c.expires_at_ms,"revoked_at_ms":c.revoked_at_ms,
            "state":if c.revoked_at_ms.is_some() {"revoked"} else {"active"},
            "revocation_scope":if c.project_scoped {"shared_project_access"} else {"shared_account_access"},
            "can_revoke":self.is_self
        })).collect()
    }
}

pub struct ConnectionStore {
    pool: Arc<PgPool>,
}

impl ConnectionStore {
    pub(crate) fn from_pool(pool: Arc<PgPool>) -> Self {
        Self { pool }
    }

    pub async fn operate(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        subject: Option<&ConnectionSubject>,
        revoke: Option<&str>,
        effect: impl FnOnce(&ConnectionScope) -> PgResult<Value>,
    ) -> PgResult<Value> {
        if subject.is_some_and(|s| {
            [&s.actor_id, &s.client_id]
                .iter()
                .any(|v| v.is_empty() || v.len() > 128 || v.chars().any(char::is_control))
        }) {
            return Err(PgError::Protocol("invalid connection subject".into()));
        }
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let mut auth = if revoke.is_some() {
            authenticate_writer(&tx, tenant, project, bearer).await?
        } else {
            authenticate(&tx, tenant, project, bearer).await?
        };
        let actor = subject
            .map(|s| s.actor_id.as_str())
            .unwrap_or(&auth.actor_id)
            .to_owned();
        let caller = subject
            .map(|s| s.client_id.as_str())
            .unwrap_or(&auth.client_id)
            .to_owned();
        let is_self = actor == auth.actor_id && caller == auth.client_id;
        if is_self {
            crate::delegation_auth::authorize_own_connections(&tx, &auth, project).await?;
        } else {
            crate::delegation_auth::authorize_project_action(
                &tx,
                &mut auth,
                project,
                awr_team::Action::AccessManageProject,
            )
            .await?;
            for stream in &auth.catalog.workstreams {
                if stream.state == awr_core::WorkstreamState::Active {
                    auth.access
                        .authorize(&auth.catalog, stream.id, awr_core::WorkstreamAction::Manage)
                        .map_err(|_| PgError::Forbidden)?;
                }
            }
        }
        let member = tx
            .query_opt(
                "SELECT a.display_name FROM awr_team.actors a
             JOIN awr_team.project_memberships m ON m.tenant_id=a.tenant_id AND m.actor_id=a.id
             WHERE a.tenant_id=$1 AND a.id=$2 AND m.project_id=$3 AND a.status='active'
             FOR SHARE OF a,m",
                &[&tenant, &actor, &project],
            )
            .await?
            .ok_or(PgError::Forbidden)?;
        let rows = tx
            .query(
                "SELECT id,secret_hash,(extract(epoch FROM expires_at)*1000)::bigint,
                    (extract(epoch FROM revoked_at)*1000)::bigint,project_id IS NOT NULL
             FROM awr_team.credentials
             WHERE tenant_id=$1 AND actor_id=$2 AND client_id=$3
               AND (project_id=$4 OR ($5 AND project_id IS NULL))
               AND (expires_at IS NULL OR expires_at>clock_timestamp())
             ORDER BY id LIMIT 257 FOR SHARE",
                &[&tenant, &actor, &caller, &project, &is_self],
            )
            .await?;
        if rows.len() > 256 {
            return Err(PgError::Unsupported(
                "connection binding exceeds the supported credential limit".into(),
            ));
        }
        let credentials: Vec<_> = rows
            .into_iter()
            .map(|r| {
                let id: String = r.get(0);
                let reference = format!(
                    "access_{:x}",
                    Sha256::digest(format!("awr-connection-reference-v1:{tenant}:{id}"))
                );
                Credential {
                    id,
                    reference,
                    hash: r.get(1),
                    expires_at_ms: r.get(2),
                    revoked_at_ms: r.get(3),
                    project_scoped: r.get(4),
                }
            })
            .collect();
        let scope = ConnectionScope {
            live_hashes: credentials
                .iter()
                .filter(|c| c.revoked_at_ms.is_none())
                .map(|c| c.hash.clone())
                .collect(),
            credentials,
            is_self,
            member_name: member.get(0),
        };
        let result = if let Some(reference) = revoke.filter(|r| r.starts_with("access_")) {
            // Project administration already has a separately previewed member
            // credential-revocation entry. This entry may only end its own binding.
            if !is_self {
                return Err(PgError::Forbidden);
            }
            let credential = scope
                .credentials
                .iter()
                .find(|c| c.reference == reference)
                .ok_or(PgError::Forbidden)?;
            let row = tx.query_one(
                "UPDATE awr_team.credentials SET revoked_at=COALESCE(revoked_at,clock_timestamp())
                 WHERE tenant_id=$1 AND id=$2
                 RETURNING (extract(epoch FROM revoked_at)*1000)::bigint",
                &[&tenant, &credential.id],
            ).await?;
            json!({
                "connection_id": reference,
                "state": "revoked",
                "ends_current": workstream_credential_hash(bearer).is_ok_and(|hash| hash == credential.hash),
                "revoked_at_ms": row.get::<_, i64>(0),
                "revocation_scope": if credential.project_scoped {"shared_project_access"} else {"shared_account_access"}
            })
        } else {
            effect(&scope)?
        };
        tx.commit().await?;
        Ok(result)
    }
}
