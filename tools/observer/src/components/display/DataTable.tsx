import { Table, theme } from 'antd';
import type { ColumnsType } from 'antd/es/table';
import { ChevronRight } from 'lucide-react';
import type { KeyboardEvent, MouseEvent, ReactNode } from 'react';
import { useCompactViewport } from '../../lib/viewport';
import { DisplayIcon } from './DisplayIcon';
import { EmptyState } from './EmptyState';
import './display.css';

export interface DataTableColumn {
  key: string;
  title?: ReactNode;
  width?: number | string;
  align?: 'left' | 'right' | 'center';
  hideOnSmall?: boolean;
}

export interface DataTableRow {
  key: string;
  cells: readonly ReactNode[];
  onOpen?: () => void;
  highlighted?: boolean;
}

export interface DataTableProps {
  columns: readonly DataTableColumn[];
  rows: readonly DataTableRow[];
  empty?: ReactNode;
}

function isNestedInteraction(target: EventTarget, row: EventTarget) {
  return target instanceof Element
    && target.closest('button, a, input, select, textarea') !== null
    && target !== row;
}

/** Read-only table with one reserved trailing navigation column only when a row can open. */
export function DataTable({ columns, rows, empty }: DataTableProps) {
  const { token } = theme.useToken();
  const compactViewport = useCompactViewport();
  const visibleColumns = compactViewport ? columns.filter((column) => !column.hideOnSmall) : [...columns];
  const hasOpenableRows = rows.some((row) => Boolean(row.onOpen));
  const tableColumns: ColumnsType<DataTableRow> = visibleColumns.map((column) => ({
    key: column.key,
    title: column.title,
    width: column.width,
    align: column.align,
    className: column.hideOnSmall ? 'obs-table-optional-column' : undefined,
    render: (_value, row) => row.cells[columns.indexOf(column)] ?? null,
  }));

  if (hasOpenableRows) {
    tableColumns.push({
      key: 'open',
      title: '',
      width: token.controlHeight,
      fixed: 'right',
      className: 'obs-table-arrow-column',
      onHeaderCell: () => ({ 'aria-hidden': true }),
      render: (_value, row) => row.onOpen ? <DisplayIcon icon={ChevronRight} /> : null,
    });
  }

  const openWithKeyboard = (event: KeyboardEvent<HTMLTableRowElement>, row: DataTableRow) => {
    if (!row.onOpen || event.key !== 'Enter' || isNestedInteraction(event.target, event.currentTarget)) return;
    event.preventDefault();
    row.onOpen();
  };
  const openWithPointer = (event: MouseEvent<HTMLTableRowElement>, row: DataTableRow) => {
    if (!row.onOpen || isNestedInteraction(event.target, event.currentTarget)) return;
    row.onOpen();
  };

  return (
    <div className="obs-table-scroll" data-compact={compactViewport || undefined}>
      <Table<DataTableRow>
        className="obs-data-table"
        columns={tableColumns}
        dataSource={rows}
        rowKey="key"
        pagination={false}
        tableLayout="fixed"
        showHeader={visibleColumns.some((column) => column.title !== undefined && column.title !== null && column.title !== '')}
        locale={{ emptyText: empty ?? <EmptyState /> }}
        rowClassName={(row) => [
          row.onOpen ? 'obs-table-openable-row' : '',
          row.highlighted ? 'obs-table-highlighted-row' : '',
        ].filter(Boolean).join(' ')}
        onRow={(row) => ({
          tabIndex: row.onOpen ? 0 : undefined,
          'data-openable': row.onOpen ? 'true' : undefined,
          onClick: (event) => openWithPointer(event, row),
          onKeyDown: (event) => openWithKeyboard(event, row),
        })}
      />
    </div>
  );
}
