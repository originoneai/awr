use crate::Cli;
use clap::{Arg, ArgAction, ArgMatches, Command, CommandFactory, FromArgMatches, error::ErrorKind};
use std::{
    ffi::{OsStr, OsString},
    fmt::Write,
};

struct Group {
    name: &'static str,
    title: &'static str,
    commands: &'static [(&'static str, &'static str)],
}

static COMMAND_GROUPS: &[Group] = &[
    Group {
        name: "daily",
        title: "Daily work",
        commands: &[
            ("status", "What to continue, claim or wait for"),
            ("ready", "Work that can be claimed now"),
            ("work", "Read, prepare, progress and complete one work item"),
            ("session", "Start, checkpoint and end a work session"),
            ("context", "Compile the context for one work item"),
            ("evidence", "Record or read evidence"),
            ("search", "Search work, decisions and evidence"),
            ("doctor", "Find and repair inconsistent runtime state"),
            ("overview", "Open the local project hub"),
        ],
    },
    Group {
        name: "planning",
        title: "Planning",
        commands: &[
            ("init", "Set up source mappings for a project"),
            ("intake", "Inspect project organization and readiness"),
            ("source", "List, scan and index authoritative sources"),
            ("batch", "Preview and apply a batch of source edits"),
            (
                "host",
                "Save source edits with request identity and provenance",
            ),
            ("document", "Edit documents and create source-backed drafts"),
            ("proposal", "Create and review source mutation proposals"),
            ("organization", "Maintain project phase, scope and focus"),
        ],
    },
    Group {
        name: "records",
        title: "Records",
        commands: &[
            ("event", "Append events and read event history"),
            ("object", "Read a referenced project object"),
            ("decision", "Read an authoritative decision"),
            ("artifact", "Import and read registered artifacts"),
            (
                "spec",
                "Read specification versions and requirement coverage",
            ),
            ("execution", "Run and inspect recorded executions"),
            ("recovery", "Inspect saved work and execution evidence"),
            ("assessment", "Replay assessments and compare advice"),
            ("nav", "Navigate scope, dependencies and blockers"),
        ],
    },
    Group {
        name: "hosts",
        title: "Hosts",
        commands: &[
            (
                "client",
                "Bind conversations and save lifecycle checkpoints",
            ),
            ("capabilities", "Inspect supported host capabilities"),
        ],
    },
    Group {
        name: "environments",
        title: "Environments",
        commands: &[
            ("runtime", "Inspect, back up and restore a local runtime"),
            ("workspace", "Share tracked files between machines"),
            ("branch", "Manage Agent Work Branches"),
        ],
    },
    Group {
        name: "collaboration",
        title: "Collaboration",
        commands: &[
            (
                "team",
                "Inspect Team capabilities and submit remote requests",
            ),
            (
                "remote",
                "Configure Team endpoints and credential references",
            ),
        ],
    },
];

static WORK_GROUPS: &[Group] = &[
    Group {
        name: "daily",
        title: "Daily work",
        commands: &[
            (
                "show",
                "Read one work item's state, acceptance and evidence",
            ),
            ("prepare", "Prepare readiness and required context"),
            ("claim", "Claim work for a session"),
            ("progress", "Record progress in the source ledger"),
            ("block", "Record a concrete blocker"),
            ("unblock", "Clear a blocker and recheck dependencies"),
            (
                "prepare-completion",
                "Validate a report and prepare evidence metadata",
            ),
            (
                "complete",
                "Complete work with verified acceptance evidence",
            ),
            ("release", "Release a session's claim"),
        ],
    },
    Group {
        name: "planning",
        title: "Planning",
        commands: &[
            ("create", "Preview or accept a new task draft"),
            ("edit", "Preview and apply a source-backed work edit"),
            ("graph", "Inspect dependencies, readiness and claims"),
            ("assess", "Explain management requirements"),
        ],
    },
    Group {
        name: "governance",
        title: "Governance",
        commands: &[
            ("admit", "Record an admission proposal or decision"),
            ("contain", "Record a containment proposal or decision"),
            ("reassess", "Record a reassessment decision"),
            ("gap", "Open, resolve or withdraw a work gap"),
            ("finding", "Record or close a review finding"),
            ("manage", "Record management observations"),
        ],
    },
    Group {
        name: "lifecycle",
        title: "Lifecycle",
        commands: &[
            ("cancel", "Cancel work and release its claims"),
            ("reopen", "Reopen completed or cancelled work"),
            ("handoff", "Hand off a session and its latest checkpoint"),
            ("history", "Read event summaries for one work item"),
        ],
    },
    Group {
        name: "recovery",
        title: "Recovery",
        commands: &[
            ("create-status", "Read the outcome of a creation request"),
            ("create-recover", "Recover a pending creation request"),
        ],
    },
];

fn rows(group: &Group) -> String {
    let width = group
        .commands
        .iter()
        .map(|(name, _)| name.len())
        .max()
        .unwrap_or(0);
    let mut text = format!("{}\n", group.title);
    for (name, description) in group.commands {
        writeln!(text, "  {name:width$}  {description}").unwrap();
    }
    text
}

fn overview(work: bool) -> String {
    let (scope, groups, heading, usage) = if work {
        (
            "awr work",
            WORK_GROUPS,
            "Read and manage one work item",
            "awr work [OPTIONS] <COMMAND>",
        )
    } else {
        (
            "awr",
            COMMAND_GROUPS,
            "Persistent work state and minimal context for long-running agents",
            "awr [--project <DIR>] [--json] <COMMAND>",
        )
    };
    let mut text = format!(
        "{heading}\n\nUsage: {usage}\n\n{}\nMore: {scope} help <group>\n",
        rows(&groups[0])
    );
    let width = groups
        .iter()
        .skip(1)
        .map(|group| group.name.len())
        .max()
        .unwrap_or(0);
    for group in groups.iter().skip(1) {
        let members = group
            .commands
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(text, "  {:width$}  {members}", group.name).unwrap();
    }
    writeln!(text, "\nAll commands: {scope} help all").unwrap();
    text
}

fn grouped(command: Command, work: bool) -> Command {
    command
        .override_help(overview(work))
        .disable_help_subcommand(true)
        .subcommand(
            Command::new("help")
                .hide(true)
                .about("Print this message or the help of the given subcommand(s)")
                .arg(Arg::new("command").num_args(0..).action(ArgAction::Append)),
        )
}

pub(crate) fn command() -> Command {
    grouped(Cli::command(), false).mut_subcommand("work", |command| grouped(command, true))
}

pub(crate) fn parse_from(args: &[OsString]) -> Result<Cli, clap::Error> {
    if let Ok(matches) = command().try_get_matches_from(args) {
        let request = match matches.subcommand() {
            Some(("help", help)) => Some((false, help)),
            Some(("work", work)) => work.subcommand_matches("help").map(|help| (true, help)),
            _ => None,
        };
        if let Some((work, help)) = request {
            let program = args.first().map_or(OsStr::new("awr"), OsString::as_os_str);
            return Err(render_request(work, help, program));
        }
    }
    // Keep clap's inherited help behavior for ordinary commands and detailed help.
    let matches = Cli::command()
        .override_help(overview(false))
        .mut_subcommand("work", |command| command.override_help(overview(true)))
        .try_get_matches_from(args)?;
    Cli::from_arg_matches(&matches)
}

fn display(text: String) -> clap::Error {
    Command::new("awr")
        .override_help(text)
        .try_get_matches_from(["awr", "--help"])
        .unwrap_err()
}

fn legacy_help(path: &[&str], program: &OsStr) -> clap::Error {
    Cli::command()
        .try_get_matches_from(
            [program, OsStr::new("help")]
                .into_iter()
                .chain(path.iter().map(OsStr::new)),
        )
        .unwrap_err()
}

fn render_request(work: bool, matches: &ArgMatches, program: &OsStr) -> clap::Error {
    // Keep the derive command tree intact for legacy lists and detailed help.
    let mut command = Cli::command();
    let mut command_path = Vec::new();
    if work {
        command = command.find_subcommand("work").unwrap().clone();
        command_path.push("work");
    }
    let path = matches
        .get_many::<String>("command")
        .unwrap_or_default()
        .collect::<Vec<_>>();
    for (index, name) in path.iter().enumerate() {
        let groups = match command_path.as_slice() {
            [] => Some(COMMAND_GROUPS),
            ["work"] => Some(WORK_GROUPS),
            _ => None,
        };
        if let Some(groups) = groups {
            if index + 1 == path.len() {
                if *name == "all" {
                    return legacy_help(&command_path, program);
                }
                if let Some(group) = groups.iter().find(|group| group.name == name.as_str()) {
                    return display(rows(group));
                }
            }
        }
        command.build();
        let Some(child) = command.find_subcommand(name).cloned() else {
            let available = groups
                .map(|groups| {
                    let mut names = groups.iter().map(|group| group.name).collect::<Vec<_>>();
                    names.push("all");
                    format!("\nAvailable groups: {}\n", names.join(", "))
                })
                .unwrap_or_default();
            return clap::Error::raw(
                ErrorKind::InvalidSubcommand,
                format!("Unknown help topic '{name}'.{available}"),
            );
        };
        command = child;
        command_path.push(name.as_str());
    }
    match command_path.as_slice() {
        [] => display(overview(false)),
        ["work"] if work => display(overview(true)),
        _ => legacy_help(&command_path, program),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validate_groups(command: &Command, groups: &[Group]) -> Result<(), String> {
        for group in groups {
            if command
                .get_subcommands()
                .any(|child| !child.is_hide_set() && child.get_name() == group.name)
            {
                return Err(format!(
                    "help group '{}' shadows command '{}'",
                    group.name, group.name
                ));
            }
        }
        for child in command
            .get_subcommands()
            .filter(|child| !child.is_hide_set())
        {
            let count = groups
                .iter()
                .flat_map(|group| group.commands)
                .filter(|(name, _)| *name == child.get_name())
                .count();
            if count != 1 {
                return Err(format!(
                    "{} belongs to {count} groups; expected exactly one",
                    child.get_name()
                ));
            }
        }
        for group in groups {
            for (name, description) in group.commands {
                if command.find_subcommand(name).is_none() {
                    return Err(format!("group {} names unknown command {name}", group.name));
                }
                if description.is_empty() || description.chars().count() > 60 {
                    return Err(format!(
                        "description for {name} must contain 1 to 60 characters"
                    ));
                }
            }
        }
        Ok(())
    }

    #[test]
    fn ac1_every_visible_command_belongs_to_exactly_one_group() {
        let mut command = command();
        command.build();
        validate_groups(&command, COMMAND_GROUPS).unwrap();
        validate_groups(command.find_subcommand("work").unwrap(), WORK_GROUPS).unwrap();
    }

    #[test]
    fn ac1_ungrouped_new_command_is_rejected_by_name() {
        for (command, groups) in [
            (command(), COMMAND_GROUPS),
            (
                command().find_subcommand("work").unwrap().clone(),
                WORK_GROUPS,
            ),
        ] {
            let command = command.subcommand(Command::new("ungrouped-command"));
            let error = validate_groups(&command, groups).unwrap_err();
            assert!(
                error.contains("ungrouped-command belongs to 0 groups"),
                "{error}"
            );
        }
    }

    #[test]
    fn ac3_group_names_never_shadow_visible_commands() {
        let mut command = command();
        command.build();
        validate_groups(&command, COMMAND_GROUPS).unwrap();
        validate_groups(command.find_subcommand("work").unwrap(), WORK_GROUPS).unwrap();
        for (command, groups) in [
            (command.clone(), COMMAND_GROUPS),
            (
                command.find_subcommand("work").unwrap().clone(),
                WORK_GROUPS,
            ),
        ] {
            for group in groups {
                assert!(
                    command.find_subcommand(group.name).is_none(),
                    "help group '{}' shadows command '{}'",
                    group.name,
                    group.name
                );
            }
        }
    }

    #[test]
    fn ac3_new_group_or_command_collision_is_rejected_by_name() {
        for (command, groups) in [
            (command(), COMMAND_GROUPS),
            (
                command().find_subcommand("work").unwrap().clone(),
                WORK_GROUPS,
            ),
        ] {
            let name = groups[0].commands[0].0;
            let mut new_groups = vec![Group {
                name,
                title: "New group",
                commands: &[],
            }];
            new_groups.extend(groups.iter().map(|group| Group {
                name: group.name,
                title: group.title,
                commands: group.commands,
            }));
            let error = validate_groups(&command, &new_groups).unwrap_err();
            assert!(
                error.contains(&format!("help group '{name}' shadows command '{name}'")),
                "{error}"
            );

            for group in groups {
                let command = command.clone().subcommand(Command::new(group.name));
                let error = validate_groups(&command, groups).unwrap_err();
                let name = group.name;
                assert!(
                    error.contains(&format!("help group '{name}' shadows command '{name}'")),
                    "{error}"
                );
            }
        }
    }

    #[test]
    fn duplicate_group_membership_is_rejected() {
        let group = Group {
            name: "duplicate",
            title: "Duplicate",
            commands: &[("status", "Read status"), ("status", "Read status twice")],
        };
        let error = validate_groups(
            &Command::new("awr").subcommand(Command::new("status")),
            &[group],
        )
        .unwrap_err();
        assert!(error.contains("status belongs to 2 groups"), "{error}");
    }

    #[test]
    fn group_descriptions_are_at_most_sixty_characters() {
        for group in COMMAND_GROUPS.iter().chain(WORK_GROUPS) {
            for (name, description) in group.commands {
                assert!(description.chars().count() <= 60, "{name}: {description}");
            }
        }
    }
}
