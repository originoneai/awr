//! What the Team entry gives for each of the five result layers of AWR, and which entries of the personal
//! CLI and MCP server it has no operation for. The layers are the ones frozen by the semantic contract
//! (work readiness, execution admission, context completeness, delivery observation, completion validity).
//! A request for an operation the Team entry does not have is refused as `Unsupported`; this report names
//! the gaps beforehand, so a host that works in both modes does not have to find them by failing.
use serde_json::{Value, json};

struct Gap {
    entry: &'static str,
    reason: &'static str,
}

struct Layer {
    id: &'static str,
    queries: &'static [&'static str],
    commands: &'static [&'static str],
    without_team_operation: &'static [Gap],
}

const LAYERS: &[Layer] = &[
    Layer {
        id: "work_readiness",
        queries: &["work.list", "work.next", "work.inbox", "work.prepare"],
        commands: &[],
        without_team_operation: &[],
    },
    Layer {
        id: "execution_admission",
        queries: &[
            "work.admission",
            "claim.inspect",
            "session.inspect",
            "execution.inspect",
        ],
        commands: &[
            "admission.decide",
            "session.start",
            "session.end",
            "claim.acquire",
            "claim.renew",
            "claim.release",
            "execution.prepare",
            "execution.start",
        ],
        without_team_operation: &[Gap {
            entry: "client bind",
            reason: "identity is a bearer token on every request, not a binding of a client",
        }],
    },
    Layer {
        id: "context_completeness",
        queries: &["work.prepare", "source.content", "artifact.content"],
        commands: &["session.checkpoint"],
        without_team_operation: &[
            Gap {
                entry: "context compile",
                reason: "no standalone context pack; work.prepare returns the completeness and the context hash",
            },
            Gap {
                entry: "branch context",
                reason: "Team scopes work by workstream, not by branch",
            },
        ],
    },
    Layer {
        id: "delivery_observation",
        queries: &["work.observe", "execution.inspect", "delivery.inspect"],
        commands: &[
            "execution.report",
            "delivery.observe_pr",
            "delivery.facts.ingest",
        ],
        without_team_operation: &[
            Gap {
                entry: "client hook",
                reason: "no hook receiver; a host reports through session.checkpoint and execution.report",
            },
            Gap {
                entry: "workflow ack",
                reason: "no acknowledgement of a consumed context hash",
            },
        ],
    },
    Layer {
        id: "completion_validity",
        queries: &["completion.inspect", "evidence.inspect", "review.inspect"],
        commands: &[
            "evidence.submit",
            "review.open",
            "review.accept",
            "review.return",
            "review.decide",
            "work.complete",
        ],
        without_team_operation: &[
            Gap {
                entry: "intake inspect",
                reason: "intake of an existing ledger is a personal-mode operation",
            },
            Gap {
                entry: "host confirm_ordinary",
                reason: "no ordinary completion without evidence and review in a Team",
            },
        ],
    },
];

/// The report added to the capabilities of the Team workstream protocol.
pub(crate) fn report() -> Value {
    let layers: serde_json::Map<String, Value> = LAYERS
        .iter()
        .map(|layer| {
            (
                layer.id.to_string(),
                json!({
                    "queries": layer.queries,
                    "commands": layer.commands,
                    "without_team_operation": layer.without_team_operation.iter()
                        .map(|gap| json!({"entry": gap.entry, "reason": gap.reason})).collect::<Vec<_>>(),
                }),
            )
        })
        .collect();
    json!({
        "version": 1,
        "basis": "awr-semantic-contract-five-layers",
        "unsupported_operation": "Unsupported",
        "availability": "work_readiness_and_the_other_layers_are_answered_only_for_works_the_caller_is_authorized_to_see",
        "layers": layers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matrix() -> Value {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/evolution/AWR-EVO-010/semantic-contract-matrix.json"
        );
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn the_layers_are_the_five_of_the_semantic_contract() {
        let ours: Vec<&str> = LAYERS.iter().map(|layer| layer.id).collect();
        let frozen: Vec<String> = matrix()["layer_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_str().unwrap().to_string())
            .collect();
        assert_eq!(ours, frozen);
    }

    #[test]
    fn every_operation_a_layer_names_exists_in_the_protocol() {
        for layer in LAYERS {
            for query in layer.queries {
                assert!(
                    crate::workstream_read::QUERIES.contains(query),
                    "{} names the query {query}",
                    layer.id
                );
            }
            for command in layer.commands {
                assert!(
                    crate::workstream_command::COMMANDS.contains(command),
                    "{} names the command {command}",
                    layer.id
                );
            }
        }
    }

    #[test]
    fn an_entry_without_a_team_operation_is_an_entry_of_the_personal_cli_for_that_layer() {
        let matrix = matrix();
        for layer in LAYERS {
            let entries: Vec<String> = matrix["layers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|l| l["id"] == layer.id)
                .unwrap()["sources"]["local_cli"]["entrypoints"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e.as_str().unwrap().to_string())
                .collect();
            for gap in layer.without_team_operation {
                assert!(
                    entries.iter().any(|e| e == gap.entry),
                    "{} is not a personal entry of {}",
                    gap.entry,
                    layer.id
                );
                assert!(!gap.reason.is_empty());
            }
        }
    }

    #[test]
    fn what_is_reported_missing_really_is_not_an_operation() {
        for layer in LAYERS {
            for gap in layer.without_team_operation {
                let name = gap.entry.replace(' ', ".");
                assert!(!crate::workstream_read::QUERIES.contains(&name.as_str()));
                assert!(!crate::workstream_command::COMMANDS.contains(&name.as_str()));
            }
        }
    }

    #[test]
    fn the_report_names_every_layer_with_its_operations_and_gaps() {
        let report = report();
        assert_eq!(report["unsupported_operation"], "Unsupported");
        let layers = report["layers"].as_object().unwrap();
        assert_eq!(layers.len(), 5);
        for (id, layer) in layers {
            assert!(
                layer["queries"].is_array()
                    && layer["commands"].is_array()
                    && layer["without_team_operation"].is_array(),
                "{id}"
            );
        }
        assert_eq!(
            layers["context_completeness"]["without_team_operation"][0]["entry"],
            "context compile"
        );
    }
}
