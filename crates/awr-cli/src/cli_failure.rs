//! Keep dynamic Team service errors at the CLI boundary, without changing core errors.
use serde_json::{Value, json};

pub type CliResult<T> = Result<T, CliFailure>;

#[derive(Debug)]
pub enum CliFailure {
    Local(awr_core::Error),
    TeamRemote { report: Value, status: Option<u16> },
}

impl From<awr_core::Error> for CliFailure {
    fn from(error: awr_core::Error) -> Self {
        Self::Local(error)
    }
}

impl CliFailure {
    pub fn remote(report: Value, status: u16) -> Self {
        Self::TeamRemote {
            report,
            status: Some(status),
        }
    }

    pub fn client(code: &'static str, message: &'static str, details: Value) -> Self {
        Self::TeamRemote {
            report: json!({"code":code,"message":message,"details":details}),
            status: None,
        }
    }

    pub fn invalid_team_response(reason: &'static str, status: u16) -> Self {
        Self::client(
            "TeamResponseInvalid",
            "invalid Team service response",
            json!({"reason":reason,"http_status":status}),
        )
    }

    pub fn print(&self, json: bool) {
        match self {
            Self::Local(error) => {
                let report = error.report();
                if json {
                    eprintln!(
                        "{}",
                        serde_json::to_string(&report).expect("error report serializes")
                    );
                } else {
                    eprintln!(
                        "{}: {}{}",
                        report.code,
                        report.message,
                        awr_core::render_diagnostic_details(report.details.as_ref())
                    );
                }
            }
            Self::TeamRemote { report, status } => {
                if json {
                    eprintln!("{report}");
                } else {
                    let code = report["code"].as_str().unwrap_or("TeamResponseInvalid");
                    // JSON string rendering escapes terminal controls in untrusted text.
                    if let Some(message) = report.get("message") {
                        eprintln!("{code}: {}", terminal_json(message, false));
                    } else {
                        eprintln!("{code}: Team query failed (HTTP {}).", status.unwrap_or(0));
                    }
                    let mut extra = report.clone();
                    if let Some(object) = extra.as_object_mut() {
                        object.remove("code");
                        object.remove("message");
                        if !object.is_empty() {
                            eprintln!("{}", terminal_json(&extra, true));
                        }
                    }
                }
            }
        }
    }
}

/// serde_json escapes ASCII controls; escape the remaining Unicode controls for
/// terminal display as well. Generated pretty-print newlines remain readable.
pub fn terminal_json(value: &Value, pretty: bool) -> String {
    let json = if pretty {
        serde_json::to_string_pretty(value).expect("JSON value")
    } else {
        value.to_string()
    };
    json.chars()
        .map(|c| {
            if c.is_control() && c != '\n' {
                format!("\\u{:04x}", c as u32)
            } else {
                c.to_string()
            }
        })
        .collect()
}
