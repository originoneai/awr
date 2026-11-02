import type { TechnicalDetailItem } from '../components/TechnicalDetails';
import type { TeamIdentity } from './types';

/** The connection's exact identity facts for the technical details: ids, names and flags as the service gave them. */
export function connectionTechnicalItems(identity: TeamIdentity | undefined, project: string | null): TechnicalDetailItem[] {
  return [
    { label: 'actor_id', value: identity?.actor_id },
    { label: 'client_id', value: identity?.client_id },
    { label: 'project', value: project },
    { label: 'role', value: identity?.role },
    { label: 'business_roles', value: identity?.business_roles },
    { label: 'membership_action_ceiling', value: identity?.membership_action_ceiling },
    { label: 'can_manage_members', value: identity?.can_manage_members },
    { label: 'can_read_project_audit', value: identity?.can_read_project_audit },
  ];
}
