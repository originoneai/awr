use crate::error::{PgError, PgResult, SourcePackageLimitKind};

/// Byte and file-count bounds selected by the server code path, never by wire input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourcePackageLimits {
    pub max_files: usize,
    pub max_file_bytes: usize,
    pub max_package_bytes: usize,
}

/// Client-submitted packages retain the original hand-written-document budget.
pub const UPLOAD: SourcePackageLimits = SourcePackageLimits {
    max_files: 64,
    max_file_bytes: 1024 * 1024,
    max_package_bytes: 4 * 1024 * 1024,
};

/// Packages derived from a registered server source have a larger deterministic budget.
pub const GENERATED: SourcePackageLimits = SourcePackageLimits {
    max_files: 256,
    max_file_bytes: 16 * 1024 * 1024,
    max_package_bytes: 64 * 1024 * 1024,
};

// Retain the original public names as upload-budget aliases.
pub const MAX_SOURCE_FILES: usize = UPLOAD.max_files;
pub const MAX_FILE_BYTES: usize = UPLOAD.max_file_bytes;
pub const MAX_PACKAGE_BYTES: usize = UPLOAD.max_package_bytes;

/// Reject path traversal, absolute paths, NUL, and platform aliases before
/// any file bytes are persisted. This runs outside the project lock.
pub fn validate_source_path(path: &str) -> PgResult<()> {
    if path.is_empty() {
        return Err(PgError::UnsafeSourcePath("empty path".into()));
    }
    if path.contains('\0') {
        return Err(PgError::UnsafeSourcePath("NUL in path".into()));
    }
    if path.contains('\\') {
        return Err(PgError::UnsafeSourcePath(path.into()));
    }
    if path.starts_with('/') || path.starts_with("./") {
        return Err(PgError::UnsafeSourcePath(path.into()));
    }
    if path.chars().nth(1) == Some(':') {
        return Err(PgError::UnsafeSourcePath(path.into()));
    }
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(PgError::UnsafeSourcePath(path.into()));
        }
        if segment.ends_with(' ') || segment.ends_with('.') {
            return Err(PgError::UnsafeSourcePath(path.into()));
        }
    }
    Ok(())
}

/// Validate a client-submitted package with the unchanged upload budget.
pub fn validate_package(files: &[(String, Vec<u8>)]) -> PgResult<()> {
    validate_package_with_limits(files, UPLOAD)
}

/// Validate with bounds already selected by the calling code path. This is a
/// library primitive, not a transport option; request schemas expose no limits.
pub fn validate_package_with_limits(
    files: &[(String, Vec<u8>)],
    limits: SourcePackageLimits,
) -> PgResult<()> {
    if files.is_empty() {
        return Err(PgError::Protocol("source package is empty".into()));
    }
    let package_bytes = files.iter().fold(0usize, |total, (_, bytes)| {
        total.saturating_add(bytes.len())
    });
    let too_large = |limit_kind, file: Option<String>, actual: usize, limit: usize| {
        PgError::SourcePackageTooLarge {
            limit_kind,
            file,
            actual: actual as u64,
            limit: limit as u64,
            package_bytes: package_bytes as u64,
            files: files.len(),
        }
    };
    if files.len() > limits.max_files {
        return Err(too_large(
            SourcePackageLimitKind::FileCount,
            None,
            files.len(),
            limits.max_files,
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for (path, bytes) in files {
        validate_source_path(path)?;
        // Text-only input contract: snapshots persist file content as text
        // and bind it to the sha256 of the original bytes. Lossy conversion
        // would break that binding, so invalid UTF-8 is rejected before
        // anything is persisted (CR #37 P2-3).
        if std::str::from_utf8(bytes).is_err() {
            return Err(PgError::InvalidUtf8(path.clone()));
        }
        if !seen.insert(path.clone()) {
            return Err(PgError::UnsafeSourcePath(format!("duplicate path {path}")));
        }
        if bytes.len() > limits.max_file_bytes {
            return Err(too_large(
                SourcePackageLimitKind::FileBytes,
                Some(path.clone()),
                bytes.len(),
                limits.max_file_bytes,
            ));
        }
    }
    if package_bytes > limits.max_package_bytes {
        return Err(too_large(
            SourcePackageLimitKind::PackageBytes,
            None,
            package_bytes,
            limits.max_package_bytes,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_file_is_allowed() {
        validate_source_path("work-ledger.yaml").unwrap();
        validate_source_path("rules/hard.md").unwrap();
    }

    #[test]
    fn traversal_and_absolute_paths_are_rejected() {
        for path in [
            "",
            "../secret",
            "foo/../bar",
            "/etc/passwd",
            "C:windows",
            "foo\\bar",
            "./hidden",
            "foo//bar",
            "foo/./bar",
            "foo/\0bar",
            "foo/bar.",
        ] {
            assert!(
                validate_source_path(path).is_err(),
                "should reject {path:?}"
            );
        }
    }

    fn files_with_sizes(sizes: &[usize]) -> Vec<(String, Vec<u8>)> {
        sizes
            .iter()
            .enumerate()
            .map(|(index, size)| (format!("file-{index}.txt"), vec![b'x'; *size]))
            .collect()
    }

    fn assert_boundaries(limits: SourcePackageLimits) {
        validate_package_with_limits(&files_with_sizes(&vec![0; limits.max_files]), limits)
            .unwrap();
        let error =
            validate_package_with_limits(&files_with_sizes(&vec![0; limits.max_files + 1]), limits)
                .unwrap_err();
        assert!(matches!(
            error,
            PgError::SourcePackageTooLarge {
                limit_kind: SourcePackageLimitKind::FileCount,
                file: None,
                actual,
                limit,
                ..
            } if actual == (limits.max_files + 1) as u64 && limit == limits.max_files as u64
        ));

        validate_package_with_limits(&files_with_sizes(&[limits.max_file_bytes]), limits).unwrap();
        let error =
            validate_package_with_limits(&files_with_sizes(&[limits.max_file_bytes + 1]), limits)
                .unwrap_err();
        assert!(matches!(
            error,
            PgError::SourcePackageTooLarge {
                limit_kind: SourcePackageLimitKind::FileBytes,
                file: Some(ref file),
                actual,
                limit,
                package_bytes,
                files: 1,
            } if file == "file-0.txt" && actual == (limits.max_file_bytes + 1) as u64
                && limit == limits.max_file_bytes as u64
                && package_bytes == actual
        ));

        let whole = limits.max_package_bytes / 4;
        validate_package_with_limits(&files_with_sizes(&[whole; 4]), limits).unwrap();
        let mut oversized = files_with_sizes(&[whole; 4]);
        oversized.push(("one-more-byte.txt".into(), vec![b'x']));
        let error = validate_package_with_limits(&oversized, limits).unwrap_err();
        assert!(matches!(
            error,
            PgError::SourcePackageTooLarge {
                limit_kind: SourcePackageLimitKind::PackageBytes,
                file: None,
                actual,
                limit,
                package_bytes,
                files: 5,
            } if actual == (limits.max_package_bytes + 1) as u64
                && limit == limits.max_package_bytes as u64
                && package_bytes == actual
        ));
    }

    #[test]
    fn upload_limits_pass_at_the_bound_and_reject_one_more() {
        assert_boundaries(UPLOAD);
    }

    #[test]
    fn generated_limits_pass_at_the_bound_and_reject_one_more() {
        assert_boundaries(GENERATED);
    }

    #[test]
    fn upload_still_rejects_a_file_one_byte_over_its_old_limit() {
        let files = files_with_sizes(&[MAX_FILE_BYTES + 1]);
        let error = validate_package(&files).unwrap_err();
        assert!(matches!(
            error,
            PgError::SourcePackageTooLarge {
                limit_kind: SourcePackageLimitKind::FileBytes,
                actual,
                limit,
                ..
            } if actual == (MAX_FILE_BYTES + 1) as u64 && limit == MAX_FILE_BYTES as u64
        ));
        validate_package_with_limits(&files, GENERATED).unwrap();
    }

    #[test]
    fn test_only_lowered_limits_produce_complete_size_facts() {
        let lowered = SourcePackageLimits {
            max_files: 1,
            max_file_bytes: 8,
            max_package_bytes: 8,
        };
        let error = validate_package_with_limits(&files_with_sizes(&[9]), lowered).unwrap_err();
        assert!(matches!(
            error,
            PgError::SourcePackageTooLarge {
                limit_kind: SourcePackageLimitKind::FileBytes,
                file: Some(ref file),
                actual: 9,
                limit: 8,
                package_bytes: 9,
                files: 1,
            } if file == "file-0.txt"
        ));
    }

    #[test]
    fn generated_budget_does_not_relax_path_safety() {
        let files = vec![("../outside".into(), vec![b'x'; MAX_FILE_BYTES + 1])];
        assert!(matches!(
            validate_package_with_limits(&files, GENERATED),
            Err(PgError::UnsafeSourcePath(_))
        ));
    }
}
