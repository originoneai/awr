//! Project-qualified organization metadata. Grouping never grants access or
//! identifies people across projects.
use crate::{TeamError, TeamResult, request_hash};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryUnit {
    pub id: String,
    pub name: String,
}

/// A department in the hierarchy. `parent_id` is absent or null for a top-level
/// department, so directories recorded before the hierarchy field exist read
/// back with every department at the top level.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryDepartment {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

/// Deepest allowed department chain. A top-level department has depth 1, so a
/// chain may hold at most four nested departments under the organization.
pub const MAX_DEPARTMENT_DEPTH: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryMember {
    /// An existing person in the bound project, never an actor or display name.
    pub person_id: String,
    pub department_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectDirectoryPlan {
    pub organization: Option<DirectoryUnit>,
    pub departments: Vec<DirectoryDepartment>,
    pub project_department_ids: Vec<String>,
    pub members: Vec<DirectoryMember>,
}

pub fn directory_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

fn invalid() -> TeamError {
    TeamError::InvalidInput("invalid project directory plan".into())
}

fn valid_unit(unit: &DirectoryUnit) -> bool {
    directory_id(&unit.id)
        && !unit.name.trim().is_empty()
        && unit.name.len() <= 512
        && !unit.name.chars().any(char::is_control)
}

fn valid_department(unit: &DirectoryDepartment) -> bool {
    directory_id(&unit.id)
        && !unit.name.trim().is_empty()
        && unit.name.len() <= 512
        && !unit.name.chars().any(char::is_control)
        && unit.parent_id.as_deref().is_none_or(directory_id)
}

fn valid_references(values: &[String], departments: &BTreeSet<&str>) -> bool {
    let unique: BTreeSet<_> = values.iter().map(String::as_str).collect();
    unique.len() == values.len() && unique.is_subset(departments)
}

/// Depth of every department, counting the department itself (top level = 1).
/// Walks each parent chain to termination, so a cycle is an error instead of a
/// silent cap; an undeclared parent is an error for the same reason.
fn hierarchy_depths(departments: &[DirectoryDepartment]) -> TeamResult<BTreeMap<&str, usize>> {
    let by_id: BTreeMap<&str, &DirectoryDepartment> = departments
        .iter()
        .map(|unit| (unit.id.as_str(), unit))
        .collect();
    let mut depths = BTreeMap::new();
    for unit in departments {
        let mut visited = BTreeSet::new();
        let mut current = unit;
        let mut depth = 0usize;
        loop {
            depth += 1;
            if !visited.insert(current.id.as_str()) {
                return Err(invalid());
            }
            match current.parent_id.as_deref() {
                None => break,
                Some(parent) => {
                    current = by_id.get(parent).copied().ok_or_else(invalid)?;
                }
            }
        }
        depths.insert(unit.id.as_str(), depth);
    }
    Ok(depths)
}

impl ProjectDirectoryPlan {
    pub fn validate(&self) -> TeamResult<()> {
        if self.departments.len() > 128
            || self.members.len() > 256
            || serde_json::to_vec(self).map_err(|_| invalid())?.len() > 65_536
            || self
                .organization
                .as_ref()
                .is_some_and(|unit| !valid_unit(unit))
            || self.departments.iter().any(|unit| !valid_department(unit))
        {
            return Err(invalid());
        }
        if self.organization.is_none()
            && (!self.departments.is_empty()
                || !self.project_department_ids.is_empty()
                || !self.members.is_empty())
        {
            return Err(invalid());
        }
        let departments: BTreeSet<_> = self
            .departments
            .iter()
            .map(|unit| unit.id.as_str())
            .collect();
        let people: BTreeSet<_> = self
            .members
            .iter()
            .map(|member| member.person_id.as_str())
            .collect();
        if departments.len() != self.departments.len()
            || people.len() != self.members.len()
            || !valid_references(&self.project_department_ids, &departments)
            || self.members.iter().any(|member| {
                awr_core::PersonId::new(member.person_id.clone()).is_err()
                    || !valid_references(&member.department_ids, &departments)
            })
        {
            return Err(invalid());
        }
        // Hierarchy: parents resolve, chains terminate within the depth bound,
        // and departments sharing one parent (including the top level) carry
        // distinct names after trimming.
        let depths = hierarchy_depths(&self.departments)?;
        if depths.values().any(|depth| *depth > MAX_DEPARTMENT_DEPTH) {
            return Err(invalid());
        }
        let mut sibling_names: BTreeMap<Option<&str>, BTreeSet<&str>> = BTreeMap::new();
        for unit in &self.departments {
            if !sibling_names
                .entry(unit.parent_id.as_deref())
                .or_default()
                .insert(unit.name.trim())
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    pub fn normalized(&self) -> TeamResult<Self> {
        self.validate()?;
        let mut plan = self.clone();
        plan.departments
            .sort_by(|left, right| left.id.cmp(&right.id));
        plan.project_department_ids.sort();
        plan.members
            .sort_by(|left, right| left.person_id.cmp(&right.person_id));
        for member in &mut plan.members {
            member.department_ids.sort();
        }
        Ok(plan)
    }

    pub fn digest(&self) -> TeamResult<String> {
        request_hash(&serde_json::to_value(self.normalized()?).map_err(|_| invalid())?)
    }

    /// Deepest department chain, counting the department itself: a directory of
    /// flat top-level departments has depth 1, an empty one 0. Reports a cycle
    /// as an error so an unvalidated plan is never summarized with a capped number.
    pub fn department_depth(&self) -> TeamResult<usize> {
        Ok(hierarchy_depths(&self.departments)?
            .values()
            .copied()
            .max()
            .unwrap_or(0))
    }

    /// Count of leaf departments (departments with no children). The product
    /// presents leaf departments as groups; a project attached under a group
    /// lists that leaf in `project_department_ids`.
    pub fn leaf_department_count(&self) -> usize {
        let parents: BTreeSet<&str> = self
            .departments
            .iter()
            .filter_map(|unit| unit.parent_id.as_deref())
            .collect();
        self.departments
            .iter()
            .filter(|unit| !parents.contains(unit.id.as_str()))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> ProjectDirectoryPlan {
        ProjectDirectoryPlan {
            organization: Some(DirectoryUnit {
                id: "company".into(),
                name: "Example company".into(),
            }),
            departments: vec![
                DirectoryDepartment {
                    id: "engineering".into(),
                    name: "Engineering".into(),
                    parent_id: None,
                },
                DirectoryDepartment {
                    id: "research".into(),
                    name: "Research".into(),
                    parent_id: None,
                },
            ],
            project_department_ids: vec!["research".into(), "engineering".into()],
            members: vec![DirectoryMember {
                person_id: "member_one".into(),
                department_ids: vec!["research".into(), "engineering".into()],
            }],
        }
    }

    fn department(id: &str, name: &str, parent_id: Option<&str>) -> DirectoryDepartment {
        DirectoryDepartment {
            id: id.into(),
            name: name.into(),
            parent_id: parent_id.map(str::to_owned),
        }
    }

    fn hierarchical() -> ProjectDirectoryPlan {
        ProjectDirectoryPlan {
            organization: Some(DirectoryUnit {
                id: "company".into(),
                name: "Example company".into(),
            }),
            departments: vec![
                department("engineering", "Engineering", None),
                department("platform", "Platform", Some("engineering")),
                department("runtime", "Runtime", Some("platform")),
                department("scheduling", "Scheduling", Some("runtime")),
            ],
            project_department_ids: vec!["scheduling".into()],
            members: vec![DirectoryMember {
                person_id: "member_one".into(),
                department_ids: vec!["scheduling".into()],
            }],
        }
    }

    #[test]
    fn order_does_not_change_the_plan_digest() {
        let original = plan();
        let mut reordered = original.clone();
        reordered.departments.reverse();
        reordered.project_department_ids.reverse();
        reordered.members[0].department_ids.reverse();
        assert_eq!(original.digest().unwrap(), reordered.digest().unwrap());
    }

    #[test]
    fn renames_keep_identity_and_change_the_version_basis() {
        let original = plan();
        let mut renamed = original.clone();
        renamed.departments[0].name = "Product engineering".into();
        assert_eq!(original.departments[0].id, renamed.departments[0].id);
        assert_ne!(original.digest().unwrap(), renamed.digest().unwrap());
    }

    #[test]
    fn reparenting_changes_the_version_basis() {
        let original = hierarchical();
        let mut moved = original.clone();
        moved.departments[1].parent_id = None;
        assert_ne!(original.digest().unwrap(), moved.digest().unwrap());
    }

    #[test]
    fn same_names_do_not_merge_department_references() {
        let mut value = hierarchical();
        value.departments[1].name = value.departments[0].name.clone();
        assert_eq!(value.normalized().unwrap().departments.len(), 4);
    }

    #[test]
    fn rejects_duplicate_and_undeclared_relationships() {
        let mut value = plan();
        value.members.push(value.members[0].clone());
        assert!(value.validate().is_err());
        let mut value = plan();
        value.members[0].department_ids.push("unregistered".into());
        assert!(value.validate().is_err());
        let mut value = plan();
        value.project_department_ids.push("engineering".into());
        assert!(value.validate().is_err());
        let mut value = plan();
        value.departments[1].id = value.departments[0].id.clone();
        assert!(value.validate().is_err());
    }

    #[test]
    fn clearing_grouping_is_explicit_and_does_not_retain_children() {
        assert!(ProjectDirectoryPlan::default().validate().is_ok());
        let mut value = plan();
        value.organization = None;
        assert!(value.validate().is_err());
    }

    #[test]
    fn rejects_unbounded_or_authority_shaped_input() {
        let mut value = plan();
        value.members = (0..257)
            .map(|n| DirectoryMember {
                person_id: format!("person_{n}"),
                department_ids: vec![],
            })
            .collect();
        assert!(value.validate().is_err());
        let mut json = serde_json::to_value(plan()).unwrap();
        json["grants"] = serde_json::json!(["admin"]);
        assert!(serde_json::from_value::<ProjectDirectoryPlan>(json).is_err());
        let mut json = serde_json::to_value(plan()).unwrap();
        json["organization"]["parent_id"] = serde_json::json!("engineering");
        assert!(serde_json::from_value::<ProjectDirectoryPlan>(json).is_err());
    }

    #[test]
    fn existing_opaque_person_ids_keep_the_core_identity_contract() {
        let mut value = plan();
        value.members[0].person_id = "member@example.com".into();
        assert!(value.validate().is_ok());
        value.members[0].person_id = "成员甲".into();
        assert!(value.validate().is_ok());
        value.members[0].person_id = "member\nother".into();
        assert!(value.validate().is_err());
    }

    #[test]
    fn hierarchy_depth_is_bounded_and_summarized_from_the_plan() {
        let value = hierarchical();
        value.validate().unwrap();
        assert_eq!(value.department_depth().unwrap(), MAX_DEPARTMENT_DEPTH);
        assert_eq!(value.leaf_department_count(), 1);
        assert_eq!(plan().department_depth().unwrap(), 1);
        assert_eq!(plan().leaf_department_count(), 2);
        assert_eq!(
            ProjectDirectoryPlan::default().department_depth().unwrap(),
            0
        );
        let mut deep = hierarchical();
        deep.departments
            .push(department("fair_share", "Fair share", Some("scheduling")));
        assert!(deep.validate().is_err());
        assert!(deep.digest().is_err());
    }

    #[test]
    fn parent_chains_must_terminate_at_a_declared_department() {
        let mut cycle = hierarchical();
        cycle.departments[0].parent_id = Some("scheduling".into());
        assert!(cycle.validate().is_err());
        assert!(cycle.department_depth().is_err());
        // A minimal two-node cycle must fail on its own, not only via the
        // depth bound a longer cycle would trip.
        let mut pair = hierarchical();
        pair.departments[1].parent_id = Some("runtime".into());
        pair.departments[2].parent_id = Some("platform".into());
        assert!(pair.validate().is_err());
        assert!(pair.department_depth().is_err());
        let mut self_parent = hierarchical();
        self_parent.departments[0].parent_id = Some("engineering".into());
        assert!(self_parent.validate().is_err());
        let mut missing = hierarchical();
        missing.departments[1].parent_id = Some("unregistered".into());
        assert!(missing.validate().is_err());
    }

    #[test]
    fn departments_sharing_a_parent_need_unique_trimmed_names() {
        let mut siblings = hierarchical();
        // Two children of "platform" whose names only differ by trim → collision.
        siblings
            .departments
            .push(department("observability", " Runtime ", Some("platform")));
        assert!(siblings.validate().is_err());
        let mut branched = hierarchical();
        branched
            .departments
            .push(department("research", "Research", None));
        branched
            .departments
            .push(department("research_runtime", "Runtime", Some("research")));
        branched.validate().unwrap();
        assert_eq!(branched.leaf_department_count(), 2);
    }

    #[test]
    fn directories_recorded_before_the_hierarchy_read_as_all_top_level() {
        let legacy = serde_json::json!({"organization":{"id":"company","name":"Example company"},
            "departments":[{"id":"engineering","name":"Engineering"},
                {"id":"research","name":"Research"}],
            "project_department_ids":["engineering"],
            "members":[{"person_id":"member_one","department_ids":["engineering"]}]});
        let parsed: ProjectDirectoryPlan = serde_json::from_value(legacy.clone()).unwrap();
        parsed.validate().unwrap();
        assert!(
            parsed
                .departments
                .iter()
                .all(|unit| unit.parent_id.is_none())
        );
        assert_eq!(parsed.department_depth().unwrap(), 1);
        // Flat plans serialize without parent references, so the stored shape and
        // the digest stay byte-identical to directories recorded before the field.
        assert_eq!(serde_json::to_value(&parsed).unwrap(), legacy);
        assert_eq!(parsed.digest().unwrap(), request_hash(&legacy).unwrap());
        let explicit_null: ProjectDirectoryPlan = serde_json::from_value(
            serde_json::json!({"organization":{"id":"company","name":"Example company"},
                "departments":[{"id":"engineering","name":"Engineering","parent_id":null},
                    {"id":"research","name":"Research"}],
                "project_department_ids":["engineering"],
                "members":[{"person_id":"member_one","department_ids":["engineering"]}]}),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&explicit_null).unwrap(),
            serde_json::to_value(&parsed).unwrap()
        );
    }
}
