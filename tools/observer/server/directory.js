'use strict';

const object = (value) => value && typeof value === 'object' && !Array.isArray(value);
const text = (value) => typeof value === 'string' && value.length > 0 && value.length <= 512;
const unique = (values) => new Set(values).size === values.length;
const same = (left, right) => left.length === right.length && left.every((value) => right.includes(value));

/** Validate the full relationship set before exposing its product projection. */
function projectDirectory(value) {
  if (!object(value) || !text(value.project_id) || typeof value.configured !== 'boolean'
    || typeof value.version !== 'string' || value.version.length > 19 || !/^(0|[1-9][0-9]*)$/.test(value.version)
    || !(value.updated_at_ms === null || Number.isSafeInteger(value.updated_at_ms) && value.updated_at_ms > 0)) return null;
  const plan = value.plan;
  const unit = (item) => object(item) && text(item.id) && text(item.name);
  if (!object(plan) || !(plan.organization === null || unit(plan.organization))
    || !Array.isArray(plan.departments) || plan.departments.length > 128 || !plan.departments.every(unit)
    || !unique(plan.departments.map((item) => item.id))
    || !Array.isArray(plan.members) || plan.members.length > 256
    || !Array.isArray(value.members) || value.members.length !== plan.members.length) return null;
  const maxDepartmentDepth = 4;
  const departments = new Map(plan.departments.map((item) => [item.id, {
    key: item.id, name: item.name, parentKey: item.parent_id ?? null,
  }]));
  if (!plan.departments.every((item) => (item.parent_id === undefined || item.parent_id === null
    || text(item.parent_id) && departments.has(item.parent_id) && item.parent_id !== item.id))) return null;
  for (const item of plan.departments) {
    const visited = new Set([item.id]);
    let current = departments.get(item.id);
    let depth = 1;
    while (current.parentKey !== null) {
      if (visited.has(current.parentKey) || depth >= maxDepartmentDepth) return null;
      visited.add(current.parentKey);
      current = departments.get(current.parentKey);
      depth += 1;
    }
  }
  const references = (items) => Array.isArray(items) && unique(items) && items.every((id) => departments.has(id));
  if (!references(plan.project_department_ids) || !plan.members.every((member) => object(member)
    && text(member.person_id) && references(member.department_ids)) || !unique(plan.members.map((member) => member.person_id))) return null;
  const expected = new Map(plan.members.map((member) => [member.person_id, member.department_ids]));
  if (!value.members.every((member) => object(member) && object(member.member_ref)
    && member.member_ref.project_id === value.project_id && expected.has(member.member_ref.person_id)
    && typeof member.available === 'boolean' && (member.display_name === null || text(member.display_name))
    && (!member.available ? member.display_name === null : text(member.display_name))
    && (member.duty === undefined || text(member.duty))
    && references(member.department_ids) && same(member.department_ids, expected.get(member.member_ref.person_id)))
    || !unique(value.members.map((member) => member.member_ref.person_id))) return null;
  const empty = plan.organization === null && plan.departments.length === 0
    && plan.project_department_ids.length === 0 && plan.members.length === 0;
  if (!value.configured && (value.version !== '0' || value.updated_at_ms !== null || !empty)
    || value.configured && (value.version === '0' || value.updated_at_ms === null)
    || plan.organization === null && !empty) return null;
  return {
    configured: value.configured,
    organization: plan.organization?.name ?? null,
    departments: plan.project_department_ids.map((id) => departments.get(id)),
    updated_at_ms: value.updated_at_ms,
    members: value.members.map((member) => ({
      personId: member.member_ref.person_id,
      name: member.display_name,
      available: member.available,
      departments: member.department_ids.map((id) => departments.get(id)),
      // Demo-only optional recorded duty; live directory replies do not provide it yet.
      ...(member.duty === undefined ? {} : { duty: member.duty }),
    })),
  };
}

module.exports = { projectDirectory };
