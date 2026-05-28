import { useState } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { StatCard } from '../components/widgets/StatCard';
import { GovernancePanel } from '../components/widgets/GovernancePanel';
import { Badge, statusVariant } from '../components/shared/Badge';
import { Modal } from '../components/shared/Modal';
import { EmptyState } from '../components/shared/EmptyState';
import {
  fetchGovernance, createPolicy, updatePolicy, deletePolicy,
  type Policy, type TrustScore, type AuditLog,
} from '../lib/api';

const PRISM_LABELS: Record<string, string> = {
  privacy: 'Privacy',
  reliability: 'Reliability',
  integrity: 'Integrity',
  safety: 'Safety',
  misuse: 'Misuse Prevention',
};

function ScoreBar({ score }: { score: number }) {
  const pct = (score / 1000) * 100;
  const color = score >= 800 ? 'bg-green-500' : score >= 500 ? 'bg-yellow-500' : 'bg-red-500';
  return (
    <div className="flex items-center gap-2">
      <div className="flex-1 h-1.5 bg-zinc-700 rounded-full overflow-hidden">
        <div className={`h-full rounded-full ${color}`} style={{ width: `${pct}%` }} />
      </div>
      <span className="text-zinc-300 text-xs font-mono w-10 text-right">{score}</span>
    </div>
  );
}

type Tab = 'queue' | 'policies' | 'trust' | 'audit' | 'prism';

interface PolicyFormData {
  name: string;
  description: string;
  rule: string;
  active: boolean;
}

function PolicyModal({
  open,
  onClose,
  initial,
}: {
  open: boolean;
  onClose: () => void;
  initial?: Policy;
}) {
  const qc = useQueryClient();
  const [form, setForm] = useState<PolicyFormData>(
    initial ?? { name: '', description: '', rule: '', active: true },
  );
  const [error, setError] = useState('');

  const mutation = useMutation({
    mutationFn: (data: PolicyFormData) =>
      initial ? updatePolicy(initial.id, data) : createPolicy(data),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['governance'] });
      onClose();
    },
    onError: (e) => setError(String(e)),
  });

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={initial ? 'Edit Policy' : 'New Policy'}
      size="lg"
      footer={
        <>
          <button onClick={onClose} className="px-3 py-1.5 rounded-lg text-sm bg-zinc-700 text-zinc-300 hover:bg-zinc-600">
            Cancel
          </button>
          <button
            onClick={() => { if (!form.name.trim()) { setError('Name required'); return; } mutation.mutate(form); }}
            disabled={mutation.isPending}
            className="px-3 py-1.5 rounded-lg text-sm bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50"
          >
            {mutation.isPending ? 'Saving...' : 'Save Policy'}
          </button>
        </>
      }
    >
      <div className="space-y-3">
        {error && <div className="px-3 py-2 bg-red-900/30 border border-red-700 rounded text-red-400 text-sm">{error}</div>}
        <div>
          <label className="block text-zinc-400 text-xs mb-1">Name *</label>
          <input className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 focus:outline-none focus:border-blue-500"
            value={form.name} onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))} />
        </div>
        <div>
          <label className="block text-zinc-400 text-xs mb-1">Description</label>
          <input className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 focus:outline-none focus:border-blue-500"
            value={form.description} onChange={(e) => setForm((f) => ({ ...f, description: e.target.value }))} />
        </div>
        <div>
          <label className="block text-zinc-400 text-xs mb-1">Rule (YAML/JSON)</label>
          <textarea className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 focus:outline-none focus:border-blue-500 resize-none h-24 font-mono"
            value={form.rule} onChange={(e) => setForm((f) => ({ ...f, rule: e.target.value }))} />
        </div>
        <label className="flex items-center gap-2 cursor-pointer">
          <input type="checkbox" checked={form.active} onChange={(e) => setForm((f) => ({ ...f, active: e.target.checked }))} className="rounded" />
          <span className="text-zinc-300 text-sm">Active</span>
        </label>
      </div>
    </Modal>
  );
}

function TrustTier({ tier }: { tier: TrustScore['tier'] }) {
  const map: Record<TrustScore['tier'], string> = {
    platinum: 'bg-purple-500/20 text-purple-300',
    gold: 'bg-yellow-500/20 text-yellow-300',
    silver: 'bg-zinc-400/20 text-zinc-300',
    bronze: 'bg-orange-700/20 text-orange-400',
    restricted: 'bg-red-600/20 text-red-400',
  };
  return (
    <span className={`px-2 py-0.5 rounded text-xs font-medium ${map[tier]}`}>{tier}</span>
  );
}

export function Governance() {
  const qc = useQueryClient();
  const [tab, setTab] = useState<Tab>('queue');
  const [policyModal, setPolicyModal] = useState<{ open: boolean; policy?: Policy }>({ open: false });
  const [auditFilter, setAuditFilter] = useState('');

  const { data, isLoading, isError, error } = useQuery({
    queryKey: ['governance'],
    queryFn: fetchGovernance,
    refetchInterval: 15_000,
  });

  const deleteMut = useMutation({
    mutationFn: deletePolicy,
    onSuccess: () => qc.invalidateQueries({ queryKey: ['governance'] }),
  });

  const tabs: { id: Tab; label: string }[] = [
    { id: 'queue', label: 'Approval Queue' },
    { id: 'policies', label: 'Policies' },
    { id: 'trust', label: 'Trust Scores' },
    { id: 'audit', label: 'Audit Log' },
    { id: 'prism', label: 'PRISM' },
  ];

  const pending = (data?.approvals ?? []).filter((a) => a.status === 'pending');
  const filteredLogs = (data?.audit_logs ?? []).filter((l: AuditLog) =>
    !auditFilter || l.agent_name.toLowerCase().includes(auditFilter.toLowerCase()) ||
    l.action.toLowerCase().includes(auditFilter.toLowerCase()),
  );

  return (
    <div className="flex flex-col h-full overflow-y-auto p-4 gap-4">
      <div className="flex items-center justify-between">
        <h2 className="text-zinc-100 font-semibold">Governance</h2>
      </div>

      {isError && (
        <div className="rounded-lg border border-red-500/30 bg-red-900/20 px-3 py-2 text-red-300 text-sm">
          Failed to load governance data: {String(error)}
        </div>
      )}

      {/* Stats */}
      <div className="grid grid-cols-4 gap-3">
        <StatCard label="Pending Approvals" value={pending.length} loading={isLoading} variant={pending.length > 0 ? 'warning' : 'default'} />
        <StatCard label="Policies" value={data?.policies?.length ?? 0} loading={isLoading} />
        <StatCard label="Avg Trust Score" value={
          (data?.trust_scores?.length ?? 0) > 0
            ? Math.round(
                (data?.trust_scores ?? []).reduce((s, t) => s + t.score, 0) /
                  (data?.trust_scores?.length ?? 1),
              )
            : 0
        } loading={isLoading} />
        <StatCard label="Audit Events Today" value={data?.audit_logs?.length ?? 0} loading={isLoading} />
      </div>

      {/* Tabs */}
      <div className="widget-3d rounded-xl bg-zinc-900 border border-zinc-800 overflow-hidden flex-1">
        <div className="flex border-b border-zinc-800">
          {tabs.map((t) => (
            <button
              key={t.id}
              onClick={() => setTab(t.id)}
              className={`px-4 py-2.5 text-sm font-medium transition-colors ${
                tab === t.id
                  ? 'text-blue-400 border-b-2 border-blue-500'
                  : 'text-zinc-500 hover:text-zinc-300'
              }`}
            >
              {t.label}
              {t.id === 'queue' && pending.length > 0 && (
                <span className="ml-1.5 px-1.5 py-0.5 bg-yellow-500/20 text-yellow-400 text-xs rounded-full">
                  {pending.length}
                </span>
              )}
            </button>
          ))}
        </div>

        <div className="p-4">
          {tab === 'queue' && (
            <GovernancePanel approvals={data?.approvals ?? []} loading={isLoading} />
          )}

          {tab === 'policies' && (
            <div className="space-y-3">
              <div className="flex justify-end">
                <button
                  onClick={() => setPolicyModal({ open: true })}
                  className="px-3 py-1.5 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-500"
                >
                  + New Policy
                </button>
              </div>
              {(data?.policies ?? []).length === 0 && !isLoading ? (
                <EmptyState icon="⚖️" title="No policies defined" description="Create a policy to govern agent behavior." />
              ) : (
                <div className="space-y-2">
                  {(data?.policies ?? []).map((p) => (
                    <div key={p.id} className="flex items-center gap-3 p-3 rounded-lg bg-zinc-800">
                      <div className="flex-1 min-w-0">
                        <div className="flex items-center gap-2 mb-0.5">
                          <span className="text-zinc-100 text-sm font-medium">{p.name}</span>
                          <Badge variant={p.active ? 'success' : 'default'}>{p.active ? 'Active' : 'Inactive'}</Badge>
                        </div>
                        <div className="text-zinc-500 text-xs">{p.description}</div>
                      </div>
                      <div className="flex gap-1">
                        <button onClick={() => setPolicyModal({ open: true, policy: p })} className="px-2 py-1 text-xs rounded bg-zinc-700 text-zinc-300 hover:bg-zinc-600">Edit</button>
                        <button onClick={() => deleteMut.mutate(p.id)} className="px-2 py-1 text-xs rounded bg-red-600/20 text-red-400 hover:bg-red-600/30">Delete</button>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}

          {tab === 'trust' && (
            <div>
              {(data?.trust_scores ?? []).length === 0 && !isLoading ? (
                <EmptyState icon="🏅" title="No trust scores" description="Trust scores are computed from agent behavior." />
              ) : (
                <table className="w-full text-sm">
                  <thead>
                    <tr className="border-b border-zinc-800">
                      <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Agent</th>
                      <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Score</th>
                      <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Tier</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-zinc-800">
                    {(data?.trust_scores ?? []).map((ts: TrustScore) => (
                      <tr key={ts.agent_id} className="hover:bg-zinc-800/50">
                        <td className="px-3 py-2.5 text-zinc-200">{ts.agent_name}</td>
                        <td className="px-3 py-2.5 w-48"><ScoreBar score={ts.score} /></td>
                        <td className="px-3 py-2.5"><TrustTier tier={ts.tier} /></td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>
          )}

          {tab === 'audit' && (
            <div className="space-y-3">
              <input
                className="w-full max-w-xs bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-1.5 focus:outline-none focus:border-blue-500 placeholder-zinc-500"
                placeholder="Filter by agent or action..."
                value={auditFilter}
                onChange={(e) => setAuditFilter(e.target.value)}
              />
              {filteredLogs.length === 0 && !isLoading ? (
                <EmptyState icon="📋" title="No audit events" />
              ) : (
                <table className="w-full text-sm">
                  <thead>
                    <tr className="border-b border-zinc-800">
                      <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Time</th>
                      <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Agent</th>
                      <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Action</th>
                      <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Outcome</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-zinc-800">
                    {filteredLogs.map((log: AuditLog) => (
                      <tr key={log.id} className="hover:bg-zinc-800/50">
                        <td className="px-3 py-2 text-zinc-500 text-xs font-mono">
                          {new Date(log.timestamp).toLocaleString()}
                        </td>
                        <td className="px-3 py-2 text-zinc-200 text-sm">{log.agent_name}</td>
                        <td className="px-3 py-2 text-zinc-400 text-xs">{log.action}</td>
                        <td className="px-3 py-2">
                          <Badge variant={statusVariant(log.outcome)}>{log.outcome}</Badge>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>
          )}

          {tab === 'prism' && (
            <div className="space-y-3">
              <p className="text-zinc-500 text-sm">
                PRISM — Privacy, Reliability, Integrity, Safety, Misuse Prevention compliance scores.
              </p>
              {(data?.prism_scores ?? []).length === 0 && !isLoading ? (
                <EmptyState icon="🔬" title="No PRISM data" description="PRISM scores will appear after agent activity." />
              ) : (
                <div className="space-y-3">
                  {(data?.prism_scores ?? []).map((ps) => (
                    <div key={ps.dimension} className="p-3 rounded-lg bg-zinc-800 flex items-center gap-4">
                      <div className="w-32 text-zinc-300 text-sm">{PRISM_LABELS[ps.dimension] ?? ps.dimension}</div>
                      <div className="flex-1">
                        <div className="h-2 bg-zinc-700 rounded-full overflow-hidden">
                          <div
                            className={`h-full rounded-full ${
                              ps.status === 'pass' ? 'bg-green-500' : ps.status === 'warn' ? 'bg-yellow-500' : 'bg-red-500'
                            }`}
                            style={{ width: `${ps.score}%` }}
                          />
                        </div>
                      </div>
                      <div className="w-10 text-zinc-300 text-xs font-mono text-right">{ps.score}%</div>
                      <Badge variant={statusVariant(ps.status)}>{ps.status}</Badge>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}
        </div>
      </div>

      <PolicyModal
        open={policyModal.open}
        onClose={() => setPolicyModal({ open: false })}
        initial={policyModal.policy}
      />
    </div>
  );
}
