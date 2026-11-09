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
        let mut quoted = false;
        let mut escaped = false;
        let mut started = false;
        for c in command.chars() {
            if escaped {
                word.push(c);
                escaped = false;
                started = true;
            } else if c == '\\' && !quoted {
                escaped = true;
            } else if c == '\'' {
                quoted = !quoted;
                started = true;
            } else if c.is_whitespace() && !quoted {
                if started {
                    args.push(std::mem::take(&mut word));
                    started = false;
                }
            } else {
                word.push(c);
                started = true;
            }
        }
        assert!(!quoted && !escaped, "incomplete shell quoting: {command}");
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
