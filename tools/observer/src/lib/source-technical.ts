import type { TechnicalDetailItem } from '../components/TechnicalDetails';
import type { WorkSnapshotData } from './types';

/** Which read a plan page shows, field by field with its exact value: engineering facts for the technical details only. */
export function sourceStateTechnicalItems(result?: {
  source_snapshot_id?: unknown;
  project_revision?: unknown;
  coordinator_epoch?: unknown;
  authority_version?: unknown;
  selection_basis?: unknown;
  data?: WorkSnapshotData;
}): TechnicalDetailItem[] {
  return [
    { label: 'source_snapshot_id', value: result?.source_snapshot_id, testId: 'source-snapshot-id' },
    { label: 'project_revision', value: result?.project_revision, testId: 'source-project-revision' },
    { label: 'coordinator_epoch', value: result?.coordinator_epoch, testId: 'source-coordinator-epoch' },
    { label: 'contract_hash', value: result?.data?.contract_hash },
    { label: 'definition_state', value: result?.data?.definition_state },
    { label: 'authority_version', value: result?.authority_version },
    { label: 'selection_basis', value: result?.selection_basis },
  ];
}
