// Exercise the production clap tree, including its derived argument constraints.
include!("../src/main.rs");

mod next_action_tests {
    use super::Cli;
    use awr_core::Id;
    use clap::Parser;
    use serde_json::{Value, json};
    use std::{
        fs,
        path::PathBuf,
        process::{Command, Output, Stdio},
    };

    const RULE: &str =
        "# Delivery {#delivery severity=hard scope=project value=*}\n\nKeep the reviewed note.";

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let f = Self(std::env::temp_dir().join(format!("awr-next-{}", Id::new())));
            fs::create_dir(&f.0).unwrap();
            f.write("work.yaml", "goals:\n- id: G\n  title: Review a delivery\n  status: active\n  summary: Deliver a reviewed note\nwork_items:\n- id: W\n  title: Review the note\n  status: planned\n  goal: G\n  acceptance: [The note is reviewed]\n  next_action: Read the note\n- id: D\n  title: Draft note\n  status: draft\n  goal: G\n  acceptance: [The draft is reviewed]\n  next_action: Read the draft\n");
            f.write("rules.md", RULE);
            f.write("mapping.toml", "[project]\nname='Recovery fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='rules'\nrole='primary'\npath='rules.md'\nadapter='markdown-rules-v1'\n");
            f.ok(&["init", "--manifest", "mapping.toml", "--accept"]);
            f
        }
        fn write(&self, path: &str, text: &str) {
            fs::write(self.0.join(path), text).unwrap();
        }
        fn run(&self, args: &[&str]) -> Output {
            Command::new(env!("CARGO_BIN_EXE_awr"))
                .env("AWR_LOCAL_WEB", "0")
                .args(["--project", self.0.to_str().unwrap(), "--json"])
                .args(args)
                .stdin(Stdio::null())
                .output()
                .unwrap()
        }
        fn ok(&self, args: &[&str]) -> Value {
            let o = self.run(args);
            assert!(
                o.status.success(),
                "{args:?}: {}\n{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            serde_json::from_slice(&o.stdout).unwrap()
        }
        fn error(&self, args: &[&str], code: &str) -> Value {
            let o = self.run(args);
            assert!(!o.status.success(), "unexpected success: {args:?}");
            let e: Value = serde_json::from_slice(&o.stderr).expect("structured CLI error");
            assert_eq!(e["code"], code, "{e}");
            e
        }
        fn revision(&self) -> String {
            self.ok(&["session", "list"])["project_revision"].to_string()
        }
        fn session(&self, work: &str, claim: bool) -> Value {
            let rev = self.revision();
            let mut args = vec![
                "session",
                "start",
                "--work",
                work,
                "--agent",
                "fixture",
                "--provider",
                "fixture",
                "--model",
                "fixture",
                "--expected-revision",
                &rev,
            ];
            if claim {
                args.extend(["--claim", "--ttl-ms", "60000"]);
            }
            self.ok(&args)
        }
        fn claim_error(&self, work: &str, code: &str) -> Value {
            self.error(
                &[
                    "session",
                    "start",
                    "--work",
                    work,
                    "--agent",
                    "other",
                    "--provider",
                    "fixture",
                    "--model",
                    "fixture",
                    "--claim",
                    "--expected-revision",
                    &self.revision(),
                ],
                code,
            )
        }
        fn create_input(&self, goal: &str) {
            self.write("create.json", &json!({"version":1,"request_key":"new-note","title":"Review a new note", "fields":{"goal":[goal],"acceptance":["The note is reviewed"],"next_action":"Read the note"}}).to_string());
        }
        fn preview(&self) -> Value {
            self.ok(&["work", "create", "--input", "create.json"])
        }
        fn accept_args<'a>(&self, preview: &'a Value, revision: &'a str) -> Vec<&'a str> {
            vec![
                "work",
                "create",
                "--input",
                "create.json",
                "--accept",
                "--expected-preview",
                preview["preview"]["fingerprint"].as_str().unwrap(),
                "--expected-revision",
                revision,
            ]
        }
        fn follow(&self, error: &Value) -> Value {
            let args = validate_action(error);
            let refs = args[1..].iter().map(String::as_str).collect::<Vec<_>>();
            self.ok(&refs)
        }
        fn required_tokens(&self) -> usize {
            self.ok(&["source", "reindex"]);
            self.error(
                &[
                    "context",
                    "compile",
                    "--work",
                    "W",
                    "--detached",
                    "--after-revision",
                    &self.revision(),
                    "--budget",
                    "1",
                ],
                "BudgetExceeded",
            )["details"]["required"]
                .as_u64()
                .unwrap() as usize
        }
        fn pad_context(&self, target: usize) {
            let mut words = target.saturating_sub(self.required_tokens());
            // Fingerprint token counts can make two padding lengths oscillate.
            // A distinct suffix prevents revisiting that same pair of source snapshots.
            for attempt in 0..256 {
                self.write(
                    "rules.md",
                    &format!(
                        "{RULE}{}\nCalibration variant {attempt}.\n",
                        " datum".repeat(words)
                    ),
                );
                let actual = self.required_tokens();
                if actual == target {
                    return;
                }
                words = (words as isize + target as isize - actual as isize).max(0) as usize;
            }
            panic!("could not construct a {target}-token context");
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    // Independently tokenize POSIX quoting before passing argv to production clap.
    fn argv(command: &str) -> Vec<String> {
        let mut args = Vec::new();
        let mut word = String::new();
        let mut quoted = None;
        let mut escaped = false;
        let mut started = false;
        for c in command.chars() {
            if escaped {
                word.push(c);
                escaped = false;
                started = true;
            } else if c == '\\' && quoted != Some('\'') {
                escaped = true;
            } else if quoted == Some(c) {
                quoted = None;
                started = true;
            } else if (c == '\'' || c == '"') && quoted.is_none() {
                quoted = Some(c);
                started = true;
            } else if c.is_whitespace() && quoted.is_none() {
                if started {
                    args.push(std::mem::take(&mut word));
                    started = false;
                }
            } else {
                word.push(c);
                started = true;
            }
        }
        assert!(
            quoted.is_none() && !escaped,
            "incomplete shell quoting: {command}"
        );
        if started {
            args.push(word);
        }
        args
    }
    fn validate_action(error: &Value) -> Vec<String> {
        let action = error["details"]["next_action"]
            .as_str()
            .expect("missing next_action");
        assert!(!action.trim().is_empty() && action.len() <= 300, "{action}");
        if action.starts_with("awr ") {
            let args = argv(action);
            Cli::try_parse_from(&args)
                .unwrap_or_else(|e| panic!("invalid next_action {action}: {e}"));
            args
        } else {
            Vec::new()
        }
    }

    fn guidance_text(f: &Fixture, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .env("AWR_LOCAL_WEB", "0")
            .args(["--project", f.0.to_str().unwrap()])
            .args(args)
            .output()
            .unwrap()
    }

    fn govern(f: &Fixture) {
        let manifest = fs::read_to_string(f.0.join(".awr/project.toml")).unwrap();
        f.write(".awr/project.toml", &manifest.replacen(
            "adapter = \"yaml-ledger-v1\"",
            "adapter = \"yaml-ledger-v1\"\n[sources.options.admission_policy]\nversion=1\nmode='governed_enforce'\nscope='all'",
            1,
        ));
        f.ok(&["source", "reindex"]);
        assert!(f.ok(&["work", "show", "W"])["work"]["admission"]["governed"] == true);
    }

    fn definition_example(f: &Fixture) -> String {
        let out = guidance_text(f, &["work", "help", "governance"]);
        assert!(out.status.success());
        let text = String::from_utf8(out.stdout).unwrap();
        text.split_once("```yaml\n")
            .expect("governance help needs a YAML starter")
            .1
            .split_once("```")
            .unwrap()
            .0
            .to_owned()
    }

    fn follow_definition_guidance(f: &Fixture, text: &str) {
        let next = text
            .lines()
            .find_map(|line| line.strip_prefix("Next: "))
            .expect("missing Next");
        assert_eq!(next, "awr work help governance", "{text}");
        let parsed =
            super::help::parse_from(&argv(next).into_iter().map(Into::into).collect::<Vec<_>>())
                .unwrap_err();
        assert_eq!(parsed.kind(), clap::error::ErrorKind::DisplayHelp);
        let out = guidance_text(
            f,
            &argv(next)[1..]
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        assert!(out.status.success());
        let help = String::from_utf8(out.stdout).unwrap();
        for field in ["definition:", "examples:", "check:"] {
            assert!(help.contains(field), "missing {field}: {help}");
        }
    }

    #[test]
    fn next_step_ac1_creation_refusal_leads_to_definition_fields_within_budget() {
        let f = Fixture::new();
        govern(&f);
        f.create_input("G");
        let p = f.ok(&["work", "create", "--input", "create.json", "--activate"]);
        let rev = p["project_revision"].to_string();
        let mut args = f.accept_args(&p, &rev);
        args.push("--activate");
        let out = guidance_text(&f, &args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            out.stdout.len() + out.stderr.len() <= 1536,
            "{} bytes",
            out.stdout.len() + out.stderr.len()
        );
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("Activation: \"rejected\""), "{text}");
        follow_definition_guidance(&f, &text);
        let key = p["preview"]["plan"]["external_key"].as_str().unwrap();
        assert_eq!(f.ok(&["work", "show", key])["work"]["status"], "draft");
    }

    #[test]
    fn next_step_ac1_activation_refusal_never_stops_at_show_or_repository_docs() {
        let f = Fixture::new();
        govern(&f);
        let args = [
            "work",
            "activate",
            "D",
            "--request-key",
            "missing-definition",
        ];
        let out = guidance_text(&f, &args);
        assert!(!out.status.success());
        assert!(out.stdout.len() + out.stderr.len() <= 1536);
        let text = String::from_utf8(out.stderr).unwrap();
        assert!(!text.contains("Next: awr work show"), "{text}");
        assert!(!text.contains("docs/"), "{text}");
        follow_definition_guidance(&f, &text);
        let error = f.error(&args, "DependencyBlocked");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("definition_not_declared")
        );
    }

    #[test]
    fn next_step_ac1_unrelated_activation_refusal_keeps_planning_repair() {
        let f = Fixture::new();
        let ledger = fs::read_to_string(f.0.join("work.yaml"))
            .unwrap()
            .replace("  next_action: Read the draft\n", "");
        f.write("work.yaml", &ledger);
        f.ok(&["source", "reindex"]);
        let out = guidance_text(
            &f,
            &["work", "activate", "D", "--request-key", "missing-action"],
        );
        assert!(!out.status.success());
        let text = String::from_utf8(out.stderr).unwrap();
        assert!(text.contains("Next: awr work edit D "), "{text}");
        assert!(!text.contains("work help governance"), "{text}");
    }

    #[test]
    fn next_step_ac2_help_example_is_accepted_verbatim_by_work_show() {
        let f = Fixture::new();
        govern(&f);
        let example = definition_example(&f);
        let mut ledger = fs::read_to_string(f.0.join("work.yaml")).unwrap();
        ledger.push_str(
            &example
                .lines()
                .map(|line| format!("  {line}\n"))
                .collect::<String>(),
        );
        f.write("work.yaml", &ledger);
        let shown = f.ok(&["work", "show", "D"]);
        let definition = &shown["work"]["definition"];
        assert!(definition.is_object(), "{shown}");
        assert_ne!(definition["state"], "not_declared");
        assert!(
            !definition["gaps"]
                .as_array()
                .unwrap()
                .iter()
                .any(|gap| gap["code"] == "definition_not_declared")
        );
        assert_eq!(definition["examples"][0]["has_check"], true);
        // A starter declares a definition; remaining business obligations still block admission.
        assert!(!shown["work"]["ready"].as_bool().unwrap());
    }

    #[test]
    fn next_step_ac2_unknown_example_fields_are_rejected_by_the_current_parser() {
        let f = Fixture::new();
        let example = definition_example(&f);
        let mut value: Value = serde_yaml_ng::from_str(&example).unwrap();
        let parse = |value: &Value| {
            serde_json::from_value::<awr_core::WorkDefinition>(value["definition"].clone())
        };
        parse(&value).unwrap().validate().unwrap();
        value["definition"]["examples"][0]["check"]["unknown_field"] = json!(true);
        assert!(
            parse(&value).is_err(),
            "unknown check fields must not enter guidance"
        );
    }

    #[test]
    fn next_step_ac3_show_links_to_the_same_definition_guidance_once_within_budget() {
        let f = Fixture::new();
        govern(&f);
        let out = guidance_text(&f, &["work", "show", "W"]);
        assert!(out.status.success());
        assert!(out.stdout.len() <= 1536, "{} bytes", out.stdout.len());
        let text = String::from_utf8(out.stdout).unwrap();
        assert_eq!(
            text.matches("awr work help governance").count(),
            1,
            "{text}"
        );
        follow_definition_guidance(&f, &text);
    }

    #[test]
    fn next_step_ac3_ungoverned_show_does_not_add_definition_guidance() {
        let f = Fixture::new();
        let out = guidance_text(&f, &["work", "show", "W"]);
        assert!(out.status.success());
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(!text.contains("awr work help governance"), "{text}");
        assert!(!text.contains("```yaml"), "{text}");
    }

    #[test]
    fn gap_hints_ac4_refusal_and_show_keep_their_original_content_and_budget() {
        let f = Fixture::new();
        govern(&f);
        let refusal = guidance_text(
            &f,
            &["work", "activate", "D", "--request-key", "gap-byte-compare"],
        );
        assert!(!refusal.status.success());
        assert!(refusal.stdout.is_empty());
        assert_eq!(refusal.stderr.len(), 291);
        let shown = guidance_text(&f, &["work", "show", "W"]);
        assert!(shown.status.success());
        assert!(shown.stdout.len() <= 1536);
        // Only source URLs and their projection times vary across platforms and runs.
        let normalized = String::from_utf8(shown.stdout.clone())
            .unwrap()
            .lines()
            .filter(|line| !line.trim_start().starts_with("Source:"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            normalized,
            "W — Review the note\nStatus: planned; ready: false; revision: 1 (project 8)\nSummary: Review the note\nBlocker: none\n  Source next: Read the note\n  Latest checkpoint: none in this work/branch scope\nadmission_missing [W]: no admission decision is recorded for this work\nadmission_obligations_unmet [W]: definition gaps: definition_not_declared\nNext: awr work help governance\nAcceptance:\n- The note is reviewed\nDependencies: 0; decisions: 0; evidence: 0"
        );
        let help =
            String::from_utf8(guidance_text(&f, &["work", "help", "governance"]).stdout).unwrap();
        let reference = help
            .split_once("Gap codes | ")
            .expect("gap field reference")
            .1;
        for bytes in [refusal.stderr, shown.stdout] {
            assert!(!String::from_utf8_lossy(&bytes).contains("Gap codes | "));
            assert!(
                bytes.len() + reference.len() > 1536,
                "inlining the reference exceeds the receipt budget"
            );
        }
    }

    #[test]
    fn next_step_ac4_prepare_explains_rule_heading_annotations_and_recovers() {
        let f = Fixture::new();
        f.write("rules.md", "# Delivery\n\nKeep the reviewed note.\n");
        let out = f.run(&["work", "prepare", "W"]);
        assert!(!out.status.success());
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        let issues = value["context"]["completeness"]["issues"]
            .as_array()
            .unwrap();
        let reason = issues
            .iter()
            .find(|i| i["code"] == "hard_rule_unresolved")
            .unwrap()["reason"]
            .as_str()
            .unwrap();
        let annotation = "{severity=hard scope=project value=*}";
        assert!(reason.contains(annotation), "{reason}");
        assert!(reason.contains("end of the heading line"), "{reason}");
        let text = guidance_text(&f, &["work", "prepare", "W"]);
        assert!(String::from_utf8_lossy(&text.stdout).contains(annotation));
        f.write(
            "rules.md",
            &format!("# Delivery {annotation}\n\nKeep the reviewed note.\n"),
        );
        assert_eq!(
            f.ok(&["work", "prepare", "W"])["context"]["completeness"]["rules_complete"],
            true
        );
    }

    #[test]
    fn next_step_ac4_annotation_in_rule_body_keeps_prepare_incomplete() {
        let f = Fixture::new();
        f.write(
            "rules.md",
            "# Delivery\n\n{severity=hard scope=project value=*}\nKeep the reviewed note.\n",
        );
        let out = f.run(&["work", "prepare", "W"]);
        assert!(!out.status.success());
        let error: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(error["code"], "ContextIncomplete");
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value["context"]["completeness"]["rules_complete"], false);
    }

    #[test]
    fn acceptance_1_revision_only_conflict_retries_without_preview_or_scan() {
        let f = Fixture::new();
        f.create_input("G");
        let p = f.preview();
        f.session("W", false);
        let e = f.error(
            &f.accept_args(&p, &p["project_revision"].to_string()),
            "RevisionConflict",
        );
        assert_eq!(e["details"]["preview_still_valid"], true);
        assert_eq!(e["details"]["current_revision"].to_string(), f.revision());
        let saved = f.follow(&e);
        assert_eq!(saved["status"], "completed");
        assert_eq!(saved["preview_fingerprint"], p["preview"]["fingerprint"]);
        assert_ne!(e["code"], "SourceStale");
    }

    #[test]
    fn rework_f3_long_creation_retry_preserves_reviewed_fields_and_activation() {
        let f = Fixture::new();
        let title = format!("Review the delivery {}", "x".repeat(70));
        let first = "a".repeat(110);
        let second = "b".repeat(110);
        assert_eq!(title.chars().count(), 90);
        let args = [
            "work",
            "create",
            "--title",
            &title,
            "--request-key",
            "long-review",
            "--acceptance",
            &first,
            "--acceptance",
            &second,
            "--goal",
            "G",
            "--next-action",
            "Read the delivery report",
            "--activate",
        ];
        let preview = f.ok(&args);
        let before = fs::read(f.0.join("work.yaml")).unwrap();
        f.session("W", false);
        let rev = preview["project_revision"].to_string();
        let mut accept = args.to_vec();
        accept.extend([
            "--accept",
            "--expected-preview",
            preview["preview"]["fingerprint"].as_str().unwrap(),
            "--expected-revision",
            &rev,
        ]);
        let error = f.error(&accept, "RevisionConflict");
        assert_eq!(error["details"]["preview_still_valid"], true);
        assert!(
            error["details"]["next_action"]
                .as_str()
                .unwrap()
                .starts_with("awr "),
            "{error}"
        );
        assert_eq!(
            error["details"]["retry"]["expected_revision"].to_string(),
            f.revision()
        );
        assert_eq!(fs::read(f.0.join("work.yaml")).unwrap(), before);
        let saved = f.follow(&error);
        assert_eq!(saved["status"], "completed");
        assert_eq!(saved["draft"], false);
        assert_eq!(
            saved["preview_fingerprint"],
            preview["preview"]["fingerprint"]
        );
        let key = saved["external_key"].as_str().unwrap();
        let work = f.ok(&["work", "show", key]);
        assert_eq!(work["work"]["title"], title);
        assert_eq!(work["acceptance"], json!([first, second]));
        assert!(f.session(key, true)["claim"]["id"].is_string());
    }

    #[test]
    fn rework_f3_control_characters_in_fields_keep_executable_retry() {
        let f = Fixture::new();
        let args = [
            "work",
            "create",
            "--title",
            "Review a multiline criterion",
            "--request-key",
            "multiline-review",
            "--acceptance",
            "The delivery is reviewed\nand the result is recorded",
        ];
        let preview = f.ok(&args);
        f.session("W", false);
        let rev = preview["project_revision"].to_string();
        let mut accept = args.to_vec();
        accept.extend([
            "--accept",
            "--expected-preview",
            preview["preview"]["fingerprint"].as_str().unwrap(),
            "--expected-revision",
            &rev,
        ]);
        let error = f.error(&accept, "RevisionConflict");
        let action = error["details"]["next_action"].as_str().unwrap();
        assert!(action.starts_with("awr "), "{error}");
        assert!(!action.chars().any(char::is_control), "{action}");
        assert_eq!(f.follow(&error)["status"], "completed");
    }

    #[test]
    fn rework_f3_long_context_filters_keep_exact_recovery_request() {
        let f = Fixture::new();
        let path = format!("notes/{}/review.md", "p".repeat(180));
        let tag = "t".repeat(180);
        let args = [
            "context",
            "compile",
            "--work",
            "W",
            "--detached",
            "--path",
            &path,
            "--tag",
            &tag,
            "--after-revision",
            "0",
            "--budget",
            "1",
        ];
        let error = f.error(&args, "BudgetExceeded");
        assert!(
            error["details"]["next_action"]
                .as_str()
                .unwrap()
                .starts_with("awr "),
            "{error}"
        );
        let recovered = f.follow(&error);
        let required = error["details"]["required"].to_string();
        let mut expected_args = args.to_vec();
        *expected_args.last_mut().unwrap() = &required;
        let expected = f.ok(&expected_args);
        assert_eq!(recovered["work_context"], expected["work_context"]);
        let mut mixed = validate_action(&error);
        mixed.extend(["--work".into(), "W".into()]);
        assert!(
            Cli::try_parse_from(mixed).is_err(),
            "saved input must not silently replace selection flags"
        );
    }

    #[test]
    fn rework_f3_malformed_saved_context_input_is_invalid_input() {
        let f = Fixture::new();
        let before = fs::read(f.0.join("work.yaml")).unwrap();
        for text in ["not json", "[]", "{}"] {
            f.write("bad-context.json", text);
            f.error(
                &["context", "compile", "--input", "bad-context.json"],
                "InvalidInput",
            );
            assert_eq!(fs::read(f.0.join("work.yaml")).unwrap(), before);
        }
    }

    #[test]
    fn rework_f3_long_retry_requires_repreview_after_source_change() {
        let f = Fixture::new();
        let title = "n".repeat(90);
        let criterion = "c".repeat(110);
        let args = [
            "work",
            "create",
            "--title",
            &title,
            "--request-key",
            "changed-long-review",
            "--acceptance",
            &criterion,
            "--goal",
            "G",
            "--next-action",
            "Read the note",
            "--activate",
        ];
        let preview = f.ok(&args);
        f.write("rules.md", &format!("{RULE}\nKeep the delivery trace.\n"));
        let before = fs::read(f.0.join("work.yaml")).unwrap();
        let rev = preview["project_revision"].to_string();
        let mut accept = args.to_vec();
        accept.extend([
            "--accept",
            "--expected-preview",
            preview["preview"]["fingerprint"].as_str().unwrap(),
            "--expected-revision",
            &rev,
        ]);
        let error = f.error(&accept, "SourceStale");
        assert_eq!(error["details"]["preview_still_valid"], false);
        assert!(
            error["details"]["next_action"]
                .as_str()
                .unwrap()
                .starts_with("awr "),
            "{error}"
        );
        let fresh = f.follow(&error);
        assert_eq!(fresh["status"], "preview");
        assert_ne!(
            fresh["preview"]["fingerprint"],
            preview["preview"]["fingerprint"]
        );
        assert_eq!(fs::read(f.0.join("work.yaml")).unwrap(), before);
    }

    #[test]
    fn rework_f3_long_unsupported_name_retains_bounded_capability_command() {
        let f = Fixture::new();
        let name = format!("unavailable-{}", "x".repeat(400));
        let error = f.error(&[&name], "Unsupported");
        assert!(
            error["details"]["next_action"]
                .as_str()
                .unwrap()
                .starts_with("awr "),
            "{error}"
        );
        assert_eq!(f.follow(&error)["ok"], true);
    }

    #[test]
    fn rework_f3_long_draft_key_uses_bounded_activation_command() {
        let f = Fixture::new();
        let key = format!("DRAFT-{}", "x".repeat(350));
        let ledger = fs::read_to_string(f.0.join("work.yaml"))
            .unwrap()
            .replace("- id: D\n", &format!("- id: {key}\n"));
        f.write("work.yaml", &ledger);
        f.ok(&["source", "reindex"]);
        let error = f.claim_error(&key, "DependencyBlocked");
        assert!(
            error["details"]["next_action"]
                .as_str()
                .unwrap()
                .starts_with("awr work activate "),
            "{error}"
        );
        assert_eq!(f.follow(&error)["status"], "preview");
    }

    #[test]
    fn rework_f4_missing_draft_fields_offer_edit_then_activate_and_claim() {
        for missing in ["next_action", "acceptance", "goal", "unknown_goal"] {
            let f = Fixture::new();
            let mut args = vec![
                "work",
                "create",
                "--title",
                "Review a new note",
                "--request-key",
                "repair-review",
                "--activate",
            ];
            if missing != "next_action" {
                args.extend(["--next-action", "Read the note"]);
            }
            if missing != "acceptance" {
                args.extend(["--acceptance", "The note is reviewed"]);
            }
            if missing != "goal" {
                args.extend([
                    "--goal",
                    if missing == "unknown_goal" {
                        "ABSENT"
                    } else {
                        "G"
                    },
                ]);
            }
            let preview = f.ok(&args);
            let rev = preview["project_revision"].to_string();
            args.extend([
                "--accept",
                "--expected-preview",
                preview["preview"]["fingerprint"].as_str().unwrap(),
                "--expected-revision",
                &rev,
            ]);
            let saved = f.ok(&args);
            assert_eq!(saved["status"], "completed", "{missing}: {saved}");
            assert_eq!(saved["draft"], true, "{missing}: {saved}");
            let key = saved["external_key"].as_str().unwrap();
            let rejected = f.error(
                &[
                    "work",
                    "activate",
                    key,
                    "--request-key",
                    "unrepaired-activation",
                ],
                saved["activation"]["error"]["code"].as_str().unwrap(),
            );
            assert_eq!(
                rejected["message"], saved["activation"]["error"]["message"],
                "{missing}: activation alone must not erase its refusal"
            );
            let text = Command::new(env!("CARGO_BIN_EXE_awr"))
                .env("AWR_LOCAL_WEB", "0")
                .args([
                    "--project",
                    f.0.to_str().unwrap(),
                    "work",
                    "activate",
                    key,
                    "--request-key",
                    "unrepaired-text",
                ])
                .output()
                .unwrap();
            assert!(!text.status.success());
            assert!(
                String::from_utf8_lossy(&text.stderr)
                    .lines()
                    .last()
                    .unwrap()
                    .starts_with("Next: awr work edit "),
                "{missing}: {}",
                String::from_utf8_lossy(&text.stderr)
            );
            let action = saved["activation"]["next_action"].as_str().unwrap();
            assert!(
                action.starts_with("awr work edit "),
                "{missing}: recovery repeats activation without repairing its cause: {action}"
            );
            validate_action(&json!({"details":{"next_action":action}}));
            // These are caller-supplied business facts, not invented recovery defaults.
            let mut command = action.to_owned();
            for (name, value) in [
                ("AWR_NEXT_ACTION", "Read the note"),
                ("AWR_ACCEPTANCE", "The note is reviewed"),
                ("AWR_GOAL", "G"),
            ] {
                command = command.replace(
                    &format!("\"${{{name}:?}}\""),
                    &awr_core::cli_next_action(&[value]),
                );
            }
            assert!(!command.contains("${"), "{command}");
            let mut edit = argv(&command);
            let p = f.ok(&edit[1..].iter().map(String::as_str).collect::<Vec<_>>());
            if missing == "unknown_goal" {
                assert_eq!(
                    p["review"]["before"]["goal"],
                    json!(["ABSENT"]),
                    "goal repair must retain the existing goal links in its review"
                );
            }
            if missing == "goal" {
                assert_eq!(p["review"]["before"]["goal"], json!([]));
            }
            edit.extend([
                "--accept".into(),
                "--source-fingerprint".into(),
                p["change"]["source_fingerprint"].as_str().unwrap().into(),
                "--expected-preview".into(),
                p["preview_fingerprint"].as_str().unwrap().into(),
                "--expected-revision".into(),
                p["project_revision"].to_string(),
            ]);
            f.ok(&edit[1..].iter().map(String::as_str).collect::<Vec<_>>());
            let activate = [
                "work",
                "activate",
                key,
                "--request-key",
                "activate-repaired",
            ];
            let p = f.ok(&activate);
            let revision = p["project_revision"].to_string();
            let mut accept = activate.to_vec();
            accept.extend([
                "--accept",
                "--source-fingerprint",
                p["source_fingerprint"].as_str().unwrap(),
                "--expected-preview",
                p["preview"]["fingerprint"].as_str().unwrap(),
                "--expected-revision",
                &revision,
            ]);
            f.ok(&accept);
            assert!(f.session(key, true)["claim"]["id"].is_string(), "{missing}");
        }
    }

    #[test]
    fn rework_f4_direct_activation_refusal_has_repair_in_text_and_json() {
        let f = Fixture::new();
        let ledger = fs::read_to_string(f.0.join("work.yaml"))
            .unwrap()
            .replace("  next_action: Read the draft\n", "");
        f.write("work.yaml", &ledger);
        f.ok(&["source", "reindex"]);
        let error = f.error(
            &["work", "activate", "D", "--request-key", "repair-direct"],
            "RuleViolation",
        );
        assert!(
            error["details"]["next_action"]
                .as_str()
                .unwrap()
                .starts_with("awr work edit "),
            "{error}"
        );
        validate_action(&error);
        let text = Command::new(env!("CARGO_BIN_EXE_awr"))
            .env("AWR_LOCAL_WEB", "0")
            .args([
                "--project",
                f.0.to_str().unwrap(),
                "work",
                "activate",
                "D",
                "--request-key",
                "repair-direct-text",
            ])
            .output()
            .unwrap();
        assert!(!text.status.success());
        assert!(
            String::from_utf8_lossy(&text.stderr)
                .lines()
                .last()
                .unwrap()
                .starts_with("Next: awr work edit "),
            "{}",
            String::from_utf8_lossy(&text.stderr)
        );
    }

    #[test]
    fn rework_f4_planning_repairs_preserve_active_work_and_source() {
        let f = Fixture::new();
        let before = fs::read(f.0.join("work.yaml")).unwrap();
        for fields in [["--acceptance", "The note is reviewed"], ["--goal", "G"]] {
            f.error(
                &[
                    "work",
                    "edit",
                    "W",
                    "--request-key",
                    "active-repair",
                    "--actor",
                    "fixture",
                    "--reason",
                    "Review missing planning facts",
                    fields[0],
                    fields[1],
                ],
                "RuleViolation",
            );
            assert_eq!(fs::read(f.0.join("work.yaml")).unwrap(), before);
        }
    }

    #[test]
    fn rework_f3_activation_receipt_replays_after_source_work_is_removed() {
        let f = Fixture::new();
        let args = [
            "work",
            "activate",
            "D",
            "--request-key",
            "retained-activation",
        ];
        let preview = f.ok(&args);
        let rev = preview["project_revision"].to_string();
        let mut accept = args.to_vec();
        accept.extend([
            "--accept",
            "--source-fingerprint",
            preview["source_fingerprint"].as_str().unwrap(),
            "--expected-preview",
            preview["preview"]["fingerprint"].as_str().unwrap(),
            "--expected-revision",
            &rev,
        ]);
        f.ok(&accept);
        let ledger = fs::read_to_string(f.0.join("work.yaml")).unwrap();
        f.write("work.yaml", ledger.split("- id: D\n").next().unwrap());
        f.ok(&["source", "reindex"]);
        let before = fs::read(f.0.join("work.yaml")).unwrap();
        assert_eq!(f.ok(&accept)["already_recorded"], true);
        assert_eq!(fs::read(f.0.join("work.yaml")).unwrap(), before);
    }

    #[test]
    fn acceptance_1_changed_source_requires_repreview_and_preserves_bytes() {
        let f = Fixture::new();
        f.create_input("G");
        let p = f.preview();
        let text = format!(
            "{}\nAdditional authored rule.\n",
            fs::read_to_string(f.0.join("rules.md")).unwrap()
        );
        f.write("rules.md", &text);
        let before = fs::read(f.0.join("work.yaml")).unwrap();
        let e = f.error(
            &f.accept_args(&p, &p["project_revision"].to_string()),
            "SourceStale",
        );
        assert_eq!(e["details"]["preview_still_valid"], false);
        let fresh = f.follow(&e);
        assert_eq!(fresh["status"], "preview");
        assert_ne!(fresh["preview"]["fingerprint"], p["preview"]["fingerprint"]);
        assert_eq!(fs::read(f.0.join("work.yaml")).unwrap(), before);
    }

    #[test]
    fn acceptance_2_work_create_flags_activate_in_two_calls_and_claim() {
        let f = Fixture::new();
        let args = [
            "work",
            "create",
            "--title",
            "New note",
            "--request-key",
            "flags",
            "--acceptance",
            "The note is reviewed",
            "--goal",
            "G",
            "--next-action",
            "Read the note",
            "--activate",
        ];
        let p = f.ok(&args);
        let mut accept = args.to_vec();
        let rev = p["project_revision"].to_string();
        accept.extend([
            "--accept",
            "--expected-preview",
            p["preview"]["fingerprint"].as_str().unwrap(),
            "--expected-revision",
            &rev,
        ]);
        let saved = f.ok(&accept);
        assert_eq!(saved["draft"], false);
        let key = saved["external_key"].as_str().unwrap();
        assert!(f.session(key, true)["claim"]["id"].is_string());
        assert_eq!(f.ok(&accept)["work_id"], saved["work_id"]);
    }

    #[test]
    fn acceptance_2_missing_goal_keeps_created_draft_with_activation_reason() {
        let f = Fixture::new();
        f.create_input("ABSENT");
        let p = f.ok(&["work", "create", "--input", "create.json", "--activate"]);
        let rev = p["project_revision"].to_string();
        let mut args = f.accept_args(&p, &rev);
        args.push("--activate");
        let saved = f.ok(&args);
        assert_eq!(saved["status"], "completed");
        assert_eq!(saved["draft"], true);
        assert_eq!(saved["activation"]["ok"], false);
        assert!(
            !saved["activation"]["error"]["message"]
                .as_str()
                .unwrap()
                .is_empty()
        );
        let key = saved["external_key"].as_str().unwrap();
        assert_eq!(f.ok(&["work", "show", key])["work"]["status"], "draft");
        assert_eq!(
            f.ok(&["work", "create-status", "--request-key", "new-note"])["activation"],
            saved["activation"]
        );
    }

    #[test]
    fn acceptance_3_draft_refusal_suggests_parseable_activate_then_accept_and_claim() {
        let f = Fixture::new();
        let e = f.claim_error("D", "DependencyBlocked");
        assert!(
            e["details"]["next_action"]
                .as_str()
                .unwrap()
                .starts_with("awr work activate D ")
        );
        let p = f.follow(&e);
        assert_eq!(p["status"], "preview");
        let mut args = validate_action(&e);
        let rev = p["project_revision"].to_string();
        args.extend([
            "--source-fingerprint".into(),
            p["source_fingerprint"].as_str().unwrap().into(),
            "--accept".into(),
            "--expected-preview".into(),
            p["preview"]["fingerprint"].as_str().unwrap().into(),
            "--expected-revision".into(),
            rev,
        ]);
        f.ok(&args[1..].iter().map(String::as_str).collect::<Vec<_>>());
        f.session("D", true);
    }

    #[test]
    fn acceptance_3_draft_refusal_never_has_an_empty_next_action() {
        let f = Fixture::new();
        validate_action(&f.claim_error("D", "DependencyBlocked"));
    }

    #[test]
    fn acceptance_4_shape_errors_list_missing_fields_and_bounded_skeleton() {
        let f = Fixture::new();
        for command in [["batch", "change"], ["host", "preview"]] {
            f.write(
                "bad.json",
                &json!({"version":1,"request_key":"shape","reason":"Review","change":{}})
                    .to_string(),
            );
            let e = f.error(
                &[command[0], command[1], "--input", "bad.json"],
                "InvalidInput",
            );
            assert_eq!(e["details"]["missing"], json!(["actor"]));
            let expected = e["details"]["expected"].as_str().unwrap();
            assert!(expected.len() <= 600);
            assert!(serde_json::from_str::<Value>(expected).unwrap()["actor"].is_object());
            validate_action(&e);
        }
    }

    #[test]
    fn acceptance_4_wrong_types_and_non_objects_are_not_fixed_text_errors() {
        let f = Fixture::new();
        for text in [
            "[]",
            "null",
            r#"{"version":1,"request_key":3,"actor":{},"reason":"x","change":{}}"#,
            "{",
        ] {
            f.write("bad.json", text);
            for command in [["batch", "change"], ["host", "preview"]] {
                let e = f.error(
                    &[command[0], command[1], "--input", "bad.json"],
                    "InvalidInput",
                );
                assert!(e["details"]["missing"].is_array());
                assert!(e["details"]["expected"].is_string());
                validate_action(&e);
            }
        }
    }

    #[test]
    fn acceptance_5_default_budget_raises_to_7653_required_tokens() {
        let f = Fixture::new();
        f.pad_context(7653);
        let c = f.ok(&[
            "context",
            "compile",
            "--work",
            "W",
            "--detached",
            "--after-revision",
            &f.revision(),
        ]);
        assert_eq!(
            c["budget"],
            json!({"applied":7653,"basis":"raised_to_required"})
        );
        assert_eq!(c["work_context"]["required_tokens"], 7653);
    }

    #[test]
    fn acceptance_5_explicit_5000_stays_strict_with_executable_recovery() {
        let f = Fixture::new();
        f.pad_context(7653);
        let e = f.error(
            &[
                "context",
                "compile",
                "--work",
                "W",
                "--detached",
                "--after-revision",
                &f.revision(),
                "--budget",
                "5000",
            ],
            "BudgetExceeded",
        );
        assert_eq!(e["details"]["budget"], 5000);
        assert_eq!(f.follow(&e)["work_context"]["required_tokens"], 7653);
    }

    #[test]
    fn default_budget_above_10000_requires_an_explicit_choice() {
        let f = Fixture::new();
        f.write("rules.md", &format!("{RULE}{}", " datum".repeat(12000)));
        let e = f.error(
            &["context", "compile", "--work", "W", "--detached"],
            "BudgetExceeded",
        );
        assert!(e["details"]["required"].as_u64().unwrap() > 10000);
        validate_action(&e);
    }

    #[test]
    fn acceptance_6_next_action_table_uses_production_parser() {
        let f = Fixture::new();
        f.create_input("G");
        let p = f.preview();
        let holder = f.session("W", true);
        let conflict = f.claim_error("W", "ClaimConflict");
        assert_eq!(
            conflict["details"]["holding_session"],
            holder["session"]["id"]
        );
        assert_eq!(
            conflict["details"]["expires_at"],
            holder["claim"]["expires_at"]
        );
        f.write("bad.json", "{}");
        let cases = [
            (
                "revision",
                f.error(
                    &f.accept_args(&p, &p["project_revision"].to_string()),
                    "RevisionConflict",
                ),
            ),
            ("draft", f.claim_error("D", "DependencyBlocked")),
            ("claim", conflict),
            (
                "batch shape",
                f.error(&["batch", "change", "--input", "bad.json"], "InvalidInput"),
            ),
            (
                "host shape",
                f.error(&["host", "preview", "--input", "bad.json"], "InvalidInput"),
            ),
            (
                "budget",
                f.error(
                    &["context", "compile", "--work", "W", "--budget", "1"],
                    "BudgetExceeded",
                ),
            ),
            (
                "unsupported",
                f.error(&["fixture.unsupported"], "Unsupported"),
            ),
        ];
        for (name, error) in cases {
            assert!(!error.is_null(), "{name}");
            validate_action(&error);
        }
    }

    #[test]
    fn acceptance_6_parser_rejects_a_misspelled_suggested_parameter() {
        let f = Fixture::new();
        let e = f.claim_error("D", "DependencyBlocked");
        let original = e["details"]["next_action"].as_str().unwrap();
        validate_action(&e);
        let mutated = original.replace("--request-key", "--request-kee");
        assert_ne!(mutated, original);
        assert!(Cli::try_parse_from(argv(&mutated)).is_err());
        let mut invalid_suggestion = e.clone();
        invalid_suggestion["details"]["next_action"] = json!(mutated);
        assert!(std::panic::catch_unwind(|| validate_action(&invalid_suggestion)).is_err());
    }

    #[test]
    fn request_key_aliases_and_help_cover_create_and_batch_recovery() {
        let f = Fixture::new();
        for command in [
            vec!["work", "create-status"],
            vec!["work", "create-recover"],
            vec!["batch", "status"],
            vec!["batch", "recover"],
        ] {
            let mut a = vec!["awr"];
            a.extend(command.clone());
            a.extend(["--request-key", "request"]);
            if command.last().unwrap().ends_with("recover") {
                a.extend(["--expected-revision", "1"]);
            }
            Cli::try_parse_from(a).unwrap();
            let o = Command::new(env!("CARGO_BIN_EXE_awr"))
                .env("AWR_LOCAL_WEB", "0")
                .args(&command)
                .arg("--help")
                .output()
                .unwrap();
            assert!(String::from_utf8_lossy(&o.stdout).contains("--request-key"));
        }
        assert_eq!(
            f.ok(&["work", "create-status", "--request-key", "absent"])["found"],
            false
        );
        assert_eq!(
            f.ok(&["batch", "status", "--request-key", "absent"])["found"],
            false
        );
    }

    #[test]
    fn friction_rows_1_through_12_remain_recoverable() {
        let f = Fixture::new();
        f.create_input("G");
        let p = f.preview(); // 1: initial preview.
        f.session("W", false);
        let conflict = f.error(
            &f.accept_args(&p, &p["project_revision"].to_string()),
            "RevisionConflict",
        ); // 2.
        assert_eq!(
            f.ok(&["work", "create-status", "--request-key", "new-note"])["found"],
            false
        ); // 3.
        let scan = f.ok(&["source", "scan"]);
        assert!(scan["ok"].as_bool().unwrap_or(true)); // 4.
        let saved = f.follow(&conflict); // 5: no repreview required.
        let key = saved["external_key"].as_str().unwrap();
        let draft = f.claim_error(key, "DependencyBlocked");
        validate_action(&draft); // 6.
        f.write("bad.json", "{}");
        validate_action(&f.error(&["batch", "change", "--input", "bad.json"], "InvalidInput")); // 7.
        assert_eq!(f.follow(&draft)["status"], "preview"); // 8: activation without reading code.
        let session = f.session(key, false); // 9.
        let sid = session["session"]["id"].as_str().unwrap();
        let context = f.ok(&["context", "compile", "--work", key, "--session", sid]); // 10.
        assert_eq!(context["ok"], true);
        let context = f.ok(&[
            "context",
            "compile",
            "--work",
            key,
            "--session",
            sid,
            "--budget",
            "9000",
        ]); // 11.
        let hash = context["work_context"]["context_hash"].as_str().unwrap();
        let cp = f.ok(&[
            "session",
            "checkpoint",
            "--session",
            sid,
            "--work",
            key,
            "--context-hash",
            hash,
            "--digest",
            "Reviewed draft context",
            "--next-action",
            "Activate the draft",
            "--expected-project-revision",
            &f.revision(),
        ]); // 12.
        assert!(cp["checkpoint"]["id"].is_string());
    }

    #[test]
    fn work_create_flags_match_every_supported_json_field_and_reject_mixing() {
        let f = Fixture::new();
        let spec = json!({"specification":"guide","item":"R","source_fingerprint":format!("sha256:{}","a".repeat(64))}).to_string();
        let fields = json!({"acceptance":["The note is reviewed","The author responds"],"goal":["G"],
            "depends_on":["W"],"paths":["docs/note.md","docs/review.md"],"tags":["review","writing"],
            "kind":"feature","priority":"P1","summary":"A note for review","next_action":"Read the note",
            "spec_refs":[serde_json::from_str::<Value>(&spec).unwrap()],"owner":"writer","milestone":"M"});
        let input =
            json!({"version":1,"request_key":"all-fields","title":"Full creation","fields":fields});
        f.write("all.json", &input.to_string());
        let flags = [
            "work",
            "create",
            "--title",
            "Full creation",
            "--request-key",
            "all-fields",
            "--acceptance",
            "The note is reviewed",
            "--acceptance",
            "The author responds",
            "--goal",
            "G",
            "--depends-on",
            "W",
            "--path",
            "docs/note.md",
            "--path",
            "docs/review.md",
            "--tag",
            "review",
            "--tag",
            "writing",
            "--kind",
            "feature",
            "--priority",
            "P1",
            "--summary",
            "A note for review",
            "--next-action",
            "Read the note",
            "--spec-ref",
            &spec,
            "--owner",
            "writer",
            "--milestone",
            "M",
        ];
        let cli = f.ok(&flags);
        let json = f.ok(&["work", "create", "--input", "all.json"]);
        assert_eq!(cli["preview"]["plan"]["request"]["fields"], fields);
        assert_eq!(
            cli["preview"]["fingerprint"],
            json["preview"]["fingerprint"]
        );
        assert!(
            Cli::try_parse_from([
                "awr", "work", "create", "--input", "all.json", "--goal", "G"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "awr",
                "work",
                "create",
                "--title",
                "x",
                "--request-key",
                "r",
                "--spec-ref",
                "{}"
            ])
            .is_err()
        );
    }

    #[test]
    fn wrong_preview_without_source_changes_remains_source_conflict() {
        let f = Fixture::new();
        f.create_input("G");
        let p = f.preview();
        let mut wrong = p.clone();
        wrong["preview"]["fingerprint"] = json!(format!("sha256:{}", "0".repeat(64)));
        let e = f.error(
            &f.accept_args(&wrong, &p["project_revision"].to_string()),
            "SourceConflict",
        );
        assert_eq!(e["details"]["preview_still_valid"], false);
        validate_action(&e);
        assert!(
            !fs::read_to_string(f.0.join("work.yaml"))
                .unwrap()
                .contains("Review a new note")
        );
    }

    #[test]
    fn activation_keeps_dependency_gates_and_replay_is_read_only() {
        let f = Fixture::new();
        f.create_input("G");
        let mut input: Value =
            serde_json::from_str(&fs::read_to_string(f.0.join("create.json")).unwrap()).unwrap();
        input["fields"]["depends_on"] = json!(["W"]);
        f.write("create.json", &input.to_string());
        let p = f.ok(&["work", "create", "--input", "create.json", "--activate"]);
        let rev = p["project_revision"].to_string();
        let mut args = f.accept_args(&p, &rev);
        args.push("--activate");
        let saved = f.ok(&args);
        assert_eq!(saved["draft"], true);
        assert_eq!(saved["activation"]["ok"], false);
        let before = fs::read(f.0.join("work.yaml")).unwrap();
        let revision = f.revision();
        let replay = f.ok(&args);
        assert_eq!(replay["activation"], saved["activation"]);
        assert_eq!(replay["source_write_performed"], false);
        assert_eq!(revision, f.revision());
        assert_eq!(before, fs::read(f.0.join("work.yaml")).unwrap());
        let mut changed = args.clone();
        changed.retain(|a| *a != "--activate");
        f.error(&changed, "SourceConflict");
    }

    #[test]
    fn claim_without_expiry_reports_holder_and_handoff_guidance() {
        let f = Fixture::new();
        let holder = f.ok(&[
            "session",
            "start",
            "--work",
            "W",
            "--agent",
            "holder",
            "--provider",
            "fixture",
            "--model",
            "fixture",
            "--claim",
            "--no-expiry",
            "--expected-revision",
            &f.revision(),
        ]);
        let e = f.claim_error("W", "ClaimConflict");
        assert_eq!(e["details"]["holding_session"], holder["session"]["id"]);
        assert!(e["details"]["expires_at"].is_null());
        validate_action(&e);
    }

    #[test]
    fn suggested_commands_quote_apostrophes_unicode_and_shell_metacharacters() {
        let f = Fixture::new();
        let title = "Review O'Brien's 文档 $(literal)";
        let p = f.ok(&[
            "work",
            "create",
            "--title",
            title,
            "--request-key",
            "quoted",
        ]);
        f.session("W", false);
        let e = f.error(
            &[
                "work",
                "create",
                "--title",
                title,
                "--request-key",
                "quoted",
                "--accept",
                "--expected-preview",
                p["preview"]["fingerprint"].as_str().unwrap(),
                "--expected-revision",
                &p["project_revision"].to_string(),
            ],
            "RevisionConflict",
        );
        let created = f.follow(&e);
        assert_eq!(
            f.ok(&["work", "show", created["external_key"].as_str().unwrap()])["work"]["title"],
            title
        );
    }

    fn realistic_create_input() -> Value {
        json!({"version":1,"request_key":"supplier-reconciliation",
            "title":"Review the monthly supplier reconciliation and retain the finance team's signed audit summary",
            "fields":{"goal":["G"],"acceptance":[
                "The retained reconciliation lists every supplier difference with its source invoice, reviewer and final resolution.",
                "The finance reviewer can reproduce each total from the retained invoices and approve the monthly summary without gaps."
            ],"next_action":"Review the supplier invoices against the monthly reconciliation"}})
    }

    fn creation_text(f: &Fixture, args: &[&str]) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_awr"))
            .env("AWR_LOCAL_WEB", "0")
            .args(["--project", f.0.to_str().unwrap()])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.stderr.is_empty());
        String::from_utf8(out.stdout).unwrap()
    }

    fn preview_accept_command(text: &str) -> Result<Vec<String>, String> {
        if text.len() > 1024 {
            return Err(format!("preview is {} bytes", text.len()));
        }
        let next = text
            .lines()
            .last()
            .and_then(|line| line.strip_prefix("Next: "))
            .ok_or("missing final Next command")?;
        let args = argv(next);
        if !args.windows(2).any(|pair| {
            pair[0] == "--expected-preview" && pair[1].starts_with("sha256:") && pair[1].len() == 71
        }) || !args.iter().any(|arg| arg == "--accept")
        {
            return Err("missing reviewed fingerprint or acceptance flag".into());
        }
        Cli::try_parse_from(&args).map_err(|error| error.to_string())?;
        Ok(args)
    }

    fn follow_preview(f: &Fixture, text: &str) -> Value {
        let args = preview_accept_command(text).unwrap();
        f.ok(&args[1..].iter().map(String::as_str).collect::<Vec<_>>())
    }

    #[test]
    fn work_create_preview_1_text_is_short_parseable_and_creates_with_activation_intent() {
        for activate in [false, true] {
            let f = Fixture::new();
            let input = realistic_create_input();
            f.write("create.json", &input.to_string());
            let mut args = vec!["work", "create", "--input", "create.json"];
            if activate {
                args.push("--activate");
            }
            let text = creation_text(&f, &args);
            assert!(text.contains(input["title"].as_str().unwrap()), "{text}");
            assert!(text.contains("Goals: [\"G\"]"), "{text}");
            assert!(text.contains("Acceptance: 2"), "{text}");
            assert!(
                text.contains(&format!("Activation requested: {activate}")),
                "{text}"
            );
            let saved = follow_preview(&f, &text);
            assert_eq!(saved["status"], "completed");
            assert_eq!(saved["draft"], !activate);
            let work = f.ok(&["work", "show", saved["external_key"].as_str().unwrap()]);
            assert_eq!(work["work"]["title"], input["title"]);
        }
    }

    #[test]
    fn work_create_preview_1_long_flags_use_a_private_literal_input_and_short_flags_quote_safely() {
        for long in [false, true] {
            let f = Fixture::new();
            let title = if long {
                realistic_create_input()["title"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            } else {
                "Review O'Brien's 文档 $(literal)".into()
            };
            let mut args = vec![
                "work",
                "create",
                "--title",
                &title,
                "--request-key",
                "literal",
            ];
            if long {
                args.extend(["--goal", "G", "--acceptance", "The reviewed supplier differences are retained with the original invoices and the final finance review decision.",
                    "--next-action", "Read the reviewed supplier invoices before preparing the reconciliation", "--activate"]);
            }
            let text = creation_text(&f, &args);
            let command = preview_accept_command(&text).unwrap();
            if long {
                let index = command.iter().position(|arg| arg == "--input").unwrap();
                let path = f.0.join(&command[index + 1]);
                assert!(path.starts_with(f.0.join(".awr/mutations")));
                let retained: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                assert_eq!(retained["title"], title);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o077, 0);
                }
            } else {
                assert!(!command.iter().any(|arg| arg == "--input"));
            }
            assert_eq!(follow_preview(&f, &text)["status"], "completed");
        }
    }

    #[test]
    fn work_create_preview_1_missing_fingerprint_and_invalid_next_command_are_rejected() {
        let f = Fixture::new();
        f.create_input("G");
        let text = creation_text(&f, &["work", "create", "--input", "create.json"]);
        preview_accept_command(&text).unwrap();
        let args = argv(text.lines().last().unwrap().strip_prefix("Next: ").unwrap());
        let fingerprint = &args[args
            .iter()
            .position(|arg| arg == "--expected-preview")
            .unwrap()
            + 1];
        assert!(preview_accept_command(&text.replace(fingerprint, "")).is_err());
        assert!(
            preview_accept_command(&text.replace("--expected-revision", "--invalid-revision"))
                .is_err()
        );
    }

    #[test]
    fn work_create_preview_2_json_fits_three_kib_and_contains_every_acceptance_input() {
        let f = Fixture::new();
        let input = realistic_create_input();
        f.write("create.json", &input.to_string());
        let out = f.run(&["work", "create", "--input", "create.json", "--activate"]);
        assert!(out.status.success());
        assert!(
            out.stdout.len() <= 3072,
            "preview is {} bytes",
            out.stdout.len()
        );
        let preview: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(preview["status"], "preview");
        assert_eq!(preview["requires_accept"], true);
        assert_eq!(preview["activation_requested"], true);
        assert!(preview["project_revision"].is_u64());
        assert!(
            preview["preview"]["plan"]["external_key"]
                .as_str()
                .unwrap()
                .starts_with("WORK-")
        );
        let mut expected = input;
        expected["source_id"] = Value::Null;
        assert_eq!(preview["preview"]["plan"]["request"], expected);
        let command = format!("Next: {}", preview["next_action"].as_str().unwrap());
        let saved = follow_preview(&f, &command);
        assert_eq!(
            saved["preview_fingerprint"],
            preview["preview"]["fingerprint"]
        );
        assert_eq!(saved["draft"], false);
    }

    #[test]
    fn work_create_preview_2_default_omits_source_arrays_and_complete_texts_even_as_sources_grow() {
        let f = Fixture::new();
        f.create_input("G");
        f.write(
            "rules.md",
            &format!("{RULE}{}", " Review retained invoices.".repeat(1000)),
        );
        let preview = f.preview();
        let p = &preview["preview"];
        assert_eq!(p["plan"]["observations"], json!({"count":2,"omitted":true}));
        for field in ["before_text", "after_text", "new_record"] {
            assert!(
                p[field]["bytes"].as_u64().unwrap() > 0,
                "{field}: {}",
                p[field]
            );
            assert_eq!(p[field]["omitted"], true);
            assert!(!p[field].is_string());
        }
        assert!(p["plan"].get("manifest_fingerprint").is_none());
        assert!(p["plan"].get("source").is_none());
        assert!(
            !serde_json::to_string(p)
                .unwrap()
                .contains("Review retained invoices.")
        );
    }

    #[test]
    fn work_create_preview_3_compact_and_runtime_fingerprints_match_and_wrong_fingerprints_refuse()
    {
        let f = Fixture::new();
        f.create_input("G");
        let input = serde_json::from_slice(&fs::read(f.0.join("create.json")).unwrap()).unwrap();
        let mut store = awr_store::Store::open_existing(&f.0.join(".awr/state.db")).unwrap();
        let baseline = awr_runtime::create_work(&mut store, &f.0, input, false, None, None)
            .unwrap()
            .value;
        drop(store);
        let mut plan = baseline["preview"]["plan"].clone();
        plan.as_object_mut().unwrap().remove("project_revision");
        let original_fingerprint = awr_source::fingerprint(&serde_json::to_vec(&plan).unwrap());
        let mut compact = f.preview();
        assert_eq!(compact["preview"]["fingerprint"], original_fingerprint);
        compact["preview"]["fingerprint"] = json!(format!("sha256:{}", "0".repeat(64)));
        let error = f.error(
            &f.accept_args(&compact, &compact["project_revision"].to_string()),
            "SourceConflict",
        );
        assert_eq!(error["details"]["preview_still_valid"], false);
        let saved = f.ok(&f.accept_args(&baseline, &baseline["project_revision"].to_string()));
        assert_eq!(saved["preview_fingerprint"], original_fingerprint);
    }

    #[test]
    fn work_create_preview_3_compact_source_change_refuses_but_revision_only_retries() {
        for source_change in [false, true] {
            let f = Fixture::new();
            f.create_input("G");
            let preview = f.preview();
            if source_change {
                f.write("rules.md", &format!("{RULE}\nKeep the signed copy too.\n"));
            } else {
                f.session("W", false);
            }
            let error = f.error(
                &f.accept_args(&preview, &preview["project_revision"].to_string()),
                if source_change {
                    "SourceStale"
                } else {
                    "RevisionConflict"
                },
            );
            assert_eq!(error["details"]["preview_still_valid"], !source_change);
            let retried = f.follow(&error);
            assert_eq!(
                retried["status"],
                if source_change {
                    "preview"
                } else {
                    "completed"
                }
            );
            if source_change {
                assert_ne!(
                    retried["preview"]["fingerprint"],
                    preview["preview"]["fingerprint"]
                );
            } else {
                assert_eq!(
                    retried["preview_fingerprint"],
                    preview["preview"]["fingerprint"]
                );
            }
        }
    }

    fn contains_original_payload(full: &Value, original: &Value) -> bool {
        match original {
            Value::Object(fields) => fields.iter().all(|(key, value)| {
                full.get(key)
                    .is_some_and(|actual| contains_original_payload(actual, value))
            }),
            _ => full == original,
        }
    }

    #[test]
    fn work_create_preview_4_full_preserves_original_runtime_payload_and_detects_missing_fields() {
        let f = Fixture::new();
        f.create_input("G");
        let input = serde_json::from_slice(&fs::read(f.0.join("create.json")).unwrap()).unwrap();
        let mut store = awr_store::Store::open_existing(&f.0.join(".awr/state.db")).unwrap();
        let original = awr_runtime::create_work_with_activation(
            &mut store, &f.0, input, false, None, None, true,
        )
        .unwrap()
        .value;
        drop(store);
        let full = f.ok(&[
            "work",
            "create",
            "--input",
            "create.json",
            "--activate",
            "--full",
        ]);
        assert!(contains_original_payload(&full, &original));
        assert!(full["preview"]["before_text"].is_string());
        assert!(full["preview"]["plan"]["observations"].is_array());
        for key in original["preview"]["plan"].as_object().unwrap().keys() {
            let mut missing = full.clone();
            missing["preview"]["plan"]
                .as_object_mut()
                .unwrap()
                .remove(key);
            assert!(
                !contains_original_payload(&missing, &original),
                "missing {key} was accepted"
            );
        }
    }

    #[test]
    fn work_create_preview_4_full_is_preview_only_and_cannot_be_used_for_acceptance() {
        let out = Cli::try_parse_from([
            "awr",
            "--json",
            "work",
            "create",
            "--title",
            "Review",
            "--request-key",
            "full",
            "--full",
        ]);
        assert!(out.is_ok());
        assert!(
            Cli::try_parse_from([
                "awr",
                "work",
                "create",
                "--title",
                "Review",
                "--request-key",
                "full",
                "--full",
                "--accept"
            ])
            .is_err()
        );
    }

    fn creation_path_fits(calls: &[usize]) -> bool {
        calls.len() <= 4 && calls.iter().sum::<usize>() <= 6144
    }

    fn text_creation_path() -> (Fixture, Vec<usize>) {
        let f = Fixture::new();
        f.write("create.json", &realistic_create_input().to_string());
        let preview = creation_text(
            &f,
            &["work", "create", "--input", "create.json", "--activate"],
        );
        let key = preview
            .lines()
            .find_map(|line| line.strip_prefix("Work key: "))
            .unwrap();
        let accept = preview_accept_command(&preview).unwrap();
        let created = creation_text(
            &f,
            &accept[1..].iter().map(String::as_str).collect::<Vec<_>>(),
        );
        // Inspect bookkeeping directly; every CLI invocation in the path is measured.
        let store = awr_store::Store::open_readonly(&f.0.join(".awr/state.db")).unwrap();
        let revision = store
            .project_by_root(&f.0.canonicalize().unwrap())
            .unwrap()
            .project_revision
            .to_string();
        drop(store);
        let started = creation_text(
            &f,
            &[
                "session",
                "start",
                "--work",
                key,
                "--agent",
                "author",
                "--provider",
                "fixture",
                "--model",
                "local",
                "--claim",
                "--ttl-ms",
                "60000",
                "--expected-revision",
                &revision,
            ],
        );
        let session = started
            .lines()
            .find_map(|line| line.strip_prefix("Session: "))
            .unwrap();
        let prepared = creation_text(
            &f,
            &[
                "work",
                "prepare",
                key,
                "--session",
                session,
                "--response-view",
                "action",
            ],
        );
        assert!(
            prepared.contains(
                realistic_create_input()["fields"]["acceptance"][0]
                    .as_str()
                    .unwrap()
            )
        );
        let calls = vec![preview.len(), created.len(), started.len(), prepared.len()];
        println!(
            "Creation path: {} calls, {} bytes; steps {calls:?}",
            calls.len(),
            calls.iter().sum::<usize>()
        );
        assert_ne!(f.ok(&["work", "show", key])["work"]["status"], "draft");
        assert!(
            f.ok(&["session", "show", session])["claims"]
                .as_array()
                .unwrap()
                .iter()
                .any(|claim| claim["status"] == "active")
        );
        (f, calls)
    }

    #[test]
    fn work_create_preview_5_text_path_starts_real_work_in_four_calls_within_six_kib() {
        let (_f, calls) = text_creation_path();
        assert_eq!(calls.len(), 4);
        assert!(creation_path_fits(&calls), "{calls:?}");
    }

    #[test]
    fn work_create_preview_5_extra_call_and_relaxed_byte_budget_are_rejected() {
        let (f, mut calls) = text_creation_path();
        calls.push(creation_text(&f, &["status"]).len());
        assert!(!creation_path_fits(&calls));
        assert!(creation_path_fits(&[6144, 0, 0, 0]));
        assert!(!creation_path_fits(&[6145, 0, 0, 0]));
        assert!(!creation_path_fits(&[0, 0, 0, 0, 0]));
    }

    #[test]
    fn text_error_ends_with_the_same_next_action_as_json() {
        let f = Fixture::new();
        let e = f.claim_error("D", "DependencyBlocked");
        let o = Command::new(env!("CARGO_BIN_EXE_awr"))
            .env("AWR_LOCAL_WEB", "0")
            .args([
                "--project",
                f.0.to_str().unwrap(),
                "session",
                "start",
                "--work",
                "D",
                "--agent",
                "other",
                "--provider",
                "fixture",
                "--model",
                "fixture",
                "--claim",
                "--expected-revision",
                &f.revision(),
            ])
            .output()
            .unwrap();
        assert!(!o.status.success());
        let text = String::from_utf8_lossy(&o.stderr);
        let next = text.lines().last().unwrap().strip_prefix("Next: ").unwrap();
        Cli::try_parse_from(argv(next)).unwrap();
        assert!(next.starts_with("awr work activate D --request-key activate-"));
        validate_action(&e);
    }
}
