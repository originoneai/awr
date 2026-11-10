use serde::{Deserialize, Serialize};

/// Provenance of an output observation, separate from currency or acceptance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trust {
    Source,
    #[default]
    Reported,
    Recorded,
    Verified,
    Reviewed,
}

impl Trust {
    pub const fn legend(self) -> &'static str {
        match self {
            Self::Source => "Read from an authoritative project source.",
            Self::Reported => "Supplied by a caller without an independent program check.",
            Self::Recorded => "Observed and stored in a program-generated evidence event.",
            Self::Verified => "Compared by the program against a separate recorded fact.",
            Self::Reviewed => "Approved by a different session or a human reviewer.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path};

    const VALUES: [Trust; 5] = [
        Trust::Source,
        Trust::Reported,
        Trust::Recorded,
        Trust::Verified,
        Trust::Reviewed,
    ];

    #[test]
    fn trust_legend_has_five_short_distinct_explanations() {
        let names = ["source", "reported", "recorded", "verified", "reviewed"];
        let mut legends = std::collections::BTreeSet::new();
        for (value, name) in VALUES.into_iter().zip(names) {
            assert_eq!(serde_json::to_value(value).unwrap(), name);
            assert_eq!(
                serde_json::from_value::<Trust>(serde_json::json!(name)).unwrap(),
                value
            );
            let legend = value.legend();
            assert!(!legend.is_empty());
            assert!(legend.chars().count() <= 80, "{name}: {legend}");
            assert!(!legend.contains('\n'));
            assert!(
                legends.insert(legend),
                "each value needs its own explanation"
            );
        }
    }

    fn occurrences(path: &Path, legend: &str) -> usize {
        if path.is_dir() {
            fs::read_dir(path)
                .unwrap()
                .map(|entry| occurrences(&entry.unwrap().path(), legend))
                .sum()
        } else if matches!(path.extension().and_then(|s| s.to_str()), Some("rs" | "md")) {
            fs::read_to_string(path).unwrap().matches(legend).count()
        } else {
            0
        }
    }

    #[test]
    fn trust_legend_explanations_have_no_copies_in_other_source_files() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        for value in VALUES {
            let count = occurrences(&root.join("crates"), value.legend())
                + occurrences(&root.join("docs/dev/reference"), value.legend())
                + occurrences(&root.join("docs/dev/release"), value.legend());
            assert_eq!(
                count, 1,
                "the canonical explanation must occur only in trust.rs: {value:?}"
            );
        }
    }

    #[test]
    fn trust_legend_copy_detection_rejects_a_duplicate_definition() {
        let root = std::env::temp_dir().join(format!("awr-trust-legend-{}", crate::Id::new()));
        fs::create_dir(&root).unwrap();
        let legend = Trust::Reported.legend();
        fs::write(root.join("canonical.rs"), legend).unwrap();
        fs::write(root.join("copy.md"), legend).unwrap();
        let count = occurrences(&root, legend);
        fs::remove_dir_all(root).unwrap();
        assert_eq!(
            count, 2,
            "a copied explanation must fail the single-definition check"
        );
    }

    #[test]
    fn trust_rejects_unknown_or_noncanonical_values() {
        for value in ["trusted", "Verified", "caller_reported", ""] {
            assert!(serde_json::from_value::<Trust>(serde_json::json!(value)).is_err());
        }
    }
}
