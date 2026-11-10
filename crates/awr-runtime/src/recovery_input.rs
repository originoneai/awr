//! Retain literal CLI request data when recovery would exceed the command bound.
use crate::mutation_apply::{directory, new_file, recovery_root};
use awr_core::{Error, Id, Result, ensure_public_data};
use serde::Serialize;
use std::{ffi::OsStr, io::Write, path::Path};

/// Write one new private input file; existing journals and source files stay untouched.
pub fn save_cli_recovery_input(root: &Path, request: &impl Serialize) -> Result<String> {
    let root = root.canonicalize()?;
    ensure_public_data(request)?;
    let bytes = serde_json::to_vec_pretty(request)?;
    if bytes.len() > 64 * 1024 {
        return Err(Error::InvalidInput(
            "CLI recovery input exceeds 64 KiB".into(),
        ));
    }
    let parent = recovery_root(&root)?;
    let name = format!("retry-{}", Id::new());
    parent.create_dir(&name)?;
    let dir = directory(&parent, &name, &root.join(".awr/mutations").join(&name))?;
    let mut file = new_file(&dir, OsStr::new("input.json"))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    crate::fs_sync::sync_directory(&dir)?;
    crate::fs_sync::sync_directory(&parent)?;
    Ok(format!(".awr/mutations/{name}/input.json"))
}
