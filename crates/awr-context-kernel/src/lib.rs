//! The pure contract of AWR's context packing kernel (EVO-020).
//!
//! A host hands the kernel fixed segments of text with stable names, the entities they are about, which of them are
//! required, which belong together (atomic groups), how to rank the rest and what the budget is; the kernel's answer is
//! a selection that is a consistent partition of what it was given. Nothing here reads a file, a store, a clock, the
//! network or a model, nothing initializes a project, and nothing says whether the work is authorized, ready, complete
//! or accepted: those stay with the full AWR work compilation that builds the request.
//!
//! This crate fixes the shapes and the checks. [`validate_request`] says whether a request is well formed and within
//! bounds; [`check_result`] says whether a result is a consistent answer to a request, and is the oracle any packing code
//! is held to. The packing itself, parsing and the consumer budgets are later steps (EVO-021, EVO-022, EVO-023).
mod check;
mod model;
pub use check::{Limits, PackIssue, PackIssueCode, check_result, request_digest, validate_request};
pub use model::*;
