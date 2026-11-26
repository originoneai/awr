//! Mechanical lint advisories for work item acceptance entries.
//!
//! Purely syntactic checks with zero model calls. Findings are advisories
//! only: they never reject a write, never block indexing and never flip the
//! index report to failed. The lint does not judge whether an acceptance
//! criterion is semantically sufficient; it only makes obvious textual
//! defects explicit at ingestion time.
//!
//! The word lists follow a strict false-negative-over-false-positive policy.
//! An advisory is read by agents as "rewrite this entry", so a hit on an
//! ordinary verifiable criterion (for example "优化到 200ms 以内") would
//! cause legitimate wording to be rewritten. The lists therefore only carry
//! wording that is almost never part of a real criterion: explicit
//! placeholders and open-ended adverbs, never common verbs or evaluative
//! adjectives.

use awr_core::WorkItem;

pub const EMPTY_CODE: &str = "acceptance_lint.empty";
pub const PLACEHOLDER_CODE: &str = "acceptance_lint.placeholder";

/// CJK placeholder fragments. Matched as substrings because Chinese has no
/// word boundaries. The list avoids fragments that occur inside legitimate
/// technical terms (for example 等 is excluded because 等级 contains it) and
/// excludes common verbs and evaluative adjectives (优化, 完善, 良好) that
/// appear inside ordinary verifiable criteria.
const CJK_PLACEHOLDERS: &[&str] = &[
    "等等",
    "待定",
    "待补充",
    "待完善",
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
/// Common evaluative words (good, better, optimize) are excluded: they appear
/// inside ordinary verifiable criteria far too often.
const ASCII_PLACEHOLDERS: &[&str] = &[
    "tbd",
    "tbc",
    "todo",
    "properly",
    "appropriately",
    "reasonably",
    "adequately",
    "timely",
    "seamlessly",
    "etc",
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
        let findings = lint_work_item(&work(&["流程待定，等等", "Works properly, TBD"]));
        assert_eq!(findings.len(), 2);
        let mut hits = findings[0].matched.clone();
        hits.sort();
        assert_eq!(hits, vec!["待定", "等等"]);
        let mut hits = findings[1].matched.clone();
        hits.sort();
        assert_eq!(hits, vec!["properly", "tbd"]);
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
        assert!(lint_work_item(&work(&["运行良好即可通过"])).is_empty());
    }

    #[test]
    fn verifiable_criteria_with_common_verbs_stay_silent() {
        // Review requirement: ordinary verifiable criteria that merely contain
        // "优化"/"良好"/"good"/"better"/"optimize" must never be flagged.
        assert!(
            lint_work_item(&work(&[
                "接口 P95 延迟从 400ms 优化到 200ms 以内，压测脚本随 PR 附带",
                "全部单测通过，运行良好即可发布",
                "Optimize the query plan; good coverage is better than none",
            ]))
            .is_empty()
        );
    }

    #[test]
    fn quantity_words_stay_silent() {
        // Review requirement: quantity words are ordinary verifiable wording;
        // "The export handles several concurrent clients" was flagged by the
        // first list and that hit was a false positive.
        assert!(
            lint_work_item(&work(&[
                "The export handles several concurrent clients",
                "汇总报表覆盖 various 报表模板的导出",
            ]))
            .is_empty()
        );
    }

    #[test]
    fn explicit_placeholders_are_flagged() {
        let findings = lint_work_item(&work(&["待补充", "acceptance TBD", "status tbc"]));
        assert_eq!(findings.len(), 3);
        assert_eq!(findings[0].matched, vec!["待补充"]);
        assert_eq!(findings[1].matched, vec!["tbd"]);
        assert_eq!(findings[2].matched, vec!["tbc"]);
    }
}
