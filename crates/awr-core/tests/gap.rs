//! Positive and negative examples for gap requests, the fold of recorded gap events and the rule
//! that decides whether a gap may be closed.
use awr_core::*;
use serde_json::{Value, json};

fn request(edit: impl FnOnce(&mut Value)) -> Result<GapRequest> {
    let mut value = json!({"work": "W", "agent": "claude-run", "expected_revision": 3,
        "request_key": "k1", "gap_key": "g1", "action": "open", "route": "acceptance",
        "resolution": "fact_lookup", "subject": "The second criterion does not say what happens offline"});
    edit(&mut value);
    let parsed: GapRequest = serde_json::from_value(value)?;
    parsed.validate()?;
    Ok(parsed)
}

fn remove(value: &mut Value, field: &str) {
    value.as_object_mut().unwrap().remove(field);
}

#[test]
fn opening_a_gap_needs_a_route_a_resolution_a_subject_and_the_anchor_of_that_resolution() {
    assert!(request(|_| {}).is_ok());
    for route in ["intent", "acceptance", "design", "plan", "work_definition"] {
        assert!(request(|v| v["route"] = json!(route)).is_ok(), "{route}");
    }
    assert!(request(|v| v["route"] = json!("code")).is_err());
    assert!(request(|v| remove(v, "route")).is_err());
    assert!(request(|v| remove(v, "resolution")).is_err());
    assert!(request(|v| remove(v, "subject")).is_err());
    assert!(request(|v| v["subject"] = json!(" ")).is_err());
    // An experiment names the operation key it will run under.
    let experiment = |v: &mut Value| {
        v["resolution"] = json!("feasibility_experiment");
        v["experiment"] = json!("probe-offline-save");
    };
    assert!(request(experiment).is_ok());
    assert!(
        request(|v| {
            experiment(v);
            v["authority"] = json!(["lead"]);
        })
        .is_err(),
        "an experiment names no authority"
    );
    assert!(
        request(|v| {
            experiment(v);
            remove(v, "experiment");
        })
        .is_err()
    );
    assert!(
        request(|v| v["experiment"] = json!("probe")).is_err(),
        "a lookup has no experiment"
    );
    // An authorized choice may name its authority; without one the policy's supervisors decide.
    let choice = |v: &mut Value| v["resolution"] = json!("authorized_choice");
    assert!(request(choice).is_ok());
    assert!(
        request(|v| {
            choice(v);
            v["authority"] = json!(["lead", "owner"]);
        })
        .is_ok()
    );
    assert!(
        request(|v| {
            choice(v);
            v["authority"] = json!([" lead"]);
        })
        .is_err()
    );
    assert!(
        request(|v| {
            choice(v);
            v["authority"] = json!((0..33).map(|i| format!("a{i}")).collect::<Vec<_>>());
        })
        .is_err()
    );
    // An environment wait says what it waits for.
    let wait = |v: &mut Value| v["resolution"] = json!("environment_wait");
    assert!(
        request(|v| {
            wait(v);
            v["waiting_for"] = json!("The staging database is restored");
        })
        .is_ok()
    );
    assert!(request(wait).is_err());
    assert!(
        request(|v| {
            wait(v);
            v["waiting_for"] = json!("The staging database is restored");
            v["experiment"] = json!("probe-offline-save");
        })
        .is_err(),
        "a wait runs no experiment"
    );
    // Opening says nothing about how a gap was closed.
    for field in ["outcome", "observed", "reason"] {
        assert!(request(|v| v[field] = json!("done")).is_err(), "{field}");
    }
    assert!(request(|v| v["sources"] = json!(["docs/offline.md"])).is_err());
}

#[test]
fn resolving_and_withdrawing_state_only_what_they_need() {
    let resolve = |v: &mut Value| {
        for field in ["route", "resolution", "subject"] {
            remove(v, field);
        }
        v["action"] = json!("resolve");
        v["outcome"] = json!("Offline saves are queued and replayed");
    };
    assert!(request(resolve).is_ok());
    assert!(
        request(|v| {
            resolve(v);
            remove(v, "outcome");
        })
        .is_err()
    );
    assert!(
        request(|v| {
            resolve(v);
            v["route"] = json!("plan");
        })
        .is_err()
    );
    assert!(
        request(|v| {
            resolve(v);
            v["sources"] = json!(["docs/offline.md", "decisions/offline.md"]);
            v["observed"] = json!("The staging database answers again");
        })
        .is_ok()
    );
    assert!(
        request(|v| {
            resolve(v);
            v["sources"] = json!([""]);
        })
        .is_err()
    );
    assert!(
        request(|v| {
            resolve(v);
            v["reason"] = json!("no longer needed");
        })
        .is_err()
    );
    let withdraw = |v: &mut Value| {
        for field in ["route", "resolution", "subject"] {
            remove(v, field);
        }
        v["action"] = json!("withdraw");
        v["reason"] = json!("The criterion was rewritten by the user");
    };
    assert!(request(withdraw).is_ok());
    assert!(
        request(|v| {
            withdraw(v);
            remove(v, "reason");
        })
        .is_err()
    );
    assert!(
        request(|v| {
            withdraw(v);
            v["outcome"] = json!("found");
        })
        .is_err()
    );
}

#[test]
fn a_request_names_one_decider_and_is_bounded_and_public() {
    assert!(
        request(|v| {
            remove(v, "agent");
            v["session"] = json!(Id::new());
        })
        .is_ok()
    );
    assert!(request(|v| v["session"] = json!(Id::new())).is_err());
    assert!(request(|v| remove(v, "agent")).is_err());
    assert!(request(|v| v["gap_key"] = json!("")).is_err());
    assert!(request(|v| v["gap_key"] = json!("x".repeat(257))).is_err());
    assert!(request(|v| v["request_key"] = json!(" ")).is_err());
    assert!(request(|v| v["subject"] = json!("password: hunter2hunter2")).is_err());
    assert!(serde_json::from_value::<GapRequest>(json!({"work": "W", "extra": 1})).is_err());
    // The key binds the request without its revision guard and its own spelling.
    let first = request(|_| {}).unwrap();
    let mut second = first.clone();
    second.request_key = "k2".into();
    second.expected_revision = 99;
    assert_eq!(first.input(), second.input());
    second.gap_key = "g2".into();
    assert_ne!(first.input(), second.input());
}

fn record(action: GapAction, key: &str, edit: impl FnOnce(&mut GapInput)) -> RecordedGap {
    let mut input = GapInput {
        work: "W".into(),
        gap_key: key.into(),
        action: Some(action),
        ..Default::default()
    };
    if action == GapAction::Open {
        input.route = Some(GapRoute::Acceptance);
        input.resolution = Some(GapResolution::FactLookup);
        input.subject = Some("The criterion is ambiguous".into());
    }
    edit(&mut input);
    RecordedGap {
        event: Id::new(),
        decider_agent: "claude-run".into(),
        input,
    }
}

#[test]
fn the_fold_keeps_every_gap_in_the_order_it_was_opened_and_never_rewrites_history() {
    let opened = record(GapAction::Open, "g1", |_| {});
    let other = record(GapAction::Open, "g2", |i| i.route = Some(GapRoute::Plan));
    let resolved = record(GapAction::Resolve, "g1", |i| {
        i.outcome = Some("Offline saves are queued".into());
        i.sources = vec!["docs/offline.md".into()];
    });
    let withdrawn = record(GapAction::Withdraw, "g2", |i| {
        i.reason = Some("Rewritten by the user".into())
    });
    let gaps = fold_gaps(&[opened.clone(), other.clone()]);
    assert_eq!(
        gaps.iter()
            .map(|g| (g.key.as_str(), g.state))
            .collect::<Vec<_>>(),
        [("g1", "open"), ("g2", "open")]
    );
    let gaps = fold_gaps(&[
        opened.clone(),
        other.clone(),
        resolved.clone(),
        withdrawn.clone(),
    ]);
    assert_eq!(
        gaps.iter()
            .map(|g| (g.key.as_str(), g.state))
            .collect::<Vec<_>>(),
        [("g1", "resolved"), ("g2", "withdrawn")]
    );
    assert_eq!(gaps[0].closed_event, Some(resolved.event));
    assert_eq!(gaps[0].opened_event, opened.event);
    assert_eq!(gaps[0].sources, ["docs/offline.md"]);
    assert_eq!(gaps[1].reason.as_deref(), Some("Rewritten by the user"));
    // A closed gap cannot be closed again, and a key cannot be opened twice.
    let again = record(GapAction::Withdraw, "g1", |i| {
        i.reason = Some("late".into())
    });
    let reopened = record(GapAction::Open, "g1", |i| {
        i.subject = Some("Another".into())
    });
    let gaps = fold_gaps(&[opened, resolved, again, reopened]);
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].state, "resolved");
    assert_eq!(gaps[0].subject, "The criterion is ambiguous");
    // Closing what was never opened records nothing.
    assert!(
        fold_gaps(&[record(GapAction::Resolve, "ghost", |i| i.outcome =
            Some("x".into()))])
        .is_empty()
    );
}

#[test]
fn an_open_gap_puts_in_question_only_the_judgments_that_read_its_layer() {
    let gap = |route: GapRoute, key: &str, open: bool| {
        let mut gaps = fold_gaps(&[record(GapAction::Open, key, |i| i.route = Some(route))]);
        if !open {
            gaps = fold_gaps(&[
                record(GapAction::Open, key, |i| i.route = Some(route)),
                record(GapAction::Withdraw, key, |i| {
                    i.reason = Some("no longer".into())
                }),
            ]);
        }
        gaps
    };
    let judgments = |route: GapRoute| -> Vec<GapJudgment> {
        suspensions(&gap(route, "g", true))
            .iter()
            .map(|s| s.judgment)
            .collect()
    };
    assert_eq!(judgments(GapRoute::Acceptance), [GapJudgment::Admission]);
    assert_eq!(
        judgments(GapRoute::WorkDefinition),
        [GapJudgment::Admission]
    );
    assert_eq!(judgments(GapRoute::Plan), [GapJudgment::Containment]);
    assert_eq!(
        judgments(GapRoute::Intent),
        [GapJudgment::Admission, GapJudgment::Containment]
    );
    assert!(
        judgments(GapRoute::Design).is_empty(),
        "AWR keeps no judgment on design"
    );
    // A closed gap puts nothing in question; several open gaps are all named.
    assert!(suspensions(&gap(GapRoute::Acceptance, "g", false)).is_empty());
    let both = fold_gaps(&[
        record(GapAction::Open, "a", |i| {
            i.route = Some(GapRoute::Acceptance)
        }),
        record(GapAction::Open, "b", |i| i.route = Some(GapRoute::Intent)),
        record(GapAction::Open, "c", |i| i.route = Some(GapRoute::Design)),
    ]);
    let found = suspensions(&both);
    assert_eq!(found[0].judgment, GapJudgment::Admission);
    assert_eq!(found[0].gaps, ["a", "b"]);
    assert_eq!(found[1].judgment, GapJudgment::Containment);
    assert_eq!(found[1].gaps, ["b"]);
    let status = GapStatus::of("W", both);
    assert_eq!(status.open, ["a", "b", "c"]);
}

fn gap_of(resolution: GapResolution, edit: impl FnOnce(&mut GapInput)) -> Gap {
    fold_gaps(&[record(GapAction::Open, "g1", |i| {
        i.resolution = Some(resolution);
        edit(i)
    })])
    .remove(0)
}

fn resolve(edit: impl FnOnce(&mut Value)) -> GapRequest {
    request(|v| {
        for field in ["route", "resolution", "subject"] {
            remove(v, field);
        }
        v["action"] = json!("resolve");
        v["outcome"] = json!("Concluded");
        edit(v)
    })
    .unwrap()
}

fn facts<'a>(ran: bool, decider: &'a str, supervisors: &'a [String]) -> ClosingFacts<'a> {
    ClosingFacts {
        experiment_ran: ran,
        decider,
        supervisors,
    }
}

fn refused(result: std::result::Result<(), (&'static str, String)>) -> &'static str {
    result.expect_err("the gap must not close").0
}

#[test]
fn each_kind_of_gap_is_closed_only_the_way_it_requires() {
    // A fact lookup names its sources.
    let lookup = gap_of(GapResolution::FactLookup, |_| {});
    assert_eq!(
        refused(check_closing(
            &lookup,
            &resolve(|_| {}),
            &facts(false, "a", &[])
        )),
        "gap_sources_missing"
    );
    assert!(
        check_closing(
            &lookup,
            &resolve(|v| v["sources"] = json!(["docs/offline.md"])),
            &facts(false, "a", &[])
        )
        .is_ok()
    );
    // An experiment is closed by the run, whatever else was repaired.
    let experiment = gap_of(GapResolution::FeasibilityExperiment, |i| {
        i.experiment = Some("probe-offline-save".into())
    });
    let code = refused(check_closing(
        &experiment,
        &resolve(|_| {}),
        &facts(false, "a", &[]),
    ));
    assert_eq!(code, "gap_experiment_not_run");
    let detail = check_closing(&experiment, &resolve(|_| {}), &facts(false, "a", &[]))
        .unwrap_err()
        .1;
    assert!(detail.contains("probe-offline-save") && detail.contains("repairing the plan"));
    assert!(check_closing(&experiment, &resolve(|_| {}), &facts(true, "a", &[])).is_ok());
    // A choice is made by those authorized: the named agents, else the policy's supervisors.
    let named = gap_of(GapResolution::AuthorizedChoice, |i| {
        i.authority = vec!["owner".into()]
    });
    let supervisors = vec!["lead".to_string()];
    assert_eq!(
        refused(check_closing(
            &named,
            &resolve(|_| {}),
            &facts(false, "lead", &supervisors)
        )),
        "gap_not_authorized",
        "named authority replaces the supervisors"
    );
    assert!(
        check_closing(
            &named,
            &resolve(|_| {}),
            &facts(false, "owner", &supervisors)
        )
        .is_ok()
    );
    let unnamed = gap_of(GapResolution::AuthorizedChoice, |_| {});
    assert!(
        check_closing(
            &unnamed,
            &resolve(|_| {}),
            &facts(false, "lead", &supervisors)
        )
        .is_ok()
    );
    assert_eq!(
        refused(check_closing(
            &unnamed,
            &resolve(|_| {}),
            &facts(false, "intern", &supervisors)
        )),
        "gap_not_authorized"
    );
    assert_eq!(
        refused(check_closing(
            &unnamed,
            &resolve(|_| {}),
            &facts(false, "lead", &[])
        )),
        "gap_not_authorized",
        "nobody is authorized when no one is named"
    );
    // An environment wait is closed by saying what was observed.
    let wait = gap_of(GapResolution::EnvironmentWait, |i| {
        i.waiting_for = Some("The staging database is restored".into())
    });
    assert_eq!(
        refused(check_closing(
            &wait,
            &resolve(|_| {}),
            &facts(false, "a", &[])
        )),
        "gap_observation_missing"
    );
    assert!(
        check_closing(
            &wait,
            &resolve(|v| v["observed"] = json!("It answers again")),
            &facts(false, "a", &[])
        )
        .is_ok()
    );
    // Withdrawing is a choice of the one who opened it, not a closing that needs proof.
    let withdraw = request(|v| {
        for field in ["route", "resolution", "subject"] {
            remove(v, field);
        }
        v["action"] = json!("withdraw");
        v["reason"] = json!("No longer relevant");
    })
    .unwrap();
    assert!(check_closing(&experiment, &withdraw, &facts(false, "a", &[])).is_ok());
}
