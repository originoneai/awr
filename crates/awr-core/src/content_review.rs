//! Explicit, byte-bound Agent decisions. These are source attestations, never a
//! process-wide scanner exemption and never authority to accept credentials.
use crate::{Error, Result, SECRET_POLICY_VERSION, SensitiveCategory};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const CONTENT_REVIEW_VERSION: u32 = 1;
const MAX_FINDINGS: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContentFinding {
    pub id: String,
    pub category: SensitiveCategory,
    /// Digest of the complete detected clause; never the matched value.
    pub signature: String,
    pub reviewable: bool,
    /// Source coordinates, or decoded-string coordinates when structured_path is set.
    pub line: Option<usize>,
    pub column: Option<usize>,
    /// Ordinals through decoded JSON containers; no possibly sensitive key names.
    pub structured_path: Option<Vec<usize>>,
    /// Decoding used for structured_path; coordinates are relative to that value.
    pub decoded_format: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContentAssessment {
    pub version: u32,
    pub policy_version: u32,
    pub locator: String,
    pub source_sha256: String,
    pub findings: Vec<ContentFinding>,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

impl ContentAssessment {
    pub fn scan(bytes: &[u8], locator: &str) -> Result<Self> {
        crate::ensure_public_text(locator)?;
        let text = std::str::from_utf8(bytes)
            .map_err(|_| Error::InvalidInput("content review requires UTF-8 source text".into()))?;
        let mut assessment = Self {
            version: CONTENT_REVIEW_VERSION,
            policy_version: SECRET_POLICY_VERSION,
            locator: locator.into(),
            source_sha256: digest(bytes),
            findings: Vec::new(),
        };
        for (category, line, column, signature) in crate::secrets::review_text_findings(text) {
            assessment.push(category, Some(line), Some(column), None, None, signature)?;
        }
        // Decoded strings/keys must not conceal hard findings with JSON escapes.
        if let Ok(value) =
            serde_json::from_str::<Value>(text).or_else(|_| serde_yaml_ng::from_str::<Value>(text))
        {
            assessment.scan_value(&value, &[], "json_or_yaml")?;
        } else {
            // Match the Markdown adapters' decoded text without trusting source
            // labels or granting approval to arbitrary transformed values.
            use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
            let mut fragment = None;
            let mut ordinal = 0;
            for event in Parser::new_ext(text, Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS) {
                match event {
                    Event::Start(Tag::TableCell | Tag::Paragraph) => fragment = Some(String::new()),
                    Event::Text(v) | Event::Code(v) => {
                        if let Some(s) = &mut fragment {
                            s.push_str(&v);
                        }
                    }
                    Event::SoftBreak | Event::HardBreak => {
                        if let Some(s) = &mut fragment {
                            s.push(' ');
                        }
                    }
                    Event::End(TagEnd::TableCell | TagEnd::Paragraph) => {
                        if let Some(s) = fragment.take() {
                            assessment.scan_value(
                                &Value::String(s.trim().into()),
                                &[ordinal],
                                "markdown_text",
                            )?;
                            ordinal += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(assessment)
    }

    /// Assess decoded projected fields without treating serialization punctuation
    /// as part of their content. Used only after source/row identity is verified.
    pub fn derived_value(value: &Value) -> Result<Self> {
        let mut assessment = Self {
            version: CONTENT_REVIEW_VERSION,
            policy_version: SECRET_POLICY_VERSION,
            locator: "derived".into(),
            source_sha256: digest(&serde_json::to_vec(value)?),
            findings: vec![],
        };
        assessment.scan_value(value, &[], "derived")?;
        Ok(assessment)
    }

    fn push(
        &mut self,
        category: SensitiveCategory,
        line: Option<usize>,
        column: Option<usize>,
        path: Option<Vec<usize>>,
        decoded_format: Option<&str>,
        signature: String,
    ) -> Result<()> {
        let id = digest(&serde_json::to_vec(&(
            self.version,
            self.policy_version,
            &self.locator,
            &self.source_sha256,
            category,
            line,
            column,
            &path,
            decoded_format,
        ))?);
        if self.findings.iter().any(|finding| finding.id == id) {
            return Ok(());
        }
        if self.findings.len() >= MAX_FINDINGS {
            return Err(Error::InvalidInput(
                "content review exceeds 512 findings; split the source and rescan".into(),
            ));
        }
        self.findings.push(ContentFinding {
            id,
            category,
            signature,
            reviewable: category != SensitiveCategory::Credential,
            line,
            column,
            structured_path: path,
            decoded_format: decoded_format.map(str::to_owned),
        });
        Ok(())
    }

    fn scan_value(&mut self, value: &Value, path: &[usize], format: &str) -> Result<()> {
        match value {
            Value::String(text) => {
                for (category, line, column, signature) in
                    crate::secrets::review_text_findings(text)
                {
                    self.push(
                        category,
                        Some(line),
                        Some(column),
                        Some(path.to_vec()),
                        Some(format),
                        signature,
                    )?;
                }
            }
            Value::Array(values) => {
                for (index, value) in values.iter().enumerate() {
                    let mut child = path.to_vec();
                    child.push(index);
                    self.scan_value(value, &child, format)?;
                }
            }
            Value::Object(values) => {
                for (index, (key, value)) in values.iter().enumerate() {
                    let mut child = path.to_vec();
                    child.push(index);
                    for (category, _, _, signature) in crate::secrets::review_text_findings(key) {
                        self.push(
                            category,
                            None,
                            None,
                            Some(child.clone()),
                            Some(format),
                            signature,
                        )?;
                    }
                    if let Some(category) = crate::secrets::sensitive_field_category(key, value) {
                        self.push(
                            category,
                            None,
                            None,
                            Some(child.clone()),
                            Some(format),
                            field_signature(key, value)?,
                        )?;
                    }
                    self.scan_value(value, &child, format)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicContentDecision {
    pub finding_id: String,
    /// A short explanation of why this exact occurrence is public.
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceContentReview {
    pub project_root: String,
    pub version: u32,
    pub assessment: ContentAssessment,
    pub reviewer: String,
    pub reviewed_at: i64,
    pub decisions: Vec<PublicContentDecision>,
}

/// Constructible only after rescanning the exact source and covering every
/// current finding. Scope owners must still bind it to their project/source.
#[derive(Debug, Clone)]
pub struct VerifiedSourceContentReview {
    review: SourceContentReview,
}
fn field_signature(key: &str, value: &Value) -> Result<String> {
    Ok(digest(&serde_json::to_vec(&(key, value))?))
}

impl VerifiedSourceContentReview {
    pub fn ensure_text(&self, text: &str) -> Result<()> {
        self.review.ensure_derived_text(text)
    }
    pub fn ensure_value(&self, value: &Value) -> Result<()> {
        self.review.ensure_derived_value(value)
    }
    pub fn receipt(&self) -> &SourceContentReview {
        &self.review
    }
    pub fn verify_bytes(&self, bytes: &[u8], locator: &str) -> Result<()> {
        self.review.verify(bytes, locator).map(|_| ())
    }
}
impl SourceContentReview {
    /// Coverage check for an already trusted archive; not a substitute for verify().
    pub fn ensure_derived_text(&self, text: &str) -> Result<()> {
        if self.version != CONTENT_REVIEW_VERSION
            || self.assessment.policy_version != SECRET_POLICY_VERSION
        {
            return Err(Error::SourceConflict(
                "content review policy changed; review the current source".into(),
            ));
        }
        crate::ensure_no_credentials_text(text)?;
        let findings = crate::secrets::review_text_findings(text);
        if findings.len() > MAX_FINDINGS
            || findings.iter().any(|(_, _, _, signature)| {
                !self
                    .assessment
                    .findings
                    .iter()
                    .any(|f| f.reviewable && &f.signature == signature)
            })
        {
            return Err(Error::RuleViolation(
                "derived text is not covered by this source content review".into(),
            ));
        }
        Ok(())
    }
    pub fn ensure_derived_value(&self, value: &Value) -> Result<()> {
        match value {
            Value::String(text) => self.ensure_derived_text(text),
            Value::Array(items) => items.iter().try_for_each(|v| self.ensure_derived_value(v)),
            Value::Object(items) => items.iter().try_for_each(|(key, value)| {
                self.ensure_derived_text(key)?;
                if crate::secrets::sensitive_field_category(key, value).is_some() {
                    let signature = field_signature(key, value)?;
                    if !self
                        .assessment
                        .findings
                        .iter()
                        .any(|f| f.reviewable && f.signature == signature)
                    {
                        return Err(Error::RuleViolation(
                            "derived field is not covered by this source content review".into(),
                        ));
                    }
                }
                self.ensure_derived_value(value)
            }),
            _ => Ok(()),
        }
    }

    pub fn verify(&self, bytes: &[u8], locator: &str) -> Result<VerifiedSourceContentReview> {
        if self.project_root.is_empty()
            || self.project_root.len() > 4096
            || self.version != CONTENT_REVIEW_VERSION
            || self.reviewed_at <= 0
            || self.reviewer.trim().is_empty()
            || self.reviewer.len() > 128
        {
            return Err(Error::InvalidInput(
                "invalid content review version or reviewer metadata".into(),
            ));
        }
        crate::ensure_public_text(&self.project_root)?;
        crate::ensure_public_text(&self.reviewer)?;
        let current = ContentAssessment::scan(bytes, locator)?;
        if self.assessment != current {
            return Err(Error::SourceConflict("content review source, findings or policy changed; rescan and review the current content".into()));
        }
        if current.findings.iter().any(|finding| !finding.reviewable) {
            return Err(Error::RuleViolation(
                "recognizable credentials and private keys cannot be approved by content review"
                    .into(),
            ));
        }
        let mut decisions = BTreeSet::new();
        for decision in &self.decisions {
            if decision.reason.trim().is_empty()
                || decision.reason.len() > 1000
                || !decisions.insert(decision.finding_id.as_str())
            {
                return Err(Error::InvalidInput(
                    "content review requires unique decisions with bounded nonempty reasons".into(),
                ));
            }
            crate::ensure_public_text(&decision.reason)?;
        }
        let expected = current
            .findings
            .iter()
            .map(|finding| finding.id.as_str())
            .collect();
        if decisions != expected {
            return Err(Error::RuleViolation(
                "content review must cover every current finding exactly once".into(),
            ));
        }
        Ok(VerifiedSourceContentReview {
            review: self.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_literals_survive_markdown_decoding_and_structured_assessment() {
        for text in [
            "# Protocol notes\n\nUse `refreshToken=false`. Continue with `checkpoint_token=null`.\n",
            "# Protocol notes\n\nUse `refreshToken=false`; the checkpoint is `checkpoint_token=null`、then continue.\n",
            "# Protocol notes\n\n| Option | Meaning |\n| --- | --- |\n| refreshToken=false | checkpoint_token=null、continue |\n",
            "{\"refreshToken\":false,\"client_secret\":true,\"checkpoint_token\":null}",
            "refreshToken: false\nclient_secret: true\ncheckpoint_token: null\n",
        ] {
            let scan = ContentAssessment::scan(text.as_bytes(), "arbitrary-source.txt").unwrap();
            assert!(scan.findings.is_empty(), "{text}: {:?}", scan.findings);
        }
        let scan =
            ContentAssessment::derived_value(&serde_json::json!({"refreshToken": false})).unwrap();
        assert!(scan.findings.is_empty());
    }

    #[test]
    fn public_literals_do_not_hide_later_unsafe_findings() {
        let text =
            "# Protocol notes\n\nUse `refreshToken=false`; password: synthetic-private-value\n";
        let scan = ContentAssessment::scan(text.as_bytes(), "arbitrary-source.txt").unwrap();
        assert!(!scan.findings.is_empty());
        assert!(
            scan.findings
                .iter()
                .all(|f| f.category == SensitiveCategory::LabelledValue)
        );
        let public = serde_json::to_string(&scan).unwrap();
        assert!(!public.contains("synthetic-private-value"));
    }

    #[test]
    fn scalar_prefixes_cannot_hide_shell_values_or_private_blocks() {
        for (text, category) in [
            (
                "export PASSWORD=false\" synthetic-private-value\"",
                SensitiveCategory::LabelledValue,
            ),
            (
                "export PASSWORD=true' synthetic-private-value'",
                SensitiveCategory::LabelledValue,
            ),
            (
                "# Private prompt\nTrue customer identities must be included.\nInternal customer: synthetic-private-value.",
                SensitiveCategory::PrivatePrompt,
            ),
            (
                "# Private prompt\nfalse\nInternal customer: synthetic-private-value.",
                SensitiveCategory::PrivatePrompt,
            ),
        ] {
            let scan = ContentAssessment::scan(text.as_bytes(), "synthetic-notes.md").unwrap();
            assert!(
                scan.findings
                    .iter()
                    .any(|finding| finding.category == category),
                "{text}"
            );
            assert!(
                !serde_json::to_string(&scan)
                    .unwrap()
                    .contains("synthetic-private-value")
            );
            let bytes = serde_json::to_vec(&serde_json::json!({"body": text})).unwrap();
            let decoded = ContentAssessment::scan(&bytes, "synthetic-notes.json").unwrap();
            assert!(
                decoded
                    .findings
                    .iter()
                    .any(|finding| finding.category == category)
            );
        }
    }
    const LOCATION: &str = "file:///synthetic/project/notes.md";
    fn review(text: &str) -> SourceContentReview {
        let assessment = ContentAssessment::scan(text.as_bytes(), LOCATION).unwrap();
        let decisions = assessment
            .findings
            .iter()
            .map(|finding| PublicContentDecision {
                finding_id: finding.id.clone(),
                reason: "Public synthetic protocol field, verified against its schema.".into(),
            })
            .collect();
        SourceContentReview {
            project_root: "/synthetic/project".into(),
            version: 1,
            assessment,
            reviewer: "agent-reviewer".into(),
            reviewed_at: 1000,
            decisions,
        }
    }
    #[test]
    fn public_review_covers_exact_findings_without_weakening_the_default_guard() {
        let text = "# Protocol\npassword: public-example\nTOKEN=public-marker\n";
        let receipt = review(text);
        assert!(receipt.assessment.findings.len() >= 2);
        assert!(crate::ensure_public_text(text).is_err());
        receipt.verify(text.as_bytes(), LOCATION).unwrap();
        assert!(crate::ensure_public_text(text).is_err());
        let output = serde_json::to_string(&receipt.assessment).unwrap();
        assert!(!output.contains("public-example"));
        assert!(!output.contains("public-marker"));
        for mutate in 0..4 {
            let mut changed = receipt.clone();
            match mutate {
                0 => {
                    changed.decisions.pop();
                }
                1 => changed.decisions.push(changed.decisions[0].clone()),
                2 => changed.assessment.policy_version = 0,
                _ => changed.assessment.findings[0].reviewable = false,
            }
            assert!(changed.verify(text.as_bytes(), LOCATION).is_err());
        }
        assert!(
            receipt
                .verify(format!("{text}\n").as_bytes(), LOCATION)
                .is_err()
        );
        assert!(
            receipt
                .verify(text.as_bytes(), "file:///synthetic/other/notes.md")
                .is_err()
        );
    }
    #[test]
    fn multiline_values_and_appended_clauses_are_not_covered_by_label_only() {
        for text in [
            "password:\npublic-marker",
            "password:\n  public-marker",
            "password: |\n  public-marker",
            "Environment: {\n  HOME: public-marker\n}",
        ] {
            let permit = review(text).verify(text.as_bytes(), LOCATION).unwrap();
            permit.ensure_text(text).unwrap();
            assert!(
                permit
                    .ensure_text(&text.replace("public-marker", "different-marker"))
                    .is_err()
            );
            assert!(permit.ensure_text(&format!("{text}-extra")).is_err());
        }
    }
    #[test]
    fn oversized_review_is_never_silently_truncated() {
        let text = (0..513)
            .map(|i| format!("password: public-marker-{i}\n"))
            .collect::<String>();
        assert!(ContentAssessment::scan(text.as_bytes(), LOCATION).is_err());
        assert!(crate::ensure_public_text(&text).is_err());
    }
    #[test]
    fn hard_credentials_cannot_hide_behind_public_decisions_or_json_escapes() {
        for text in [
            "password: public-example\nsk-synthetic-private-value",
            "-----BEGIN PRIVATE KEY-----\nsynthetic material",
            r#"{"body":"ordinary\nBearer abc123XYZ456TOKEN7890abcTOKEN12345"}"#,
            r#"{"body":"\u0073\u006b-synthetic-private-value"}"#,
        ] {
            let receipt = review(text);
            assert!(
                receipt
                    .assessment
                    .findings
                    .iter()
                    .any(|finding| !finding.reviewable)
            );
            assert!(receipt.verify(text.as_bytes(), LOCATION).is_err());
            assert!(
                !serde_json::to_string(&receipt.assessment)
                    .unwrap()
                    .contains("synthetic-private-value")
            );
        }
    }
}
