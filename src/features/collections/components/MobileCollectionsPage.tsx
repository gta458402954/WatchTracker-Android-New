import { useEffect, useMemo, useState } from 'react';
import type { WatchCollection, WatchRecord } from '../../../shared/types';
import { displayTitlesOf } from '../../../shared/lib/displayTitle';
import SafePosterImage from '../../watchlist/components/SafePosterImage';
import { useCollections } from '../hooks/useCollections';

interface MobileCollectionsPageProps {
  records: WatchRecord[];
  onDetail: (record: WatchRecord) => void;
  onNotify: (tone: 'success' | 'warning' | 'error' | 'info', message: string) => void;
}

function titleOf(record: WatchRecord): string {
  const titles = displayTitlesOf(record);
  return titles.primary || titles.secondary || '未命名记录';
}

function kindLabel(kind: WatchCollection['collectionKind']): string {
  switch (kind) {
    case 'universe':
      return '电影宇宙';
    case 'movie-series':
      return '电影系列';
    case 'tv-series':
      return '剧集系列';
    case 'manual':
    default:
      return '主题片单';
  }
}

export default function MobileCollectionsPage({ records, onDetail, onNotify }: MobileCollectionsPageProps) {
  const {
    collections,
    members,
    refresh,
    create,
    remove,
    addMembers,
    removeMember,
  } = useCollections();

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const [selectedCollectionId, setSelectedCollectionId] = useState<string | null>(null);
  const [showCreateSheet, setShowCreateSheet] = useState(false);
  const [newTitle, setNewTitle] = useState('');
  const [newDesc, setNewDesc] = useState('');
  const [newKind, setNewKind] = useState<WatchCollection['collectionKind']>('universe');
  const [showAddMembersSheet, setShowAddMembersSheet] = useState(false);
  const [memberSearch, setMemberSearch] = useState('');

  // Map recordId to WatchRecord for fast lookup
  const recordMap = useMemo(() => new Map(records.map(r => [r.id, r])), [records]);

  // Active collection
  const activeCollection = useMemo(
    () => collections.find(c => c.id === selectedCollectionId) ?? null,
    [collections, selectedCollectionId],
  );

  // Members of active collection
  const activeMembers = useMemo(() => {
    if (!selectedCollectionId) return [];
    return members
      .filter(m => m.collectionId === selectedCollectionId)
      .sort((a, b) => a.position - b.position);
  }, [members, selectedCollectionId]);

  const activeRecords = useMemo(() => {
    return activeMembers
      .map(m => ({ member: m, record: recordMap.get(m.recordId) }))
      .filter((item): item is { member: typeof item.member; record: WatchRecord } => Boolean(item.record));
  }, [activeMembers, recordMap]);

  // Collections with stats
  const collectionCards = useMemo(() => {
    return collections.map(col => {
      const colMembers = members.filter(m => m.collectionId === col.id);
      const colRecords = colMembers.map(m => recordMap.get(m.recordId)).filter(Boolean) as WatchRecord[];
      const completedCount = colRecords.filter(r => r.status === '已看').length;
      const totalCount = colRecords.length;
      const progressPct = totalCount > 0 ? Math.round((completedCount / totalCount) * 100) : 0;
      const previewPosters = colRecords
        .map(r => r.posterPath)
        .filter(Boolean)
        .slice(0, 4) as string[];

      return {
        collection: col,
        totalCount,
        completedCount,
        progressPct,
        previewPosters,
      };
    });
  }, [collections, members, recordMap]);

  const handleCreate = async () => {
    if (!newTitle.trim()) {
      onNotify('warning', '请输入合辑名称');
      return;
    }
    try {
      const created = await create(newTitle.trim(), newDesc.trim() || null, newKind);
      setNewTitle('');
      setNewDesc('');
      setShowCreateSheet(false);
      setSelectedCollectionId(created.id);
      onNotify('success', `合辑“${created.name}”已创建`);
    } catch {
      onNotify('error', '创建合辑失败');
    }
  };

  const handleDelete = async (col: WatchCollection) => {
    if (!window.confirm(`确定删除合辑“${col.name}”吗？合辑内的影视记录仍会保存在片库中。`)) return;
    try {
      await remove(col);
      setSelectedCollectionId(null);
      onNotify('success', `合辑“${col.name}”已删除`);
    } catch {
      onNotify('error', '删除合辑失败');
    }
  };

  // Records available to add to active collection
  const existingMemberIds = useMemo(() => new Set(activeMembers.map(m => m.recordId)), [activeMembers]);
  const candidatesToAdd = useMemo(() => {
    return records
      .filter(r => !existingMemberIds.has(r.id))
      .filter(r => {
        if (!memberSearch.trim()) return true;
        const q = memberSearch.toLowerCase();
        return titleOf(r).toLowerCase().includes(q) || (r.originalName || '').toLowerCase().includes(q);
      })
      .slice(0, 20);
  }, [existingMemberIds, memberSearch, records]);

  const handleAddMember = async (record: WatchRecord) => {
    if (!activeCollection) return;
    try {
      await addMembers(activeCollection, [record.id]);
      onNotify('success', `已添加“${titleOf(record)}”`);
    } catch {
      onNotify('error', '添加失败');
    }
  };

  const handleRemoveMember = async (member: (typeof activeMembers)[number], record: WatchRecord) => {
    try {
      await removeMember(member);
      onNotify('success', `已从合辑移出“${titleOf(record)}”`);
    } catch {
      onNotify('error', '移出失败');
    }
  };

  // If viewing details of a specific collection
  if (activeCollection) {
    const completedCount = activeRecords.filter(item => item.record.status === '已看').length;
    const totalCount = activeRecords.length;
    const pct = totalCount > 0 ? Math.round((completedCount / totalCount) * 100) : 0;

    return (
      <div className="mobile-collections-page" aria-labelledby="mobile-col-detail-title">
        <header className="mobile-page-header">
          <button
            type="button"
            className="mobile-icon-button"
            aria-label="返回合辑列表"
            onClick={() => setSelectedCollectionId(null)}
          >
            ‹
          </button>
          <h1 id="mobile-col-detail-title" className="mobile-col-detail-heading">{activeCollection.name}</h1>
          <button
            type="button"
            className="mobile-icon-button mobile-danger"
            aria-label="删除合辑"
            title="删除合辑"
            onClick={() => void handleDelete(activeCollection)}
          >
            🗑
          </button>
        </header>

        {/* Collection Hero Card */}
        <section className="mobile-col-hero">
          <div className="mobile-col-hero-main">
            <span className="mobile-col-badge">
              {kindLabel(activeCollection.collectionKind)}
            </span>
            <h2>{activeCollection.name}</h2>
            {activeCollection.description && <p>{activeCollection.description}</p>}
          </div>

          <div className="mobile-col-progress-box">
            <div className="progress-labels">
              <span>观影进度</span>
              <strong>{completedCount} / {totalCount} 部 ({pct}%)</strong>
            </div>
            <div className="mobile-stats-progress-track" style={{ height: '8px' }}>
              <div
                className="mobile-stats-progress-seg completed"
                style={{ width: `${pct}%`, background: '#10b981' }}
              />
            </div>
          </div>

          <div className="mobile-col-actions-bar">
            <button
              type="button"
              className="mobile-primary-button"
              onClick={() => setShowAddMembersSheet(true)}
            >
              ＋ 添加影视记录
            </button>
          </div>
        </section>

        {/* Member Records List */}
        <div className="mobile-col-records-list">
          {activeRecords.length === 0 ? (
            <div className="mobile-empty-hint">
              <span>🎞️</span>
              <p>合辑中暂无影视记录，点击上方按钮添加。</p>
            </div>
          ) : (
            activeRecords.map(({ member, record }, idx) => {
              const itemTitle = titleOf(record);
              return (
                <div key={member.id} className="mobile-col-record-row">
                  <span className="row-index">{String(idx + 1).padStart(2, '0')}</span>
                  <div
                    className="row-main"
                    onClick={() => onDetail(record)}
                    role="button"
                    tabIndex={0}
                  >
                    {record.posterPath ? (
                      <SafePosterImage
                        posterPath={record.posterPath}
                        alt={itemTitle}
                        className="row-poster"
                        compact
                        autoDownload={false}
                      />
                    ) : (
                      <div className="row-poster-placeholder">🎬</div>
                    )}
                    <div className="row-info">
                      <div className="row-title">{itemTitle}</div>
                      <div className="row-meta">
                        <span>{record.mediaType}</span>
                        {record.releaseYear && <span>{record.releaseYear}</span>}
                        {record.rating != null && <span>⭐ {record.rating}</span>}
                      </div>
                    </div>
                    <span className={`row-status status-${record.status}`}>{record.status}</span>
                  </div>
                  <button
                    type="button"
                    className="row-remove-btn"
                    aria-label={`从合辑移出 ${itemTitle}`}
                    title="从合辑移出"
                    onClick={() => void handleRemoveMember(member, record)}
                  >
                    ✕
                  </button>
                </div>
              );
            })
          )}
        </div>

        {/* Add Members Sheet */}
        {showAddMembersSheet && (
          <div className="mobile-sheet-backdrop" role="presentation" onMouseDown={e => { if (e.target === e.currentTarget) setShowAddMembersSheet(false); }}>
            <div className="mobile-sheet" role="dialog" aria-modal="true" aria-labelledby="add-members-title">
              <div className="mobile-sheet-header">
                <h2 id="add-members-title">添加记录至合辑</h2>
                <button type="button" className="mobile-icon-button" onClick={() => setShowAddMembersSheet(false)}>×</button>
              </div>

              <div className="mobile-search-row" style={{ marginTop: '10px' }}>
                <input
                  type="text"
                  placeholder="搜索片库标题…"
                  value={memberSearch}
                  onChange={e => setMemberSearch(e.target.value)}
                  style={{ width: '100%', minHeight: '44px', padding: '0 12px', border: '1px solid #dbe3ef', borderRadius: '12px' }}
                />
              </div>

              <div className="mobile-add-candidates-list" style={{ maxHeight: '50vh', overflowY: 'auto', marginTop: '10px' }}>
                {candidatesToAdd.map(r => (
                  <div key={r.id} className="candidate-item" style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '10px', borderBottom: '1px solid #f1f5f9' }}>
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div style={{ fontSize: '14px', fontWeight: 700, color: '#1e293b', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{titleOf(r)}</div>
                      <small style={{ color: '#64748b' }}>{r.mediaType} · {r.releaseYear || '未知年份'}</small>
                    </div>
                    <button
                      type="button"
                      className="mobile-quiet-button"
                      style={{ minHeight: '36px', padding: '0 12px' }}
                      onClick={() => void handleAddMember(r)}
                    >
                      ＋ 加入
                    </button>
                  </div>
                ))}
              </div>
            </div>
          </div>
        )}
      </div>
    );
  }

  // Collections Gallery List
  return (
    <div className="mobile-collections-page" aria-labelledby="mobile-collections-title">
      <header className="mobile-page-heading">
        <div>
          <h1 id="mobile-collections-title">收藏合辑</h1>
          <p>电影宇宙、自建主题片单与系列观影流</p>
        </div>
        <button
          type="button"
          className="mobile-primary-button"
          style={{ minHeight: '40px', padding: '0 14px', fontSize: '13px' }}
          onClick={() => setShowCreateSheet(true)}
        >
          ＋ 新建合辑
        </button>
      </header>

      {/* Grid of Collection Cards */}
      {collectionCards.length === 0 ? (
        <section className="mobile-empty" aria-live="polite">
          <span aria-hidden="true">♡</span>
          <h2>暂无自定义收藏合辑</h2>
          <p>你可以将漫威宇宙、哈利波特、诺兰作品或个人年度最佳归整为合辑随时查阅。</p>
          <button
            type="button"
            className="mobile-primary-button"
            onClick={() => setShowCreateSheet(true)}
          >
            创建第一个合辑
          </button>
        </section>
      ) : (
        <div className="mobile-collections-grid">
          {collectionCards.map(({ collection, totalCount, completedCount, progressPct, previewPosters }) => (
            <article
              key={collection.id}
              className="mobile-col-card"
              onClick={() => setSelectedCollectionId(collection.id)}
              role="button"
              tabIndex={0}
            >
              {/* Overlapping poster stack preview */}
              <div className="mobile-col-card-preview">
                {previewPosters.length > 0 ? (
                  <div className="poster-stack">
                    {previewPosters.map((poster, i) => (
                      <div key={poster} className={`stack-item stack-${i}`}>
                        <SafePosterImage
                          posterPath={poster}
                          alt=""
                          className="stack-img"
                          compact
                          autoDownload={false}
                        />
                      </div>
                    ))}
                  </div>
                ) : (
                  <div className="poster-empty-stack">🎬</div>
                )}
              </div>

              <div className="mobile-col-card-body">
                <div className="mobile-col-card-title-row">
                  <span className="col-kind-tag">
                    {kindLabel(collection.collectionKind)}
                  </span>
                  <span className="col-count-tag">{totalCount} 部</span>
                </div>
                <h2 className="mobile-col-card-title">{collection.name}</h2>
                {collection.description && (
                  <p className="mobile-col-card-desc">{collection.description}</p>
                )}

                <div className="mobile-col-card-progress">
                  <div className="progress-bar-track">
                    <div className="progress-bar-fill" style={{ width: `${progressPct}%` }} />
                  </div>
                  <div className="progress-text">
                    已看 {completedCount} / {totalCount} ({progressPct}%)
                  </div>
                </div>
              </div>
            </article>
          ))}
        </div>
      )}

      {/* Create Collection Sheet */}
      {showCreateSheet && (
        <div className="mobile-sheet-backdrop" role="presentation" onMouseDown={e => { if (e.target === e.currentTarget) setShowCreateSheet(false); }}>
          <div className="mobile-sheet" role="dialog" aria-modal="true" aria-labelledby="create-col-title">
            <div className="mobile-sheet-header">
              <h2 id="create-col-title">创建新合辑</h2>
              <button type="button" className="mobile-icon-button" onClick={() => setShowCreateSheet(false)}>×</button>
            </div>

            <form onSubmit={e => { e.preventDefault(); void handleCreate(); }} style={{ marginTop: '14px', display: 'flex', flexDirection: 'column', gap: '12px' }}>
              <label className="mobile-settings-input-group">
                <span>合辑名称</span>
                <input
                  type="text"
                  placeholder="如：漫威电影宇宙 (MCU)、诺兰导演作品集"
                  value={newTitle}
                  onChange={e => setNewTitle(e.target.value)}
                  required
                  autoFocus
                />
              </label>

              <label className="mobile-settings-input-group">
                <span>合辑类型</span>
                <select
                  value={newKind}
                  onChange={e => setNewKind(e.target.value as WatchCollection['collectionKind'])}
                  style={{ minHeight: '44px', borderRadius: '12px', border: '1px solid #dbe3ef', padding: '0 10px', background: 'white' }}
                >
                  <option value="universe">电影宇宙 (MCU、DC 等)</option>
                  <option value="movie-series">电影系列合辑</option>
                  <option value="tv-series">剧集系列合辑</option>
                  <option value="manual">自定义主题片单</option>
                </select>
              </label>

              <label className="mobile-settings-input-group">
                <span>简介或备注 (可选)</span>
                <input
                  type="text"
                  placeholder="简述该系列或片单的主题"
                  value={newDesc}
                  onChange={e => setNewDesc(e.target.value)}
                />
              </label>

              <div className="mobile-sheet-actions" style={{ marginTop: '16px' }}>
                <button type="button" className="mobile-quiet-button" onClick={() => setShowCreateSheet(false)}>取消</button>
                <button type="submit" className="mobile-primary-button">确认创建</button>
              </div>
            </form>
          </div>
        </div>
      )}
    </div>
  );
}
