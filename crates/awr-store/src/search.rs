use crate::{
    Store,
    catalog::{id_at, revision_at},
    db_error,
};
use awr_core::{Error, Freshness, Id, Result, Revision, SourceRef};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

// Rebuild derived summaries when content classification changes.
const POLICY_VERSION: i64 = 9;
const KINDS: &[&str] = &[
    "goal",
    "plan",
    "rule",
    "work_item",
    "decision",
    "evidence",
    "event",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub work_item_key: Option<String>,
    pub limit: usize,
}
impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            text: None,
            kind: None,
            status: None,
            work_item_key: None,
            limit: 10,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub id: Id,
    pub kind: String,
    pub external_key: String,
    pub title: String,
    pub summary: String,
    pub status: Option<String>,
    pub work_item_key: Option<String>,
    pub source_ref: Option<SourceRef>,
    pub source_freshness: Option<Freshness>,
    pub origin: String,
    pub revision: Revision,
    /// SQLite BM25: lower is more relevant. Structured-only results use zero.
    pub rank: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchReport {
    pub hits: Vec<SearchHit>,
    pub truncated: bool,
    pub project_revision: Revision,
    pub index_policy_version: i64,
}

// Do not index source bodies or tool payloads. For curated summary fields, only one bounded line
// is retained; code/log blocks, binary control text and credential-bearing lines are suppressed.
fn safe_text(text: &str, max: usize) -> String {
    if awr_core::contains_sensitive_text(text) {
        return "[redacted]".into();
    }
    let Some(line) = text.lines().map(str::trim).find(|s| !s.is_empty()) else {
        return String::new();
    };
    if line.starts_with("```")
        || line.starts_with("~~~")
        || line.chars().any(|c| c.is_control() && c != '\t')
    {
        return "[omitted]".into();
    }
    line.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}
fn safe_source_text(
    conn: &Connection,
    project: Id,
    kind: &str,
    row: &Document,
    text: &str,
    max: usize,
) -> String {
    let Ok(id) = row.id.parse::<Id>() else {
        return safe_text(text, max);
    };
    let entities = vec![(kind.into(), id, row.revision as u64)];
    if row.source_id.is_none()
        || crate::content_review::ensure_entity_text(conn, project, &entities, text).is_err()
    {
        return safe_text(text, max);
    }
    let Some(line) = text.lines().map(str::trim).find(|s| !s.is_empty()) else {
        return String::new();
    };
    if line.starts_with("```")
        || line.starts_with("~~~")
        || line.chars().any(|c| c.is_control() && c != '\t')
    {
        return "[omitted]".into();
    }
    let candidate = line
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect::<String>();
    if crate::content_review::ensure_entity_text(conn, project, &entities, &candidate).is_ok() {
        candidate
    } else {
        "[redacted]".into()
    }
}

fn cjk(c: char) -> bool {
    matches!(c as u32,0x3400..=0x4dbf|0x4e00..=0x9fff|0xf900..=0xfaff|0x20000..=0x3134f)
}
fn cjk_segments(text: &str) -> Vec<Vec<char>> {
    let mut result = Vec::new();
    let mut segment = Vec::new();
    for c in text.chars() {
        if cjk(c) {
            segment.push(c);
        } else if !segment.is_empty() {
            result.push(std::mem::take(&mut segment));
        }
    }
    if !segment.is_empty() {
        result.push(segment);
    }
    result
}
fn terms(text: &str) -> String {
    cjk_segments(text)
        .iter()
        .flat_map(|s| {
            s.iter()
                .map(|c| c.to_string())
                .chain(s.windows(2).map(|w| w.iter().collect::<String>()))
        })
        .collect::<Vec<_>>()
        .join(" ")
}
fn match_query(text: &str) -> Result<String> {
    if text.trim().is_empty() || text.chars().count() > 500 {
        return Err(Error::InvalidInput(
            "search text must contain 1..500 characters".into(),
        ));
    }
    let mut tokens = Vec::new();
    let mut non_cjk = String::new();
    for c in text.chars() {
        if cjk(c) {
            non_cjk.push(' ');
        } else {
            non_cjk.push(c);
        }
    }
    tokens.extend(
        non_cjk
            .split_whitespace()
            .filter(|s| s.chars().any(char::is_alphanumeric))
            .map(String::from),
    );
    for segment in cjk_segments(text) {
        if segment.len() == 1 {
            tokens.push(segment[0].to_string());
        } else {
            tokens.extend(segment.windows(2).map(|w| w.iter().collect::<String>()));
        }
    }
    if tokens.is_empty() {
        return Err(Error::InvalidInput(
            "search text requires letters or digits".into(),
        ));
    }
    // Natural search terms are quoted, not interpreted as FTS expressions or SQL.
    Ok(tokens
        .iter()
        .map(|s| format!("\"{}\"", s.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND "))
}

struct Document {
    id: String,
    key: String,
    title: String,
    summary: String,
    status: Option<String>,
    work_key: Option<String>,
    source_ref: Option<String>,
    source_id: Option<String>,
    revision: i64,
    explicit_status: bool,
}

fn rebuild(conn: &Connection, project: Id, revision: i64, scoped: bool) -> Result<()> {
    let cached = conn
        .query_row(
            "SELECT project_revision,policy_version FROM search_state WHERE project_id=?1",
            [project.to_string()],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(db_error)?;
    if cached == Some((revision, POLICY_VERSION)) {
        return Ok(());
    }
    conn.execute("INSERT INTO search_fts(search_fts,rowid,external_key,title,summary,terms)
        SELECT 'delete',rowid,external_key,title,summary,terms FROM search_documents WHERE project_id=?1",[project.to_string()]).map_err(db_error)?;
    conn.execute(
        "DELETE FROM search_documents WHERE project_id=?1",
        [project.to_string()],
    )
    .map_err(db_error)?;
    // Static field allowlist. Markdown goal/plan bodies, decision rationale, event payload/logs,
    // artifact bodies and binary files are never copied into the search cache.
    let inputs = [
        ("goal", "goals", "''", "e.status", "NULL"),
        ("plan", "plans", "''", "e.status", "NULL"),
        ("rule", "rules", "e.text", "e.severity", "NULL"),
        (
            "work_item",
            "work_items",
            "e.summary",
            "e.status",
            "e.external_key",
        ),
        ("decision", "decisions", "e.decision", "e.status", "NULL"),
        (
            "evidence",
            "evidence",
            "e.summary",
            "e.level",
            "(SELECT external_key FROM work_items WHERE id=e.work_item_id AND project_id=e.project_id)",
        ),
        (
            "event",
            "events",
            "e.summary",
            "CASE WHEN json_type(e.payload_json,'$.status')='text' THEN json_extract(e.payload_json,'$.status') ELSE e.event_type END",
            "(SELECT external_key FROM work_items WHERE id=e.work_item_id AND project_id=e.project_id)",
        ),
    ];
    for (kind, table, summary, status, work_key) in inputs {
        let (key, title, source_ref, source_id, revision_col, filter) = match kind {
            "event" => (
                "e.id",
                "e.event_type",
                "NULL",
                "NULL",
                "e.project_revision",
                "",
            ),
            "evidence" => (
                "e.external_key",
                "e.evidence_type",
                "e.source_ref_json",
                "e.source_id",
                "e.revision",
                "AND e.active=1 AND (e.source_id IS NULL OR EXISTS(SELECT 1 FROM sources s WHERE s.id=e.source_id AND s.project_id=e.project_id AND s.active=1))",
            ),
            _ => (
                "e.external_key",
                "e.title",
                "e.source_ref_json",
                "e.source_id",
                "e.revision",
                "AND e.active=1 AND EXISTS(SELECT 1 FROM sources s WHERE s.id=e.source_id AND s.project_id=e.project_id AND s.active=1)",
            ),
        };
        let explicit_status = if kind == "event" {
            "coalesce(json_type(e.payload_json,'$.status')='text',0)"
        } else {
            "0"
        };
        let visibility = if scoped {
            crate::scoped_read::visible(table, "e.id")
        } else {
            "1".into()
        };
        let sql = format!(
            "SELECT e.id,{key},{title},{summary},{status},{work_key},{source_ref},{source_id},{revision_col},{explicit_status} FROM {table} e WHERE e.project_id=?1 {filter} AND {visibility} ORDER BY e.id"
        );
        let rows = conn
            .prepare(&sql)
            .map_err(db_error)?
            .query_map([project.to_string()], |r| {
                Ok(Document {
                    id: r.get(0)?,
                    key: r.get(1)?,
                    title: r.get(2)?,
                    summary: r.get(3)?,
                    status: r.get(4)?,
                    work_key: r.get(5)?,
                    source_ref: r.get(6)?,
                    source_id: r.get(7)?,
                    revision: r.get(8)?,
                    explicit_status: r.get(9)?,
                })
            })
            .map_err(db_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error)?;
        for row in rows {
            // Summary fields can be withheld individually. Sensitive identity/provenance cannot
            // be rewritten into a different, apparently actionable entity: omit that document.
            let metadata = (&row.key, &row.status, &row.work_key, &row.source_ref);
            if awr_core::ensure_public_data(&metadata).is_err() {
                continue;
            }
            let title = safe_source_text(conn, project, kind, &row, &row.title, 160);
            let summary = safe_source_text(
                conn,
                project,
                kind,
                &row,
                if row.summary.is_empty() {
                    &row.title
                } else {
                    &row.summary
                },
                280,
            );
            let extra = terms(&format!("{title} {summary}"));
            let status = if kind == "event" && !row.explicit_status {
                row.status
                    .map(|s| s.rsplit(['.', '_']).next().unwrap_or(&s).to_owned())
            } else {
                row.status
            };
            let rowid:i64=conn.query_row("INSERT INTO search_documents(project_id,entity_id,kind,external_key,title,summary,terms,status,work_item_key,source_ref_json,source_id,revision)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12) RETURNING rowid",params![project.to_string(),row.id,kind,row.key,title,summary,extra,status,row.work_key,row.source_ref,row.source_id,row.revision],|r|r.get(0)).map_err(db_error)?;
            conn.execute("INSERT INTO search_fts(rowid,external_key,title,summary,terms) SELECT rowid,external_key,title,summary,terms FROM search_documents WHERE rowid=?1",[rowid]).map_err(db_error)?;
        }
    }
    conn.execute("INSERT INTO search_state(project_id,project_revision,policy_version) VALUES(?1,?2,?3)
        ON CONFLICT(project_id) DO UPDATE SET project_revision=excluded.project_revision,policy_version=excluded.policy_version",params![project.to_string(),revision,POLICY_VERSION]).map_err(db_error)?;
    Ok(())
}

impl Store {
    /// Refresh a derived index and search it in one transaction; source and runtime facts are untouched.
    pub fn search(&mut self, project: Id, query: &SearchQuery) -> Result<SearchReport> {
        self.search_scoped(project, query, false)
    }
    pub(crate) fn search_scoped(
        &mut self,
        project: Id,
        query: &SearchQuery,
        scoped: bool,
    ) -> Result<SearchReport> {
        awr_core::ensure_public_data(query)?;
        if query.limit == 0 || query.limit > 100 {
            return Err(Error::InvalidInput("search limit must be 1..100".into()));
        }
        if query
            .kind
            .as_ref()
            .is_some_and(|s| !KINDS.contains(&s.as_str()))
        {
            return Err(Error::InvalidInput(format!(
                "search type must be one of {}",
                KINDS.join(", ")
            )));
        }
        let text = query.text.as_deref().map(match_query).transpose()?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let revision = tx
            .query_row(
                "SELECT project_revision FROM projects WHERE id=?1",
                [project.to_string()],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map_err(db_error)?
            .ok_or_else(|| Error::NotFound(format!("project {project}")))?;
        rebuild(&tx, project, revision, scoped)?;
        let (join, match_clause, rank) = if text.is_some() {
            (
                "JOIN search_fts ON search_fts.rowid=d.rowid",
                "search_fts MATCH ?5",
                "bm25(search_fts,8.0,4.0,1.0,0.5)",
            )
        } else {
            ("", "?5 IS NULL", "0.0")
        };
        let sql=format!("SELECT d.entity_id,d.kind,d.external_key,d.title,d.summary,d.status,d.work_item_key,d.source_ref_json,s.freshness,d.revision,{rank} AS score
            FROM search_documents d {join} LEFT JOIN sources s ON d.source_id=s.id AND d.project_id=s.project_id
            WHERE d.project_id=?1 AND (?2 IS NULL OR d.kind=?2) AND (?3 IS NULL OR d.status=?3) AND (?4 IS NULL OR d.work_item_key=?4)
              AND {match_clause} ORDER BY score,d.kind,d.external_key LIMIT ?6");
        let mut hits = tx
            .prepare(&sql)
            .map_err(db_error)?
            .query_map(
                params![
                    project.to_string(),
                    query.kind,
                    query.status,
                    query.work_item_key,
                    text,
                    (query.limit + 1) as i64
                ],
                |r| {
                    let reference = r
                        .get::<_, Option<String>>(7)?
                        .map(|s| serde_json::from_str(&s))
                        .transpose()
                        .map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                7,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        })?;
                    let freshness = r
                        .get::<_, Option<String>>(8)?
                        .map(|s| serde_json::from_value(serde_json::Value::String(s)))
                        .transpose()
                        .map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                8,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        })?;
                    Ok(SearchHit {
                        id: id_at(r, 0)?,
                        kind: r.get(1)?,
                        external_key: r.get(2)?,
                        title: r.get(3)?,
                        summary: r.get(4)?,
                        status: r.get(5)?,
                        work_item_key: r.get(6)?,
                        origin: if reference.is_some() {
                            "source"
                        } else {
                            "runtime"
                        }
                        .into(),
                        source_ref: reference,
                        source_freshness: freshness,
                        revision: revision_at(r, 9)?,
                        rank: r.get(10)?,
                    })
                },
            )
            .map_err(db_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error)?;
        let truncated = hits.len() > query.limit;
        hits.truncate(query.limit);
        crate::content_review::ensure_output(&tx, &serde_json::to_value(&hits)?)?;
        tx.commit().map_err(db_error)?;
        Ok(SearchReport {
            hits,
            truncated,
            project_revision: revision as Revision,
            index_policy_version: POLICY_VERSION,
        })
    }
}
