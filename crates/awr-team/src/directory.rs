//! Project-qualified organization metadata. Grouping never grants access or
//! identifies people across projects.
use crate::{TeamError, TeamResult, request_hash};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryUnit {
    pub id: String,
    pub name: String,
}

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
    pub departments: Vec<DirectoryUnit>,
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

fn valid_references(values: &[String], departments: &BTreeSet<&str>) -> bool {
    let unique: BTreeSet<_> = values.iter().map(String::as_str).collect();
    unique.len() == values.len() && unique.is_subset(departments)
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
            || self.departments.iter().any(|unit| !valid_unit(unit))
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
                DirectoryUnit {
                    id: "engineering".into(),
                    name: "Engineering".into(),
                },
                DirectoryUnit {
                    id: "research".into(),
                    name: "Research".into(),
                },
            ],
            project_department_ids: vec!["research".into(), "engineering".into()],
            members: vec![DirectoryMember {
                person_id: "member_one".into(),
                department_ids: vec!["research".into(), "engineering".into()],
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
    fn same_names_do_not_merge_department_references() {
        let mut value = plan();
        value.departments[1].name = value.departments[0].name.clone();
        assert_eq!(value.normalized().unwrap().departments.len(), 2);
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
}
