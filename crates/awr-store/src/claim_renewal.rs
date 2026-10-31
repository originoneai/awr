//! Recorded work extends the claims its session holds, when the project's policy says so.
use crate::{catalog::id_at, db_error};
use awr_core::{ClaimRenewal, Error, Id, Result, now_millis};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

/// What the clock says about one claim that has an expiry.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// It ran out at or before the work: it stays out, and the receipt says so.
    RanOut,
    /// It would run out before `until`: it now runs to `until`.
    Extend(i64),
    /// It already runs at least that far.
    Keep,
}

/// The boundary is the one `Claim::active_at` draws: a claim whose expiry is the instant of the work is out.
fn verdict(expires_at: i64, at: i64, until: i64) -> Verdict {
    if expires_at <= at {
        Verdict::RanOut
    } else if until > expires_at {
        Verdict::Extend(until)
    } else {
        Verdict::Keep
    }
}

/// Extends the live claims of `session` to `renewal.ttl_ms` from now, inside the transaction of the work that
/// earns it, so the work, the new expiry and the receipt commit in one revision. A claim without an expiry stays
/// without one, a claim that already runs further is not shortened, and a claim that ran out is never revived:
/// it is marked expired, as the next claim of its work would have marked it. What changed is the
/// `claim_renewal` field of `payload`; nothing is said when nothing changed.
pub(crate) fn renew_session_claims(
    conn: &Connection,
    project: Id,
    session: Id,
    renewal: ClaimRenewal,
    payload: &mut Value,
) -> Result<()> {
    let at = now_millis()?;
    let until = i64::try_from(renewal.ttl_ms)
        .ok()
        .and_then(|ttl| at.checked_add(ttl))
        .ok_or_else(|| Error::InvalidInput("claim renewal overflow".into()))?;
    let claims = conn
        .prepare(
            "SELECT id,work_item_id,expires_at FROM claims
             WHERE project_id=?1 AND session_id=?2 AND status='active' AND released_at IS NULL
               AND expires_at IS NOT NULL ORDER BY id",
        )
        .map_err(db_error)?
        .query_map(params![project.to_string(), session.to_string()], |row| {
            Ok((id_at(row, 0)?, id_at(row, 1)?, row.get::<_, i64>(2)?))
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    let mut renewed = Vec::new();
    let mut ran_out = Vec::new();
    for (claim, work, expires_at) in claims {
        match verdict(expires_at, at, until) {
            Verdict::Keep => (),
            Verdict::Extend(until) => {
                conn.execute(
                    "UPDATE claims SET expires_at=?1,revision=revision+1
                     WHERE project_id=?2 AND id=?3 AND status='active'",
                    params![until, project.to_string(), claim.to_string()],
                )
                .map_err(db_error)?;
                renewed.push(json!({
                    "claim_id": claim,
                    "work_item_id": work,
                    "previous_expires_at": expires_at,
                    "expires_at": until,
                }));
            }
            Verdict::RanOut => {
                conn.execute(
                    "UPDATE claims SET status='expired',revision=revision+1
                     WHERE project_id=?1 AND id=?2 AND status='active'",
                    params![project.to_string(), claim.to_string()],
                )
                .map_err(db_error)?;
                ran_out.push(claim);
            }
        }
    }
    if !renewed.is_empty() || !ran_out.is_empty() {
        payload["claim_renewal"] = json!({
            "ttl_ms": renewal.ttl_ms,
            "renewed": renewed,
            "expired_claim_ids": ran_out,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Verdict, verdict};

    #[test]
    fn the_instant_a_claim_runs_out_is_out_and_the_instant_after_is_not() {
        let (at, until) = (1_000, 5_000);
        assert_eq!(verdict(999, at, until), Verdict::RanOut);
        assert_eq!(verdict(1_000, at, until), Verdict::RanOut);
        assert_eq!(verdict(1_001, at, until), Verdict::Extend(5_000));
    }

    #[test]
    fn a_claim_is_extended_only_to_a_later_instant() {
        let (at, until) = (1_000, 5_000);
        assert_eq!(verdict(4_999, at, until), Verdict::Extend(5_000));
        assert_eq!(verdict(5_000, at, until), Verdict::Keep);
        assert_eq!(verdict(5_001, at, until), Verdict::Keep);
        assert_eq!(verdict(i64::MAX, at, until), Verdict::Keep);
    }
}
