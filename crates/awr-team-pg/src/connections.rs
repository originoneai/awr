//! Connection management reuses live project identity and policy locks. The
//! synchronous adapter runs under these locks, so permission revocation cannot
//! race between a successful authorization check and the local cutoff.
use crate::{
    PgError, PgResult,
    pool::PgPool,
    workstream_auth::{
        authenticate, authenticate_writer, credential_reference, workstream_credential_hash,
    },
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc};

/// The longest client name a credential keeps. The server bounds a client's
/// registration to the same 120 characters.
pub const MAX_CLIENT_NAME_CHARS: usize = 120;

/// The name a client registered for itself, reduced to display text: control
/// characters are dropped, surrounding whitespace is trimmed and at most
/// [`MAX_CLIENT_NAME_CHARS`] characters are kept. Blank input yields no name.
/// The result is self-reported text for display only; it identifies nothing.
pub fn client_display_name(raw: &str) -> Option<String> {
    let trimmed = raw
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>()
        .trim()
        .to_owned();
    (!trimmed.is_empty()).then(|| trimmed.chars().take(MAX_CLIENT_NAME_CHARS).collect())
}

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

    /// Record the name a client registered for itself when the member approved a
    /// connection made with `bearer` (AWR-PPD-062). The name reaches the credential
    /// through [`client_display_name`] first. Only a live credential that the caller
    /// holds the secret of, usable for `project`, can be named.
    ///
    /// The first approved connection names a credential and later connections
    /// through it keep that name, so recorded history is never relabelled. The name is
    /// self-reported display text: it grants nothing, and two credentials may carry
    /// the same name without being merged.
    pub async fn record_client_name(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        raw_name: &str,
    ) -> PgResult<()> {
        let Some(name) = client_display_name(raw_name) else {
            return Ok(());
        };
        let reference = credential_reference(bearer)?;
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        crate::tx::bind_workstream_scope(&tx, tenant, project).await?;
        tx.execute(
            "UPDATE awr_team.credentials SET client_name=$4
             WHERE tenant_id=$1 AND id=$2 AND secret_hash=$3
               AND (project_id IS NULL OR project_id=$5)
               AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>clock_timestamp())
               AND client_name IS NULL",
            &[
                &tenant,
                &reference.credential_id,
                &reference.secret_hash,
                &name,
                &project,
            ],
        )
        .await?;
        tx.commit().await?;
        Ok(())
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
            crate::delegation_auth::authorize_project_entry(&tx, &auth, project).await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn every_character_is_kept(raw: &str, expected: &str) {
        assert_eq!(client_display_name(raw).as_deref(), Some(expected));
    }

    // A registered name passes through unchanged.
    #[test]
    fn a_plain_name_is_kept_as_registered() {
        every_character_is_kept("Synthetic Native Client", "Synthetic Native Client");
        every_character_is_kept("客户端 one-two.three", "客户端 one-two.three");
    }

    // Control characters (Unicode Cc, including DEL and the C1 range) are dropped before
    // the name can be stored; everything else stays in order.
    #[test]
    fn control_characters_are_dropped_and_the_rest_keeps_its_order() {
        every_character_is_kept("Bad\u{7f}Name", "BadName");
        every_character_is_kept(
            "two\nlines\tand\u{0}a\u{85}c1\u{9f}end",
            "twolinesandac1end",
        );
        every_character_is_kept("\u{1f}\u{1e}edged\u{b}", "edged");
    }

    // Only surrounding whitespace is trimmed; a name that is blank afterwards is no name.
    #[test]
    fn surrounding_whitespace_is_trimmed_and_a_blank_name_is_no_name() {
        every_character_is_kept("  Claude Code  ", "Claude Code");
        every_character_is_kept("\n\t Synth \u{3000}", "Synth");
        for blank in ["", "   ", "\t\n\u{0}", "\u{3000}\u{a0}"] {
            assert_eq!(client_display_name(blank), None, "{blank:?}");
        }
    }

    // The stored name holds at most 120 characters, cut on a character boundary.
    #[test]
    fn a_long_name_is_cut_to_the_registered_bound_on_a_character_boundary() {
        let exact: String = "n".repeat(MAX_CLIENT_NAME_CHARS);
        assert_eq!(client_display_name(&exact).as_deref(), Some(exact.as_str()));
        let cut = client_display_name(&format!("{exact}overflow")).unwrap();
        assert_eq!(cut, exact);
        let wide = client_display_name(&"客".repeat(400)).unwrap();
        assert_eq!(wide.chars().count(), MAX_CLIENT_NAME_CHARS);
        assert!(wide.chars().all(|character| character == '客'));
        let mixed = client_display_name(&format!("a{}", "é".repeat(500))).unwrap();
        assert_eq!(mixed.chars().count(), MAX_CLIENT_NAME_CHARS);
        assert!(mixed.starts_with('a') && mixed.chars().skip(1).all(|c| c == 'é'));
    }

    // Whatever arrives, the result always satisfies the column's own bound: 1..=120
    // characters and no control character. The reduction and the database agree.
    #[test]
    fn every_output_satisfies_the_column_bound() {
        let sources = [
            String::new(),
            "x".repeat(10_000),
            "\u{7f}".repeat(300),
            format!("{}{}", "a\u{a}".repeat(200), " tail"),
            "客".repeat(130),
            format!("{}{}", "\u{2028}".repeat(50), "z".repeat(130)),
            "line\u{2028}separator name".to_owned(),
        ];
        for raw in &sources {
            if let Some(name) = client_display_name(raw) {
                assert!(
                    (1..=MAX_CLIENT_NAME_CHARS).contains(&name.chars().count()),
                    "{name:?}"
                );
                assert!(
                    name.chars().all(|character| !character.is_control()),
                    "{name:?}"
                );
                assert_eq!(client_display_name(&name).as_deref(), Some(name.as_str()));
            }
        }
    }
}
