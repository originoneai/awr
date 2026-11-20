import { Modal } from 'antd';
import type { TFunction } from 'i18next';
import { Search } from 'lucide-react';
import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from 'react';
import { useTranslation } from 'react-i18next';
import { useNavigate } from 'react-router';
import {
  buildSearchIndex,
  recentSearchItems,
  SEARCH_KINDS,
  SEARCH_RECENT_LIMIT,
  searchIndex,
  type SearchItem,
  type SearchKind,
  type SearchSource,
} from '../lib/search-index';
import './search-panel.css';

export const SEARCH_RECENT_STORAGE_KEY = 'awr-observer-search-recent-v1';

export function readRecentSearchKeys(): string[] {
  try {
    const value: unknown = JSON.parse(window.localStorage.getItem(SEARCH_RECENT_STORAGE_KEY) ?? '[]');
    if (!Array.isArray(value)) return [];
    // A bounded raw list leaves enough room to discard stale and duplicate references before taking six.
    return value.filter((entry): entry is string => typeof entry === 'string' && entry.length <= 2048)
      .slice(0, 60);
  } catch {
    return [];
  }
}

function writeRecentSearchKeys(keys: readonly string[]) {
  try {
    window.localStorage.setItem(SEARCH_RECENT_STORAGE_KEY, JSON.stringify(keys.slice(0, SEARCH_RECENT_LIMIT)));
  } catch {
    // The open action still works when persistence is blocked or full.
  }
}

interface SearchDialogProps {
  open: boolean;
  sources: readonly SearchSource[];
  loading?: boolean;
  onClose: () => void;
  afterClose?: () => void;
  initialQuery?: string;
}

function coverageText(sources: readonly SearchSource[], failed: boolean, showFailure: boolean, t: TFunction): string {
  const kindLabel = (kind: SearchKind) => t(`shell.searchTypes.${kind}`);
  const loaded = sources.filter((source) => source.coverage !== 'unread');
  const unavailable = SEARCH_KINDS.filter((kind) => sources.find((source) => source.kind === kind)?.coverage === 'unread');
  const partial = sources.some((source) => source.coverage === 'partial');
  const fragments: string[] = [];
  if (loaded.length > 0) {
    fragments.push(t(partial ? 'shell.searchCoveragePartial' : 'shell.searchCoverageLoaded', {
      types: loaded.map((source) => kindLabel(source.kind)).join(t('common.listSeparator')),
    }));
  } else {
    fragments.push(t('shell.searchCoverageNone'));
  }
  if (unavailable.length > 0) {
    fragments.push(t('shell.searchCoverageUnavailable', { types: unavailable.map(kindLabel).join(t('common.listSeparator')) }));
  }
  if (failed && !showFailure) fragments.push(t('shell.searchReadFailed'));
  return fragments.join('；');
}

/** Prototype search surface; Ant Design Modal owns the modal focus boundary and backdrop. */
export function SearchDialog({ open, sources, loading = false, onClose, afterClose, initialQuery = '' }: SearchDialogProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [query, setQuery] = useState(initialQuery);
  const [activeKey, setActiveKey] = useState<string | null>(null);
  const [recentKeys, setRecentKeys] = useState<string[]>([]);
  const inputRef = useRef<HTMLInputElement>(null);
  const instanceId = useId();
  const listId = `search-results-${instanceId}`;
  const coverageId = `search-coverage-${instanceId}`;
  const index = useMemo(() => buildSearchIndex(sources), [sources]);
  const failed = sources.some((source) => source.failed);
  const hasLoadedSource = sources.some((source) => source.coverage !== 'unread');
  const showFailure = failed && !hasLoadedSource;
  const items = useMemo(() => (
    query.trim() ? searchIndex(index, query) : recentSearchItems(index, recentKeys)
  ), [index, query, recentKeys]);
  const activeIndex = Math.max(0, items.findIndex((item) => item.key === activeKey));
  const activeItem = items[activeIndex] ?? items[0];
  const footer = coverageText(sources, failed, showFailure, t);

  useEffect(() => {
    if (!open) return;
    setQuery(initialQuery);
    setActiveKey(null);
    setRecentKeys(readRecentSearchKeys());
    inputRef.current?.focus();
  }, [open, initialQuery]);

  const openItem = (item: SearchItem | undefined) => {
    if (!item) return;
    const nextRecent = [item.key, ...recentKeys.filter((key) => key !== item.key)].slice(0, SEARCH_RECENT_LIMIT);
    setRecentKeys(nextRecent);
    writeRecentSearchKeys(nextRecent);
    onClose();
    navigate(item.path);
  };

  const onInputKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      onClose();
      return;
    }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp' || event.key === 'Home' || event.key === 'End') {
      event.preventDefault();
      if (items.length === 0) return;
      const nextIndex = event.key === 'Home' ? 0
        : event.key === 'End' ? items.length - 1
          : event.key === 'ArrowDown' ? (activeIndex + 1) % items.length
            : (activeIndex - 1 + items.length) % items.length;
      setActiveKey(items[nextIndex]?.key ?? null);
      return;
    }
    if (event.key === 'Enter') {
      event.preventDefault();
      openItem(activeItem ?? items[0]);
    }
  };

  return (
    <Modal
      open={open}
      onCancel={onClose}
      afterClose={afterClose}
      afterOpenChange={(visible) => {
        if (visible) inputRef.current?.focus();
      }}
      width={560}
      title={t('shell.search')}
      footer={null}
      closable={false}
      destroyOnHidden
      keyboard
      transitionName=""
      maskTransitionName=""
      mask={{ closable: true }}
      focusable={{ focusTriggerAfterClose: false }}
      className="search-dialog"
      rootClassName="search-dialog-root"
      classNames={{ container: 'sr', mask: 'sr-back', body: 'sr-body' }}
      style={{ top: '12vh', margin: '0 auto', maxWidth: 'calc(100vw - 32px)', paddingBottom: 0 }}
      aria-label={t('shell.search')}
    >
      <div className="sr-top">
        <Search size={16} strokeWidth={1.5} aria-hidden />
        <input
          ref={inputRef}
          className="sr-input"
          type="text"
          role="combobox"
          aria-label={t('shell.search')}
          aria-autocomplete="list"
          aria-expanded={open}
          aria-controls={items.length > 0 ? listId : undefined}
          aria-activedescendant={items.length > 0 ? `search-option-${instanceId}-${activeIndex}` : undefined}
          aria-describedby={coverageId}
          aria-haspopup="listbox"
          autoFocus
          placeholder={t('shell.searchPlaceholder')}
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            setActiveKey(null);
          }}
          onKeyDown={onInputKeyDown}
        />
        <span className="kbd" aria-hidden>Esc</span>
      </div>
      <span className="sr-only" role="status" aria-live="polite">
        {loading ? t('shell.searchLoading') : t('shell.searchResultCount', { count: items.length })}
      </span>
      <div className="sr-list" aria-busy={loading}>
        {loading ? <div className="empty" role="status">{t('shell.searchLoading')}</div>
          : showFailure ? <div className="empty" role="status">{t('shell.searchReadFailed')}</div>
            : items.length === 0 ? (
              <div className="empty" role="status">
                {t(query.trim() ? 'shell.searchNoResults' : 'shell.searchRecentEmpty')}
              </div>
            ) : (
              <div role="listbox" id={listId} aria-label={t('shell.searchResults')}>
                {items.map((item, itemIndex) => (
                  <button
                    type="button"
                    role="option"
                    id={`search-option-${instanceId}-${itemIndex}`}
                    aria-selected={itemIndex === activeIndex}
                    className="sr-row"
                    key={item.key}
                    onMouseDown={(event) => event.preventDefault()}
                    onMouseEnter={() => setActiveKey(item.key)}
                    onClick={() => openItem(item)}
                  >
                    <span className="tag">{t(`shell.searchTypes.${item.kind}`)}</span>
                    <bdi className="sr-tt">{item.title}</bdi>
                    <bdi className="sr-mm">{item.meta}</bdi>
                  </button>
                ))}
              </div>
            )}
      </div>
      <div className="sr-coverage" id={coverageId}>{footer}</div>
    </Modal>
  );
}
