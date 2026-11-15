import { Flex, Segmented } from 'antd';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { DependencyObservations } from '../components/DependencyObservations';
import { ProjectMaps } from './ProjectMaps';

export function ProjectDependencies({ project }: { project: string }) {
  const { t } = useTranslation();
  const [view, setView] = useState('map');
  return <Flex vertical gap={16}>
    <Segmented value={view} onChange={setView} options={[
      { value: 'map', label: t('collaboration.graph') },
      { value: 'list', label: t('collaboration.list') },
    ]} />
    {view === 'map' ? <ProjectMaps project={project} /> : <DependencyObservations key={project} project={project} />}
  </Flex>;
}
