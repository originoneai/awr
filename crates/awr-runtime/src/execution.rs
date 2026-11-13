//! Read-only recovery of an execution's evidence. A numeric PID never grants liveness or control.
use awr_core::*;
use awr_store::Store;
use std::{
    fs,
    io::{Read, Write},
    net::{Ipv4Addr, Shutdown, SocketAddr, TcpStream},
    path::Path,
    time::{Duration, Instant},
};

fn unknown(e: &Execution, basis: &str) -> Result<ExecutionObservation> {
    Ok(ExecutionObservation{execution_id:e.id,operation_key:e.intent.operation_key.clone(),purpose:e.intent.purpose.clone(),origin_session_id:e.session_id,branch_id:e.branch_id,recorded_revision:e.revision,state:ObservedExecutionState::Unknown,recorded_state:e.state,verified:false,observed_at:now_millis()?,evidence_at:None,basis:basis.into(),exit_code:None,signal:None,error:None,stdout:e.stdout.clone(),stderr:e.stderr.clone(),receipt:e.receipt.clone(),next_action:"Verify the original executor and any side effects before choosing a new operation. No automatic retry or process termination.".into()})
}
fn outcome(
    mut o: ExecutionObservation,
    result: ExecutionResult,
    basis: &str,
) -> ExecutionObservation {
    o.state = if result.success {
        ObservedExecutionState::Succeeded
    } else {
        ObservedExecutionState::Failed
    };
    o.verified = true;
    o.evidence_at = Some(result.finished_at);
    o.basis = basis.into();
    o.exit_code = result.exit_code;
    o.signal = result.signal;
    o.error = result.error;
    o.next_action=if result.success{"Read the recorded result and logs, then continue the dependent work. Do not repeat the operation."}else{"Inspect the failure and its side effects before planning a new operation with a new key."}.into();
    o
}
fn receipt(root: &Path, e: &Execution) -> Result<Option<ExecutionResult>> {
    let path = root.join(format!(".awr/executions/{}/result.json", e.id));
    match fs::symlink_metadata(&path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
        Ok(metadata) if !metadata.is_file() => {
            return Err(Error::InvalidInput("receipt is not a regular file".into()));
        }
        _ => {}
    }
    let bytes = awr_source::read_capped(&path, 64 * 1024)?;
    let r: ExecutionResult = serde_json::from_slice(&bytes)
        .map_err(|_| Error::InvalidInput("invalid execution receipt".into()))?;
    ensure_public_data(&r)?;
    if r.execution_id != e.id
        || e.worker.as_ref().is_none_or(|w| w.nonce != r.nonce)
        || r.finished_at < e.started_at.unwrap_or(e.registered_at)
        || (r.success && (r.exit_code != Some(0) || r.signal.is_some() || r.error.is_some()))
        || r.error.as_ref().is_some_and(|s| s.len() > 8192)
    {
        return Err(Error::InvalidInput(
            "execution receipt identity or outcome mismatch".into(),
        ));
    }
    Ok(Some(r))
}
enum ProbeOnce {
    Ready(ExecutionProbeReply),
    /// Identity does not match this supervisor. Retrying cannot make it live.
    Rejected,
    /// Connect, read, or accept was late. A loaded host can miss one attempt.
    Transient,
}

fn probe_once(e: &Execution) -> ProbeOnce {
    let Some(worker) = e.worker.as_ref() else {
        return ProbeOnce::Rejected;
    };
    let Some(started_at) = e.started_at else {
        return ProbeOnce::Rejected;
    };
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, worker.port));
    let mut socket = match TcpStream::connect_timeout(&address, Duration::from_millis(200)) {
        Ok(socket) => socket,
        Err(_) => return ProbeOnce::Transient,
    };
    if socket
        .set_read_timeout(Some(Duration::from_millis(250)))
        .is_err()
        || socket
            .set_write_timeout(Some(Duration::from_millis(150)))
            .is_err()
    {
        return ProbeOnce::Transient;
    }
    let payload = match serde_json::to_vec(&ExecutionProbe {
        execution_id: e.id,
        nonce: worker.nonce,
    }) {
        Ok(payload) => payload,
        Err(_) => return ProbeOnce::Rejected,
    };
    if socket.write_all(&payload).is_err() {
        return ProbeOnce::Transient;
    }
    if socket.shutdown(Shutdown::Write).is_err() {
        return ProbeOnce::Transient;
    }
    let mut bytes = Vec::new();
    let read = socket.take(4096).read_to_end(&mut bytes);
    // A full reply can arrive before the supervisor closes. Timing out while
    // waiting for EOF must not discard those bytes.
    if bytes.is_empty() {
        let _ = read;
        return ProbeOnce::Transient;
    }
    let Ok(response) = serde_json::from_slice::<ExecutionProbeReply>(&bytes) else {
        return ProbeOnce::Transient;
    };
    if response.execution_id != e.id
        || response.nonce != worker.nonce
        || response.worker_pid != worker.pid
        || worker.child_pid.is_some_and(|id| id != response.child_pid)
        || response.child_pid == 0
        || response.observed_at < started_at
    {
        return ProbeOnce::Rejected;
    }
    ProbeOnce::Ready(response)
}

fn probe(e: &Execution) -> Option<ExecutionProbeReply> {
    for attempt in 0..4 {
        match probe_once(e) {
            ProbeOnce::Ready(reply) => return Some(reply),
            ProbeOnce::Rejected => return None,
            ProbeOnce::Transient if attempt == 3 => return None,
            ProbeOnce::Transient => std::thread::sleep(Duration::from_millis(40)),
        }
    }
    None
}
pub fn inspect_execution(root: &Path, e: &Execution) -> Result<ExecutionObservation> {
    let root = root.canonicalize()?;
    let mut o = unknown(e, "supervisor_unreachable_without_completion_receipt")?;
    if Path::new(&e.intent.cwd) != root {
        o.basis = "project_root_mismatch".into();
        return Ok(o);
    }
    if e.intent.executor == ExecutorKind::External {
        o.basis = "external_reference_requires_executor_adapter".into();
        return Ok(o);
    }
    if e.state.terminal() {
        return Ok(outcome(
            o,
            ExecutionResult {
                execution_id: e.id,
                nonce: e
                    .worker
                    .as_ref()
                    .ok_or_else(|| {
                        Error::Storage(
                            "managed terminal execution lacks supervisor identity".into(),
                        )
                    })?
                    .nonce,
                finished_at: e
                    .finished_at
                    .ok_or_else(|| Error::Storage("terminal execution lacks finish time".into()))?,
                success: e.state == ExecutionState::Succeeded,
                exit_code: e.exit_code,
                signal: e.signal,
                error: e.error.clone(),
            },
            "immutable_supervisor_completion_event",
        ));
    }
    if e.worker.is_none() {
        o.basis = "registered_without_supervisor_identity".into();
        return Ok(o);
    }
    match receipt(&root, e) {
        Ok(Some(r)) => return Ok(outcome(o, r, "supervisor_result_receipt")),
        Err(_) => {
            o.basis = "invalid_or_unreadable_completion_receipt".into();
            return Ok(o);
        }
        Ok(None) => {}
    }
    if let Some(reply) = probe(e) {
        o.state = ObservedExecutionState::Running;
        o.verified = true;
        o.evidence_at = Some(reply.observed_at);
        o.basis = "owned_supervisor_identity_and_live_child_probe".into();
        o.observed_at = now_millis()?;
        o.next_action="The managed command is still running at the observation time. Wait for its result; do not launch it again.".into();
        return Ok(o);
    }
    // Child completion can race a failed probe. Re-read the atomic receipt before saying unknown.
    match receipt(&root, e) {
        Ok(Some(r)) => Ok(outcome(o, r, "supervisor_result_receipt_after_probe")),
        Err(_) => {
            o.basis = "invalid_or_unreadable_completion_receipt".into();
            Ok(o)
        }
        Ok(None) => Ok(o),
    }
}
pub fn inspect_work_executions(
    store: &Store,
    root: &Path,
    project: Id,
    work: Id,
    branch: Option<Id>,
) -> Result<Vec<ExecutionObservation>> {
    let executions = store.executions(project, Some(work))?;
    // The budget starts after the store read. A slow query must not skip the
    // only probe of a still-running child.
    let deadline = Instant::now() + Duration::from_secs(2);
    executions
        .iter()
        .filter(|e| e.branch_id == branch)
        .map(|e| {
            if !e.state.terminal() && Instant::now() > deadline {
                unknown(
                    e,
                    "inspection_time_budget_exhausted_requires_explicit_inspect",
                )
            } else {
                inspect_execution(root, e)
            }
        })
        .collect()
}
pub fn render_execution_observations(observations: &[ExecutionObservation]) -> Result<String> {
    if observations.is_empty() {
        return Ok(String::new());
    }
    let mut text =
        "Execution recovery observations (separate from the source/context hash):\n".to_owned();
    for o in observations {
        text.push_str(&format!("{} {}: {} | verified={} | observed_at={} | evidence_at={:?} | basis={} | exit={:?} | signal={:?}\nPurpose: {}\nNext: {}\n",o.execution_id,serde_json::to_string(&o.operation_key)?,serde_json::to_string(&o.state)?,o.verified,o.observed_at,o.evidence_at,o.basis,o.exit_code,o.signal,serde_json::to_string(&o.purpose)?,o.next_action));
    }
    Ok(text)
}

/// Host-verified isolation evidence. Metadata fencing alone is never enough
/// to claim physical/strong isolation (WS-021).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostIsolationEvidence {
    pub filesystem_sandbox: bool,
    pub os_boundary: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IsolationClass {
    /// Reservations/fences only; no verified host sandbox or OS boundary.
    MetadataFencingOnly,
    /// Host capability negotiation confirmed a sandbox or OS boundary.
    VerifiedHostBoundary,
}

pub fn classify_isolation(evidence: Option<&HostIsolationEvidence>) -> IsolationClass {
    match evidence {
        Some(e) if e.filesystem_sandbox || e.os_boundary => IsolationClass::VerifiedHostBoundary,
        _ => IsolationClass::MetadataFencingOnly,
    }
}

pub fn isolation_basis(evidence: Option<&HostIsolationEvidence>) -> &'static str {
    match classify_isolation(evidence) {
        IsolationClass::VerifiedHostBoundary => "verified_host_sandbox_or_os_boundary",
        IsolationClass::MetadataFencingOnly => {
            "metadata_fencing_only_not_physical_strong_isolation"
        }
    }
}

/// Refuse advertising physical strong isolation when host capabilities are
/// missing or unverified. Callers may still proceed with metadata fencing.
pub fn refuse_unverified_strong_isolation(evidence: Option<&HostIsolationEvidence>) -> Result<()> {
    if classify_isolation(evidence) != IsolationClass::VerifiedHostBoundary {
        return Err(Error::InvalidInput(
            "physical strong isolation requires verified host sandbox or OS boundary; AWR metadata fencing is not sufficient".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_fencing_is_not_physical_strong_isolation() {
        assert_eq!(
            classify_isolation(None),
            IsolationClass::MetadataFencingOnly
        );
        assert_eq!(
            classify_isolation(Some(&HostIsolationEvidence::default())),
            IsolationClass::MetadataFencingOnly
        );
        assert!(refuse_unverified_strong_isolation(None).is_err());
        assert_eq!(
            isolation_basis(None),
            "metadata_fencing_only_not_physical_strong_isolation"
        );
    }

    #[test]
    fn verified_host_boundary_allows_strong_isolation_claim() {
        let evidence = HostIsolationEvidence {
            filesystem_sandbox: true,
            os_boundary: false,
        };
        assert_eq!(
            classify_isolation(Some(&evidence)),
            IsolationClass::VerifiedHostBoundary
        );
        assert!(refuse_unverified_strong_isolation(Some(&evidence)).is_ok());
        assert_eq!(
            isolation_basis(Some(&evidence)),
            "verified_host_sandbox_or_os_boundary"
        );
    }

    #[test]
    fn probe_keeps_a_complete_reply_when_eof_is_late() {
        use std::io::{Read, Write};
        use std::net::{Ipv4Addr, TcpListener};
        use std::thread;
        use std::time::Duration;

        let dir = std::env::temp_dir().join(format!("awr-probe-eof-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let root = dir.canonicalize().unwrap();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let id = Id::new();
        let nonce = Id::new();
        let pid = std::process::id();
        let started = now_millis().unwrap();
        let server_id = id;
        let server_nonce = nonce;
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut bytes = Vec::new();
            let _ = Read::by_ref(&mut socket).take(2048).read_to_end(&mut bytes);
            let reply = ExecutionProbeReply {
                execution_id: server_id,
                nonce: server_nonce,
                worker_pid: pid,
                child_pid: pid,
                observed_at: now_millis().unwrap(),
            };
            socket
                .write_all(&serde_json::to_vec(&reply).unwrap())
                .unwrap();
            // Stay open longer than the probe read timeout. The reply is already complete.
            thread::sleep(Duration::from_millis(400));
        });
        let execution = Execution {
            id,
            project_id: Id::new(),
            work_item_id: Id::new(),
            session_id: Id::new(),
            branch_id: None,
            revision: 1,
            intent: ExecutionIntent {
                operation_key: "probe".into(),
                purpose: "late eof".into(),
                executor: ExecutorKind::ManagedLocal,
                command: vec!["true".into()],
                cwd: root.display().to_string(),
                external_reference: None,
                inputs: Vec::new(),
                input_binding: awr_core::InputBinding::None,
            },
            state: ExecutionState::Running,
            worker: Some(WorkerIdentity {
                nonce,
                pid,
                port,
                child_pid: Some(pid),
            }),
            registered_at: started,
            started_at: Some(started),
            finished_at: None,
            exit_code: None,
            signal: None,
            error: None,
            stdout: None,
            stderr: None,
            receipt: None,
        };
        let observed = inspect_execution(&root, &execution).unwrap();
        assert_eq!(observed.state, ObservedExecutionState::Running);
        assert!(observed.verified);
        assert_eq!(
            observed.basis,
            "owned_supervisor_identity_and_live_child_probe"
        );
        server.join().unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
