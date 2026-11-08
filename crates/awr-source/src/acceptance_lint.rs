//! Mechanical lint advisories for work item acceptance entries.
//!
//! Purely syntactic checks with zero model calls. Findings are advisories
//! only: they never reject a write, never block indexing and never flip the
//! index report to failed. The lint does not judge whether an acceptance
//! criterion is semantically sufficient; it only makes obvious textual
//! defects explicit at ingestion time.

use awr_core::WorkItem;

pub const EMPTY_CODE: &str = "acceptance_lint.empty";
pub const PLACEHOLDER_CODE: &str = "acceptance_lint.placeholder";

/// CJK placeholder fragments. Matched as substrings because Chinese has no
/// word boundaries. The list avoids fragments that occur inside legitimate
/// technical terms (for example 等 is excluded because 等级 contains it).
const CJK_PLACEHOLDERS: &[&str] = &[
    "等等",
    "良好",
    "完善",
    "优化",
    "尽量",
    "尽可能",
    "必要时",
    "酌情",
    "若干",
    "基本上",
    "适当地",
];

/// ASCII placeholder words. Matched case-insensitively on word boundaries so
/// that longer legitimate words (for example "robustness") do not trigger.
const ASCII_PLACEHOLDERS: &[&str] = &[
    "properly",
    "appropriately",
    "reasonably",
    "adequately",
    "timely",
    "seamlessly",
    "various",
    "several",
    "optimize",
    "better",
    "etc",
    "good",
];

/// ASCII placeholder phrases. Matched case-insensitively on the lowercased
/// text after collapsing whitespace.
const ASCII_PLACEHOLDER_PHRASES: &[&str] = &[
    "as needed",
    "if necessary",
    "if needed",
    "user friendly",
    "user-friendly",
];

#[derive(Debug, Clone)]
pub struct AcceptanceFinding {
    pub index: usize,
    pub code: &'static str,
    pub message: String,
    pub matched: Vec<String>,
}

/// Lints every acceptance entry of one work item. Entry order and content are
/// read only; nothing is rewritten.
pub fn lint_work_item(work: &WorkItem) -> Vec<AcceptanceFinding> {
    let mut findings = Vec::new();
    for (index, entry) in work.acceptance.iter().enumerate() {
        if entry.trim().is_empty() {
            findings.push(AcceptanceFinding {
                index,
                code: EMPTY_CODE,
                message:
                    "acceptance entry is blank; state one verifiable outcome or remove the entry"
                        .into(),
                matched: vec![],
            });
            continue;
        }
        let matched = match_placeholders(entry);
        if !matched.is_empty() {
            findings.push(AcceptanceFinding {
                index,
                code: PLACEHOLDER_CODE,
                message: "acceptance entry contains vague placeholder wording; replace it with an observable, verifiable outcome".into(),
                matched,
            });
        }
    }
    findings
}

fn match_placeholders(entry: &str) -> Vec<String> {
    let mut matched = Vec::new();
    let lower = entry.to_lowercase();
    for word in CJK_PLACEHOLDERS {
        if entry.contains(word) {
            matched.push((*word).to_string());
        }
    }
    for word in ASCII_PLACEHOLDERS {
        if contains_ascii_word(&lower, word) {
            matched.push((*word).to_string());
        }
    }
    for phrase in ASCII_PLACEHOLDER_PHRASES {
        if lower.contains(phrase) {
            matched.push((*phrase).to_string());
        }
    }
    matched
}

/// Case-sensitive caller supplies a lowercased haystack. A match requires the
/// characters around the word to be non alphanumeric, so "roast" does not
/// match "as" and "robustness" does not match "robust".
fn contains_ascii_word(lower_haystack: &str, word: &str) -> bool {
    let mut start = 0;
    while let Some(position) = lower_haystack[start..].find(word) {
        let begin = start + position;
        let end = begin + word.len();
        let before = lower_haystack[..begin]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_ascii_alphanumeric());
        let after = lower_haystack[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_ascii_alphanumeric());
        if before && after {
            return true;
        }
        start = end;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use awr_core::{Id, ProjectionMeta, SourceRef, WorkStatus};

    fn work(acceptance: &[&str]) -> WorkItem {
        WorkItem {
            archived: false,
            ordinary_completion: None,
            meta: ProjectionMeta {
                id: Id::new(),
                external_key: "W".into(),
                revision: 1,
                source_ref: SourceRef {
                    source_id: Id::new(),
                    locator: "test.yaml".into(),
                    source_revision: 1,
                    source_fingerprint: "test".into(),
                    pointer: Some("/work_items/0".into()),
                    start_line: None,
                    end_line: None,
                    section_fingerprint: None,
                },
            },
            title: "t".into(),
            kind: None,
            owner: None,
            required: true,
            raw_status: "ready".into(),
            status: WorkStatus::Ready,
            priority: None,
            milestone: None,
            score: None,
            evidence_level: None,
            summary: String::new(),
            next_action: String::new(),
            blocker: None,
            acceptance: acceptance.iter().map(|s| (*s).to_string()).collect(),
            tags: vec![],
            paths: vec![],
            spec_refs: vec![],
            definition: None,
        }
    }

    #[test]
    fn blank_entries_are_flagged() {
        let findings = lint_work_item(&work(&["", "  ", "real criterion"]));
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].code, EMPTY_CODE);
        assert_eq!(findings[0].index, 0);
        assert_eq!(findings[1].index, 1);
    }

    #[test]
    fn placeholder_hits_are_reported_once_per_entry() {
        let findings = lint_work_item(&work(&["优化流程，等等", "Works properly"]));
        assert_eq!(findings.len(), 2);
        let mut hits = findings[0].matched.clone();
        hits.sort();
        assert_eq!(hits, vec!["优化", "等等"]);
        assert_eq!(findings[1].matched, vec!["properly"]);
    }

    #[test]
    fn word_boundaries_prevent_substring_false_hits() {
        assert!(!contains_ascii_word("robustness", "robust"));
        assert!(contains_ascii_word("a robust api", "robust"));
        assert!(contains_ascii_word("keep it simple, etc.", "etc"));
    }

    #[test]
    fn technical_terms_do_not_trigger_cjk_list() {
        assert!(lint_work_item(&work(&["验证搜索相关性阈值", "等待状态迁移完成"])).is_empty());
        assert_eq!(lint_work_item(&work(&["运行良好即可通过"])).len(), 1);
    }
}
