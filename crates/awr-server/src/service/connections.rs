use super::{ProjectBinding, StateData};
use awr_team_pg::{ConnectionSubject, PgError, PgResult};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConnectionRequest {
    protocol_version: u32,
    subject: Option<ConnectionSubject>,
    connection_id: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
}

pub(super) fn schema(revoke: bool) -> Value {
    let mut schema = json!({"type":"object","additionalProperties":false,"required":["protocol_version"],
        "properties":{"protocol_version":{"type":"integer","const":1},
            "subject":{"type":"object","additionalProperties":false,"required":["actor_id","client_id"],
                "properties":{"actor_id":{"type":"string","minLength":1,"maxLength":128},"client_id":{"type":"string","minLength":1,"maxLength":128}}}}});
    if revoke {
        schema["required"]
            .as_array_mut()
            .unwrap()
            .push(json!("connection_id"));
        schema["properties"]["connection_id"] =
            json!({"type":"string","minLength":1,"maxLength":128});
    } else {
        schema["properties"]["cursor"] = json!({"type":"string","minLength":1,"maxLength":128});
        schema["properties"]["limit"] = json!({"type":"integer","minimum":1,"maximum":100});
    }
    schema
}

fn unavailable() -> PgError {
    PgError::Unsupported(
        "connection state is temporarily unavailable; inspect before retrying".into(),
    )
}

pub(super) async fn operate(
    state: &StateData,
    project: &ProjectBinding,
    bearer: &str,
    request: ConnectionRequest,
    revoke: bool,
    current_browser: Option<&str>,
) -> PgResult<Value> {
    let reference = request.connection_id.as_deref();
    if request.protocol_version != 1
        || revoke != reference.is_some()
        || request
            .limit
            .is_some_and(|limit| !(1..=100).contains(&limit))
        || (revoke && (request.cursor.is_some() || request.limit.is_some()))
        || reference
            .into_iter()
            .chain(request.cursor.as_deref())
            .any(|s| {
                s.is_empty()
                    || s.len() > 128
                    || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            })
    {
        return Err(PgError::Protocol("invalid connection request".into()));
    }
    state.store.connections().operate(
        &project.tenant_id,
        &project.project_id,
        bearer,
        request.subject.as_ref(),
        reference,
        |scope| {
            let now = Instant::now();
            if let Some(reference) = reference {
                if reference.starts_with("agent_") {
                    let oauth = state.oauth.as_ref().ok_or(PgError::Forbidden)?;
                    let cutoff = oauth.store.revoke_connection(
                        &oauth.resource(&project.key), reference, |bearer| scope.allows(bearer), now,
                    ).map_err(|error| match error {
                        crate::oauth::OAuthError::AccessDenied => PgError::Forbidden,
                        _ => unavailable(),
                    })?;
                    return Ok(json!({"connection_id":reference,"state":"revoked","revoked_at_ms":cutoff,"revocation_scope":"connection_family"}));
                }
                if reference.starts_with("browser_") && state.web_sessions.revoke_connection(reference, |b| scope.allows(b)).map_err(|_| unavailable())? {
                    return Ok(json!({"connection_id":reference,"state":"revoked","ends_current":current_browser==Some(reference),"revocation_scope":"connection"}));
                }
                return Err(PgError::Forbidden);
            }
            let mut items = scope.credentials();
            let mut browsers = state.web_sessions.connections(|b| scope.allows(b)).map_err(|_| unavailable())?;
            for browser in &mut browsers {
                browser["is_current"] = json!(current_browser.is_some_and(|id| browser["connection_id"] == id));
                browser["can_revoke"] = json!(true);
            }
            items.extend(browsers);
            if let Some(oauth) = &state.oauth {
                let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| unavailable())?.as_millis() as u64;
                let agents = oauth.store.connections(&oauth.resource(&project.key), |b| scope.allows(b), now).map_err(|_| unavailable())?;
                items.extend(agents.into_iter().map(|connection| json!({
                    "connection_id":connection.connection_id,"kind":"agent","client_name":connection.client_name,
                    "created_at_ms":connection.created_at_ms,"expires_at_ms":now_ms+connection.expires_in_seconds*1000,
                    "revoked_at_ms":connection.revoked_at_ms,"state":if connection.revoked_at_ms.is_some() {"revoked"} else {"active"},
                    "revocation_scope":"connection_family","can_revoke":true
                })));
            }
            items.sort_by(|a,b| a["connection_id"].as_str().cmp(&b["connection_id"].as_str()));
            let limit = request.limit.unwrap_or(50);
            let mut visible = items.into_iter().filter(|item| request.cursor.as_deref().is_none_or(|cursor| item["connection_id"].as_str().is_some_and(|id| id>cursor)));
            let page: Vec<_> = visible.by_ref().take(limit).collect();
            let next_cursor = visible.next().and_then(|_| page.last()).map(|item| item["connection_id"].clone());
            Ok(json!({"items":page,"next_cursor":next_cursor,"scope":if scope.is_self {"own_client"} else {"managed_client"},
                "derived_connections_persistence":if state.oauth.as_ref().is_some_and(|o| o.durable) {"single_instance_encrypted"} else {"process_lifetime"}}))
        },
    ).await
}
