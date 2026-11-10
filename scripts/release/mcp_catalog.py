"""Required installed stdio API, shared by package smoke and assembly checks."""

DOMAIN_TOOLS = frozenset({
    "awr_query", "awr_context", "awr_work", "awr_evidence",
    "awr_session", "awr_continuity", "awr_change", "awr_compaction",
})

FLAT_TOOLS = frozenset({
    "awr_project_status", "awr_work_ready", "awr_work_get", "awr_context_compile",
    "awr_work_transition", "awr_event_append", "awr_evidence_record", "awr_search",
    "awr_session_start", "awr_session_get", "awr_session_list",
    "awr_session_checkpoint", "awr_session_end", "awr_session_resume", "awr_session_claim",
    "awr_session_wait", "awr_session_reply", "awr_operation_get",
    "awr_operation_recover", "awr_source_reindex",
    "awr_work_prepare", "awr_completion_prepare",
    "awr_work_assess", "awr_work_manage",
    "awr_work_check", "awr_work_admit", "awr_work_contain", "awr_work_reassess",
    "awr_work_gap", "awr_work_finding", "awr_spec_check",
    "awr_work_graph", "awr_change_preview", "awr_change_apply",
    "awr_change_status", "awr_change_recover",
    "awr_compaction_observe", "awr_compaction_get", "awr_compaction_defer",
})


def validate_stdio_tools(names):
    """Accept the default hierarchical catalog; flat names stay callable."""
    assert isinstance(names, list) and all(isinstance(name, str) for name in names), "invalid MCP tool names"
    actual = set(names)
    assert len(actual) == len(names), "duplicate MCP tool names"
    assert actual == DOMAIN_TOOLS, (
        f"MCP catalog mismatch: missing={sorted(DOMAIN_TOOLS - actual)}, "
        f"unexpected={sorted(actual - DOMAIN_TOOLS)}"
    )
    return sorted(actual)
